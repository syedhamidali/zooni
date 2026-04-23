//! TOML config loader for the trading bot.
//!
//! Pass the config path as the first CLI argument. Never commit real creds;
//! see `config.example.toml` for the shape.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use crate::alerts::AlertConfig;
use crate::scanner::ScannerConfig;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub bybit: BybitCfg,
    pub grid: GridCfg,
    pub risk: RiskCfg,
    #[serde(default)]
    pub alerts: AlertConfig,
    #[serde(default)]
    pub bot: BotSettings,
    #[serde(default)]
    pub autopilot: AutoPilotConfig,
}

/// Auto-pilot mode: the bot autonomously scans, picks symbols, and manages grids.
#[derive(Debug, Deserialize, Clone)]
pub struct AutoPilotConfig {
    /// Enable auto-pilot mode. When true, grid.symbol/lower_price/upper_price are ignored
    /// and the bot picks them automatically.
    #[serde(default)]
    pub enabled: bool,
    /// How often to re-scan the market (seconds). Default: 30 minutes.
    #[serde(default = "default_scan_interval")]
    pub scan_interval_secs: u64,
    /// Minimum score improvement to justify switching symbols.
    #[serde(default = "default_switch_threshold")]
    pub switch_threshold: f64,
    /// Maximum number of simultaneous grids (symbols). Default: 1.
    #[serde(default = "default_max_grids")]
    pub max_grids: usize,
    /// Scanner configuration.
    #[serde(default)]
    pub scanner: ScannerConfig,
}

impl Default for AutoPilotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_secs: default_scan_interval(),
            switch_threshold: default_switch_threshold(),
            max_grids: default_max_grids(),
            scanner: ScannerConfig::default(),
        }
    }
}

fn default_scan_interval() -> u64 { 1800 }  // 30 minutes
fn default_switch_threshold() -> f64 { 15.0 } // need 15+ score improvement to switch
fn default_max_grids() -> usize { 1 }

#[derive(Debug, Deserialize)]
pub struct BybitCfg {
    pub api_key: String,
    pub api_secret: String,
    #[serde(default = "default_true")]
    pub testnet: bool,
    #[serde(default = "default_recv_window")]
    pub recv_window_ms: u64,
    /// Account type for wallet-balance lookups. For testnet this is usually "UNIFIED".
    #[serde(default = "default_account_type")]
    pub account_type: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GridCfg {
    pub symbol: String,
    pub lower_price: f64,
    pub upper_price: f64,
    pub levels: u32,
    /// Quantity in base asset per order (e.g., BTC amount per grid line).
    pub qty_per_level_base: f64,
    /// How often to poll open orders to detect fills (seconds).
    #[serde(default = "default_poll_secs")]
    pub poll_secs: u64,
    /// If true, cancel all open orders on the symbol at startup before placing the grid.
    #[serde(default = "default_true")]
    pub cancel_existing_on_start: bool,
    /// Dynamic grid rebalancing: if price exits the band, shift the grid to re-center.
    #[serde(default)]
    pub auto_rebalance: bool,
    /// Trailing profit: lock in gains when grid fills accumulate beyond this threshold (USDT).
    #[serde(default)]
    pub trailing_profit_threshold: f64,
}

#[derive(Debug, Deserialize)]
pub struct RiskCfg {
    pub starting_equity_quote: f64,
    #[serde(default = "default_max_total")] pub max_total_exposure: f64,
    #[serde(default = "default_max_market")] pub max_per_market: f64,
    #[serde(default = "default_max_symbol")] pub max_per_symbol: f64,
    #[serde(default = "default_max_strategy")] pub max_per_strategy: f64,
    #[serde(default = "default_daily_loss")] pub daily_loss_limit: f64,
    #[serde(default = "default_weekly_loss")] pub weekly_loss_limit: f64,
    #[serde(default = "default_drawdown")] pub max_drawdown: f64,
}

#[derive(Debug, Deserialize)]
pub struct BotSettings {
    /// Dry-run mode: simulate orders without hitting the exchange.
    #[serde(default)]
    pub dry_run: bool,
    /// Path for the SQLite database. Default: "trading-bot.db".
    #[serde(default = "default_db_path")]
    pub db_path: String,
    /// Max retries for API calls.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Circuit breaker threshold: consecutive failures before pausing.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker cooldown in seconds.
    #[serde(default = "default_circuit_breaker_cooldown")]
    pub circuit_breaker_cooldown_secs: u64,
    /// Graceful shutdown: seconds to wait for in-flight orders.
    #[serde(default = "default_shutdown_wait")]
    pub shutdown_wait_secs: u64,
}

impl Default for BotSettings {
    fn default() -> Self {
        Self {
            dry_run: false,
            db_path: default_db_path(),
            max_retries: default_max_retries(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_cooldown_secs: default_circuit_breaker_cooldown(),
            shutdown_wait_secs: default_shutdown_wait(),
        }
    }
}

fn default_true() -> bool { true }
fn default_recv_window() -> u64 { 5_000 }
fn default_account_type() -> String { "UNIFIED".to_string() }
fn default_poll_secs() -> u64 { 3 }
fn default_max_total() -> f64 { 0.80 }
fn default_max_market() -> f64 { 0.40 }
fn default_max_symbol() -> f64 { 0.20 }
fn default_max_strategy() -> f64 { 0.15 }
fn default_daily_loss() -> f64 { 0.05 }
fn default_weekly_loss() -> f64 { 0.10 }
fn default_drawdown() -> f64 { 0.15 }
fn default_db_path() -> String { "trading-bot.db".to_string() }
fn default_max_retries() -> u32 { 3 }
fn default_circuit_breaker_threshold() -> u32 { 5 }
fn default_circuit_breaker_cooldown() -> u64 { 60 }
fn default_shutdown_wait() -> u64 { 10 }

impl BotConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading config at {}", path.as_ref().display()))?;
        let cfg: BotConfig = toml::from_str(&text).context("parsing config TOML")?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        // In auto-pilot mode, grid params are computed dynamically
        if !self.autopilot.enabled {
            anyhow::ensure!(self.grid.levels >= 2, "grid.levels must be >= 2");
            anyhow::ensure!(
                self.grid.lower_price > 0.0 && self.grid.upper_price > self.grid.lower_price,
                "grid.lower_price must be > 0 and < grid.upper_price",
            );
            anyhow::ensure!(self.grid.qty_per_level_base > 0.0, "grid.qty_per_level_base must be > 0");
        }
        if !self.bot.dry_run {
            anyhow::ensure!(!self.bybit.api_key.is_empty(), "bybit.api_key must be set (or enable dry_run)");
            anyhow::ensure!(!self.bybit.api_secret.is_empty(), "bybit.api_secret must be set (or enable dry_run)");
        }
        anyhow::ensure!(self.risk.starting_equity_quote > 0.0, "risk.starting_equity_quote must be > 0");
        Ok(())
    }
}
