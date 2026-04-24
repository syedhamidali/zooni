//! Bot orchestration: wires venue abstraction + Grid + Risk + persistence + alerts
//! + retry + circuit breaker + dry-run + graceful shutdown + PnL tracking.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{Datelike, Utc};
use chrono_tz::Tz;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{error, info, warn};

use crate::alerts::Alerter;
use crate::config::BotConfig;
use crate::grid::{Grid, LevelOrder, PlannedOrder};
use crate::market_hours;
use crate::metrics::Metrics;
use crate::persistence::{Db, FillRow, OrderRow, OrderStatus};
use crate::retry::{CircuitBreaker, RetryPolicy, with_retry};
use crate::risk::{Market, RiskConfig, RiskEngine};
use crate::telegram::TelegramBot;
use crate::tui::{GridLevelDisplay, TuiState};
use crate::types::{OrderKind, Side};
use crate::venue::Venue;
use crate::venues::bybit::rest::{Client as BybitRestClient, ExchangeOrderStatus};
use crate::ws_private::{self, WsEvent};

/// Shared context passed to helper functions instead of many individual args.
struct BotCtx<'a> {
    venue: &'a Venue,
    engine: &'a Arc<Mutex<RiskEngine>>,
    symbol: &'a str,
    db: &'a Arc<Mutex<Db>>,
    alerter: &'a Arc<Alerter>,
    retry_policy: &'a RetryPolicy,
    breaker: &'a Arc<CircuitBreaker>,
    dry_run: bool,
    metrics: &'a Arc<Metrics>,
    /// When true, all limit orders are sent as PostOnly to capture maker fees (0.02%).
    use_post_only: bool,
    /// Tick size string (e.g. "0.01") used for adjusting price on PostOnly rejection.
    tick_size: String,
}

/// Run the bot, optionally sharing TUI state for dashboard rendering.
pub async fn run(cfg: BotConfig, venue: Venue) -> Result<()> {
    run_inner(cfg, venue, None).await
}

/// Run the bot with a shared TUI state (used by `--tui` mode).
pub async fn run_with_shared_tui(
    cfg: BotConfig,
    venue: Venue,
    tui: Arc<Mutex<TuiState>>,
) -> Result<()> {
    run_inner(cfg, venue, Some(tui)).await
}

