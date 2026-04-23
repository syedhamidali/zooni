//! Trading bot — entry point.
//!
//! Usage:
//!   cargo run --release --bin trading-bot -- config.toml
//!   cargo run --release --bin trading-bot -- config.toml --dry-run
//!   cargo run --release --bin trading-bot -- config.toml --tui
//!   cargo run --release --bin backtest -- config.toml --data prices.csv

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use trading_bot::config;

const VERSION: &str = env!("ZOONI_VERSION");
const GIT_HASH: &str = match option_env!("ZOONI_GIT_HASH") {
    Some(h) => h,
    None => "unknown",
};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("zooni {VERSION} ({GIT_HASH})");
        return Ok(());
    }

    let use_tui = args.iter().any(|a| a == "--tui");

    if !use_tui {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .init();
    }

    let config_path = args
        .iter()
        .find(|a| !a.starts_with('-') && *a != &args[0])
        .map(|s| s.as_str())
        .unwrap_or("config.toml");

    let mut cfg = config::BotConfig::load(config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    // CLI overrides
    if args.iter().any(|a| a == "--dry-run") {
        cfg.bot.dry_run = true;
    }

    let autopilot = cfg.autopilot.enabled || args.iter().any(|a| a == "--autopilot");

    if autopilot {
        tracing::info!(
            config = %config_path,
            testnet = cfg.bybit.testnet,
            dry_run = cfg.bot.dry_run,
            max_grids = cfg.autopilot.max_grids,
            scan_interval = cfg.autopilot.scan_interval_secs,
            "starting in AUTO-PILOT mode",
        );
    } else {
        tracing::info!(
            config = %config_path,
            testnet = cfg.bybit.testnet,
            symbol = %cfg.grid.symbol,
            levels = cfg.grid.levels,
            lower = cfg.grid.lower_price,
            upper = cfg.grid.upper_price,
            dry_run = cfg.bot.dry_run,
            "starting",
        );
    }

    if autopilot {
        trading_bot::autopilot::run(cfg).await
    } else if use_tui {
        trading_bot::tui::run_with_tui(cfg).await
    } else {
        trading_bot::bot::run(cfg).await
    }
}
