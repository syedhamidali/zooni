//! Bot orchestration: wires Bybit REST + Grid + Risk + persistence + alerts
//! + retry + circuit breaker + dry-run + graceful shutdown + PnL tracking.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{Datelike, Utc};
use tokio::sync::Mutex;
use tokio::time;
use tracing::{error, info, warn};

use crate::alerts::Alerter;
use crate::config::BotConfig;
use crate::grid::{Grid, LevelOrder, PlannedOrder};
use crate::metrics::Metrics;
use crate::persistence::{Db, FillRow, OrderRow, OrderStatus};
use crate::retry::{CircuitBreaker, RetryPolicy, with_retry};
use crate::risk::{Market, RiskConfig, RiskEngine};
use crate::telegram::TelegramBot;
use crate::tui::{GridLevelDisplay, TuiState};
use crate::types::Side;
use crate::venues::bybit;
use crate::ws_private::{self, WsEvent};

/// Shared context passed to helper functions instead of many individual args.
struct BotCtx<'a> {
    client: &'a bybit::rest::Client,
    engine: &'a Arc<Mutex<RiskEngine>>,
    symbol: &'a str,
    db: &'a Arc<Mutex<Db>>,
    alerter: &'a Arc<Alerter>,
    retry_policy: &'a RetryPolicy,
    breaker: &'a Arc<CircuitBreaker>,
    dry_run: bool,
    metrics: &'a Arc<Metrics>,
}

/// Run the bot, optionally sharing TUI state for dashboard rendering.
pub async fn run(cfg: BotConfig) -> Result<()> {
    run_inner(cfg, None).await
}

/// Run the bot with a shared TUI state (used by `--tui` mode).
pub async fn run_with_shared_tui(cfg: BotConfig, tui: Arc<Mutex<TuiState>>) -> Result<()> {
    run_inner(cfg, Some(tui)).await
}

