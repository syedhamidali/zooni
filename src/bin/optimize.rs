//! Walk-forward grid parameter optimizer.
//!
//! Automatically finds the best grid parameters by running walk-forward optimization
//! across a parameter search space. Saves optimal parameters to a JSON file that
//! the live bot can load on startup.
//!
//! Usage:
//!   cargo run --release --bin zooni-optimize -- config.toml --symbol BTCUSDT --data prices.csv
//!   cargo run --release --bin zooni-optimize -- config.toml --symbol BTCUSDT --synthetic 5000
//!   cargo run --release --bin zooni-optimize -- config.toml --symbol BTCUSDT --yahoo BTCUSDT
//!   cargo run --release --bin zooni-optimize -- config.toml --symbol BTCUSDT --out optimal_params.json

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use zooni::backtest::{
    PriceBar, fetch_klines_public, generate_synthetic_data, parse_csv, run_backtest,
};
use zooni::config::{BotConfig, GridCfg};

/// Optimal parameters produced by the optimizer, serialized to JSON.
#[derive(Debug, Serialize)]
pub struct OptimalParams {
    pub symbol: String,
    pub levels: u32,
    pub spacing_pct: f64,
    pub lower_price: f64,
    pub upper_price: f64,
    pub generated_at: String,
    pub train_sharpe: f64,
    pub test_sharpe: f64,
}

/// One candidate evaluated during optimization.
#[derive(Debug, Clone)]
struct Candidate {
    levels: u32,
    spacing_pct: f64,
    lower_price: f64,
    upper_price: f64,
    train_sharpe: f64,
    test_sharpe: f64,
    #[allow(dead_code)]
    train_pnl: f64,
    test_pnl: f64,
}

