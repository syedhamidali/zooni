//! Market scanner CLI — finds the best trading pairs for grid strategy.
//!
//! Usage:
//!   scan                         # Scan with defaults (USDT pairs, $1M+ volume)
//!   scan --config config.toml    # Use Bybit credentials from config
//!   scan --top 5                 # Show top 5 results
//!   scan --quote USDC            # Scan USDC pairs
//!   scan --min-turnover 5000000  # $5M+ daily turnover

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use trading_bot::config::BotConfig;
use trading_bot::scanner::{self, ScannerConfig};
use trading_bot::venues::bybit;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = std::env::args().collect();

    let mut scanner_cfg = ScannerConfig::default();
    let mut config_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                config_path = Some(args.get(i).cloned().unwrap_or_default());
            }
            "--top" | "-n" => {
                i += 1;
                scanner_cfg.max_results = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(10);
            }
            "--quote" | "-q" => {
                i += 1;
                if let Some(q) = args.get(i) {
                    scanner_cfg.quote_currencies = vec![q.clone()];
                }
            }
            "--min-turnover" => {
                i += 1;
                scanner_cfg.min_turnover =
                    args.get(i).and_then(|s| s.parse().ok()).unwrap_or(1_000_000.0);
            }
            "--interval" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    scanner_cfg.kline_interval = v.clone();
                }
            }
            "--help" | "-h" => {
                println!("Usage: scan [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c PATH     Config file for Bybit credentials");
                println!("  --top, -n N           Show top N results (default: 10)");
                println!("  --quote, -q CURRENCY  Quote currency (default: USDT)");
                println!("  --min-turnover AMT    Minimum 24h turnover in USD (default: 1000000)");
                println!("  --interval INTERVAL   Kline interval: 1,5,15,30,60,240,D (default: 60)");
                println!("  --help, -h            Show this help");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
            }
        }
        i += 1;
    }

    // Create Bybit client
    let bybit_cfg = if let Some(path) = config_path {
        let bot_cfg = BotConfig::load(&path)?;
        bybit::Config {
            testnet: bot_cfg.bybit.testnet,
            recv_window_ms: bot_cfg.bybit.recv_window_ms,
            credentials: Some(bybit::Credentials {
                api_key: bot_cfg.bybit.api_key.clone(),
                api_secret: bot_cfg.bybit.api_secret.clone(),
            }),
        }
    } else {
        // Public endpoints only — no credentials needed for scanning
        bybit::Config {
            testnet: false, // scan mainnet for real data
            recv_window_ms: 5000,
            credentials: None,
        }
    };

    let client = bybit::rest::Client::new(bybit_cfg);
    let results = scanner::scan(&client, &scanner_cfg).await?;

    // Print formatted report
    println!("\n{}", scanner::format_report(&results));

    // Print CSV-style summary
    println!("symbol,regime,score,adx,rsi,vol%,spread%,turnover_24h,suggested_range");
    for r in &results {
        let range = r
            .analysis
            .suggested_range
            .map(|(l, u)| format!("{:.4}-{:.4}", l, u))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{},{},{:.1},{:.1},{:.1},{:.2},{:.3},{:.0},{}",
            r.symbol,
            r.analysis.regime,
            r.score,
            r.analysis.adx,
            r.analysis.rsi,
            r.analysis.volatility_pct,
            r.spread_pct,
            r.turnover_24h,
            range,
        );
    }

    Ok(())
}
