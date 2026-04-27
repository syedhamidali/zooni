//! Trading bot — entry point.
//!
//! Usage:
//!   cargo run --release --bin zooni -- config.toml
//!   cargo run --release --bin zooni -- config.toml --dry-run
//!   cargo run --release --bin zooni -- config.toml --tui
//!   cargo run --release --bin zooni -- config.toml --venue zerodha
//!   cargo run --release --bin zooni-backtest -- config.toml --data prices.csv
//!   cargo run --release --bin zooni-kite-login -- config.toml

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use zooni::config;

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
    let funding_arb = args.iter().any(|a| a == "--funding-arb");
    let orchestrator = args.iter().any(|a| a == "--orchestrator");

    let venue_override = args
        .iter()
        .position(|a| a == "--venue")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let is_zerodha = venue_override
        .map(|v| v.eq_ignore_ascii_case("zerodha") || v.eq_ignore_ascii_case("kite"))
        .unwrap_or(false);
    let is_coindcx = venue_override
        .map(|v| v.eq_ignore_ascii_case("coindcx"))
        .unwrap_or(false);

    let venue_name = if is_coindcx {
        "CoinDCX"
    } else if is_zerodha {
        "Zerodha"
    } else {
        "Bybit"
    };

    if orchestrator {
        tracing::info!(
            config = %config_path,
            venue = venue_name,
            dry_run = cfg.bot.dry_run,
            ema_decay = cfg.orchestrator.ema_decay,
            temperature = cfg.orchestrator.temperature,
            rebalance_interval = cfg.orchestrator.rebalance_interval,
            "starting in ORCHESTRATOR mode (multi-strategy adaptive)",
        );
    } else if funding_arb {
        if is_zerodha {
            anyhow::bail!("funding-arb mode requires perpetual contracts (Bybit only)");
        }
        tracing::info!(
            config = %config_path,
            venue = venue_name,
            dry_run = cfg.bot.dry_run,
            "starting in FUNDING-ARB mode",
        );
    } else if autopilot {
        tracing::info!(
            config = %config_path,
            venue = venue_name,
            dry_run = cfg.bot.dry_run,
            max_grids = cfg.autopilot.max_grids,
            scan_interval = cfg.autopilot.scan_interval_secs,
            "starting in AUTO-PILOT mode",
        );
    } else {
        tracing::info!(
            config = %config_path,
            venue = venue_name,
            symbol = %cfg.grid.symbol,
            levels = cfg.grid.levels,
            lower = cfg.grid.lower_price,
            upper = cfg.grid.upper_price,
            dry_run = cfg.bot.dry_run,
            "starting",
        );
    }

    if orchestrator {
        let venue = cfg.build_venue(venue_override);
        zooni::orchestrator_runner::run(cfg, venue).await
    } else if funding_arb {
        zooni::funding_arb::run(cfg).await
    } else if autopilot {
        zooni::autopilot::run(cfg).await
    } else if use_tui {
        let venue = cfg.build_venue(venue_override);
        zooni::tui::run_with_tui(cfg, venue).await
    } else {
        if is_zerodha || is_coindcx {
            let venue = cfg.build_venue(venue_override);
            zooni::orchestrator_runner::run(cfg, venue).await
        } else {
            let venue = cfg.build_venue(venue_override);
            zooni::bot::run(cfg, venue).await
        }
    }
}