async fn run_inner(cfg: BotConfig, external_tui: Option<Arc<Mutex<TuiState>>>) -> Result<()> {
    let dry_run = cfg.bot.dry_run;
    if dry_run {
        info!("*** DRY-RUN MODE — no orders will be sent to the exchange ***");
    }

    // -- 1. Persistence -------------------------------------------------------
    let db = if dry_run {
        Arc::new(Mutex::new(Db::open_in_memory()?))
    } else {
        Arc::new(Mutex::new(Db::open(&cfg.bot.db_path)?))
    };
    info!(db_path = %cfg.bot.db_path, dry_run, "persistence ready");

    // -- 2. Alerts ------------------------------------------------------------
    let alerter = Arc::new(Alerter::new(
        cfg.alerts.clone(),
        format!("grid-{}", cfg.grid.symbol),
    ));
    alerter
        .lifecycle(&format!(
            "Bot starting: {} levels={} range={:.2}–{:.2} qty={} dry_run={}",
            cfg.grid.symbol,
            cfg.grid.levels,
            cfg.grid.lower_price,
            cfg.grid.upper_price,
            cfg.grid.qty_per_level_base,
            dry_run,
        ))
        .await;

    // -- 3. Risk engine + kill switch -----------------------------------------
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

    // -- 4. Ctrl+C + SIGTERM → trip kill switch --------------------------------
    {
        let kill = kill.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("SIGINT received, tripping kill switch");
                kill.store(true, Ordering::SeqCst);
            }
        });
    }

    // -- 4b. Metrics HTTP server ------------------------------------------------
    let metrics = Arc::new(Metrics::new());
    metrics.set_equity(cfg.risk.starting_equity_quote);
    {
        let m = metrics.clone();
        tokio::spawn(async move {
            crate::metrics::serve(m, 9090).await;
        });
    }

    // -- 4c. Telegram bot -------------------------------------------------------
    if let Some(tg) = TelegramBot::from_env() {
        let tg_kill = kill.clone();
        let tg_symbol = cfg.grid.symbol.clone();
        let tg_metrics = metrics.clone();
        let status_fn = Arc::new(move || -> String {
            let m = &tg_metrics;
            let equity = m.equity_cents.load(Ordering::Relaxed) as f64 / 100.0;
            let pnl = m.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0;
            let fills = m.fills_total.load(Ordering::Relaxed);
            let open = m.open_orders.load(Ordering::Relaxed);
            let buys = m.buy_fills.load(Ordering::Relaxed);
            let sells = m.sell_fills.load(Ordering::Relaxed);
            let uptime_start = m.uptime_start.load(Ordering::Relaxed);
            let uptime = chrono::Utc::now().timestamp() - uptime_start;
            let hours = uptime / 3600;
            let mins = (uptime % 3600) / 60;
            format!(
                "📊 *{} Status*\n\
                 💰 Equity: {:.2} USDT\n\
                 📈 P&L: {:+.2} USDT\n\
                 🔄 Fills: {} ({}B / {}S)\n\
                 📋 Open orders: {}\n\
                 ⏱ Uptime: {}h {}m\n\
                 🌐 Dashboard: http://localhost:9090/status",
                tg_symbol, equity, pnl, fills, buys, sells, open, hours, mins,
            )
        });

        let tg_symbol2 = cfg.grid.symbol.clone();
        let tg_metrics2 = metrics.clone();
        let pnl_fn = Arc::new(move || -> String {
            let m = &tg_metrics2;
            let pnl = m.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0;
            let fills = m.fills_total.load(Ordering::Relaxed);
            let buys = m.buy_fills.load(Ordering::Relaxed);
            let sells = m.sell_fills.load(Ordering::Relaxed);
            format!(
                "💹 *{} P&L*\n\
                 Total: {:+.2} USDT\n\
                 Fills: {} total ({} buys, {} sells)\n\
                 Avg per fill: {:.4} USDT",
                tg_symbol2,
                pnl,
                fills,
                buys,
                sells,
                if fills > 0 { pnl / fills as f64 } else { 0.0 },
            )
        });

        tokio::spawn(async move {
            tg.poll_loop(tg_kill, Duration::from_secs(5), status_fn, pnl_fn)
                .await;
        });
        info!("Telegram bot started");
    }

    // -- 4d. TUI shared state (updated each tick for external consumers) ------
    let tui_state = external_tui.unwrap_or_else(|| Arc::new(Mutex::new(TuiState::new(&cfg))));

    // -- 5. Retry policy + circuit breaker ------------------------------------
    let retry_policy = RetryPolicy {
        max_retries: cfg.bot.max_retries,
        ..RetryPolicy::default()
    };
    let breaker = Arc::new(CircuitBreaker::new(
        cfg.bot.circuit_breaker_threshold,
        Duration::from_secs(cfg.bot.circuit_breaker_cooldown_secs),
    ));

    // -- 6. Bybit client -------------------------------------------------------
    let client = if dry_run {
        // In dry-run, create client with dummy creds — we won't call the exchange.
        let bybit_cfg = bybit::Config {
            credentials: Some(bybit::Credentials {
                api_key: "DRY_RUN".to_string(),
                api_secret: "DRY_RUN".to_string(),
            }),
            testnet: true,
            recv_window_ms: cfg.bybit.recv_window_ms,
        };
        bybit::rest::Client::new(bybit_cfg)
    } else {
        let bybit_cfg = bybit::Config {
            credentials: Some(bybit::Credentials {
                api_key: cfg.bybit.api_key.clone(),
                api_secret: cfg.bybit.api_secret.clone(),
            }),
            testnet: cfg.bybit.testnet,
            recv_window_ms: cfg.bybit.recv_window_ms,
        };
        bybit::rest::Client::new(bybit_cfg)
    };
    info!(
        testnet = cfg.bybit.testnet,
        base = client.base_url(),
        "Bybit REST client ready"
    );

    // -- 6b. Private WebSocket for instant fill detection ----------------------
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
    if !dry_run {
        let ws_api_key = cfg.bybit.api_key.clone();
        let ws_api_secret = cfg.bybit.api_secret.clone();
        let ws_testnet = cfg.bybit.testnet;
        let ws_tx_clone = ws_tx.clone();
        tokio::spawn(async move {
            ws_private::start(ws_api_key, ws_api_secret, ws_testnet, ws_tx_clone).await;
        });
        info!("private WebSocket listener spawned");
    }

    // -- 7. Clock sync check ---------------------------------------------------
    if !dry_run {
        check_clock_sync(&client).await;
    }

    // -- 8. Credential check + wallet balance ---------------------------------
    if !dry_run {
        match with_retry(&retry_policy, "wallet_balance", || {
            client.wallet_balance(&cfg.bybit.account_type)
        })
        .await
        {
            Ok(accounts) => {
                for a in &accounts {
                    for c in &a.coin {
                        if !c.wallet_balance.is_empty() && c.wallet_balance != "0" {
                            info!(
                                account = %a.account_type,
                                coin = %c.coin,
                                balance = %c.wallet_balance,
                                "balance",
                            );
                        }
                    }
                }
                // Update equity from actual USDT balance
                for a in &accounts {
                    for c in &a.coin {
                        if (c.coin == "USDT" || c.coin == "USD")
                            && let Ok(bal) = c.wallet_balance.parse::<f64>()
                        {
                            let mut guard = engine.lock().await;
                            guard.set_equity(bal);
                            info!(equity = bal, "equity updated from wallet");
                        }
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!(
                    "wallet balance check failed — credentials or account type likely wrong: {}",
                    e
                ));
            }
        }
    }

    // -- 9. Instrument info + current market ----------------------------------
    let (tick_size, base_precision, min_qty, min_amt, mid) = if dry_run {
        // Use sensible defaults for dry-run
        let mid = (cfg.grid.lower_price + cfg.grid.upper_price) / 2.0;
        info!(mid, "dry-run: using midpoint of grid range as market price");
        ("0.01".to_string(), "0.000001".to_string(), 0.0001, 5.0, mid)
    } else {
        let info = with_retry(&retry_policy, "instrument_info", || {
            client.instrument_info_spot(&cfg.grid.symbol)
        })
        .await
        .context("instrument info fetch failed")?;

        let ticker = with_retry(&retry_policy, "ticker", || {
            client.ticker_spot(&cfg.grid.symbol)
        })
        .await
        .context("ticker fetch failed")?;

        let last_price: f64 = ticker.last_price.parse().context("parse last_price")?;
        let bid: f64 = ticker.bid1_price.parse().unwrap_or(last_price);
        let ask: f64 = ticker.ask1_price.parse().unwrap_or(last_price);
        let mid = if bid > 0.0 && ask > 0.0 {
            (bid + ask) / 2.0
        } else {
            last_price
        };
        info!(symbol = %cfg.grid.symbol, last = last_price, bid, ask, mid, "market snapshot");

        let min_qty: f64 = info.lot_size_filter.min_order_qty.parse().unwrap_or(0.0);
        let min_amt: f64 = info.lot_size_filter.min_order_amt.parse().unwrap_or(0.0);
        (
            info.price_filter.tick_size,
            info.lot_size_filter.base_precision,
            min_qty,
            min_amt,
            mid,
        )
    };

    // -- 10. Build grid -------------------------------------------------------
    let mut grid = Grid::new(
        &cfg.grid,
        tick_size.clone(),
        base_precision.clone(),
        min_qty,
        min_amt,
    );
    if let Err(msg) = grid.validate_sizing() {
        return Err(anyhow!("grid sizing invalid: {}", msg));
    }
    info!(
        levels = grid.levels.len(),
        lower = grid.levels.first().copied().unwrap_or_default(),
        upper = grid.levels.last().copied().unwrap_or_default(),
        tick_size = %tick_size,
        base_precision = %base_precision,
        min_qty,
        min_amt,
        "grid built",
    );

    if mid < *grid.levels.first().unwrap_or(&0.0) || mid > *grid.levels.last().unwrap_or(&0.0) {
        warn!(
            mid,
            "current price outside grid band — consider reconfiguring"
        );
    }

    // -- 11. Position reconciliation on startup --------------------------------
    if !dry_run {
        let db_orders = db.lock().await.open_orders(&cfg.grid.symbol)?;
        if !db_orders.is_empty() {
            info!(
                count = db_orders.len(),
                "found persisted orders, reconciling with exchange"
            );
            let exchange_open = with_retry(&retry_policy, "open_orders_startup", || {
                client.open_orders_spot(&cfg.grid.symbol)
            })
            .await
            .unwrap_or_default();
            let exchange_ids: HashSet<String> =
                exchange_open.iter().map(|o| o.order_id.clone()).collect();

            for dbo in &db_orders {
                if exchange_ids.contains(&dbo.order_id) {
                    // Still open on exchange — restore into grid
                    grid.orders.insert(
                        dbo.level_index,
                        LevelOrder {
                            order_id: dbo.order_id.clone(),
                            side: dbo.side,
                            price_str: crate::grid::format_to_increment(dbo.price, &tick_size),
                            qty_str: crate::grid::format_to_increment(dbo.qty, &base_precision),
                        },
                    );
                    info!(level = dbo.level_index, order_id = %dbo.order_id, "restored open order");
                } else {
                    // Was in DB as open but not on exchange — must have filled while we were down
                    db.lock()
                        .await
                        .mark_filled(&dbo.order_id, dbo.price, dbo.qty)?;
                    info!(level = dbo.level_index, order_id = %dbo.order_id, "order filled while offline");
                }
            }
            info!(
                restored = grid.orders.len(),
                "startup reconciliation complete"
            );
        }
    }

    // -- 12. Optional pre-clear of existing orders ----------------------------
    if cfg.grid.cancel_existing_on_start && grid.orders.is_empty() && !dry_run {
        info!("cancel_existing_on_start = true; clearing existing orders on symbol");
        if let Err(e) = client.cancel_all_spot(&cfg.grid.symbol).await {
            warn!(err = %e, "cancel_all_spot failed (likely no orders to cancel)");
        }
    }

    // -- 13. Build shared context for grid helpers --------------------------------
    let ctx = BotCtx {
        client: &client,
        engine: &engine,
        symbol: &cfg.grid.symbol,
        db: &db,
        alerter: &alerter,
        retry_policy: &retry_policy,
        breaker: &breaker,
        dry_run,
        metrics: &metrics,
    };

    // -- 13b. Place initial grid (only for levels not already occupied) ---------
    if grid.orders.is_empty() {
        let plan = grid.plan_initial(mid);
        info!(plan_size = plan.len(), "placing initial grid");
        for p in &plan {
            if kill.load(Ordering::SeqCst) {
                break;
            }
            place_with_risk(&ctx, &mut grid, p).await;
        }
        info!(open_count = grid.orders.len(), "initial placement complete");
    } else {
        // Fill gaps in the grid that aren't occupied
        let plan = grid.plan_initial(mid);
        let occupied: HashSet<usize> = grid.orders.keys().copied().collect();
        let gaps: Vec<_> = plan
            .into_iter()
            .filter(|p| !occupied.contains(&p.level_index))
            .collect();
        if !gaps.is_empty() {
            info!(gap_count = gaps.len(), "filling gaps in restored grid");
            for p in &gaps {
                if kill.load(Ordering::SeqCst) {
                    break;
                }
                place_with_risk(&ctx, &mut grid, p).await;
            }
        }
    }

    // -- 14. Feed initial PnL into risk engine --------------------------------
    {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let daily = db.lock().await.daily_pnl(&today)?;
        let now = Utc::now();
        let weekday = now.weekday().num_days_from_monday();
        let week_start = (now - chrono::Duration::days(weekday as i64))
            .format("%Y-%m-%d")
            .to_string();
        let week_end = (now + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let weekly = db.lock().await.weekly_pnl(&week_start, &week_end)?;
        let mut guard = engine.lock().await;
        guard.set_daily_pnl(daily);
        guard.set_weekly_pnl(weekly);
        info!(
            daily_pnl = daily,
            weekly_pnl = weekly,
            "PnL loaded from database"
        );
    }

    // -- 15. Main loop ---------------------------------------------------------
    let mut tick = time::interval(Duration::from_secs(cfg.grid.poll_secs));
    tick.tick().await; // skip immediate first tick
    let mut pnl_report_counter: u64 = 0;
    let pnl_report_interval = if cfg.alerts.pnl_summary_interval_secs > 0 {
        cfg.alerts.pnl_summary_interval_secs / cfg.grid.poll_secs
    } else {
        0
    };

    // Regime check every ~5 minutes (in ticks)
    let regime_check_interval = (300 / cfg.grid.poll_secs).max(1);
    let mut regime_check_counter: u64 = 0;
    let mut regime_paused = false; // true if grid is paused due to unfavorable regime

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if kill.load(Ordering::SeqCst) {
                    info!("kill switch tripped; exiting main loop");
                    break;
                }

                // Circuit breaker check
                if breaker.is_open() {
                    warn!("circuit breaker open — skipping reconciliation tick");
                    continue;
                }

                // Periodic regime detection — auto-pause during unfavorable conditions
                if !dry_run {
                    regime_check_counter += 1;
                    if regime_check_counter >= regime_check_interval {
                        regime_check_counter = 0;
                        match client.klines(&cfg.grid.symbol, "60", 100).await {
                            Ok(klines) => {
                                if let Some(analysis) = crate::regime::analyze(&klines) {
                                    info!(
                                        regime = %analysis.regime,
                                        adx = format!("{:.1}", analysis.adx),
                                        rsi = format!("{:.1}", analysis.rsi),
                                        vol = format!("{:.2}%", analysis.volatility_pct),
                                        grid_ok = analysis.grid_suitable,
                                        "regime check",
                                    );
                                    // Update dashboard with regime info
                                    {
                                        let mut series = metrics.series.lock().await;
                                        series.regime = analysis.regime.to_string();
                                        series.regime_confidence = analysis.confidence;
                                        series.grid_suitable = analysis.grid_suitable;
                                    }
                                    if !analysis.grid_suitable && !regime_paused {
                                        regime_paused = true;
                                        warn!(regime = %analysis.regime, "unfavorable regime — pausing new orders");
                                        alerter
                                            .lifecycle(&format!(
                                                "⚠️ Regime: {} (ADX={:.1}, RSI={:.1}) — pausing new placements",
                                                analysis.regime, analysis.adx, analysis.rsi,
                                            ))
                                            .await;
                                    } else if analysis.grid_suitable && regime_paused {
                                        regime_paused = false;
                                        info!(regime = %analysis.regime, "favorable regime resumed — resuming orders");
                                        alerter
                                            .lifecycle(&format!(
                                                "✅ Regime: {} — resuming grid trading",
                                                analysis.regime,
                                            ))
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(err = %e, "regime check klines fetch failed");
                            }
                        }
                    }
                }

                match reconcile_once(&ctx, &mut grid, regime_paused).await {
                    Ok(_) => breaker.record_success(),
                    Err(e) => {
                        breaker.record_failure();
                        warn!(err = %e, failures = breaker.failure_count(), "reconcile iteration failed");
                        alerter.error(&format!("reconcile failed: {e}")).await;
                    }
                }

                // Update metrics + dashboard series with current state
                metrics.set_open_orders(grid.orders.len() as u64);
                {
                    let db_guard = db.lock().await;
                    if let Ok(pnl) = db_guard.total_realised_pnl(&cfg.grid.symbol) {
                        metrics.set_pnl(pnl);
                        metrics.series.lock().await.push_pnl(pnl);
                    }
                }
                // Update dashboard grid levels
                {
                    let mut series = metrics.series.lock().await;
                    series.symbol = cfg.grid.symbol.clone();
                    series.grid_levels = grid
                        .levels
                        .iter()
                        .enumerate()
                        .map(|(i, &price)| {
                            let (side, status) = if let Some(order) = grid.orders.get(&i) {
                                let s = match order.side {
                                    Side::Buy => "BUY",
                                    Side::Sell => "SELL",
                                };
                                (s.to_string(), "OPEN".to_string())
                            } else {
                                (String::new(), String::new())
                            };
                            crate::metrics::GridLevel {
                                index: i,
                                price,
                                side,
                                status,
                            }
                        })
                        .collect();
                }

                // Periodic equity + PnL refresh
                if !dry_run {
                    if let Ok(accounts) = client.wallet_balance(&cfg.bybit.account_type).await {
                        for a in &accounts {
                            for c in &a.coin {
                                if (c.coin == "USDT" || c.coin == "USD")
                                    && !c.wallet_balance.is_empty()
                                    && let Ok(bal) = c.wallet_balance.parse::<f64>()
                                {
                                    let mut guard = engine.lock().await;
                                    guard.set_equity(bal);
                                    metrics.set_equity(bal);
                                    metrics.series.lock().await.push_equity(bal);
                                }
                            }
                        }
                    }

                    // Refresh daily/weekly PnL — batch db locks
                    let today = Utc::now().format("%Y-%m-%d").to_string();
                    let now = Utc::now();
                    let weekday = now.weekday().num_days_from_monday();
                    let week_start = (now - chrono::Duration::days(weekday as i64))
                        .format("%Y-%m-%d")
                        .to_string();
                    let week_end = (now + chrono::Duration::days(1))
                        .format("%Y-%m-%d")
                        .to_string();
                    {
                        let db_guard = db.lock().await;
                        if let Ok(daily) = db_guard.daily_pnl(&today) {
                            if let Ok(weekly) = db_guard.weekly_pnl(&week_start, &week_end) {
                                let mut guard = engine.lock().await;
                                guard.set_daily_pnl(daily);
                                guard.set_weekly_pnl(weekly);
                            }
                        }
                    }
                }

                // Periodic P&L summary alert — batch db locks
                if pnl_report_interval > 0 {
                    pnl_report_counter += 1;
                    if pnl_report_counter >= pnl_report_interval {
                        pnl_report_counter = 0;
                        let today = Utc::now().format("%Y-%m-%d").to_string();
                        let (daily, total, fills) = {
                            let db_guard = db.lock().await;
                            let d = db_guard.daily_pnl(&today).unwrap_or(0.0);
                            let t = db_guard.total_realised_pnl(&cfg.grid.symbol).unwrap_or(0.0);
                            let f = db_guard.total_fill_count(&cfg.grid.symbol).unwrap_or(0);
                            (d, t, f)
                        };
                        let equity = engine.lock().await.equity();
                        alerter
                            .pnl_summary(&cfg.grid.symbol, daily, total, fills, equity)
                            .await;
                    }
                }

                // Update TUI state for dashboard consumers — batch locks
                {
                    let equity_val = engine.lock().await.equity();
                    let today_str = Utc::now().format("%Y-%m-%d").to_string();
                    let (total_pnl_val, daily_pnl_val) = {
                        let db_guard = db.lock().await;
                        let tp = db_guard.total_realised_pnl(&cfg.grid.symbol).unwrap_or(0.0);
                        let dp = db_guard.daily_pnl(&today_str).unwrap_or(0.0);
                        (tp, dp)
                    };

                    let mut ts = tui_state.lock().await;
                    ts.open_orders = grid.orders.len() as u64;
                    ts.total_fills = metrics.fills_total.load(Ordering::Relaxed);
                    ts.equity = equity_val;
                    ts.total_pnl = total_pnl_val;
                    ts.daily_pnl = daily_pnl_val;
                    ts.circuit_breaker_open = breaker.is_open();

                    // Update grid level display
                    ts.grid_state = grid
                        .levels
                        .iter()
                        .enumerate()
                        .map(|(i, &price)| {
                            let (side, status) = if let Some(order) = grid.orders.get(&i) {
                                let s = match order.side {
                                    Side::Buy => "BUY",
                                    Side::Sell => "SELL",
                                };
                                (s.to_string(), "OPEN".to_string())
                            } else {
                                ("---".to_string(), "---".to_string())
                            };
                            GridLevelDisplay {
                                level: i,
                                price,
                                side,
                                status,
                                is_current: false,
                            }
                        })
                        .collect();

                    // Mark level closest to current price
                    if ts.current_price > 0.0 {
                        let mut closest = 0;
                        let mut min_dist = f64::MAX;
                        for (i, g) in ts.grid_state.iter().enumerate() {
                            let dist = (g.price - ts.current_price).abs();
                            if dist < min_dist {
                                min_dist = dist;
                                closest = i;
                            }
                        }
                        if let Some(g) = ts.grid_state.get_mut(closest) {
                            g.is_current = true;
                        }
                    }
                }

                // Fetch current price for TUI + dashboard + rebalancing
                if !dry_run
                    && let Ok(ticker) = client.ticker_spot(&cfg.grid.symbol).await
                    && let Ok(price) = ticker.last_price.parse::<f64>()
                {
                    let mut series = metrics.series.lock().await;
                    series.push_price(price);
                    drop(series);
                    tui_state.lock().await.current_price = price;
                }

                // Dynamic grid rebalancing
                if cfg.grid.auto_rebalance
                    && !dry_run
                    && let Ok(ticker) = client.ticker_spot(&cfg.grid.symbol).await
                    && let Ok(price) = ticker.last_price.parse::<f64>()
                {
                    let lower = *grid.levels.first().unwrap_or(&0.0);
                    let upper = *grid.levels.last().unwrap_or(&0.0);
                    let range = upper - lower;
                    // If price is within 10% of either edge, rebalance
                    if price < lower + range * 0.10 || price > upper - range * 0.10 {
                        info!(
                            price,
                            lower, upper, "price near grid edge — rebalancing grid",
                        );
                        // Cancel all and re-place around new mid
                        if let Err(e) = client.cancel_all_spot(&cfg.grid.symbol).await {
                            warn!(err = %e, "cancel_all for rebalance failed");
                        } else {
                            // Mark all orders cancelled in DB
                            {
                                let db_guard = db.lock().await;
                                for (_, order) in grid.orders.drain() {
                                    let _ = db_guard.mark_cancelled(&order.order_id);
                                }
                            }
                            // Rebuild grid centered on current price
                            let new_lower = price - range / 2.0;
                            let new_upper = price + range / 2.0;
                            let new_levels = crate::grid::compute_levels(
                                new_lower.max(0.01),
                                new_upper,
                                grid.levels.len(),
                            );
                            grid.levels = new_levels;
                            let plan = grid.plan_initial(price);
                            info!(
                                new_lower = new_lower.max(0.01),
                                new_upper,
                                plan_size = plan.len(),
                                "rebalanced grid",
                            );
                            for p in &plan {
                                if kill.load(Ordering::SeqCst) {
                                    break;
                                }
                                place_with_risk(&ctx, &mut grid, p).await;
                            }
                            alerter
                                .lifecycle(&format!(
                                    "Grid rebalanced around {price:.2}: {:.2}–{:.2}",
                                    new_lower.max(0.01),
                                    new_upper
                                ))
                                .await;
                        }
                    }
                }
            }

            event = ws_rx.recv() => {
                let Some(event) = event else { continue };
                match event {
                    WsEvent::OrderFilled {
                        order_id,
                        symbol: _ws_symbol,
                        side,
                        price: fill_price,
                        qty: fill_qty,
                        status,
                        ..
                    } => {
                        // Find which grid level this order belongs to
                        let level_entry = grid.orders.iter().find(|(_, o)| o.order_id == order_id);
                        if let Some((&level, _)) = level_entry {
                            let notional = fill_price * fill_qty;
                            let grid_spacing = if grid.levels.len() >= 2 {
                                grid.levels[1] - grid.levels[0]
                            } else {
                                0.0
                            };
                            let realised_pnl = match side {
                                Side::Sell => grid_spacing * fill_qty,
                                Side::Buy => 0.0,
                            };

                            // Only process fully filled orders for grid replacement
                            if status == "Filled" {
                                grid.orders.remove(&level);
                            }

                            info!(level, ?side, notional, pnl = realised_pnl, "[WS] fill processed");

                            // Record fill in DB
                            {
                                let db_guard = db.lock().await;
                                let _ = db_guard.mark_filled(&order_id, fill_price, fill_qty);
                                let _ = db_guard.insert_fill(&FillRow {
                                    order_id: order_id.clone(),
                                    symbol: cfg.grid.symbol.clone(),
                                    side,
                                    price: fill_price,
                                    qty: fill_qty,
                                    notional,
                                    realised_pnl,
                                    filled_at: Utc::now(),
                                });
                            }

                            // Record metrics
                            ctx.metrics.record_fill(matches!(side, Side::Buy));
                            {
                                let side_str = match side {
                                    Side::Buy => "BUY",
                                    Side::Sell => "SELL",
                                };
                                ctx.metrics
                                    .series
                                    .lock()
                                    .await
                                    .add_fill(crate::metrics::FillEntry {
                                        timestamp: chrono::Utc::now().timestamp_millis(),
                                        side: side_str.to_string(),
                                        price: fill_price,
                                        qty: fill_qty,
                                        pnl: realised_pnl,
                                        order_id: order_id.clone(),
                                    });
                            }

                            // Alert
                            let side_str = match side {
                                Side::Buy => "BUY",
                                Side::Sell => "SELL",
                            };
                            alerter
                                .fill(&cfg.grid.symbol, side_str, fill_price, fill_qty, realised_pnl)
                                .await;

                            // Update risk engine
                            {
                                let mut guard = engine.lock().await;
                                guard.on_fill(Market::Crypto, &cfg.grid.symbol, "grid", -notional);
                            }

                            // Place replacement if fully filled
                            if status == "Filled" {
                                if regime_paused {
                                    info!(level, ?side, "regime paused — skipping replacement order");
                                } else if let Some(plan) = grid.plan_replacement(level, side) {
                                    place_with_risk(&ctx, &mut grid, &plan).await;
                                }
                            }

                            metrics.set_open_orders(grid.orders.len() as u64);
                        } else {
                            info!(order_id = %order_id, "[WS] fill for unknown order (may be from another session)");
                        }
                    }
                    WsEvent::OrderCancelled { order_id, .. } => {
                        // Remove from grid if we're tracking it
                        let removed = grid.orders.iter()
                            .find(|(_, o)| o.order_id == order_id)
                            .map(|(&level, _)| level);
                        if let Some(level) = removed {
                            grid.orders.remove(&level);
                            info!(level, order_id = %order_id, "[WS] order cancelled, removed from grid");
                            metrics.set_open_orders(grid.orders.len() as u64);
                        }
                    }
                    WsEvent::Connected => {
                        info!("[WS] private WebSocket connected");
                    }
                    WsEvent::Disconnected => {
                        warn!("[WS] private WebSocket disconnected (will auto-reconnect)");
                    }
                }
            }
        }
    }

    // -- 16. Graceful shutdown -------------------------------------------------
    info!(
        "shutdown: waiting {}s for in-flight orders",
        cfg.bot.shutdown_wait_secs
    );
    tokio::time::sleep(Duration::from_secs(cfg.bot.shutdown_wait_secs)).await;

    if !dry_run {
        info!("shutdown: cancelling all orders on symbol");
        if let Err(e) = client.cancel_all_spot(&cfg.grid.symbol).await {
            error!(err = %e, "cancel_all_spot at shutdown failed");
        } else {
            info!("shutdown cancel_all ok");
            // Mark all tracked orders as cancelled
            for (_, order) in grid.orders.drain() {
                let _ = db.lock().await.mark_cancelled(&order.order_id);
            }
        }
    }

    // Final P&L report
    let total_pnl = db
        .lock()
        .await
        .total_realised_pnl(&cfg.grid.symbol)
        .unwrap_or(0.0);
    let total_fills = db
        .lock()
        .await
        .total_fill_count(&cfg.grid.symbol)
        .unwrap_or(0);
    let final_equity = engine.lock().await.equity();
    info!(total_pnl, total_fills, final_equity, "session summary",);
    alerter
        .lifecycle(&format!(
            "Bot shutdown: PnL={total_pnl:+.2} fills={total_fills} equity={final_equity:.2}"
        ))
        .await;

    info!("bot exited cleanly");
    Ok(())
}

