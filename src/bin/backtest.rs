//! Backtesting CLI — run grid or orchestrator strategies against historical data.
//!
//! Usage:
//!   cargo run --release --bin zooni-backtest -- config.toml --data prices.csv
//!   cargo run --release --bin zooni-backtest -- config.toml --synthetic 5000
//!   cargo run --release --bin zooni-backtest -- config.toml --optimize
//!   cargo run --release --bin zooni-backtest -- config.toml --orchestrator --synthetic 5000
//!   cargo run --release --bin zooni-backtest -- config.toml --walk-forward --synthetic 5000
//!   cargo run --release --bin zooni-backtest -- config.toml --orchestrator --pairs-data ETHUSDT,BTCUSDT
//!   cargo run --release --bin zooni-backtest -- config.toml --export results.csv
//!   cargo run --release --bin zooni-backtest -- config.toml --orchestrator --export results.csv
//!
//! Yahoo Finance data (free — no API key needed):
//!   cargo run --release --bin zooni-backtest -- config.toml --yahoo RELIANCE.NS --orchestrator
//!   cargo run --release --bin zooni-backtest -- config.toml --yahoo HDFCBANK.NS --yahoo-range 1y
//!   cargo run --release --bin zooni-backtest -- config.toml --yahoo TCS.NS --yahoo-interval 1h --yahoo-range 1mo

use anyhow::{Context, Result};