async fn run_inner(
    cfg: BotConfig,
    venue: Venue,
    external_tui: Option<Arc<Mutex<TuiState>>>,
) -> Result<()> {
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

    // Parse timezone for daily/weekly loss reset boundaries
    let loss_tz: Tz = cfg
        .risk
        .loss_reset_timezone
        .parse()
        .unwrap_or(chrono_tz::UTC);

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

    // -- 6. Venue client (provided by caller) ------------------------------------
    info!(
        venue = venue.name(),
        base = venue.base_url(),
        "venue client ready"
    );

    // -- 6b. Private WebSocket for instant fill detection (Bybit only) ----------
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
    if !dry_run && venue.bybit_client().is_some() {
        let ws_api_key = cfg.bybit.api_key.clone();
        let ws_api_secret = cfg.bybit.api_secret.clone();
        let ws_testnet = cfg.bybit.testnet;
        let ws_tx_clone = ws_tx.clone();
        tokio::spawn(async move {
            ws_private::start(ws_api_key, ws_api_secret, ws_testnet, ws_tx_clone).await;
        });
        info!("private WebSocket listener spawned");
    }

    // -- 7. Clock sync check (Bybit only) --------------------------------------
    if !dry_run {
        check_clock_sync(&venue).await;
    }

    // -- 8. Credential check + wallet balance ---------------------------------
    if !dry_run {
        match with_retry(&retry_policy, "wallet_balance", || venue.wallet_balance()).await {
            Ok(balance) => {
                info!(
                    currency = %balance.currency,
                    total_equity = balance.total_equity,
                    available = balance.available,
                    "balance",
                );
                let mut guard = engine.lock().await;
                guard.set_equity(balance.total_equity);
                info!(equity = balance.total_equity, "equity updated from wallet");
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
        let inst = with_retry(&retry_policy, "instrument_info", || {
            venue.instrument_info(&cfg.grid.symbol)
        })
        .await
        .context("instrument info fetch failed")?;

        let ticker = with_retry(&retry_policy, "ticker", || venue.ticker(&cfg.grid.symbol))
            .await
            .context("ticker fetch failed")?;

        let last_price = ticker.last_price;
        let bid = ticker.bid_price.unwrap_or(last_price);
        let ask = ticker.ask_price.unwrap_or(last_price);
        let mid = if bid > 0.0 && ask > 0.0 {
            (bid + ask) / 2.0
        } else {
            last_price
        };
        info!(symbol = %cfg.grid.symbol, last = last_price, bid, ask, mid, "market snapshot");

        let tick_size_str = format!("{}", inst.tick_size);
        let lot_size_str = format!("{}", inst.lot_size);
        let min_qty = inst.min_qty;
        let min_amt = 0.0_f64; // venue abstraction does not expose min_order_amt
        (tick_size_str, lot_size_str, min_qty, min_amt, mid)
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
    // 11a. Deep reconciliation: query each DB-open order's exact status on the
    //      exchange to recover fills or cancellations that occurred offline.
    if !dry_run && cfg.bot.reconcile_on_startup {
        if let Some(bybit_client) = venue.bybit_client() {
            info!(
                "running startup fill reconciliation (lookback={}s)",
                cfg.bot.reconcile_lookback_secs
            );
            match reconcile_fills(
                bybit_client,
                &db,
                &cfg.grid.symbol,
                cfg.bot.reconcile_lookback_secs,
            )
            .await
            {
                Ok((filled, cancelled)) => {
                    info!(filled, cancelled, "startup fill reconciliation complete");
                }
                Err(e) => {
                    warn!(err = %e, "startup fill reconciliation failed — continuing with best-effort");
                }
            }
        }
    }

    // 11b. Restore still-open orders from DB into the in-memory grid.
    if !dry_run {
        let db_orders = db.lock().await.open_orders(&cfg.grid.symbol)?;
        if !db_orders.is_empty() {
            info!(
                count = db_orders.len(),
                "found persisted orders, reconciling with exchange"
            );
            let exchange_open = with_retry(&retry_policy, "open_orders_startup", || {
                venue.open_orders(&cfg.grid.symbol)
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
                            filled_qty: 0.0,
                            target_qty: dbo.qty,
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
        if let Err(e) = venue.cancel_all_orders(&cfg.grid.symbol).await {
            warn!(err = %e, "cancel_all_orders failed (likely no orders to cancel)");
        }
    }

    // -- 13. Build shared context for grid helpers --------------------------------
    let ctx = BotCtx {
        venue: &venue,
        engine: &engine,
        symbol: &cfg.grid.symbol,
        db: &db,
        alerter: &alerter,
        retry_policy: &retry_policy,
        breaker: &breaker,
        dry_run,
        metrics: &metrics,
        use_post_only: cfg.grid.use_post_only,
        tick_size: tick_size.clone(),
    };

    // -- 13b. Place initial grid (only for levels not already occupied) ---------
    // VWAP entry filter: if enabled, only place the initial grid when the current
    // price is within vwap_threshold_pct of VWAP computed from recent klines.
    let vwap_allows_entry = if cfg.grid.vwap_entry_filter && !dry_run {
        match venue.klines(&cfg.grid.symbol, "60", 100).await {
            Ok(klines) if !klines.is_empty() => {
                let highs: Vec<f64> = klines.iter().map(|k| k.high).collect();
                let lows: Vec<f64> = klines.iter().map(|k| k.low).collect();
                let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
                let volumes: Vec<f64> = klines.iter().map(|k| k.volume).collect();
                match crate::indicators::vwap(&highs, &lows, &closes, &volumes) {
                    Some(vwap_val) => {
                        let deviation_pct = (mid - vwap_val).abs() / vwap_val;
                        if deviation_pct <= cfg.grid.vwap_threshold_pct {
                            info!(
                                mid,
                                vwap = vwap_val,
                                deviation_pct = format!("{:.2}%", deviation_pct * 100.0),
                                "VWAP entry filter: price is near VWAP, proceeding with grid placement",
                            );
                            true
                        } else {
                            tracing::debug!(
                                "Skipping grid placement: price {:.2} is {:.2}% from VWAP {:.2}",
                                mid,
                                deviation_pct * 100.0,
                                vwap_val
                            );
                            warn!(
                                mid,
                                vwap = vwap_val,
                                deviation_pct = format!("{:.2}%", deviation_pct * 100.0),
                                threshold_pct =
                                    format!("{:.2}%", cfg.grid.vwap_threshold_pct * 100.0),
                                "VWAP entry filter: price too far from VWAP — deferring grid placement",
                            );
                            false
                        }
                    }
                    None => {
                        warn!(
                            "VWAP entry filter: could not compute VWAP from klines — proceeding anyway"
                        );
                        true
                    }
                }
            }
            Ok(_) => {
                warn!("VWAP entry filter: klines returned empty — proceeding anyway");
                true
            }
            Err(e) => {
                warn!(err = %e, "VWAP entry filter: klines fetch failed — proceeding anyway");
                true
            }
        }
    } else {
        // Filter disabled or dry-run: always allow placement
        true
    };

    if grid.orders.is_empty() {
        if !vwap_allows_entry {
            info!("VWAP entry filter active: skipping initial grid placement this cycle");
        } else {
            let plan = grid.plan_initial(mid);
            info!(plan_size = plan.len(), "placing initial grid");
            for p in &plan {
                if kill.load(Ordering::SeqCst) {
                    break;
                }
                place_with_risk(&ctx, &mut grid, p).await;
            }
            info!(open_count = grid.orders.len(), "initial placement complete");
        }
    } else {
        // Fill gaps in the grid that aren't occupied
        if !vwap_allows_entry {
            info!("VWAP entry filter active: skipping gap-fill placement this cycle");
        } else {
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
    }

    // -- 14. Feed initial PnL into risk engine --------------------------------
    {
        let now_local = Utc::now().with_timezone(&loss_tz);
        let today = now_local.format("%Y-%m-%d").to_string();
        let daily = db.lock().await.daily_pnl(&today)?;
        let weekday = now_local.weekday().num_days_from_monday();
        let week_start = (now_local - chrono::Duration::days(weekday as i64))
            .format("%Y-%m-%d")
            .to_string();
        let week_end = (now_local + chrono::Duration::days(1))
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
    // Last observed regime for hysteresis and transition detection
    let mut last_bot_regime: Option<crate::regime::Regime> = None;
    // Most recent ATR value (updated each regime check) for grid re-centering half-range.
    let mut last_atr: f64 = 0.0;

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

                // ── Market hours guard ─────────────────────────────────────────
                if cfg.grid.respect_market_hours {
                    let now = Utc::now();
                    let venue_id = cfg.grid.venue.as_str();
                    if !market_hours::is_market_open(venue_id, now) {
                        // Log and skip — the tick interval already handles re-checking.
                        // Display a human-readable next-open estimate in IST.
                        let ist: chrono_tz::Tz = chrono_tz::Asia::Kolkata;
                        let local = now.with_timezone(&ist);
                        let open_str = local
                            .date_naive()
                            .succ_opt()
                            .and_then(|d| d.and_hms_opt(9, 15, 0))
                            .map(|dt| dt.format("%Y-%m-%d %H:%M IST").to_string())
                            .unwrap_or_else(|| "next session".to_string());
                        info!(venue = venue_id, "Market closed. Sleeping until {open_str}");
                        continue;
                    }

                    // Pre-close: cancel all buy orders 15 minutes before close.
                    if let Some(mins) = market_hours::minutes_until_close(venue_id, now) {
                        if mins <= 15 && mins >= 0 {
                            info!(
                                venue = venue_id,
                                mins,
                                "Market closing in {mins}min — cancelling buy orders"
                            );
                            let buy_levels: Vec<(usize, String)> = grid
                                .orders
                                .iter()
                                .filter(|(_, o)| matches!(o.side, crate::types::Side::Buy))
                                .map(|(&lvl, o)| (lvl, o.order_id.clone()))
                                .collect();
                            for (lvl, order_id) in buy_levels {
                                match venue.cancel_order(&cfg.grid.symbol, &order_id).await {
                                    Ok(()) => {
                                        info!(
                                            level = lvl,
                                            %order_id,
                                            "buy order cancelled before market close",
                                        );
                                        grid.orders.remove(&lvl);
                                        let _ = db.lock().await.mark_cancelled(&order_id);
                                    }
                                    Err(e) => {
                                        warn!(
                                            level = lvl,
                                            %order_id,
                                            err = %e,
                                            "failed to cancel buy order before close",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                // ──────────────────────────────────────────────────────────────

                // Periodic regime detection — auto-pause during unfavorable conditions
                if !dry_run {
                    regime_check_counter += 1;
                    if regime_check_counter >= regime_check_interval {
                        regime_check_counter = 0;
                        match venue.klines(&cfg.grid.symbol, "60", 100).await {
                            Ok(venue_klines) => {
                                if let Some(analysis) = crate::regime::analyze(&venue_klines) {
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

                                    // Detect Ranging → Trending transition; cancel buy orders to
                                    // prevent accumulating base asset into a directional move.
                                    if cfg.grid.cancel_buys_on_trend {
                                        let was_ranging = matches!(
                                            last_bot_regime,
                                            Some(crate::regime::Regime::Ranging)
                                        );
                                        let now_trending = matches!(
                                            analysis.regime,
                                            crate::regime::Regime::TrendingUp
                                                | crate::regime::Regime::TrendingDown
                                        );
                                        if was_ranging && now_trending {
                                            tracing::info!(
                                                "Trend breakout detected, cancelling buy orders to prevent accumulating into trend"
                                            );
                                            // Collect buy levels to cancel; keep sell orders open
                                            let buy_levels: Vec<(usize, String)> = grid
                                                .orders
                                                .iter()
                                                .filter(|(_, o)| matches!(o.side, Side::Buy))
                                                .map(|(&lvl, o)| (lvl, o.order_id.clone()))
                                                .collect();
                                            for (lvl, order_id) in buy_levels {
                                                match venue
                                                    .cancel_order(&cfg.grid.symbol, &order_id)
                                                    .await
                                                {
                                                    Ok(()) => {
                                                        info!(
                                                            level = lvl,
                                                            %order_id,
                                                            "buy order cancelled on trend breakout",
                                                        );
                                                        grid.orders.remove(&lvl);
                                                        let _ = db
                                                            .lock()
                                                            .await
                                                            .mark_cancelled(&order_id);
                                                    }
                                                    Err(e) => {
                                                        warn!(
                                                            level = lvl,
                                                            %order_id,
                                                            err = %e,
                                                            "failed to cancel buy order on trend breakout",
                                                        );
                                                    }
                                                }
                                            }
                                            alerter
                                                .lifecycle(&format!(
                                                    "⚡ Trend breakout ({}): cancelled buy orders to avoid accumulation",
                                                    analysis.regime,
                                                ))
                                                .await;
                                        }
                                    }

                                    last_bot_regime = Some(analysis.regime);
                                    if analysis.atr > 0.0 {
                                        last_atr = analysis.atr;
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
                    if let Ok(balance) = venue.wallet_balance().await {
                        let bal = balance.total_equity;
                        let mut guard = engine.lock().await;
                        guard.set_equity(bal);
                        metrics.set_equity(bal);
                        metrics.series.lock().await.push_equity(bal);
                    }

                    // Refresh daily/weekly PnL — use configured timezone for boundaries
                    let now_local = Utc::now().with_timezone(&loss_tz);
                    let today = now_local.format("%Y-%m-%d").to_string();
                    let weekday = now_local.weekday().num_days_from_monday();
                    let week_start = (now_local - chrono::Duration::days(weekday as i64))
                        .format("%Y-%m-%d")
                        .to_string();
                    let week_end = (now_local + chrono::Duration::days(1))
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
                    && let Ok(ticker) = venue.ticker(&cfg.grid.symbol).await
                {
                    let price = ticker.last_price;
                    let mut series = metrics.series.lock().await;
                    series.push_price(price);
                    drop(series);
                    tui_state.lock().await.current_price = price;

                    // Revalue risk exposure at current mark price (not stale entry price)
                    let mut mark_prices = std::collections::HashMap::new();
                    mark_prices.insert(cfg.grid.symbol.clone(), price);
                    engine.lock().await.revalue_exposure(&mark_prices);
                }

                // Dynamic grid rebalancing
                if cfg.grid.auto_rebalance
                    && !dry_run
                    && let Ok(ticker) = venue.ticker(&cfg.grid.symbol).await
                {
                    let price = ticker.last_price;
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
                        if let Err(e) = venue.cancel_all_orders(&cfg.grid.symbol).await {
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

                // Auto re-centering: if price escapes the grid band by recenter_threshold_pct,
                // cancel all orders and rebuild the grid centered on current price using 2*ATR
                // as the half-range (ATR sourced from the most recent regime analysis).
                if cfg.grid.auto_recenter
                    && !dry_run
                    && let Ok(ticker) = venue.ticker(&cfg.grid.symbol).await
                {
                    let price = ticker.last_price;
                    let lower = *grid.levels.first().unwrap_or(&0.0);
                    let upper = *grid.levels.last().unwrap_or(&0.0);
                    let threshold = cfg.grid.recenter_threshold_pct;
                    let price_above = upper > 0.0 && price > upper * (1.0 + threshold);
                    let price_below = lower > 0.0 && price < lower * (1.0 - threshold);

                    if price_above || price_below {
                        // half_range = 2 * ATR; fall back to half the existing range if ATR unavailable
                        let half_range = if last_atr > 0.0 {
                            2.0 * last_atr
                        } else {
                            (upper - lower) / 2.0
                        };
                        let new_lower = (price - half_range).max(0.01);
                        let new_upper = price + half_range;

                        info!(
                            price,
                            old_lower = lower,
                            old_upper = upper,
                            new_lower,
                            new_upper,
                            atr = last_atr,
                            "auto re-center: price escaped grid band, rebuilding",
                        );

                        match venue.cancel_all_orders(&cfg.grid.symbol).await {
                            Err(e) => {
                                warn!(err = %e, "cancel_all for auto-recenter failed");
                            }
                            Ok(()) => {
                                // Mark all orders cancelled in DB
                                {
                                    let db_guard = db.lock().await;
                                    for (_, order) in grid.orders.drain() {
                                        let _ = db_guard.mark_cancelled(&order.order_id);
                                    }
                                }

                                // Recalculate levels and place new grid
                                let n_levels = grid.levels.len();
                                grid.levels = crate::grid::compute_levels(new_lower, new_upper, n_levels);
                                let plan = grid.plan_initial(price);
                                info!(
                                    plan_size = plan.len(),
                                    "placing re-centered grid orders",
                                );
                                for p in &plan {
                                    if kill.load(Ordering::SeqCst) {
                                        break;
                                    }
                                    place_with_risk(&ctx, &mut grid, p).await;
                                }
                                alerter
                                    .lifecycle(&format!(
                                        "Grid re-centered around {price:.2}: {new_lower:.2}–{new_upper:.2} (was {lower:.2}–{upper:.2})"
                                    ))
                                    .await;
                            }
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
                        let level_entry = grid.orders.iter_mut().find(|(_, o)| o.order_id == order_id);
                        if let Some((&level, order)) = level_entry {
                            // Track accumulated fill quantity
                            order.filled_qty += fill_qty;
                            let fully_filled = order.filled_qty >= order.target_qty
                                || status == "Filled";

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

                            // Only remove order and place replacement when fully filled
                            if fully_filled {
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

                            // Place replacement only when fully filled
                            if fully_filled {
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
        if let Err(e) = venue.cancel_all_orders(&cfg.grid.symbol).await {
            error!(err = %e, "cancel_all_orders at shutdown failed");
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

/// Check if local clock is within acceptable drift of server time (Bybit only).
async fn check_clock_sync(venue: &Venue) {
    let Some(bybit_client) = venue.bybit_client() else {
        return; // clock-sync check is Bybit-specific
    };
    match bybit_client.server_time().await {
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
        venue,
        engine,
        symbol,
        db,
        alerter,
        retry_policy,
        breaker,
        dry_run,
        metrics,
        use_post_only,
        tick_size,
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
                filled_qty: 0.0,
                target_qty: p.qty_str.parse().unwrap_or(0.0),
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
    let post_only = *use_post_only;

    // Attempt to place the order with PostOnly when configured. If Bybit rejects
    // because the order would fill as taker (error 170213), wait 200ms, adjust
    // the price by one tick further away from the market, and retry once.
    // If the retry also fails, log a warning and skip this level this cycle.
    let first_result = with_retry(retry_policy, "place_order", || {
        venue.place_order(
            symbol,
            side,
            OrderKind::Limit,
            &qty_str,
            Some(price_str.as_str()),
            Some(link_id_clone.as_str()),
            post_only,
        )
    })
    .await;

    let final_result = match first_result {
        Ok(r) => Ok(r),
        Err(ref e) if post_only && BybitRestClient::is_post_only_rejection(e) => {
            warn!(
                level = p.level_index,
                price = %price_str,
                "PostOnly rejected (would fill as taker) — adjusting price by one tick and retrying"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;

            let tick_val: f64 = tick_size.parse().unwrap_or(0.0);
            // Move away from market: buys go lower, sells go higher
            let adjusted_price = match side {
                Side::Buy => price - tick_val,
                Side::Sell => price + tick_val,
            };
            let adjusted_price_str = crate::grid::format_to_increment(adjusted_price, tick_size);
            let retry_link_id = format!(
                "grid-{}-{}-{}",
                symbol,
                p.level_index,
                uuid::Uuid::new_v4().as_simple()
            );
            let adj_clone = adjusted_price_str.clone();
            let lid_clone = retry_link_id.clone();

            match venue
                .place_order(
                    symbol,
                    side,
                    OrderKind::Limit,
                    &qty_str,
                    Some(adj_clone.as_str()),
                    Some(lid_clone.as_str()),
                    post_only,
                )
                .await
            {
                Ok(r) => {
                    info!(
                        level = p.level_index,
                        adjusted_price = %adjusted_price_str,
                        "PostOnly retry succeeded"
                    );
                    Ok(r)
                }
                Err(retry_err) => {
                    warn!(
                        level = p.level_index,
                        err = %retry_err,
                        "PostOnly retry also rejected — skipping level this cycle"
                    );
                    return; // skip level silently
                }
            }
        }
        Err(e) => Err(e),
    };

    match final_result {
        Ok(resp) => {
            info!(
                level = p.level_index,
                side = ?p.side,
                price = %p.price_str,
                qty = %p.qty_str,
                order_id = %resp.order_id,
                link_id = %link_id,
                post_only,
                "order placed",
            );
            grid.orders.insert(
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

/// Startup reconciliation: for every order the DB thinks is `Open`, query the
/// exchange for its real status and sync any fills or cancellations that
/// happened while the bot was offline.
///
/// Returns `(filled_count, cancelled_count)`.
async fn reconcile_fills(
    bybit: &BybitRestClient,
    db: &Arc<Mutex<Db>>,
    symbol: &str,
    lookback_secs: u64,
) -> Result<(usize, usize)> {
    let db_open = db.lock().await.open_orders(symbol)?;
    if db_open.is_empty() {
        return Ok((0, 0));
    }

    // Compute the earliest timestamp we care about for the fill-history query.
    let since_ms = {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now_ms.saturating_sub(lookback_secs * 1_000)
    };

    // Pre-fetch recent fills so we can look up actual fill prices.
    let recent_fills: std::collections::HashMap<String, crate::venues::bybit::rest::Fill> =
        match bybit.get_recent_fills(symbol, since_ms).await {
            Ok(fills) => fills.into_iter().map(|f| (f.order_id.clone(), f)).collect(),
            Err(e) => {
                warn!(
                    err = %e,
                    "reconcile_fills: could not fetch recent fills — will use order price as fallback"
                );
                std::collections::HashMap::new()
            }
        };

    let mut filled_count = 0usize;
    let mut cancelled_count = 0usize;

    for dbo in &db_open {
        let status_info = match bybit.get_order_status(symbol, &dbo.order_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    order_id = %dbo.order_id,
                    err = %e,
                    "reconcile_fills: could not query order status — skipping"
                );
                continue;
            }
        };

        match status_info.status {
            ExchangeOrderStatus::Filled => {
                // Use the execution fill price if available, otherwise fall back
                // to the average price returned by the order query.
                let fill_price = if let Some(exec) = recent_fills.get(&dbo.order_id) {
                    exec.exec_price
                } else if status_info.avg_fill_price > 0.0 {
                    status_info.avg_fill_price
                } else {
                    dbo.price
                };
                let fill_qty = if status_info.filled_qty > 0.0 {
                    status_info.filled_qty
                } else {
                    dbo.qty
                };

                {
                    let db_guard = db.lock().await;
                    db_guard.mark_filled(&dbo.order_id, fill_price, fill_qty)?;
                    let side = if let Some(exec) = recent_fills.get(&dbo.order_id) {
                        exec.side
                    } else {
                        dbo.side
                    };
                    let notional = fill_price * fill_qty;
                    let _ = db_guard.insert_fill(&FillRow {
                        order_id: dbo.order_id.clone(),
                        symbol: symbol.to_string(),
                        side,
                        price: fill_price,
                        qty: fill_qty,
                        notional,
                        realised_pnl: 0.0, // grid P&L computed by reconcile_once at runtime
                        filled_at: chrono::Utc::now(),
                    });
                }

                info!(
                    order_id = %dbo.order_id,
                    price = fill_price,
                    "Reconciled orphaned fill: order {} filled at {}",
                    dbo.order_id,
                    fill_price,
                );
                filled_count += 1;
            }
            ExchangeOrderStatus::Cancelled => {
                db.lock().await.mark_cancelled(&dbo.order_id)?;
                info!(
                    order_id = %dbo.order_id,
                    "Reconciled cancelled order: {}",
                    dbo.order_id,
                );
                cancelled_count += 1;
            }
            ExchangeOrderStatus::Open | ExchangeOrderStatus::Unknown(_) => {
                // Still open (or unknown) — no action needed.
            }
        }
    }

    info!(
        filled = filled_count,
        cancelled = cancelled_count,
        "Reconciliation complete: {} fills recovered, {} orders reconciled",
        filled_count,
        cancelled_count,
    );

    Ok((filled_count, cancelled_count))
}

async fn reconcile_once(ctx: &BotCtx<'_>, grid: &mut Grid, regime_paused: bool) -> Result<()> {
    if ctx.dry_run {
        // In dry-run, simulate random fills for testing
        return Ok(());
    }

    let open = with_retry(ctx.retry_policy, "open_orders", || {
        ctx.venue.open_orders(ctx.symbol)
    })
    .await?;

    let open_ids: HashSet<String> = open.iter().map(|o| o.order_id.clone()).collect();

    // Build a map of partial fill info from exchange
    let mut partial_fills: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for o in &open {
        if o.filled_qty > 0.0 {
            partial_fills.insert(o.order_id.clone(), o.filled_qty);
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
