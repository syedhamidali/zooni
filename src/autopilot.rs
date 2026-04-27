//! Auto-pilot mode — fully autonomous trading.
//!
//! The bot scans markets, picks the best symbol(s), configures grid parameters,
//! sizes positions, and manages the full lifecycle. When conditions change,
//! it gracefully winds down unprofitable grids and switches to better opportunities.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{error, info, warn};

use crate::alerts::Alerter;
use crate::config::{AutoPilotConfig, BotConfig};
use crate::grid::{Grid, LevelOrder};
use crate::metrics::Metrics;
use crate::persistence::{Db, FillRow, OrderRow, OrderStatus};
use crate::retry::{CircuitBreaker, RetryPolicy, with_retry};
use crate::risk::{Market, RiskConfig, RiskEngine};
use crate::scanner::{self, ScanResult};
use crate::sizing;
use crate::types::Side;
use crate::venues::bybit;
use crate::ws_private::{self, WsEvent};

/// Shared context for autopilot helper functions.
struct ApCtx<'a> {
    client: &'a bybit::rest::Client,
    engine: &'a Arc<Mutex<RiskEngine>>,
    db: &'a Arc<Mutex<Db>>,
    alerter: &'a Arc<Alerter>,
    retry_policy: &'a RetryPolicy,
    breaker: &'a Arc<CircuitBreaker>,
    dry_run: bool,
    ap_cfg: &'a AutoPilotConfig,
    bot_cfg: &'a BotConfig,
}

#[allow(dead_code)]
/// State for one active grid running on a symbol.
struct ActiveGrid {
    symbol: String,
    grid: Grid,
    score: f64,
    tick_size: String,
    base_precision: String,
    started_at: chrono::DateTime<Utc>,
    total_pnl: f64,
}

