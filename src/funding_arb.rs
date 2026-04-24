//! Funding Rate Arbitrage runner — delta-neutral carry trade on Bybit perpetuals.
//!
//! Long spot + short perpetual, collect positive funding every 8 hours.
//! Fully autonomous: scans rates, enters positions, monitors, and exits.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{error, info, warn};

use crate::alerts::Alerter;
use crate::config::BotConfig;
use crate::metrics::Metrics;
use crate::persistence::{Db, FillRow};
use crate::retry::{CircuitBreaker, RetryPolicy, with_retry};
use crate::risk::{Market, RiskConfig, RiskEngine};
use crate::strategy::funding_arb::{
    ActivePosition, ArbAction, FundingArbConfig, FundingCandidate, build_candidate_with_ema,
    evaluate_positions,
};
use crate::types::Side;
use crate::venues::bybit;

/// Run the funding rate arbitrage bot.
pub async fn run(cfg: BotConfig) -> Result<()> {
    let dry_run = cfg.bot.dry_run;
    if dry_run {
        info!("*** DRY-RUN MODE — no orders will be sent ***");
    }

    // Build arb config from bot config
    let arb_cfg = cfg
        .funding_arb
        .as_ref()
        .map(|fa| FundingArbConfig {
            entry_threshold: fa.entry_threshold,
            exit_threshold: fa.exit_threshold,
            min_turnover: fa.min_turnover,
            max_position_pct: fa.max_position_pct,
            leverage: fa.leverage,
            max_positions: fa.max_positions,
            min_positive_intervals: fa.min_positive_intervals,
            max_unrealised_loss_pct: fa.max_unrealised_loss_pct,
            borrow_cost_annual_pct: fa.borrow_cost_annual_pct,
            min_net_yield_pct: fa.min_net_yield_pct,
            funding_ema_decay: fa.funding_ema_decay,
            funding_lookback: fa.funding_lookback,
            require_positive_trend: fa.require_positive_trend,
            exit_confirmation_intervals: fa.exit_confirmation_intervals,
        })
        .unwrap_or_default();

    info!(
        entry = format!("{:.4}%", arb_cfg.entry_threshold * 100.0),
        exit = format!("{:.4}%", arb_cfg.exit_threshold * 100.0),
        max_pos = arb_cfg.max_positions,
        leverage = arb_cfg.leverage,
        "funding arb config",
    );

    // -- Persistence
    let db = if dry_run {
        Arc::new(Mutex::new(Db::open_in_memory()?))
    } else {
        Arc::new(Mutex::new(Db::open(&cfg.bot.db_path)?))
    };

    // -- Alerts
    let alerter = Arc::new(Alerter::new(cfg.alerts.clone(), "funding-arb".to_string()));
    alerter
        .lifecycle(&format!(
            "Funding arb starting: max_positions={} leverage={} entry_threshold={:.4}%",
            arb_cfg.max_positions,
            arb_cfg.leverage,
            arb_cfg.entry_threshold * 100.0,
        ))
        .await;

    // -- Risk engine
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

    // -- Ctrl+C handler
    {
        let kill = kill.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("SIGINT received, tripping kill switch");
                kill.store(true, Ordering::SeqCst);
            }
        });
    }

    // -- Metrics
    let metrics = Arc::new(Metrics::new());
    metrics.set_equity(cfg.risk.starting_equity_quote);
    {
        let m = metrics.clone();
        tokio::spawn(async move {
            crate::metrics::serve(m, 9091).await;
        });
    }

    // -- Retry + circuit breaker
    let retry_policy = RetryPolicy {
        max_retries: cfg.bot.max_retries,
        ..RetryPolicy::default()
    };
    let breaker = Arc::new(CircuitBreaker::new(
        cfg.bot.circuit_breaker_threshold,
        Duration::from_secs(cfg.bot.circuit_breaker_cooldown_secs),
    ));

    // -- Bybit client
    let client = {
        let bybit_cfg = bybit::Config {
            credentials: Some(bybit::Credentials {
                api_key: if dry_run {
                    "DRY_RUN".to_string()
                } else {
                    cfg.bybit.api_key.clone()
                },
                api_secret: if dry_run {
                    "DRY_RUN".to_string()
                } else {
                    cfg.bybit.api_secret.clone()
                },
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

    // -- Verify credentials + fetch equity
    if !dry_run {
        match with_retry(&retry_policy, "wallet_balance", || {
            client.wallet_balance(&cfg.bybit.account_type)
        })
        .await
        {
            Ok(accounts) => {
                for a in &accounts {
                    for c in &a.coin {
                        if (c.coin == "USDT" || c.coin == "USD")
                            && let Ok(bal) = c.wallet_balance.parse::<f64>()
                        {
                            let mut guard = engine.lock().await;
                            guard.set_equity(bal);
                            metrics.set_equity(bal);
                            info!(equity = bal, "equity from wallet");
                        }
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("wallet balance check failed: {e}"));
            }
        }
    }

    // -- Active positions
    let mut active_positions: Vec<ActivePosition> = Vec::new();

    // -- Scan interval: check funding rates every 5 minutes
    let scan_secs = cfg
        .funding_arb
        .as_ref()
        .map(|fa| fa.scan_interval_secs)
        .unwrap_or(300);
    let mut tick = time::interval(Duration::from_secs(scan_secs));
    tick.tick().await; // skip first

    info!(scan_interval = scan_secs, "starting funding arb main loop");

    loop {
        tick.tick().await;

        if kill.load(Ordering::SeqCst) {
            info!("kill switch tripped; exiting");
            break;
        }
        if breaker.is_open() {
            warn!("circuit breaker open — skipping tick");
            continue;
        }

        // -- 1. Fetch all linear tickers for funding rates
        let tickers = match with_retry(&retry_policy, "all_tickers_linear", || {
            client.all_tickers_linear()
        })
        .await
        {
            Ok(t) => t,
            Err(e) => {
                breaker.record_failure();
                warn!(err = %e, "failed to fetch linear tickers");
                continue;
            }
        };
        breaker.record_success();

        // -- 2. Build candidates from tickers with funding history
        let mut candidates: Vec<FundingCandidate> = Vec::new();
        for t in &tickers {
            let rate: f64 = t.funding_rate.parse().unwrap_or(0.0);
            let turnover: f64 = t.turnover_24h.parse().unwrap_or(0.0);
            let mark: f64 = t.mark_price.parse().unwrap_or(0.0);
            let oi: f64 = t.open_interest.parse().unwrap_or(0.0);
            let next_time: u64 = t.next_funding_time.parse().unwrap_or(0);

            // Only fetch history for symbols that pass the basic filter
            if rate < arb_cfg.entry_threshold * 0.5
                || turnover < arb_cfg.min_turnover
                || mark <= 0.0
            {
                continue;
            }

            // Fetch recent funding history for streak/EMA calculation
            let lookback = arb_cfg.funding_lookback.max(1);
            let historical_rates = if !dry_run {
                match client.funding_rate_history(&t.symbol, (lookback + 1) as u32).await {
                    Ok(entries) => {
                        let mut rates: Vec<f64> = entries
                            .iter()
                            .map(|e| e.funding_rate.parse::<f64>().unwrap_or(0.0))
                            .collect();
                        // Keep at most `lookback` recent intervals (oldest first)
                        if rates.len() > lookback {
                            rates = rates[rates.len() - lookback..].to_vec();
                        }
                        rates
                    }
                    Err(_) => vec![rate],
                }
            } else {
                vec![rate; lookback.min(5)]
            };

            let candidate = build_candidate_with_ema(
                t.symbol.clone(),
                rate,
                next_time,
                mark,
                turnover,
                oi,
                &historical_rates,
                arb_cfg.funding_ema_decay,
            );
            candidates.push(candidate);
        }

        // -- 3. Log top opportunities
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (i, c) in candidates.iter().take(5).enumerate() {
            info!(
                rank = i + 1,
                symbol = %c.symbol,
                rate = format!("{:.4}%", c.funding_rate * 100.0),
                apy = format!("{:.1}%", c.annualised_yield),
                streak = c.positive_streak,
                turnover = format!("{:.0}", c.turnover_24h),
                score = format!("{:.3}", c.score),
                "funding opportunity",
            );
        }

        // -- 4. Check active positions P&L and mark-to-market
        for pos in &mut active_positions {
            if dry_run {
                continue;
            }
            match client.positions_linear(&pos.symbol).await {
                Ok(positions) => {
                    for p in &positions {
                        if p.side == "Sell" {
                            let unrealised: f64 = p.unrealised_pnl.parse().unwrap_or(0.0);
                            let realised: f64 = p.cum_realised_pnl.parse().unwrap_or(0.0);
                            info!(
                                symbol = %pos.symbol,
                                unrealised_pnl = format!("{:.4}", unrealised),
                                realised_pnl = format!("{:.4}", realised),
                                "position status",
                            );
                            pos.funding_collected = realised;

                            // Check unrealised loss limit
                            if unrealised < 0.0
                                && (unrealised.abs() / pos.notional)
                                    > arb_cfg.max_unrealised_loss_pct
                            {
                                warn!(
                                    symbol = %pos.symbol,
                                    loss_pct = format!("{:.2}%", (unrealised.abs() / pos.notional) * 100.0),
                                    "unrealised loss exceeds limit — will exit",
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(symbol = %pos.symbol, err = %e, "failed to query position");
                }
            }
        }

        // -- 5. Update graceful-exit counters before evaluating
        for pos in &mut active_positions {
            if let Some(c) = candidates.iter().find(|c| c.symbol == pos.symbol) {
                if c.ema_funding_rate < arb_cfg.exit_threshold {
                    pos.below_threshold_count += 1;
                } else {
                    pos.below_threshold_count = 0;
                }
            }
        }

        // -- 5b. Evaluate actions
        let equity = engine.lock().await.equity();
        let actions = evaluate_positions(&active_positions, &candidates, &arb_cfg, equity);

        // -- 6. Execute actions
        for action in actions {
            match action {
                ArbAction::Enter {
                    symbol,
                    qty,
                    leverage,
                } => {
                    info!(symbol = %symbol, qty = %qty, leverage, "ENTERING funding arb position");

                    if dry_run {
                        let mark = candidates
                            .iter()
                            .find(|c| c.symbol == symbol)
                            .map(|c| c.mark_price)
                            .unwrap_or(0.0);
                        let qty_f: f64 = qty.parse().unwrap_or(0.0);
                        active_positions.push(ActivePosition {
                            symbol: symbol.clone(),
                            entry_price: mark,
                            spot_qty: qty_f,
                            perp_qty: qty_f,
                            spot_order_id: format!("DRY-SPOT-{}", uuid::Uuid::new_v4().as_simple()),
                            perp_order_id: format!("DRY-PERP-{}", uuid::Uuid::new_v4().as_simple()),
                            entry_funding_rate: candidates
                                .iter()
                                .find(|c| c.symbol == symbol)
                                .map(|c| c.funding_rate)
                                .unwrap_or(0.0),
                            funding_collected: 0.0,
                            entry_time_ms: Utc::now().timestamp_millis() as u64,
                            notional: mark * qty_f,
                            below_threshold_count: 0,
                        });
                        info!(symbol = %symbol, "[DRY-RUN] position simulated");
                        continue;
                    }

                    // Set leverage on the perp side
                    let lev_str = leverage.to_string();
                    if let Err(e) = client.set_leverage(&symbol, &lev_str, &lev_str).await {
                        // 110043 = "leverage not modified" (already set) — safe to ignore
                        let msg = format!("{e}");
                        if !msg.contains("110043") {
                            warn!(symbol = %symbol, err = %e, "set_leverage failed");
                        }
                    }

                    // Risk check
                    let mark = candidates
                        .iter()
                        .find(|c| c.symbol == symbol)
                        .map(|c| c.mark_price)
                        .unwrap_or(0.0);
                    let qty_f: f64 = qty.parse().unwrap_or(0.0);
                    let notional = mark * qty_f;
                    {
                        let guard = engine.lock().await;
                        if let Err(rej) =
                            guard.pretrade_check(Market::Crypto, &symbol, "funding_arb", notional)
                        {
                            warn!(?rej, symbol = %symbol, "risk rejected entry");
                            alerter
                                .risk_breach(&format!("Funding arb entry rejected: {rej:?}"))
                                .await;
                            continue;
                        }
                    }

                    // Leg 1: Buy spot
                    let spot_link =
                        format!("farb-spot-{}-{}", symbol, uuid::Uuid::new_v4().as_simple());
                    let spot_result = with_retry(&retry_policy, "spot_buy", || {
                        client.place_spot_order(
                            &symbol,
                            Side::Buy,
                            "Market",
                            &qty,
                            None,
                            Some(&spot_link),
                            false,
                        )
                    })
                    .await;

                    let spot_order_id = match spot_result {
                        Ok(r) => {
                            info!(symbol = %symbol, order_id = %r.order_id, "spot buy placed");
                            breaker.record_success();
                            r.order_id
                        }
                        Err(e) => {
                            breaker.record_failure();
                            error!(symbol = %symbol, err = %e, "spot buy failed — aborting entry");
                            alerter
                                .error(&format!("Funding arb spot buy failed for {symbol}: {e}"))
                                .await;
                            continue;
                        }
                    };

                    // Leg 2: Short perpetual
                    let perp_link =
                        format!("farb-perp-{}-{}", symbol, uuid::Uuid::new_v4().as_simple());
                    let perp_result = with_retry(&retry_policy, "perp_short", || {
                        client.place_linear_order(
                            &symbol,
                            Side::Sell,
                            "Market",
                            &qty,
                            None,
                            false,
                            Some(&perp_link),
                            false,
                        )
                    })
                    .await;

                    let perp_order_id = match perp_result {
                        Ok(r) => {
                            info!(symbol = %symbol, order_id = %r.order_id, "perp short placed");
                            breaker.record_success();
                            r.order_id
                        }
                        Err(e) => {
                            breaker.record_failure();
                            error!(symbol = %symbol, err = %e, "perp short failed — unwinding spot leg");
                            // Unwind the spot buy
                            let _ = client
                                .place_spot_order(&symbol, Side::Sell, "Market", &qty, None, None, false)
                                .await;
                            alerter
                                .error(&format!(
                                    "Funding arb perp short failed for {symbol}: {e} — spot unwound"
                                ))
                                .await;
                            continue;
                        }
                    };

                    // Track the active position
                    let entry_rate = candidates
                        .iter()
                        .find(|c| c.symbol == symbol)
                        .map(|c| c.funding_rate)
                        .unwrap_or(0.0);

                    active_positions.push(ActivePosition {
                        symbol: symbol.clone(),
                        entry_price: mark,
                        spot_qty: qty_f,
                        perp_qty: qty_f,
                        spot_order_id,
                        perp_order_id,
                        entry_funding_rate: entry_rate,
                        funding_collected: 0.0,
                        entry_time_ms: Utc::now().timestamp_millis() as u64,
                        notional,
                        below_threshold_count: 0,
                    });

                    // Update risk engine
                    {
                        let mut guard = engine.lock().await;
                        guard.on_fill(Market::Crypto, &symbol, "funding_arb", notional);
                    }

                    metrics.record_fill(true);
                    metrics.record_fill(false);

                    alerter
                        .lifecycle(&format!(
                            "Funding arb ENTER: {} qty={} rate={:.4}% APY={:.1}%",
                            symbol,
                            qty,
                            entry_rate * 100.0,
                            crate::strategy::funding_arb::annualised_yield(entry_rate),
                        ))
                        .await;
                }

                ArbAction::Exit { symbol, reason } => {
                    info!(symbol = %symbol, reason = %reason, "EXITING funding arb position");

                    let pos = match active_positions.iter().find(|p| p.symbol == symbol) {
                        Some(p) => p.clone(),
                        None => continue,
                    };

                    if dry_run {
                        info!(
                            symbol = %symbol,
                            funding_collected = pos.funding_collected,
                            "[DRY-RUN] position closed",
                        );
                        active_positions.retain(|p| p.symbol != symbol);
                        continue;
                    }

                    let qty_str = format!("{:.6}", pos.perp_qty);

                    // Close perp (buy to cover)
                    match client
                        .place_linear_order(
                            &symbol,
                            Side::Buy,
                            "Market",
                            &qty_str,
                            None,
                            true,
                            None,
                            false,
                        )
                        .await
                    {
                        Ok(r) => {
                            info!(symbol = %symbol, order_id = %r.order_id, "perp cover placed");
                        }
                        Err(e) => {
                            error!(symbol = %symbol, err = %e, "perp cover failed");
                            alerter
                                .error(&format!("Funding arb perp cover failed for {symbol}: {e}"))
                                .await;
                        }
                    }

                    // Sell spot
                    let spot_qty_str = format!("{:.6}", pos.spot_qty);
                    match client
                        .place_spot_order(&symbol, Side::Sell, "Market", &spot_qty_str, None, None, false)
                        .await
                    {
                        Ok(r) => {
                            info!(symbol = %symbol, order_id = %r.order_id, "spot sell placed");
                        }
                        Err(e) => {
                            error!(symbol = %symbol, err = %e, "spot sell failed");
                            alerter
                                .error(&format!("Funding arb spot sell failed for {symbol}: {e}"))
                                .await;
                        }
                    }

                    // Record P&L
                    let pnl = pos.funding_collected;
                    {
                        let db_guard = db.lock().await;
                        let _ = db_guard.insert_fill(&FillRow {
                            order_id: format!("farb-exit-{}", uuid::Uuid::new_v4().as_simple()),
                            symbol: symbol.clone(),
                            side: Side::Sell,
                            price: pos.entry_price,
                            qty: pos.perp_qty,
                            notional: pos.notional,
                            realised_pnl: pnl,
                            filled_at: Utc::now(),
                        });
                    }

                    // Release risk exposure
                    {
                        let mut guard = engine.lock().await;
                        guard.on_fill(Market::Crypto, &symbol, "funding_arb", -pos.notional);
                    }

                    alerter
                        .lifecycle(&format!(
                            "Funding arb EXIT: {} reason={} funding_collected={:.4} USDT",
                            symbol, reason, pnl,
                        ))
                        .await;

                    active_positions.retain(|p| p.symbol != symbol);
                    info!(
                        symbol = %symbol,
                        funding_pnl = format!("{:.4}", pnl),
                        active_count = active_positions.len(),
                        "position closed",
                    );
                }

                ArbAction::Hold => {}
            }
        }

        // -- 7. Summary log
        if !active_positions.is_empty() {
            let total_funding: f64 = active_positions.iter().map(|p| p.funding_collected).sum();
            let total_notional: f64 = active_positions.iter().map(|p| p.notional).sum();
            info!(
                positions = active_positions.len(),
                total_notional = format!("{:.2}", total_notional),
                total_funding = format!("{:.4}", total_funding),
                symbols = active_positions
                    .iter()
                    .map(|p| p.symbol.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                "funding arb status",
            );
        }
    }

    // -- Graceful shutdown: close all positions
    if !active_positions.is_empty() {
        info!(
            count = active_positions.len(),
            "shutdown: closing all funding arb positions"
        );
        for pos in &active_positions {
            if dry_run {
                continue;
            }
            let qty_str = format!("{:.6}", pos.perp_qty);
            // Cover perp
            if let Err(e) = client
                .place_linear_order(&pos.symbol, Side::Buy, "Market", &qty_str, None, true, None, false)
                .await
            {
                error!(symbol = %pos.symbol, err = %e, "shutdown perp cover failed");
            }
            // Sell spot
            let spot_qty_str = format!("{:.6}", pos.spot_qty);
            if let Err(e) = client
                .place_spot_order(&pos.symbol, Side::Sell, "Market", &spot_qty_str, None, None, false)
                .await
            {
                error!(symbol = %pos.symbol, err = %e, "shutdown spot sell failed");
            }
        }
    }

    let total_funding: f64 = active_positions.iter().map(|p| p.funding_collected).sum();
    info!(
        total_funding_pnl = format!("{:.4}", total_funding),
        "funding arb exited cleanly"
    );
    alerter
        .lifecycle(&format!(
            "Funding arb shutdown: total_funding_pnl={total_funding:+.4} USDT"
        ))
        .await;

    Ok(())
}