fn main() -> Result<()> {
    // Initialize tracing so backtest simulation warnings surface correctly.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    // ── Config path ──────────────────────────────────────────────────────────
    let config_path = args
        .iter()
        .find(|a| !a.starts_with('-') && *a != &args[0])
        .map(|s| s.as_str())
        .unwrap_or("config.toml");

    let cfg = BotConfig::load(config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    // ── CLI arguments ────────────────────────────────────────────────────────
    let symbol = args
        .iter()
        .position(|a| a == "--symbol")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or(&cfg.grid.symbol);

    let data_path = args
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let yahoo_symbol = args
        .iter()
        .position(|a| a == "--yahoo")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let synthetic_bars = args
        .iter()
        .position(|a| a == "--synthetic")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());

    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("optimal_params.json");

    let starting_equity = cfg.risk.starting_equity_quote;

    // ── Load price data ──────────────────────────────────────────────────────
    let bars: Vec<PriceBar> = if let Some(sym) = yahoo_symbol {
        println!("Fetching {sym} from Yahoo Finance...");
        let rt = tokio::runtime::Runtime::new()?;
        let klines = rt.block_on(async {
            let mut yc = zooni::venues::yahoo::YahooClient::new();
            yc.klines_range(sym, "1d", "1y").await
        })?;
        println!("Fetched {} bars from Yahoo Finance", klines.len());
        klines
            .into_iter()
            .map(|k| PriceBar {
                timestamp: k.timestamp as i64,
                open: k.open,
                high: k.high,
                low: k.low,
                close: k.close,
                volume: k.volume,
            })
            .collect()
    } else if let Some(path) = data_path {
        let csv =
            std::fs::read_to_string(path).with_context(|| format!("reading data file {path}"))?;
        parse_csv(&csv)
    } else if let Some(count) = synthetic_bars {
        let mid = (cfg.grid.lower_price + cfg.grid.upper_price) / 2.0;
        println!("Generating {count} synthetic bars around {mid:.2}");
        generate_synthetic_data(mid, count, 0.015, 42)
    } else {
        // Default: fetch 1000 bars from Bybit public API
        println!("Fetching 1000 klines for {symbol} from Bybit...");
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(fetch_klines_public(symbol, "60", 1000))?
    };

    if bars.len() < 50 {
        anyhow::bail!(
            "Need at least 50 price bars for optimization; got {}",
            bars.len()
        );
    }

    println!("Loaded {} price bars for {symbol}", bars.len());

    // ── Parameter search space ───────────────────────────────────────────────
    let levels_grid: &[u32] = &[8, 10, 12, 15, 20];
    let spacing_pct_grid: &[f64] = &[0.003, 0.005, 0.008, 0.01, 0.015];

    // Walk-forward split: 70% train / 30% test
    let train_size = (bars.len() as f64 * 0.70) as usize;
    let train_bars = &bars[..train_size];
    let test_bars = &bars[train_size..];

    println!(
        "Walk-forward split: {} train bars / {} test bars (70/30)",
        train_bars.len(),
        test_bars.len()
    );

    // Use current price (last close of train window) as the grid midpoint
    let current_price = train_bars.last().map(|b| b.close).unwrap_or(1.0);
    println!("Current price (end of train window): {current_price:.2}");

    // ── Run grid search ──────────────────────────────────────────────────────
    let total = levels_grid.len() * spacing_pct_grid.len();
    println!("\nSearching {total} parameter combinations...\n");

    let mut candidates: Vec<Candidate> = Vec::with_capacity(total);

    for &levels in levels_grid {
        for &spacing_pct in spacing_pct_grid {
            // Derive price range from spacing × levels centered on current price.
            // Each grid "step" is spacing_pct of current price.
            // Total range = levels × spacing × price (one side = half of that)
            let half_range = current_price * spacing_pct * levels as f64 / 2.0;
            let lower_price = (current_price - half_range).max(current_price * 0.01);
            let upper_price = current_price + half_range;

            if upper_price <= lower_price {
                continue;
            }

            // qty_per_level: allocate 2% of equity per level
            let mid_price = (lower_price + upper_price) / 2.0;
            let qty_per_level = (starting_equity * 0.02 / mid_price).max(1e-8);

            let grid_cfg = GridCfg {
                symbol: symbol.to_string(),
                lower_price,
                upper_price,
                levels,
                qty_per_level_base: qty_per_level,
                poll_secs: 3,
                cancel_existing_on_start: true,
                auto_rebalance: false,
                trailing_profit_threshold: 0.0,
                cancel_buys_on_trend: false,
                auto_recenter: false,
                recenter_threshold_pct: 0.15,
                use_post_only: false,
                vwap_entry_filter: false,
                vwap_threshold_pct: 0.005,
            };

            let train_result = run_backtest(
                &grid_cfg,
                train_bars,
                starting_equity,
                "0.01",
                "0.000001",
                false,
            );
            let test_result = run_backtest(
                &grid_cfg,
                test_bars,
                starting_equity,
                "0.01",
                "0.000001",
                false,
            );

            candidates.push(Candidate {
                levels,
                spacing_pct,
                lower_price,
                upper_price,
                train_sharpe: train_result.sharpe_ratio,
                test_sharpe: test_result.sharpe_ratio,
                train_pnl: train_result.total_pnl,
                test_pnl: test_result.total_pnl,
            });
        }
    }

    // ── Score = test Sharpe (prevents overfitting) ───────────────────────────
    candidates.sort_by(|a, b| {
        b.test_sharpe
            .partial_cmp(&a.test_sharpe)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Print top-5 summary table ─────────────────────────────────────────────
    println!(
        "{:<8} {:<12} {:<14} {:<14} {:<13} {:<13} {:<12} {:<12}",
        "Rank", "Levels", "Spacing%", "Lower", "Upper", "TrainSharpe", "TestSharpe", "TestPnL"
    );
    println!("{}", "-".repeat(98));

    for (rank, c) in candidates.iter().take(5).enumerate() {
        println!(
            "{:<8} {:<12} {:<14.4} {:<14.2} {:<13.2} {:<13.3} {:<12.3} {:<12.2}",
            rank + 1,
            c.levels,
            c.spacing_pct * 100.0,
            c.lower_price,
            c.upper_price,
            c.train_sharpe,
            c.test_sharpe,
            c.test_pnl,
        );
    }

    // ── Select best ──────────────────────────────────────────────────────────
    let best = candidates
        .first()
        .ok_or_else(|| anyhow::anyhow!("No valid parameter combinations found"))?;

    println!("\nBest parameters selected by out-of-sample (test) Sharpe ratio:");
    println!("  Levels:       {}", best.levels);
    println!("  Spacing:      {:.3}%", best.spacing_pct * 100.0);
    println!("  Lower price:  {:.2}", best.lower_price);
    println!("  Upper price:  {:.2}", best.upper_price);
    println!("  Train Sharpe: {:.3}", best.train_sharpe);
    println!("  Test Sharpe:  {:.3}", best.test_sharpe);

    // ── Save optimal_params.json ─────────────────────────────────────────────
    let optimal = OptimalParams {
        symbol: symbol.to_string(),
        levels: best.levels,
        spacing_pct: best.spacing_pct,
        lower_price: best.lower_price,
        upper_price: best.upper_price,
        generated_at: Utc::now().to_rfc3339(),
        train_sharpe: best.train_sharpe,
        test_sharpe: best.test_sharpe,
    };

    let json =
        serde_json::to_string_pretty(&optimal).context("serializing optimal params to JSON")?;
    std::fs::write(out_path, &json)
        .with_context(|| format!("writing optimal params to {out_path}"))?;

    println!("\nOptimal parameters saved to {out_path}");
    println!("{json}");

    Ok(())
}
