//! Async runtime for the multi-strategy orchestrator.
//!
//! Wires the pure-logic `Orchestrator` to the Bybit exchange: polls market data,
//! feeds ticks to all strategies, executes resulting actions, reports fills back,
//! and persists learned state for the next restart.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{error, info, warn};

/// Shared state exposed to the Telegram bot callbacks.
struct TgSharedState {
    symbol: String,
    equity: f64,
    total_pnl: f64,
    total_fills: u64,
    regime: String,
    active_strategies: usize,
    started_at: i64,
    allocations: HashMap<String, f64>,
    strategy_stats: Vec<(String, f64, u64, f64)>,
    regime_confidence: f64,
}

use crate::alerts::Alerter;
use crate::config::BotConfig;
use crate::metrics::Metrics;
use crate::orchestrator::Orchestrator;
use crate::persistence::{Db, FillRow};
use crate::pnl::PnlTracker;
use crate::regime;
use crate::retry::{CircuitBreaker, RetryPolicy, with_retry};
use crate::risk::{Market, RiskConfig, RiskEngine};
use crate::strategy::dca::DcaStrategy;
use crate::strategy::grid_strategy::{GridStrategy, GridStrategyConfig};
use crate::strategy::momentum::{MomentumConfig, MomentumStrategy};
use crate::strategy::pairs::{HedgeMethod, PairsConfig, PairsStrategy};
use crate::strategy::{MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::telegram::{self, TelegramBot};
use crate::types::Side;
use crate::venue::{Venue, VenueKline};
use crate::ws_private::{self, WsEvent};

pub async fn run(cfg: BotConfig, venue: Venue) -> Result<()> {
    let dry_run = cfg.bot.dry_run;
    if dry_run {
        info!("*** DRY-RUN MODE — no orders will be sent ***");
    }

    // Wrap in Arc so extra-symbol background loops can share it without cloning.
    let venue = Arc::new(venue);

    let symbol = cfg.grid.symbol.clone();

    // -- Persistence --
    let db = if dry_run {
        Arc::new(Mutex::new(Db::open_in_memory()?))
    } else {
        Arc::new(Mutex::new(Db::open(&cfg.bot.db_path)?))
    };

    // -- Alerts --
    let alerter = Arc::new(Alerter::new(
        cfg.alerts.clone(),
        format!("orchestrator-{}", symbol),
    ));
    alerter
        .lifecycle(&format!(
            "Orchestrator starting: {} dry_run={} strategies=DCA+Momentum",
            symbol, dry_run,
        ))
        .await;

    // -- Risk engine --
    let risk_cfg = RiskConfig {
        max_total_exposure: cfg.risk.max_total_exposure,
        max_per_market: cfg.risk.max_per_market,
        max_per_symbol: cfg.risk.max_per_symbol,
        max_per_strategy: cfg.risk.max_per_strategy,
        daily_loss_limit: cfg.risk.daily_loss_limit,
        weekly_loss_limit: cfg.risk.weekly_loss_limit,
        max_drawdown: cfg.risk.max_drawdown,
        max_consecutive_losses: cfg.risk.max_consecutive_losses,
    };
    let (engine, kill) = RiskEngine::new(risk_cfg, cfg.risk.starting_equity_quote);
    let engine = Arc::new(Mutex::new(engine));

    // -- Ctrl+C handler --
    {
        let kill = kill.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("SIGINT received, tripping kill switch");
                kill.store(true, Ordering::SeqCst);
            }
        });
    }

    // -- Metrics --
    let metrics = Arc::new(Metrics::new());
    metrics.set_equity(cfg.risk.starting_equity_quote);
    {
        let m = metrics.clone();
        let pw = cfg.bot.dashboard_password.clone();
        tokio::spawn(async move {
            crate::metrics::serve(m, 9090, pw).await;
        });
    }

    // -- Retry + circuit breaker --
    let retry_policy = RetryPolicy {
        max_retries: cfg.bot.max_retries,
        ..RetryPolicy::default()
    };
    let breaker = Arc::new(CircuitBreaker::new(
        cfg.bot.circuit_breaker_threshold,
        Duration::from_secs(cfg.bot.circuit_breaker_cooldown_secs),
    ));

    info!(venue = venue.name(), "venue client ready");

    // -- Private WebSocket (Bybit only — Zerodha uses its own ticker WS) --
    let (_ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
    if !dry_run && venue.bybit_client().is_some() {
        if let Ok((key, secret, _)) = bybit_creds(&cfg) {
            let ws_testnet = cfg.bybit.testnet;
            let ws_tx_clone = _ws_tx.clone();
            tokio::spawn(async move {
                ws_private::start(key, secret, ws_testnet, ws_tx_clone).await;
            });
            info!("private WebSocket listener spawned (Bybit)");
        }
    }

    // -- Determine symbols to trade --
    // Use orchestrator.symbols if non-empty; fall back to cfg.grid.symbol.
    let symbols: Vec<String> = if cfg.orchestrator.symbols.is_empty() {
        vec![cfg.grid.symbol.clone()]
    } else {
        cfg.orchestrator.symbols.clone()
    };
    let capital_per_symbol =
        cfg.risk.starting_equity_quote * cfg.orchestrator.capital_per_symbol_pct;

    // -- Build one Orchestrator per symbol --
    let mut symbol_orchestrators: Vec<Orchestrator> = symbols
        .iter()
        .map(|sym| {
            info!(
                symbol = %sym,
                capital = format!("{:.0}", capital_per_symbol),
                "Starting strategy set for {} with {:.0} USDT capital",
                sym,
                capital_per_symbol,
            );
            let strats = build_strategies(sym, &cfg);
            let names: Vec<String> = strats.iter().map(|s| s.name().to_string()).collect();
            info!(symbol = %sym, strategies = ?names, "strategies initialised for symbol");
            Orchestrator::new(sym.clone(), strats, cfg.orchestrator.clone())
        })
        .collect();

    // The primary orchestrator drives the rest of the function (single-symbol path).
    let mut orchestrator = symbol_orchestrators.remove(0);

    // Spawn background tick loops for additional symbols (index 1+).
    for extra_orch in symbol_orchestrators {
        let kill2 = kill.clone();
        let venue2 = venue.clone();
        let poll_secs2 = cfg.grid.poll_secs;
        let dry_run2 = dry_run;
        let extra_sym = extra_orch.symbol.clone();
        let mid2 = (cfg.grid.lower_price + cfg.grid.upper_price) / 2.0;
        let initial_snap2 = crate::strategy::MarketSnapshot {
            symbol: extra_sym.clone(),
            bid: mid2 - 1.0,
            ask: mid2 + 1.0,
            last: mid2,
            volume_24h: 1_000_000.0,
        };
        tokio::spawn(async move {
            let poll_interval2 = std::time::Duration::from_secs(poll_secs2);
            let mut orch2 = extra_orch;
            let _ = orch2.on_start(&initial_snap2);
            loop {
                if kill2.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(poll_interval2).await;
                let snap2 = if dry_run2 {
                    initial_snap2.clone()
                } else {
                    match venue2.ticker(&extra_sym).await {
                        Ok(t) => crate::strategy::MarketSnapshot {
                            symbol: extra_sym.clone(),
                            bid: t.bid_price.unwrap_or(t.last_price),
                            ask: t.ask_price.unwrap_or(t.last_price),
                            last: t.last_price,
                            volume_24h: 0.0,
                        },
                        Err(e) => {
                            warn!(
                                symbol = %extra_sym,
                                err = %e,
                                "ticker fetch failed for extra symbol"
                            );
                            continue;
                        }
                    }
                };
                if let Err(e) = orch2.on_tick(&snap2, &[], None) {
                    warn!(
                        symbol = %extra_sym,
                        err = %e,
                        "orchestrator tick failed for extra symbol"
                    );
                }
            }
            let _ = orch2.on_shutdown();
            info!(symbol = %extra_sym, "extra symbol loop shut down");
        });
    }

    // The primary symbol drives the rest of run() unchanged.
    let strategy_names: Vec<String> = orchestrator
        .all_performances()
        .iter()
        .map(|p| p.strategy_name.clone())
        .collect();
    info!(strategies = ?strategy_names, symbol = %symbol, "primary symbol strategies initialised");

    // -- Load persisted state --
    {
        let db_guard = db.lock().await;
        if let Err(e) = orchestrator.load_state(&db_guard) {
            warn!(err = %e, "failed to load orchestrator state, starting fresh");
        } else {
            let allocs = orchestrator.allocations();
            for (name, alloc) in &allocs {
                info!(strategy = %name, allocation_pct = format!("{:.1}", alloc * 100.0), "loaded allocation");
            }
        }
    }

    // -- Wallet balance check --
    if !dry_run {
        match with_retry(&retry_policy, "wallet_balance", || venue.wallet_balance()).await {
            Ok(balance) => {
                engine.lock().await.set_equity(balance.total_equity);
                metrics.set_equity(balance.total_equity);
                info!(
                    equity = balance.total_equity,
                    currency = %balance.currency,
                    "equity from wallet",
                );
            }
            Err(e) => {
                return Err(anyhow!("wallet balance check failed: {}", e));
            }
        }
    }

    // -- Initial market snapshot --
    let initial_snapshot = if dry_run {
        let mid = (cfg.grid.lower_price + cfg.grid.upper_price) / 2.0;
        MarketSnapshot {
            symbol: symbol.clone(),
            bid: mid - 1.0,
            ask: mid + 1.0,
            last: mid,
            volume_24h: 1_000_000.0,
        }
    } else {
        let ticker = with_retry(&retry_policy, "ticker", || venue.ticker(&symbol))
            .await
            .map_err(|e| anyhow!("initial ticker fetch failed: {}", e))?;
        MarketSnapshot {
            symbol: symbol.clone(),
            bid: ticker.bid_price.unwrap_or(ticker.last_price),
            ask: ticker.ask_price.unwrap_or(ticker.last_price),
            last: ticker.last_price,
            volume_24h: 0.0,
        }
    };

    // -- Telegram bot --
    let tg_state = Arc::new(Mutex::new(TgSharedState {
        symbol: symbol.clone(),
        equity: cfg.risk.starting_equity_quote,
        total_pnl: 0.0,
        total_fills: 0,
        regime: "Unknown".to_string(),
        active_strategies: orchestrator.active_count(),
        started_at: Utc::now().timestamp(),
        allocations: orchestrator.allocations(),
        strategy_stats: Vec::new(),
        regime_confidence: 0.0,
    }));

    // -- Alert channel for proactive Telegram push notifications --
    let (alert_tx_raw, alert_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Wrap in Option so the main loop can cheaply no-op when Telegram isn't configured.
    let alert_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>;

    {
        let tg_bot = TelegramBot::from_config(&cfg.telegram).or_else(TelegramBot::from_env);

        if let Some(bot) = tg_bot {
            let tg_kill = kill.clone();
            let poll_secs = cfg.telegram.poll_secs;

            let st = tg_state.clone();
            let status_fn = Arc::new(move || {
                let s = match st.try_lock() {
                    Ok(s) => s,
                    Err(_) => return "⏳ Status temporarily unavailable.".to_string(),
                };
                let uptime = Utc::now().timestamp() - s.started_at;
                telegram::format_status(
                    &s.symbol,
                    s.equity,
                    s.total_pnl,
                    s.total_fills,
                    &s.regime,
                    s.active_strategies,
                    uptime,
                )
            });

            let st = tg_state.clone();
            let pnl_fn = Arc::new(move || {
                let s = match st.try_lock() {
                    Ok(s) => s,
                    Err(_) => return "⏳ P&L temporarily unavailable.".to_string(),
                };
                telegram::format_pnl(&s.strategy_stats)
            });

            let st = tg_state.clone();
            let alloc_fn = Arc::new(move || {
                let s = match st.try_lock() {
                    Ok(s) => s,
                    Err(_) => return "⏳ Allocations temporarily unavailable.".to_string(),
                };
                telegram::format_allocations(&s.allocations)
            });

            let st = tg_state.clone();
            let regime_fn = Arc::new(move || {
                let s = match st.try_lock() {
                    Ok(s) => s,
                    Err(_) => return "⏳ Regime temporarily unavailable.".to_string(),
                };
                telegram::format_regime(&s.regime, s.regime_confidence)
            });

            let callbacks = telegram::BotCallbacks {
                status: status_fn,
                pnl: pnl_fn,
                alloc: alloc_fn,
                regime: regime_fn,
                kill_and_cancel: None,
                open_orders: None,
                zerodha_kill: None,
            };

            tokio::spawn(async move {
                bot.run(tg_kill, Duration::from_secs(poll_secs), callbacks, alert_rx)
                    .await;
            });
            alert_tx = Some(alert_tx_raw);
            info!("Telegram bot started for orchestrator");
        } else {
            // No Telegram configured — drop both ends so alerts are silently discarded.
            alert_tx = None;
            drop(alert_tx_raw);
            drop(alert_rx);
        }
    }

    // -- Start strategies --
    let start_actions = orchestrator.on_start(&initial_snapshot)?;
    for ta in &start_actions {
        if !matches!(ta.action, StrategyAction::None) {
            info!(strategy = %ta.strategy_name, action = ?ta.action, "start action");
        }
    }

    // -- Regime detection state --
    let mut kline_history: Vec<VenueKline>;
    let mut regime_tick: u64 = 0;
    let regime_interval: u64 = 60; // re-detect regime every 60 ticks

    info!(
        poll_secs = cfg.grid.poll_secs,
        active = orchestrator.active_count(),
        "orchestrator loop starting",
    );

    let poll_interval = Duration::from_secs(cfg.grid.poll_secs);
    let mut save_counter: u64 = 0;
    let mut pnl_tracker = PnlTracker::new();

    // -- Proactive alert tracking state --
    let mut prev_regime: Option<String> = None;
    let mut prev_active_count: usize = orchestrator.active_count();
    let mut prev_allocations: HashMap<String, f64> = orchestrator.allocations();
    let starting_equity = cfg.risk.starting_equity_quote;
    let mut pnl_loss_alert_sent = false;
    let mut pnl_gain_alert_sent = false;

    /// Helper: send an alert through the channel (no-op when Telegram is not configured).
    macro_rules! send_alert {
        ($tx:expr, $msg:expr) => {
            if let Some(ref tx) = $tx {
                let _ = tx.send($msg);
            }
        };
    }

    // ======================== MAIN LOOP ========================
    loop {
        if kill.load(Ordering::SeqCst) {
            info!("kill switch tripped, shutting down orchestrator");
            break;
        }

        time::sleep(poll_interval).await;

        // -- Drain WebSocket fill events --
        let mut ws_fills = Vec::new();
        while let Ok(event) = ws_rx.try_recv() {
            if let WsEvent::OrderFilled {
                order_id,
                symbol: fill_sym,
                side,
                price,
                qty,
                ..
            } = event
            {
                ws_fills.push(crate::strategy::FillEvent {
                    order_id,
                    symbol: fill_sym,
                    side,
                    price,
                    qty,
                });
            }
        }

        // -- Get current price --
        let snapshot = if dry_run {
            initial_snapshot.clone()
        } else {
            match venue.ticker(&symbol).await {
                Ok(ticker) => MarketSnapshot {
                    symbol: symbol.clone(),
                    bid: ticker.bid_price.unwrap_or(ticker.last_price),
                    ask: ticker.ask_price.unwrap_or(ticker.last_price),
                    last: ticker.last_price,
                    volume_24h: 0.0,
                },
                Err(e) => {
                    breaker.record_failure();
                    warn!(err = %e, "ticker fetch failed");
                    continue;
                }
            }
        };

        // -- Regime detection (periodic) --
        regime_tick += 1;
        let current_regime = if regime_tick % regime_interval == 0 && !dry_run {
            match venue.klines(&symbol, "15", 200).await {
                Ok(klines) => {
                    kline_history = klines;
                    let analysis = regime::analyze(&kline_history);
                    if let Some(ref a) = analysis {
                        info!(
                            regime = %a.regime,
                            confidence = format!("{:.2}", a.confidence),
                            adx = format!("{:.1}", a.adx),
                            "regime detected",
                        );
                        tg_state.lock().await.regime_confidence = a.confidence;
                    }
                    analysis.map(|a| a.regime)
                }
                Err(e) => {
                    warn!(err = %e, "kline fetch for regime detection failed");
                    orchestrator.current_regime()
                }
            }
        } else {
            orchestrator.current_regime()
        };

        // -- Tick orchestrator --
        let tagged_actions = match orchestrator.on_tick(&snapshot, &ws_fills, current_regime) {
            Ok(actions) => actions,
            Err(e) => {
                error!(err = %e, "orchestrator tick failed");
                continue;
            }
        };

        // -- Execute actions --
        for ta in &tagged_actions {
            match &ta.action {
                StrategyAction::PlaceOrder {
                    symbol: sym,
                    side,
                    price,
                    qty,
                    order_type,
                } => {
                    if *qty < 1e-12 {
                        continue;
                    }

                    // Risk check
                    let notional = price * qty;
                    let check = engine.lock().await.pretrade_check(
                        Market::Crypto,
                        sym,
                        &ta.strategy_name,
                        notional,
                    );
                    if let Err(reject) = check {
                        warn!(
                            strategy = %ta.strategy_name,
                            reject = ?reject,
                            "risk check rejected order",
                        );
                        continue;
                    }

                    if dry_run {
                        info!(
                            strategy = %ta.strategy_name,
                            side = ?side,
                            price,
                            qty,
                            order_type = ?order_type,
                            "[DRY-RUN] would place order",
                        );
                        let pnl = pnl_tracker.record_fill(&ta.strategy_name, *side, *price, *qty);
                        orchestrator.record_fill(&ta.strategy_name, pnl);
                        engine.lock().await.on_fill(
                            Market::Crypto,
                            sym,
                            &ta.strategy_name,
                            notional,
                        );
                        if pnl != 0.0 {
                            engine.lock().await.on_trade_result(pnl);
                        }
                        metrics.record_fill(*side == Side::Buy);
                    } else {
                        let qty_str = format!("{:.8}", qty);
                        let price_str = format!("{:.2}", price);
                        let (kind, price_arg) = match order_type {
                            OrderType::Market => (crate::types::OrderKind::Market, None),
                            OrderType::Limit => {
                                (crate::types::OrderKind::Limit, Some(price_str.as_str()))
                            }
                        };
                        let result = venue
                            .place_order(sym, *side, kind, &qty_str, price_arg, None, false)
                            .await;

                        match result {
                            Ok(order_resp) => {
                                let pnl =
                                    pnl_tracker.record_fill(&ta.strategy_name, *side, *price, *qty);
                                info!(
                                    strategy = %ta.strategy_name,
                                    order_id = %order_resp.order_id,
                                    side = ?side,
                                    qty,
                                    pnl = format!("{:.4}", pnl),
                                    "order filled",
                                );

                                let fill_row = FillRow {
                                    order_id: order_resp.order_id.clone(),
                                    symbol: sym.clone(),
                                    side: *side,
                                    price: *price,
                                    qty: *qty,
                                    notional,
                                    realised_pnl: pnl,
                                    filled_at: Utc::now(),
                                };
                                let _ = db.lock().await.insert_fill(&fill_row);
                                orchestrator.record_fill(&ta.strategy_name, pnl);
                                engine.lock().await.on_fill(
                                    Market::Crypto,
                                    sym,
                                    &ta.strategy_name,
                                    notional,
                                );
                                if pnl != 0.0 {
                                    engine.lock().await.on_trade_result(pnl);
                                }
                                metrics.record_fill(*side == Side::Buy);
                            }
                            Err(e) => {
                                error!(
                                    strategy = %ta.strategy_name,
                                    err = %e,
                                    "order placement failed",
                                );
                                breaker.record_failure();
                                engine.lock().await.on_trade_result(-1.0);
                                orchestrator.record_fill(&ta.strategy_name, -1.0);
                            }
                        }
                    }
                }
                StrategyAction::CancelAll { symbol: sym } => {
                    if !dry_run {
                        if let Err(e) = venue.cancel_all_orders(sym).await {
                            warn!(strategy = %ta.strategy_name, err = %e, "cancel_all failed");
                        }
                    }
                }
                StrategyAction::TripKillSwitch { reason } => {
                    error!(strategy = %ta.strategy_name, reason, "strategy tripped kill switch");
                    kill.store(true, Ordering::SeqCst);
                    alerter
                        .lifecycle(&format!("KILL SWITCH: {} — {}", ta.strategy_name, reason))
                        .await;
                }
                StrategyAction::CancelOrder { order_id } => {
                    if !dry_run {
                        let _ = venue.cancel_order(&symbol, order_id).await;
                    }
                }
                StrategyAction::None => {}
            }
        }

        // -- Process WebSocket fills as P&L feedback --
        if !ws_fills.is_empty() && !strategy_names.is_empty() {
            for fill in &ws_fills {
                for name in &strategy_names {
                    let pnl = pnl_tracker.record_fill(
                        name,
                        fill.side,
                        fill.price,
                        fill.qty / strategy_names.len() as f64,
                    );
                    if pnl != 0.0 {
                        orchestrator.record_fill(name, pnl);
                    }
                }
            }
        }

        // -- Update Telegram shared state --
        let cur_regime_str;
        let cur_active_count;
        let cur_allocations;
        let cur_total_pnl;
        {
            let mut tg = tg_state.lock().await;
            tg.equity = metrics.equity_cents.load(Ordering::Relaxed) as f64 / 100.0;
            tg.total_pnl = metrics.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0;
            tg.total_fills = metrics.fills_total.load(Ordering::Relaxed);
            tg.active_strategies = orchestrator.active_count();
            tg.allocations = orchestrator.allocations();
            if let Some(r) = orchestrator.current_regime() {
                tg.regime = r.to_string();
            }
            tg.strategy_stats = orchestrator
                .all_performances()
                .iter()
                .map(|p| (p.strategy_name.clone(), p.total_pnl, p.trades, p.win_rate()))
                .collect();

            // Snapshot values for alert comparison (while we hold the lock).
            cur_regime_str = tg.regime.clone();
            cur_active_count = tg.active_strategies;
            cur_allocations = tg.allocations.clone();
            cur_total_pnl = tg.total_pnl;
        }

        // -- Proactive Telegram push alerts --
        {
            // (a) Regime change
            let regime_changed = match &prev_regime {
                Some(prev) => *prev != cur_regime_str,
                None => cur_regime_str != "Unknown",
            };
            if regime_changed {
                let prev_display = prev_regime.as_deref().unwrap_or("None");
                send_alert!(
                    alert_tx,
                    format!(
                        "*Regime Change*\n{} -> {}\nActive strategies: {}",
                        prev_display, cur_regime_str, cur_active_count,
                    )
                );
                prev_regime = Some(cur_regime_str.clone());
            }

            // (b) Strategy disabled or re-enabled
            if cur_active_count != prev_active_count {
                if cur_active_count < prev_active_count {
                    send_alert!(
                        alert_tx,
                        format!(
                            "*Strategy Disabled*\nActive strategies: {} -> {}\n\
                             A strategy was disabled by the risk guard (excessive drawdown).",
                            prev_active_count, cur_active_count,
                        )
                    );
                } else {
                    send_alert!(
                        alert_tx,
                        format!(
                            "*Strategy Re-enabled*\nActive strategies: {} -> {}\n\
                             A strategy was re-enabled after cooldown.",
                            prev_active_count, cur_active_count,
                        )
                    );
                }
                prev_active_count = cur_active_count;
            }

            // (c) Significant rebalance — any allocation shifted by >10 percentage points
            {
                let mut big_shifts = Vec::new();
                for (name, &new_alloc) in &cur_allocations {
                    let old_alloc = prev_allocations.get(name).copied().unwrap_or(0.0);
                    let shift = (new_alloc - old_alloc).abs();
                    if shift > 0.10 {
                        big_shifts.push(format!(
                            "`{}` {:.1}% -> {:.1}%",
                            name,
                            old_alloc * 100.0,
                            new_alloc * 100.0,
                        ));
                    }
                }
                // Also check for strategies that disappeared from allocations (disabled)
                for (name, &old_alloc) in &prev_allocations {
                    if !cur_allocations.contains_key(name) && old_alloc > 0.10 {
                        big_shifts.push(format!("`{}` {:.1}% -> 0.0%", name, old_alloc * 100.0,));
                    }
                }
                if !big_shifts.is_empty() {
                    send_alert!(
                        alert_tx,
                        format!(
                            "*Rebalance Alert*\nSignificant allocation shifts:\n{}",
                            big_shifts.join("\n"),
                        )
                    );
                }
                prev_allocations = cur_allocations;
            }

            // (d) P&L threshold alerts (with one-shot guard to avoid spam)
            if starting_equity > 0.0 {
                let pnl_pct = cur_total_pnl / starting_equity;
                if pnl_pct <= -0.05 && !pnl_loss_alert_sent {
                    send_alert!(
                        alert_tx,
                        format!(
                            "*P&L Warning*\nTotal P&L has crossed -5% of starting equity.\n\
                             P&L: {:+.2} USDT ({:+.1}%)",
                            cur_total_pnl,
                            pnl_pct * 100.0,
                        )
                    );
                    pnl_loss_alert_sent = true;
                }
                if pnl_pct >= 0.10 && !pnl_gain_alert_sent {
                    send_alert!(
                        alert_tx,
                        format!(
                            "*P&L Milestone*\nTotal P&L has crossed +10% of starting equity!\n\
                             P&L: {:+.2} USDT ({:+.1}%)",
                            cur_total_pnl,
                            pnl_pct * 100.0,
                        )
                    );
                    pnl_gain_alert_sent = true;
                }
            }
        }

        // -- Periodic state save --
        save_counter += 1;
        if save_counter % 100 == 0 {
            let db_guard = db.lock().await;
            if let Err(e) = orchestrator.save_state(&db_guard) {
                warn!(err = %e, "failed to save orchestrator state");
            }
            // Update regime priors from observed data
            orchestrator.update_regime_priors();
        }
    }

    // ======================== SHUTDOWN ========================
    info!("shutting down orchestrator");
    let shutdown_actions = orchestrator.on_shutdown()?;
    for ta in &shutdown_actions {
        info!(strategy = %ta.strategy_name, action = ?ta.action, "shutdown action");
    }

    // Final state save
    {
        let db_guard = db.lock().await;
        orchestrator.save_state(&db_guard)?;
    }

    // Log final performance summary
    for perf in orchestrator.all_performances() {
        info!(
            strategy = %perf.strategy_name,
            total_pnl = format!("{:.2}", perf.total_pnl),
            trades = perf.trades,
            win_rate = format!("{:.1}%", perf.win_rate() * 100.0),
            max_drawdown = format!("{:.1}%", perf.max_drawdown * 100.0),
            sharpe = format!("{:.3}", perf.sharpe_estimate()),
            "final strategy performance",
        );
    }

    let allocs = orchestrator.allocations();
    info!(allocations = ?allocs, "final allocations (learned)");

    alerter.lifecycle("Orchestrator shutdown complete").await;
    info!("orchestrator shutdown complete");
    Ok(())
}

fn build_strategies(symbol: &str, cfg: &BotConfig) -> Vec<Box<dyn Strategy>> {
    let orch = &cfg.orchestrator;
    let mut strategies: Vec<Box<dyn Strategy>> = Vec::new();

    for name in &orch.strategies {
        match name.as_str() {
            "dca" => {
                let dca_cfg = &orch.dca;
                strategies.push(Box::new(DcaStrategy::with_exits(
                    symbol.to_string(),
                    dca_cfg.buy_amount_quote,
                    dca_cfg.interval_ticks,
                    dca_cfg.sma_period,
                    dca_cfg.use_sma_filter,
                    dca_cfg.take_profit_pct,
                    dca_cfg.trailing_stop_pct,
                )));
                info!(
                    amount = dca_cfg.buy_amount_quote,
                    interval = dca_cfg.interval_ticks,
                    sma_filter = dca_cfg.use_sma_filter,
                    take_profit = format!("{:.1}%", dca_cfg.take_profit_pct * 100.0),
                    trailing_stop = format!("{:.1}%", dca_cfg.trailing_stop_pct * 100.0),
                    "enabled DCA strategy",
                );
            }
            "momentum" => {
                let mom_cfg = &orch.momentum;
                strategies.push(Box::new(MomentumStrategy::new(MomentumConfig {
                    symbol: symbol.to_string(),
                    position_size_quote: mom_cfg.position_size_quote,
                    cooldown_ticks: mom_cfg.cooldown_ticks,
                    ..MomentumConfig::default()
                })));
                info!(
                    position_size = mom_cfg.position_size_quote,
                    cooldown = mom_cfg.cooldown_ticks,
                    "enabled Momentum strategy",
                );
            }
            "grid" => {
                let grid_cfg = &orch.grid;
                strategies.push(Box::new(GridStrategy::new(GridStrategyConfig {
                    symbol: symbol.to_string(),
                    lower_price: grid_cfg.lower_price,
                    upper_price: grid_cfg.upper_price,
                    levels: grid_cfg.levels,
                    qty_per_level: grid_cfg.qty_per_level,
                    cancel_buys_on_trend: true,
                    auto_recenter: false,
                    recenter_threshold_pct: 0.15,
                    vwap_entry_filter: false,
                    vwap_threshold_pct: 0.005,
                    venue: "bybit".to_string(),
                    respect_market_hours: false,
                })));
                info!(
                    lower = grid_cfg.lower_price,
                    upper = grid_cfg.upper_price,
                    levels = grid_cfg.levels,
                    qty = grid_cfg.qty_per_level,
                    "enabled Grid strategy",
                );
            }
            "pairs" => {
                let pairs_cfg = &orch.pairs;
                strategies.push(Box::new(PairsStrategy::new(PairsConfig {
                    symbol_a: pairs_cfg.symbol_a.clone(),
                    symbol_b: pairs_cfg.symbol_b.clone(),
                    lookback: pairs_cfg.lookback,
                    entry_z_score: pairs_cfg.entry_z_score,
                    exit_z_score: pairs_cfg.exit_z_score,
                    stop_z_score: pairs_cfg.stop_z_score,
                    position_size_quote: pairs_cfg.position_size_quote,
                    min_correlation: pairs_cfg.min_correlation,
                    hedge_ratio_method: HedgeMethod::Fixed(pairs_cfg.hedge_ratio),
                    ..PairsConfig::default()
                })));
                info!(
                    symbol_a = %pairs_cfg.symbol_a,
                    symbol_b = %pairs_cfg.symbol_b,
                    lookback = pairs_cfg.lookback,
                    entry_z = pairs_cfg.entry_z_score,
                    "enabled Pairs strategy",
                );
            }
            other => {
                warn!(strategy = other, "unknown strategy in config, skipping");
            }
        }
    }

    if strategies.is_empty() {
        warn!("no strategies configured, defaulting to DCA + Momentum");
        strategies.push(Box::new(DcaStrategy::with_exits(
            symbol.to_string(),
            100.0,
            10,
            200,
            true,
            0.05,
            0.03,
        )));
        strategies.push(Box::new(MomentumStrategy::new(MomentumConfig {
            symbol: symbol.to_string(),
            ..MomentumConfig::default()
        })));
    }

    info!(count = strategies.len(), "strategies built");
    strategies
}

fn bybit_creds(cfg: &BotConfig) -> Result<(String, String, bool)> {
    Ok((
        cfg.bybit.api_key.clone(),
        cfg.bybit.api_secret.clone(),
        cfg.bybit.testnet,
    ))
}