/// Check if local clock is within acceptable drift of Bybit server time.
async fn check_clock_sync(client: &bybit::rest::Client) {
    match client.server_time().await {
        Ok(server_ms) => {
            let local_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let drift_ms = (local_ms - server_ms as i64).abs();
            if drift_ms > 2000 {
                warn!(
                    drift_ms,
                    "clock drift detected — Bybit may reject requests (recv_window default 5s)",
                );
            } else {
                info!(drift_ms, "clock sync OK");
            }
        }
        Err(e) => {
            warn!(err = %e, "could not check server time — clock sync unknown");
        }
    }
}

async fn place_with_risk(ctx: &BotCtx<'_>, grid: &mut Grid, p: &PlannedOrder) {
    let BotCtx {
        client,
        engine,
        symbol,
        db,
        alerter,
        retry_policy,
        breaker,
        dry_run,
        metrics,
    } = ctx;
    let price: f64 = p.price_str.parse().unwrap_or(0.0);
    let qty: f64 = p.qty_str.parse().unwrap_or(0.0);
    if price <= 0.0 || qty <= 0.0 {
        warn!(
            level = p.level_index,
            price, qty, "skipping order with zero price/qty"
        );
        return;
    }
    let notional = price * qty;

    {
        let guard = engine.lock().await;
        if let Err(rej) = guard.pretrade_check(Market::Crypto, symbol, "grid", notional) {
            warn!(level = p.level_index, ?rej, notional, "risk rejected order");
            alerter
                .risk_breach(&format!(
                    "Order rejected at level {}: {:?} (notional {notional:.2})",
                    p.level_index, rej,
                ))
                .await;
            return;
        }
    }

    if breaker.is_open() {
        warn!(
            level = p.level_index,
            "circuit breaker open — skipping order placement"
        );
        return;
    }

    // Generate orderLinkId for deduplication
    let link_id = format!(
        "grid-{}-{}-{}",
        symbol,
        p.level_index,
        uuid::Uuid::new_v4().as_simple()
    );

    if *dry_run {
        let fake_id = format!("DRY-{}", uuid::Uuid::new_v4().as_simple());
        info!(
            level = p.level_index,
            side = ?p.side,
            price = %p.price_str,
            qty = %p.qty_str,
            order_id = %fake_id,
            "[DRY-RUN] order simulated",
        );
        grid.orders.insert(
            p.level_index,
            LevelOrder {
                order_id: fake_id.clone(),
                side: p.side,
                price_str: p.price_str.clone(),
                qty_str: p.qty_str.clone(),
            },
        );
        // Persist to DB
        let _ = db.lock().await.insert_order(&OrderRow {
            order_id: fake_id,
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
        let mut guard = engine.lock().await;
        guard.on_fill(Market::Crypto, symbol, "grid", notional);
        return;
    }

    let side = p.side;
    let qty_str = p.qty_str.clone();
    let price_str = p.price_str.clone();
    let link_id_clone = link_id.clone();

    match with_retry(retry_policy, "place_order", || {
        client.place_spot_order(
            symbol,
            side,
            "Limit",
            &qty_str,
            Some(&price_str),
            Some(&link_id_clone),
        )
    })
    .await
    {
        Ok(resp) => {
            info!(
                level = p.level_index,
                side = ?p.side,
                price = %p.price_str,
                qty = %p.qty_str,
                order_id = %resp.order_id,
                link_id = %link_id,
                "order placed",
            );
            grid.orders.insert(
                p.level_index,
                LevelOrder {
                    order_id: resp.order_id.clone(),
                    side: p.side,
                    price_str: p.price_str.clone(),
                    qty_str: p.qty_str.clone(),
                },
            );
            // Persist order to DB before returning
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
            let mut guard = engine.lock().await;
            guard.on_fill(Market::Crypto, symbol, "grid", notional);
            breaker.record_success();
            metrics.record_order_placed();
        }
        Err(e) => {
            breaker.record_failure();
            metrics.record_order_failed();
            warn!(level = p.level_index, err = %e, "place order failed after retries");
            alerter
                .error(&format!(
                    "Order placement failed at level {}: {e}",
                    p.level_index
                ))
                .await;
        }
    }
}

async fn reconcile_once(ctx: &BotCtx<'_>, grid: &mut Grid, regime_paused: bool) -> Result<()> {
    if ctx.dry_run {
        // In dry-run, simulate random fills for testing
        return Ok(());
    }

    let open = with_retry(ctx.retry_policy, "open_orders", || {
        ctx.client.open_orders_spot(ctx.symbol)
    })
    .await?;

    let open_ids: HashSet<String> = open.iter().map(|o| o.order_id.clone()).collect();

    // Build a map of partial fill info from exchange
    let mut partial_fills: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for o in &open {
        if let Ok(cum_qty) = o.cum_exec_qty.parse::<f64>()
            && cum_qty > 0.0
        {
            partial_fills.insert(o.order_id.clone(), cum_qty);
        }
    }

    // Orders we tracked but that are no longer on the exchange — treat as filled.
    let mut filled: Vec<(usize, Side, f64, String, f64, f64)> = Vec::new();
    grid.orders.retain(|&level, o| {
        if open_ids.contains(&o.order_id) {
            true
        } else {
            let price: f64 = o.price_str.parse().unwrap_or(0.0);
            let qty: f64 = o.qty_str.parse().unwrap_or(0.0);
            if price <= 0.0 || qty <= 0.0 {
                warn!(level, order_id = %o.order_id, "dropping order with zero price/qty");
                return false;
            }
            filled.push((level, o.side, price * qty, o.order_id.clone(), price, qty));
            false
        }
    });

    if !filled.is_empty() {
        info!(
            count = filled.len(),
            open_after = grid.orders.len(),
            "fills detected"
        );
    }

    for (level, side, notional, order_id, fill_price, fill_qty) in filled {
        // Calculate simple grid profit: for a sell fill, profit = (sell_price - buy_price) * qty
        // For grid bots the profit per cycle is approximately grid_spacing * qty
        let grid_spacing = if grid.levels.len() >= 2 {
            grid.levels[1] - grid.levels[0]
        } else {
            0.0
        };
        let realised_pnl = match side {
            Side::Sell => grid_spacing * fill_qty, // profit when we sell above where we bought
            Side::Buy => 0.0,                      // buying is cost, not profit
        };

        info!(level, ?side, notional, pnl = realised_pnl, "filled");

        // Record fill in DB
        {
            let db_guard = ctx.db.lock().await;
            let _ = db_guard.mark_filled(&order_id, fill_price, fill_qty);
            let _ = db_guard.insert_fill(&FillRow {
                order_id: order_id.clone(),
                symbol: ctx.symbol.to_string(),
                side,
                price: fill_price,
                qty: fill_qty,
                notional,
                realised_pnl,
                filled_at: Utc::now(),
            });
        }

        // Record metrics + dashboard fill entry
        ctx.metrics.record_fill(matches!(side, Side::Buy));
        {
            let side_str = match side {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
            };
            ctx.metrics
                .series
                .lock()
                .await
                .add_fill(crate::metrics::FillEntry {
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    side: side_str.to_string(),
                    price: fill_price,
                    qty: fill_qty,
                    pnl: realised_pnl,
                    order_id: order_id.clone(),
                });
        }

        // Alert on fill
        let side_str = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        ctx.alerter
            .fill(ctx.symbol, side_str, fill_price, fill_qty, realised_pnl)
            .await;

        // Release open-order exposure
        {
            let mut guard = ctx.engine.lock().await;
            guard.on_fill(Market::Crypto, ctx.symbol, "grid", -notional);
        }

        // Place replacement order (skip if regime is unfavorable)
        if regime_paused {
            info!(level, ?side, "regime paused — skipping replacement order");
        } else if let Some(plan) = grid.plan_replacement(level, side) {
            place_with_risk(ctx, grid, &plan).await;
        } else {
            info!(level, ?side, "no replacement — reached grid edge");
        }
    }
    Ok(())
}
