//! Backtesting CLI — run grid strategy against historical data.
//!
//! Usage:
//!   cargo run --release --bin backtest -- config.toml --data prices.csv
//!   cargo run --release --bin backtest -- config.toml --synthetic 5000
//!   cargo run --release --bin backtest -- config.toml --optimize

use anyhow::{Context, Result};

use zooni::backtest::{generate_synthetic_data, optimize_grid, parse_csv, run_backtest};
use zooni::config;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let config_path = args
        .iter()
        .find(|a| !a.starts_with('-') && *a != &args[0])
        .map(|s| s.as_str())
        .unwrap_or("config.toml");

    let cfg = config::BotConfig::load(config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    let data_path = args
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let synthetic_bars = args
        .iter()
        .position(|a| a == "--synthetic")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());

    let do_optimize = args.iter().any(|a| a == "--optimize");
    let adaptive = args.iter().any(|a| a == "--adaptive");

    // Load or generate price data
    let bars = if let Some(path) = data_path {
        let csv =
            std::fs::read_to_string(path).with_context(|| format!("reading data file {path}"))?;
        parse_csv(&csv)
    } else {
        let count = synthetic_bars.unwrap_or(5000);
        let mid = (cfg.grid.lower_price + cfg.grid.upper_price) / 2.0;
        println!("Generating {count} synthetic bars around {mid:.2}");
        generate_synthetic_data(mid, count, 0.015, 42)
    };

    println!("Loaded {} price bars", bars.len());
    if bars.is_empty() {
        println!("No data — exiting.");
        return Ok(());
    }

    let price_range = bars.iter().map(|b| b.high).fold(0.0_f64, f64::max)
        - bars.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    println!(
        "Price range: {:.2} – {:.2} (spread: {:.2})",
        bars.iter().map(|b| b.low).fold(f64::MAX, f64::min),
        bars.iter().map(|b| b.high).fold(0.0_f64, f64::max),
        price_range,
    );

    if do_optimize {
        println!("\n=== OPTIMIZATION MODE ===\n");
        let results = optimize_grid(
            &bars,
            cfg.risk.starting_equity_quote,
            &cfg.grid.symbol,
            5..50,
            5,
        );

        println!(
            "{:<8} {:<12} {:<12} {:<12} {:<10} {:<10} {:<10} {:<10}",
            "Levels", "Lower", "Upper", "PnL", "Fills", "MaxDD%", "Sharpe", "WinRate%"
        );
        println!("{}", "-".repeat(84));
        for (levels, lower, upper, r) in results.iter().take(20) {
            println!(
                "{:<8} {:<12.2} {:<12.2} {:<12.2} {:<10} {:<10.2} {:<10.2} {:<10.1}",
                levels,
                lower,
                upper,
                r.total_pnl,
                r.total_fills,
                r.max_drawdown * 100.0,
                r.sharpe_ratio,
                r.win_rate * 100.0,
            );
        }
        println!(
            "\nBest config: {} levels, range {:.2}–{:.2}",
            results[0].0, results[0].1, results[0].2
        );
    } else {
        let result = run_backtest(
            &cfg.grid,
            &bars,
            cfg.risk.starting_equity_quote,
            "0.01",
            "0.000001",
            adaptive,
        );

        println!("\n=== BACKTEST RESULTS ===\n");
        println!("  Bars processed:    {}", result.bars_processed);
        println!(
            "  Total fills:       {} (buy: {}, sell: {})",
            result.total_fills, result.buy_fills, result.sell_fills
        );
        println!("  Total P&L:         {:.2}", result.total_pnl);
        println!("  Final equity:      {:.2}", result.final_equity);
        println!("  Peak equity:       {:.2}", result.peak_equity);
        println!("  Holding value:     {:.2}", result.holding_value);
        println!("  Max drawdown:      {:.2}%", result.max_drawdown * 100.0);
        println!("  Sharpe ratio:      {:.2}", result.sharpe_ratio);
        println!("  Win rate:          {:.1}%", result.win_rate * 100.0);
        println!("  Avg profit/cycle:  {:.4}", result.avg_profit_per_cycle);
        if adaptive {
            println!("  Mode:              adaptive (ATR-based sizing)");
        }
        println!();

        // Print last 10 fills
        if !result.fills.is_empty() {
            println!("  Last 10 fills:");
            for f in result.fills.iter().rev().take(10).rev() {
                let side = match f.side {
                    zooni::types::Side::Buy => "BUY ",
                    zooni::types::Side::Sell => "SELL",
                };
                println!(
                    "    [{:>5}] {} lvl={:<3} price={:<12.2} qty={:<10.6} pnl={:+.4}",
                    f.timestamp, side, f.level, f.price, f.qty, f.pnl,
                );
            }
        }
    }

    Ok(())
}