use zooni::backtest::{
    export_grid_csv, export_orchestrator_csv, fetch_klines_public, generate_synthetic_data,
    optimize_grid, parse_csv, run_backtest, run_orchestrator_backtest,
    run_orchestrator_backtest_with_data, run_walk_forward,
};
use zooni::config;
use zooni::orchestrator::OrchestratorConfig;
use zooni::strategy::Strategy;
use zooni::strategy::dca::DcaStrategy;
use zooni::strategy::grid_strategy::{GridStrategy, GridStrategyConfig};
use zooni::strategy::momentum::{MomentumConfig, MomentumStrategy};
use zooni::strategy::pairs::{PairsConfig, PairsStrategy};

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
    let orchestrator_mode = args.iter().any(|a| a == "--orchestrator");
    let walk_forward_mode = args.iter().any(|a| a == "--walk-forward");

    let export_path = args
        .iter()
        .position(|a| a == "--export")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let pairs_data = args
        .iter()
        .position(|a| a == "--pairs-data")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let yahoo_symbol = args
        .iter()
        .position(|a| a == "--yahoo")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let yahoo_range = args
        .iter()
        .position(|a| a == "--yahoo-range")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("6mo");

    let yahoo_interval = args
        .iter()
        .position(|a| a == "--yahoo-interval")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("1d");

    // Load or generate price data
    let bars = if let Some(symbol) = yahoo_symbol {
        println!(
            "Fetching {symbol} from Yahoo Finance (range={yahoo_range}, interval={yahoo_interval})..."
        );
        let rt = tokio::runtime::Runtime::new()?;
        let klines = rt.block_on(async {
            let mut yc = zooni::venues::yahoo::YahooClient::new();
            yc.klines_range(symbol, yahoo_interval, yahoo_range).await
        })?;
        println!("Fetched {} bars from Yahoo Finance", klines.len());
        klines
            .into_iter()
            .map(|k| zooni::backtest::PriceBar {
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

    if walk_forward_mode {
        println!("\n=== WALK-FORWARD ANALYSIS ===\n");

        let orch_cfg = cfg.orchestrator.clone();
        let symbol = cfg.grid.symbol.clone();
        let equity = cfg.risk.starting_equity_quote;
        let window_size = bars.len() / 4; // 4 windows
        let window_size = window_size.max(200);

        println!("Window size: {} bars, Train: 70%, Test: 30%", window_size,);

        let result = run_walk_forward(
            || build_backtest_strategies(&symbol, &orch_cfg),
            &orch_cfg,
            &bars,
            equity,
            window_size,
            0.7,
        );

        println!("\n  Windows:           {}", result.windows.len());
        println!("  Total test P&L:    {:.2}", result.total_test_pnl);
        println!("  Total test fills:  {}", result.total_test_fills);
        println!("  Avg test Sharpe:   {:.2}", result.avg_test_sharpe);
        println!(
            "  Avg test drawdown: {:.2}%",
            result.avg_test_drawdown * 100.0
        );
        println!(
            "  Avg test win rate: {:.1}%",
            result.avg_test_win_rate * 100.0
        );

        println!(
            "\n  {:<8} {:<12} {:<12} {:<12} {:<12} {:<10}",
            "Window", "TrainPnL", "TestPnL", "TrainSharpe", "TestSharpe", "TestDD%"
        );
        println!("  {}", "-".repeat(66));
        for w in &result.windows {
            println!(
                "  {:<8} {:<12.2} {:<12.2} {:<12.2} {:<12.2} {:<10.2}",
                w.window_idx,
                w.train_result.total_pnl,
                w.test_result.total_pnl,
                w.train_result.sharpe_ratio,
                w.test_result.sharpe_ratio,
                w.test_result.max_drawdown * 100.0,
            );
        }

        // Overfitting detection
        let train_avg_sharpe = result
            .windows
            .iter()
            .map(|w| w.train_result.sharpe_ratio)
            .sum::<f64>()
            / result.windows.len().max(1) as f64;
        let ratio = if train_avg_sharpe.abs() > 1e-9 {
            result.avg_test_sharpe / train_avg_sharpe
        } else {
            0.0
        };
        println!("\n  Train avg Sharpe:  {:.2}", train_avg_sharpe);
        println!("  Test/Train ratio:  {:.2}", ratio);
        if ratio < 0.5 {
            println!("  ⚠  Test/Train < 0.5 — likely overfitting");
        } else if ratio > 0.8 {
            println!("  ✓  Test/Train > 0.8 — strategy generalises well");
        }

        // Export the last test window result if --export is set
        if let Some(path) = export_path {
            if let Some(last_window) = result.windows.last() {
                export_orchestrator_csv(&last_window.test_result, path)?;
                println!("Exported results to {path}");
            }
        }
    } else if orchestrator_mode {
        println!("\n=== ORCHESTRATOR BACKTEST ===\n");

        let orch_cfg = &cfg.orchestrator;
        let symbol = &cfg.grid.symbol;
        let strategies = build_backtest_strategies(symbol, orch_cfg);
        let strat_names: Vec<String> = strategies.iter().map(|s| s.name().to_string()).collect();
        println!("Strategies: {:?}", strat_names);
        println!(
            "EMA decay: {}, Temperature: {}, Rebalance every {} ticks",
            orch_cfg.ema_decay, orch_cfg.temperature, orch_cfg.rebalance_interval,
        );

        // Fetch real pairs data if --pairs-data is provided
        let real_secondary = if let Some(pair_spec) = pairs_data {
            let symbols: Vec<&str> = pair_spec.split(',').collect();
            if symbols.len() != 2 {
                println!("--pairs-data expects two comma-separated symbols, e.g. ETHUSDT,BTCUSDT");
                return Ok(());
            }
            let (sym_a, sym_b) = (symbols[0], symbols[1]);
            println!("Fetching real klines for {} and {}...", sym_a, sym_b);

            let rt = tokio::runtime::Runtime::new()?;
            let count = bars.len();
            let interval = "60";

            let (bars_a, bars_b) = rt.block_on(async {
                let a = fetch_klines_public(sym_a, interval, count).await;
                let b = fetch_klines_public(sym_b, interval, count).await;
                (a, b)
            });

            let bars_a = bars_a.with_context(|| format!("fetching klines for {sym_a}"))?;
            let bars_b = bars_b.with_context(|| format!("fetching klines for {sym_b}"))?;

            let min_len = bars_a.len().min(bars_b.len());
            println!(
                "Fetched {} bars for {}, {} bars for {} (using {})",
                bars_a.len(),
                sym_a,
                bars_b.len(),
                sym_b,
                min_len,
            );

            Some((
                sym_a.to_string(),
                sym_b.to_string(),
                bars_a,
                bars_b,
                min_len,
            ))
        } else {
            None
        };

        let result = if let Some((ref sym_a, ref _sym_b, ref bars_a, ref bars_b, min_len)) =
            real_secondary
        {
            let primary = &bars_b[..min_len];
            let secondary = &bars_a[..min_len];
            run_orchestrator_backtest_with_data(
                build_backtest_strategies(symbol, orch_cfg),
                orch_cfg.clone(),
                primary,
                Some((sym_a.as_str(), secondary)),
                cfg.risk.starting_equity_quote,
            )
        } else {
            run_orchestrator_backtest(
                strategies,
                orch_cfg.clone(),
                &bars,
                cfg.risk.starting_equity_quote,
            )
        };

        println!("\n  Bars processed:    {}", result.bars_processed);
        println!("  Total fills:       {}", result.total_fills);
        println!("  Total P&L:         {:.2}", result.total_pnl);
        println!("  Final equity:      {:.2}", result.final_equity);
        println!("  Peak equity:       {:.2}", result.peak_equity);
        println!("  Max drawdown:      {:.2}%", result.max_drawdown * 100.0);
        println!("  Sharpe ratio:      {:.2}", result.sharpe_ratio);

        println!("\n  Per-strategy results:");
        println!(
            "  {:<15} {:<12} {:<8} {:<10} {:<12}",
            "Strategy", "P&L", "Fills", "WinRate%", "FinalAlloc%"
        );
        println!("  {}", "-".repeat(57));
        for sr in &result.per_strategy {
            println!(
                "  {:<15} {:<12.2} {:<8} {:<10.1} {:<12.1}",
                sr.name,
                sr.pnl,
                sr.fills,
                sr.win_rate * 100.0,
                sr.final_allocation * 100.0,
            );
        }

        if !result.allocation_history.is_empty() {
            println!("\n  Allocation history (sampled):");
            for (bar, allocs) in result.allocation_history.iter().take(10) {
                let alloc_str: Vec<String> = allocs
                    .iter()
                    .map(|(k, v)| format!("{}:{:.0}%", k, v * 100.0))
                    .collect();
                println!("    bar {:>5}: {}", bar, alloc_str.join(" "));
            }
        }

        if let Some(path) = export_path {
            export_orchestrator_csv(&result, path)?;
            println!("Exported results to {path}");
        }
    } else if do_optimize {
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

        if let Some(path) = export_path {
            export_grid_csv(&result, path)?;
            println!("Exported results to {path}");
        }
    }

    Ok(())
}

fn build_backtest_strategies(
    symbol: &str,
    orch_cfg: &OrchestratorConfig,
) -> Vec<Box<dyn Strategy>> {
    let mut strategies: Vec<Box<dyn Strategy>> = Vec::new();

    for name in &orch_cfg.strategies {
        match name.as_str() {
            "dca" => {
                let dca = &orch_cfg.dca;
                strategies.push(Box::new(DcaStrategy::with_exits(
                    symbol.to_string(),
                    dca.buy_amount_quote,
                    dca.interval_ticks,
                    dca.sma_period,
                    dca.use_sma_filter,
                    dca.take_profit_pct,
                    dca.trailing_stop_pct,
                )));
            }
            "momentum" => {
                let mom = &orch_cfg.momentum;
                strategies.push(Box::new(MomentumStrategy::new(MomentumConfig {
                    symbol: symbol.to_string(),
                    position_size_quote: mom.position_size_quote,
                    cooldown_ticks: mom.cooldown_ticks,
                    ..MomentumConfig::default()
                })));
            }
            "grid" => {
                let grid = &orch_cfg.grid;
                strategies.push(Box::new(GridStrategy::new(GridStrategyConfig {
                    symbol: symbol.to_string(),
                    lower_price: grid.lower_price,
                    upper_price: grid.upper_price,
                    levels: grid.levels,
                    qty_per_level: grid.qty_per_level,
                })));
            }
            "pairs" => {
                let pairs = &orch_cfg.pairs;
                strategies.push(Box::new(PairsStrategy::new(PairsConfig {
                    symbol_a: pairs.symbol_a.clone(),
                    symbol_b: pairs.symbol_b.clone(),
                    lookback: pairs.lookback,
                    entry_z_score: pairs.entry_z_score,
                    exit_z_score: pairs.exit_z_score,
                    stop_z_score: pairs.stop_z_score,
                    position_size_quote: pairs.position_size_quote,
                    ..PairsConfig::default()
                })));
            }
            _ => {}
        }
    }

    if strategies.is_empty() {
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

    strategies
}