/// Run the bot in fully autonomous auto-pilot mode.
pub async fn run(cfg: BotConfig) -> Result<()> {
    let dry_run = cfg.bot.dry_run;
    let ap_cfg = cfg.autopilot.clone();

    info!("🤖 AUTO-PILOT MODE — bot will autonomously scan, pick, and trade");
    if dry_run {
        info!("*** DRY-RUN MODE — no orders will be sent to the exchange ***");
    }

    // -- Metrics HTTP server --
    let metrics = Arc::new(Metrics::new());
    metrics.set_equity(cfg.risk.starting_equity_quote);
    {
        let m = metrics.clone();
        let pw = cfg.bot.dashboard_password.clone();
        tokio::spawn(async move {
            crate::metrics::serve(m, 9090, pw).await;
        });
    }

    // -- Persistence --
    let db = if dry_run {
        Arc::new(Mutex::new(Db::open_in_memory()?))
    } else {
        Arc::new(Mutex::new(Db::open(&cfg.bot.db_path)?))
    };

    // -- Alerts --
    let alerter = Arc::new(Alerter::new(cfg.alerts.clone(), "autopilot".to_string()));
    alerter
        .lifecycle("Auto-pilot starting — scanning for opportunities...")
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
        dry_run,
    };
    let (engine, kill) = RiskEngine::new(risk_cfg, cfg.risk.starting_equity_quote);
    let engine = Arc::new(Mutex::new(engine));

    // -- Kill switch --
    {
        let kill = kill.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("SIGINT received, tripping kill switch");
                kill.store(true, Ordering::SeqCst);
            }
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

    // -- Bybit client --
    let client = if dry_run {
        bybit::rest::Client::new(bybit::Config {
            credentials: Some(bybit::Credentials {
                api_key: "DRY_RUN".into(),
                api_secret: "DRY_RUN".into(),
            }),
            testnet: true,
            recv_window_ms: cfg.bybit.recv_window_ms,
        })
    } else {
        bybit::rest::Client::new(bybit::Config {
            credentials: Some(bybit::Credentials {
                api_key: cfg.bybit.api_key.clone(),
                api_secret: cfg.bybit.api_secret.clone(),
            }),
            testnet: cfg.bybit.testnet,
            recv_window_ms: cfg.bybit.recv_window_ms,
        })
    };
    info!(testnet = cfg.bybit.testnet, "Bybit client ready");

    // -- Private WebSocket for instant fill detection --
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
    if !dry_run {
        let ws_api_key = cfg.bybit.api_key.clone();
        let ws_api_secret = cfg.bybit.api_secret.clone();
        let ws_testnet = cfg.bybit.testnet;
        let ws_tx_clone = ws_tx.clone();
        tokio::spawn(async move {
            ws_private::start(ws_api_key, ws_api_secret, ws_testnet, ws_tx_clone).await;
        });
        info!("private WebSocket listener spawned for autopilot");
    }

    // -- Wallet balance --
    let mut equity = cfg.risk.starting_equity_quote;
    if !dry_run && let Ok(accounts) = client.wallet_balance(&cfg.bybit.account_type).await {
        for a in &accounts {
            for c in &a.coin {
                if (c.coin == "USDT" || c.coin == "USD")
                    && !c.wallet_balance.is_empty()
                    && let Ok(bal) = c.wallet_balance.parse::<f64>()
                {
                    equity = bal;
                    engine.lock().await.set_equity(bal);
                    info!(equity = bal, "wallet balance loaded");
                }
            }
        }
    }

    // -- Active grids --
    let mut active_grids: HashMap<String, ActiveGrid> = HashMap::new();

    // -- Scan + trade intervals --
    let mut scan_tick = time::interval(Duration::from_secs(ap_cfg.scan_interval_secs));
    let mut trade_tick = time::interval(Duration::from_secs(cfg.grid.poll_secs));
    scan_tick.tick().await;
    trade_tick.tick().await;

    // -- Shared context --
    let ctx = ApCtx {
        client: &client,
        engine: &engine,
        db: &db,
        alerter: &alerter,
        retry_policy: &retry_policy,
        breaker: &breaker,
        dry_run,
        ap_cfg: &ap_cfg,
        bot_cfg: &cfg,
    };

    // -- Initial scan --
    info!("Running initial market scan...");
    let initial_results = if dry_run {
        // In dry-run, use the configured symbol
        info!("Dry-run: using configured symbol {}", cfg.grid.symbol);
        vec![]
    } else {
        scanner::scan(&client, &ap_cfg.scanner)
            .await
            .unwrap_or_default()
    };

    // Deploy grids on initial scan results
    if !dry_run {
        let to_deploy: Vec<&ScanResult> = initial_results
            .iter()
            .filter(|r| r.analysis.grid_suitable)
            .take(ap_cfg.max_grids)
            .collect();

        if to_deploy.is_empty() {
            warn!("No suitable trading pairs found — will retry at next scan interval");
        }

        for result in to_deploy {
            match deploy_grid(&ctx, result, equity).await {
                Ok(ag) => {
                    info!(symbol = %ag.symbol, score = ag.score, "Grid deployed");
                    active_grids.insert(ag.symbol.clone(), ag);
                }
                Err(e) => {
                    warn!(symbol = %result.symbol, err = %e, "Failed to deploy grid");
                }
            }
        }
    } else {
        // Dry-run: deploy on configured symbol with configured params
        let grid_cfg = &cfg.grid;
        let mid = (grid_cfg.lower_price + grid_cfg.upper_price) / 2.0;
        let mut g = Grid::new(grid_cfg, "0.01".into(), "0.000001".into(), 0.0001, 5.0);
        let plan = g.plan_initial(mid);
        info!(
            symbol = %grid_cfg.symbol,
            levels = g.levels.len(),
            plan = plan.len(),
            "[DRY-RUN] deploying grid",
        );
        for p in &plan {
            let fake_id = format!("DRY-{}", uuid::Uuid::new_v4().as_simple());
            g.orders.insert(
                p.level_index,
                LevelOrder {
                    order_id: fake_id,
                    side: p.side,
                    price_str: p.price_str.clone(),
                    qty_str: p.qty_str.clone(),
                    filled_qty: 0.0,
                    target_qty: p.qty_str.parse().unwrap_or(0.0),
                },
            );
        }
        active_grids.insert(
            grid_cfg.symbol.clone(),
            ActiveGrid {
                symbol: grid_cfg.symbol.clone(),
                grid: g,
                score: 50.0,
                tick_size: "0.01".into(),
                base_precision: "0.000001".into(),
                started_at: Utc::now(),
                total_pnl: 0.0,
            },
        );
    }

    alerter
        .lifecycle(&format!(
            "Auto-pilot active: {} grid(s) running on [{}]",
            active_grids.len(),
            active_grids.keys().cloned().collect::<Vec<_>>().join(", "),
        ))
        .await;

    // -- Main loop: trade ticks + periodic re-scan --
    let scan_interval_ticks = (ap_cfg.scan_interval_secs / cfg.grid.poll_secs).max(1);
    let mut scan_counter: u64 = 0;

    loop {
        tokio::select! {
            _ = trade_tick.tick() => {
                if kill.load(Ordering::SeqCst) {
                    info!("kill switch tripped; exiting auto-pilot loop");
                    break;
                }

                if breaker.is_open() {
                    warn!("circuit breaker open — skipping tick");
                    continue;
                }

                // -- Reconcile all active grids --
                let symbols: Vec<String> = active_grids.keys().cloned().collect();
                for symbol in &symbols {
                    if let Some(ag) = active_grids.get_mut(symbol) {
                        if dry_run {
                            continue;
                        }
                        match reconcile_grid(&client, ag, &engine, &db, &alerter, &retry_policy, &breaker)
                            .await
                        {
                            Ok(_) => breaker.record_success(),
                            Err(e) => {
                                breaker.record_failure();
                                warn!(symbol = %symbol, err = %e, "reconcile failed");
                            }
                        }
                    }
                }

                // -- Periodic re-scan --
                scan_counter += 1;
                if scan_counter >= scan_interval_ticks && !dry_run {
                    scan_counter = 0;

                    info!("Auto-pilot: running periodic market scan...");

                    // Refresh equity
                    if let Ok(accounts) = client.wallet_balance(&cfg.bybit.account_type).await {
                        for a in &accounts {
                            for c in &a.coin {
                                if (c.coin == "USDT" || c.coin == "USD")
                                    && !c.wallet_balance.is_empty()
                                    && let Ok(bal) = c.wallet_balance.parse::<f64>()
                                {
                                    equity = bal;
                                    engine.lock().await.set_equity(bal);
                                }
                            }
                        }
                    }

                    match scanner::scan(&client, &ap_cfg.scanner).await {
                        Ok(results) => {
                            evaluate_and_switch(&ctx, &mut active_grids, &results, equity).await;
                        }
                        Err(e) => {
                            warn!(err = %e, "periodic scan failed");
                        }
                    }

                    // Report status
                    let status_lines: Vec<String> = active_grids
                        .values()
                        .map(|ag| {
                            format!(
                                "{}: {} orders, PnL {:+.2}",
                                ag.symbol,
                                ag.grid.orders.len(),
                                ag.total_pnl,
                            )
                        })
                        .collect();
                    alerter
                        .lifecycle(&format!(
                            "📊 Auto-pilot status (equity: {:.2}):\n{}",
                            equity,
                            status_lines.join("\n"),
                        ))
                        .await;
                }
            }

            event = ws_rx.recv() => {
                let Some(event) = event else { continue };
                match event {
                    WsEvent::OrderFilled {
                        order_id,
                        symbol: ws_symbol,
                        side,
                        price: fill_price,
                        qty: fill_qty,
                        status,
                        ..
                    } => {
                        // Find which active grid this order belongs to
                        if let Some(ag) = active_grids.get_mut(&ws_symbol) {
                            let level_entry = ag.grid.orders.iter().find(|(_, o)| o.order_id == order_id);
                            if let Some((&level, _)) = level_entry {
                                let notional = fill_price * fill_qty;
                                let grid_spacing = if ag.grid.levels.len() >= 2 {
                                    ag.grid.levels[1] - ag.grid.levels[0]
                                } else {
                                    0.0
                                };
                                let pnl = match side {
                                    Side::Sell => grid_spacing * fill_qty,
                                    Side::Buy => 0.0,
                                };
                                ag.total_pnl += pnl;

                                if status == "Filled" {
                                    ag.grid.orders.remove(&level);
                                }

                                info!(symbol = %ws_symbol, level, ?side, pnl, "[WS] fill processed");

                                // Record fill in DB — single lock
                                {
                                    let db_guard = db.lock().await;
                                    let _ = db_guard.mark_filled(&order_id, fill_price, fill_qty);
                                    let _ = db_guard.insert_fill(&FillRow {
                                        order_id: order_id.clone(),
                                        symbol: ws_symbol.clone(),
                                        side,
                                        price: fill_price,
                                        qty: fill_qty,
                                        notional,
                                        realised_pnl: pnl,
                                        filled_at: Utc::now(),
                                    });
                                }

                                let side_str = match side {
                                    Side::Buy => "BUY",
                                    Side::Sell => "SELL",
                                };
                                alerter
                                    .fill(&ws_symbol, side_str, fill_price, fill_qty, pnl)
                                    .await;
                                engine
                                    .lock()
                                    .await
                                    .on_fill(Market::Crypto, &ws_symbol, "grid", -notional);

                                // Place replacement if fully filled
                                if status == "Filled" {
                                    if let Some(plan) = ag.grid.plan_replacement(level, side) {
                                        let price: f64 = plan.price_str.parse().unwrap_or(0.0);
                                        let qty: f64 = plan.qty_str.parse().unwrap_or(0.0);
                                        if price > 0.0 && qty > 0.0 {
                                            let notional = price * qty;
                                            let risk_ok = {
                                                let guard = engine.lock().await;
                                                guard.pretrade_check(Market::Crypto, &ws_symbol, "grid", notional).is_ok()
                                            };
                                            if risk_ok {
                                                let link_id = format!(
                                                    "ap-{}-{}-{}",
                                                    ws_symbol, plan.level_index, uuid::Uuid::new_v4().as_simple()
                                                );
                                                let qty_str = plan.qty_str.clone();
                                                let price_str = plan.price_str.clone();
                                                let link_clone = link_id.clone();
                                                match with_retry(&retry_policy, "place_replacement", || {
                                                    client.place_spot_order(
                                                        &ws_symbol, plan.side, "Limit",
                                                        &qty_str, Some(&price_str), Some(&link_clone), false,
                                                    )
                                                }).await {
                                                    Ok(resp) => {
                                                        ag.grid.orders.insert(
                                                            plan.level_index,
                                                            LevelOrder {
                                                                order_id: resp.order_id.clone(),
                                                                side: plan.side,
                                                                price_str: plan.price_str.clone(),
                                                                qty_str: plan.qty_str.clone(),
                                                                filled_qty: 0.0,
                                                                target_qty: plan.qty_str.parse().unwrap_or(0.0),
                                                            },
                                                        );
                                                        let _ = db.lock().await.insert_order(&OrderRow {
                                                            order_id: resp.order_id,
                                                            order_link_id: link_id,
                                                            symbol: ws_symbol.clone(),
                                                            side: plan.side,
                                                            price, qty,
                                                            level_index: plan.level_index,
                                                            status: OrderStatus::Open,
                                                            placed_at: Utc::now(),
                                                            filled_at: None,
                                                            filled_qty: 0.0,
                                                            filled_price: 0.0,
                                                        });
                                                        engine.lock().await.on_fill(
                                                            Market::Crypto, &ws_symbol, "grid", notional,
                                                        );
                                                    }
                                                    Err(e) => {
                                                        warn!(symbol = %ws_symbol, level = plan.level_index, err = %e, "WS replacement failed");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    WsEvent::OrderCancelled { order_id, symbol: ws_symbol } => {
                        if let Some(ag) = active_grids.get_mut(&ws_symbol) {
                            let removed = ag.grid.orders.iter()
                                .find(|(_, o)| o.order_id == order_id)
                                .map(|(&level, _)| level);
                            if let Some(level) = removed {
                                ag.grid.orders.remove(&level);
                                info!(symbol = %ws_symbol, level, order_id = %order_id, "[WS] order cancelled");
                            }
                        }
                    }
                    WsEvent::Connected => {
                        info!("[WS] private WebSocket connected (autopilot)");
                    }
                    WsEvent::Disconnected => {
                        warn!("[WS] private WebSocket disconnected (autopilot, will auto-reconnect)");
                    }
                }
            }
        }
    }

    // -- Shutdown: wind down all grids --
    info!(
        "Auto-pilot shutdown: winding down {} grid(s)",
        active_grids.len()
    );
    for (symbol, ag) in &mut active_grids {
        if !dry_run {
            info!(symbol = %symbol, "cancelling all orders");
            if let Err(e) = client.cancel_all_spot(symbol).await {
                error!(symbol = %symbol, err = %e, "cancel_all failed at shutdown");
            }
        }
        for (_, order) in ag.grid.orders.drain() {
            let _ = db.lock().await.mark_cancelled(&order.order_id);
        }
    }

    // Final summary
    let total_pnl: f64 = active_grids.values().map(|ag| ag.total_pnl).sum();
    let total_fills = db.lock().await.total_fill_count("*").unwrap_or(0);
    info!(
        total_pnl,
        total_fills,
        equity,
        grids = active_grids.len(),
        "auto-pilot session complete",
    );
    alerter
        .lifecycle(&format!(
            "Auto-pilot shutdown: PnL={total_pnl:+.2} equity={equity:.2}",
        ))
        .await;

    Ok(())
}

/// Deploy a new grid on a scanned symbol.
async fn deploy_grid(ctx: &ApCtx<'_>, scan: &ScanResult, equity: f64) -> Result<ActiveGrid> {
    let ApCtx {
        client,
        engine,
        db,
        alerter,
        retry_policy,
        breaker,
        dry_run,
        bot_cfg,
        ..
    } = ctx;
    let symbol = &scan.symbol;
    info!(symbol = %symbol, score = scan.score, "Deploying grid...");

    // Get the suggested range from regime analysis, or calculate from price
    let (lower, upper) = scan.analysis.suggested_range.unwrap_or_else(|| {
        let spread = scan.last_price * 0.05; // default 5% range each way
        (scan.last_price - spread, scan.last_price + spread)
    });

    let num_levels = scan.analysis.suggested_levels.unwrap_or(15);

    // Get instrument info for tick size and precision
    let info = with_retry(retry_policy, "instrument_info", || {
        client.instrument_info_spot(symbol)
    })
    .await
    .context("instrument info fetch failed")?;

    let tick_size = info.price_filter.tick_size;
    let base_precision = info.lot_size_filter.base_precision;
    let min_qty: f64 = info.lot_size_filter.min_order_qty.parse().unwrap_or(0.0);
    let min_amt: f64 = info.lot_size_filter.min_order_amt.parse().unwrap_or(0.0);

    // Smart sizing — determine qty per level
    let grid_spacing_pct = if num_levels > 1 {
        (upper - lower) / scan.last_price / (num_levels as f64 - 1.0) * 100.0
    } else {
        1.0
    };

    let sizing_rec = sizing::recommend(
        equity,
        &scan.analysis,
        grid_spacing_pct,
        num_levels,
        0.001, // 0.1% taker fee
        None,  // no historical win rate yet
        bot_cfg.sizing.kelly_fraction,
        bot_cfg.sizing.min_order_notional,
    );

    // Convert quote-per-level to base-per-level
    let qty_per_level = if scan.last_price > 0.0 {
        sizing_rec.qty_per_level / scan.last_price
    } else {
        0.0
    };

    info!(
        symbol = %symbol,
        range = format!("{:.4} - {:.4}", lower, upper),
        levels = num_levels,
        qty_per_level = format!("{:.6}", qty_per_level),
        allocation = format!("{:.2}", sizing_rec.total_allocation),
        reason = %sizing_rec.reason,
        "Grid parameters calculated",
    );

    // Build grid config
    let grid_cfg = crate::config::GridCfg {
        symbol: symbol.clone(),
        lower_price: lower,
        upper_price: upper,
        levels: num_levels,
        qty_per_level_base: qty_per_level,
        poll_secs: bot_cfg.grid.poll_secs,
        cancel_existing_on_start: true,
        auto_rebalance: true,
        trailing_profit_threshold: 0.0,
        cancel_buys_on_trend: bot_cfg.grid.cancel_buys_on_trend,
        auto_recenter: bot_cfg.grid.auto_recenter,
        recenter_threshold_pct: bot_cfg.grid.recenter_threshold_pct,
        use_post_only: bot_cfg.grid.use_post_only,
        vwap_entry_filter: bot_cfg.grid.vwap_entry_filter,
        vwap_threshold_pct: bot_cfg.grid.vwap_threshold_pct,
        venue: bot_cfg.grid.venue.clone(),
        respect_market_hours: bot_cfg.grid.respect_market_hours,
    };

    let mut g = Grid::new(
        &grid_cfg,
        tick_size.clone(),
        base_precision.clone(),
        min_qty,
        min_amt,
    );

    if let Err(msg) = g.validate_sizing() {
        return Err(anyhow!("grid sizing invalid for {}: {}", symbol, msg));
    }

    // Cancel any existing orders on this symbol
    if !*dry_run {
        let _ = client.cancel_all_spot(symbol).await;
    }

    // Place initial grid
    let mid = scan.last_price;
    let plan = g.plan_initial(mid);
    info!(symbol = %symbol, plan_size = plan.len(), "Placing initial grid orders");

    for p in &plan {
        let price: f64 = p.price_str.parse().unwrap_or(0.0);
        let qty: f64 = p.qty_str.parse().unwrap_or(0.0);
        if price <= 0.0 || qty <= 0.0 {
            continue;
        }
        let notional = price * qty;

        {
            let guard = engine.lock().await;
            if let Err(rej) = guard.pretrade_check(Market::Crypto, symbol, "grid", notional) {
                warn!(symbol = %symbol, level = p.level_index, ?rej, "risk rejected");
                continue;
            }
        }

        let link_id = format!(
            "ap-{}-{}-{}",
            symbol,
            p.level_index,
            uuid::Uuid::new_v4().as_simple()
        );

        if *dry_run {
            let fake_id = format!("DRY-{}", uuid::Uuid::new_v4().as_simple());
            g.orders.insert(
                p.level_index,
                LevelOrder {
                    order_id: fake_id,
                    side: p.side,
                    price_str: p.price_str.clone(),
                    qty_str: p.qty_str.clone(),
                    filled_qty: 0.0,
                    target_qty: p.qty_str.parse().unwrap_or(0.0),
                },
            );
        } else {
            let qty_str = p.qty_str.clone();
            let price_str = p.price_str.clone();
            let link_clone = link_id.clone();
            match with_retry(retry_policy, "place_order", || {
                client.place_spot_order(
                    symbol,
                    p.side,
                    "Limit",
                    &qty_str,
                    Some(&price_str),
                    Some(&link_clone),
                    false,
                )
            })
            .await
            {
                Ok(resp) => {
                    g.orders.insert(
                        p.level_index,
                        LevelOrder {
                            order_id: resp.order_id.clone(),
                            side: p.side,
                            price_str: p.price_str.clone(),
                            qty_str: p.qty_str.clone(),
                            filled_qty: 0.0,
                            target_qty: p.qty_str.parse().unwrap_or(0.0),
                        },
                    );
                    let _ = db.lock().await.insert_order(&OrderRow {
                        order_id: resp.order_id,
                        order_link_id: link_id,
                        symbol: symbol.to_string(),
                        side: p.side,
                        price,
                        qty,
                        level_index: p.level_index,
                        status: OrderStatus::Open,
                        placed_at: Utc::now(),
                        filled_at: None,
                        filled_qty: 0.0,
                        filled_price: 0.0,
                    });
                    engine
                        .lock()
                        .await
                        .on_fill(Market::Crypto, symbol, "grid", notional);
                    breaker.record_success();
                }
                Err(e) => {
                    breaker.record_failure();
                    warn!(symbol = %symbol, level = p.level_index, err = %e, "order placement failed");
                }
            }
        }
    }

    alerter
        .lifecycle(&format!(
            "🟢 Grid deployed: {} | {:.4}–{:.4} | {} levels | {:.2} USDT allocated",
            symbol, lower, upper, num_levels, sizing_rec.total_allocation,
        ))
        .await;

    Ok(ActiveGrid {
        symbol: symbol.clone(),
        grid: g,
        score: scan.score,
        tick_size,
        base_precision,
        started_at: Utc::now(),
        total_pnl: 0.0,
    })
}

/// Reconcile fills for one active grid (detect fills, place replacements).
async fn reconcile_grid(
    client: &bybit::rest::Client,
    ag: &mut ActiveGrid,
    engine: &Arc<Mutex<RiskEngine>>,
    db: &Arc<Mutex<Db>>,
    alerter: &Arc<Alerter>,
    retry_policy: &RetryPolicy,
    _breaker: &Arc<CircuitBreaker>,
) -> Result<()> {
    let symbol = &ag.symbol;
    let open = with_retry(retry_policy, "open_orders", || {
        client.open_orders_spot(symbol)
    })
    .await?;

    let open_ids: std::collections::HashSet<String> =
        open.iter().map(|o| o.order_id.clone()).collect();

    let mut filled: Vec<(usize, Side, f64, String, f64, f64)> = Vec::new();
    ag.grid.orders.retain(|&level, o| {
        if open_ids.contains(&o.order_id) {
            true
        } else {
            let price: f64 = o.price_str.parse().unwrap_or(0.0);
            let qty: f64 = o.qty_str.parse().unwrap_or(0.0);
            if price > 0.0 && qty > 0.0 {
                filled.push((level, o.side, price * qty, o.order_id.clone(), price, qty));
            }
            false
        }
    });

    let grid_spacing = if ag.grid.levels.len() >= 2 {
        ag.grid.levels[1] - ag.grid.levels[0]
    } else {
        0.0
    };

    for (level, side, notional, order_id, fill_price, fill_qty) in filled {
        let pnl = match side {
            Side::Sell => grid_spacing * fill_qty,
            Side::Buy => 0.0,
        };
        ag.total_pnl += pnl;

        info!(symbol = %symbol, level, ?side, pnl, total_pnl = ag.total_pnl, "fill");

        {
            let db_guard = db.lock().await;
            let _ = db_guard.mark_filled(&order_id, fill_price, fill_qty);
            let _ = db_guard.insert_fill(&FillRow {
                order_id: order_id.clone(),
                symbol: symbol.clone(),
                side,
                price: fill_price,
                qty: fill_qty,
                notional,
                realised_pnl: pnl,
                filled_at: Utc::now(),
            });
        }

        let side_str = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        alerter
            .fill(symbol, side_str, fill_price, fill_qty, pnl)
            .await;
        engine
            .lock()
            .await
            .on_fill(Market::Crypto, symbol, "grid", -notional);

        // Place replacement
        if let Some(plan) = ag.grid.plan_replacement(level, side) {
            let price: f64 = plan.price_str.parse().unwrap_or(0.0);
            let qty: f64 = plan.qty_str.parse().unwrap_or(0.0);
            if price <= 0.0 || qty <= 0.0 {
                continue;
            }
            let notional = price * qty;
            {
                let guard = engine.lock().await;
                if guard
                    .pretrade_check(Market::Crypto, symbol, "grid", notional)
                    .is_err()
                {
                    continue;
                }
            }

            let link_id = format!(
                "ap-{}-{}-{}",
                symbol,
                plan.level_index,
                uuid::Uuid::new_v4().as_simple()
            );
            let qty_str = plan.qty_str.clone();
            let price_str = plan.price_str.clone();
            let link_clone = link_id.clone();

            match with_retry(retry_policy, "place_replacement", || {
                client.place_spot_order(
                    symbol,
                    plan.side,
                    "Limit",
                    &qty_str,
                    Some(&price_str),
                    Some(&link_clone),
                    false,
                )
            })
            .await
            {
                Ok(resp) => {
                    ag.grid.orders.insert(
                        plan.level_index,
                        LevelOrder {
                            order_id: resp.order_id.clone(),
                            side: plan.side,
                            price_str: plan.price_str.clone(),
                            qty_str: plan.qty_str.clone(),
                            filled_qty: 0.0,
                            target_qty: plan.qty_str.parse().unwrap_or(0.0),
                        },
                    );
                    let _ = db.lock().await.insert_order(&OrderRow {
                        order_id: resp.order_id,
                        order_link_id: link_id,
                        symbol: symbol.clone(),
                        side: plan.side,
                        price,
                        qty,
                        level_index: plan.level_index,
                        status: OrderStatus::Open,
                        placed_at: Utc::now(),
                        filled_at: None,
                        filled_qty: 0.0,
                        filled_price: 0.0,
                    });
                    engine
                        .lock()
                        .await
                        .on_fill(Market::Crypto, symbol, "grid", notional);
                }
                Err(e) => {
                    warn!(symbol = %symbol, level = plan.level_index, err = %e, "replacement failed");
                }
            }
        }
    }

    Ok(())
}

/// Evaluate scan results and switch grids if better opportunities exist.
async fn evaluate_and_switch(
    ctx: &ApCtx<'_>,
    active_grids: &mut HashMap<String, ActiveGrid>,
    scan_results: &[ScanResult],
    equity: f64,
) {
    let suitable: Vec<&ScanResult> = scan_results
        .iter()
        .filter(|r| r.analysis.grid_suitable)
        .collect();

    if suitable.is_empty() {
        info!("No suitable pairs found in scan — keeping current grids");
        return;
    }

    // Check if any current grid should be wound down
    let mut to_remove: Vec<String> = Vec::new();
    for (symbol, ag) in active_grids.iter() {
        // Check if this symbol is still suitable
        let still_suitable = scan_results
            .iter()
            .any(|r| r.symbol == *symbol && r.analysis.grid_suitable);

        if !still_suitable {
            info!(symbol = %symbol, "Symbol no longer suitable — marking for wind-down");
            to_remove.push(symbol.clone());
            continue;
        }

        // Check if a MUCH better opportunity exists
        let current_score = ag.score;
        if let Some(best) = suitable.first()
            && best.symbol != *symbol
            && best.score > current_score + ctx.ap_cfg.switch_threshold
            && active_grids.len() >= ctx.ap_cfg.max_grids
        {
            // Find the worst performing grid
            let worst = active_grids.iter().min_by(|a, b| {
                a.1.score
                    .partial_cmp(&b.1.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some((worst_sym, _)) = worst
                && worst_sym == symbol
            {
                info!(
                    symbol = %symbol,
                    old_score = current_score,
                    new_symbol = %best.symbol,
                    new_score = best.score,
                    "Better opportunity found — winding down",
                );
                to_remove.push(symbol.clone());
            }
        }
    }

    // Wind down marked grids
    for symbol in &to_remove {
        if let Some(mut ag) = active_grids.remove(symbol) {
            info!(symbol = %symbol, pnl = ag.total_pnl, "Winding down grid");
            if !ctx.dry_run
                && let Err(e) = ctx.client.cancel_all_spot(symbol).await
            {
                warn!(symbol = %symbol, err = %e, "cancel_all failed during wind-down");
            }
            for (_, order) in ag.grid.orders.drain() {
                let _ = ctx.db.lock().await.mark_cancelled(&order.order_id);
            }
            ctx.alerter
                .lifecycle(&format!(
                    "🔴 Grid wound down: {} | PnL: {:+.2}",
                    symbol, ag.total_pnl,
                ))
                .await;
        }
    }

    // Deploy new grids if we have capacity
    let available_slots = ctx.ap_cfg.max_grids.saturating_sub(active_grids.len());
    if available_slots > 0 {
        let new_candidates: Vec<&ScanResult> = suitable
            .iter()
            .filter(|r| !active_grids.contains_key(&r.symbol))
            .take(available_slots)
            .copied()
            .collect();

        for candidate in new_candidates {
            match deploy_grid(ctx, candidate, equity).await {
                Ok(ag) => {
                    info!(symbol = %ag.symbol, score = ag.score, "New grid deployed via auto-switch");
                    active_grids.insert(ag.symbol.clone(), ag);
                }
                Err(e) => {
                    warn!(symbol = %candidate.symbol, err = %e, "Failed to deploy replacement grid");
                }
            }
        }
    }
}
