//! TOML config loader for the trading bot.
//!
//! Pass the config path as the first CLI argument. Never commit real creds;
//! see `config.example.toml` for the shape.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use crate::alerts::AlertConfig;
use crate::scanner::ScannerConfig;
use crate::slippage::SlippageConfig;
use crate::tax::TaxConfig;

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
    #[serde(default)]
    pub zerodha: Option<ZerodhaCfg>,
    #[serde(default)]
    pub funding_arb: Option<FundingArbCfg>,
    #[serde(default)]
    pub tax: TaxConfig,
    #[serde(default)]
    pub slippage: SlippageConfig,
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

fn default_scan_interval() -> u64 {
    1800
} // 30 minutes
fn default_switch_threshold() -> f64 {
    15.0
} // need 15+ score improvement to switch
fn default_max_grids() -> usize {
    1
}

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

/// Zerodha Kite Connect credentials. Optional — only needed for Indian markets.
#[derive(Debug, Deserialize, Clone)]
pub struct ZerodhaCfg {
    pub api_key: String,
    pub api_secret: String,
    /// Access token from the daily login flow. Can also be set at runtime.
    #[serde(default)]
    pub access_token: Option<String>,
}

impl ZerodhaCfg {
    pub fn into_venue_config(self) -> crate::venues::zerodha::Config {
        crate::venues::zerodha::Config {
            credentials: Some(crate::venues::zerodha::Credentials {
                api_key: self.api_key,
                api_secret: self.api_secret,
                access_token: self.access_token,
            }),
        }
    }
}

/// Funding rate arbitrage configuration. Optional — only for `--funding-arb` mode.
#[derive(Debug, Deserialize, Clone)]
pub struct FundingArbCfg {
    /// Minimum funding rate to enter (e.g., 0.0001 = 0.01% per 8h).
    #[serde(default = "default_entry_threshold")]
    pub entry_threshold: f64,
    /// Exit when funding drops below this.
    #[serde(default = "default_exit_threshold")]
    pub exit_threshold: f64,
    /// Minimum 24h turnover in USDT.
    #[serde(default = "default_arb_min_turnover")]
    pub min_turnover: f64,
    /// Max fraction of equity per position.
    #[serde(default = "default_arb_max_pos_pct")]
    pub max_position_pct: f64,
    /// Leverage for the short perp leg.
    #[serde(default = "default_arb_leverage")]
    pub leverage: u32,
    /// Max simultaneous positions.
    #[serde(default = "default_arb_max_positions")]
    pub max_positions: usize,
    /// Min consecutive positive funding intervals before entry.
    #[serde(default = "default_arb_min_positive")]
    pub min_positive_intervals: usize,
    /// Max unrealised loss as fraction of position before force-exit.
    #[serde(default = "default_arb_max_loss")]
    pub max_unrealised_loss_pct: f64,
    /// How often to scan funding rates (seconds).
    #[serde(default = "default_arb_scan_interval")]
    pub scan_interval_secs: u64,
}

fn default_entry_threshold() -> f64 {
    0.0001
}
fn default_exit_threshold() -> f64 {
    -0.00005
}
fn default_arb_min_turnover() -> f64 {
    5_000_000.0
}
fn default_arb_max_pos_pct() -> f64 {
    0.15
}
fn default_arb_leverage() -> u32 {
    1
}
fn default_arb_max_positions() -> usize {
    3
}
fn default_arb_min_positive() -> usize {
    3
}
fn default_arb_max_loss() -> f64 {
    0.02
}
fn default_arb_scan_interval() -> u64 {
    300
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
    #[serde(default = "default_max_total")]
    pub max_total_exposure: f64,
    #[serde(default = "default_max_market")]
    pub max_per_market: f64,
    #[serde(default = "default_max_symbol")]
    pub max_per_symbol: f64,
    #[serde(default = "default_max_strategy")]
    pub max_per_strategy: f64,
    #[serde(default = "default_daily_loss")]
    pub daily_loss_limit: f64,
    #[serde(default = "default_weekly_loss")]
    pub weekly_loss_limit: f64,
    #[serde(default = "default_drawdown")]
    pub max_drawdown: f64,
    #[serde(default = "default_max_consecutive_losses")]
    pub max_consecutive_losses: u32,
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

fn default_true() -> bool {
    true
}
fn default_recv_window() -> u64 {
    5_000
}
fn default_account_type() -> String {
    "UNIFIED".to_string()
}
fn default_poll_secs() -> u64 {
    3
}
fn default_max_total() -> f64 {
    0.80
}
fn default_max_market() -> f64 {
    0.40
}
fn default_max_symbol() -> f64 {
    0.20
}
fn default_max_strategy() -> f64 {
    0.15
}
fn default_daily_loss() -> f64 {
    0.05
}
fn default_weekly_loss() -> f64 {
    0.10
}
fn default_drawdown() -> f64 {
    0.15
}
fn default_max_consecutive_losses() -> u32 {
    3
}
fn default_db_path() -> String {
    "trading-bot.db".to_string()
}
fn default_max_retries() -> u32 {
    3
}
fn default_circuit_breaker_threshold() -> u32 {
    5
}
fn default_circuit_breaker_cooldown() -> u64 {
    60
}
fn default_shutdown_wait() -> u64 {
    10
}

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
            anyhow::ensure!(
                self.grid.qty_per_level_base > 0.0,
                "grid.qty_per_level_base must be > 0"
            );
        }
        if !self.bot.dry_run {
            anyhow::ensure!(
                !self.bybit.api_key.is_empty(),
                "bybit.api_key must be set (or enable dry_run)"
            );
            anyhow::ensure!(
                !self.bybit.api_secret.is_empty(),
                "bybit.api_secret must be set (or enable dry_run)"
            );
        }
        anyhow::ensure!(
            self.risk.starting_equity_quote > 0.0,
            "risk.starting_equity_quote must be > 0"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml(overrides: &str) -> String {
        format!(
            r#"
[bybit]
api_key = "test_key"
api_secret = "test_secret"
testnet = true

[grid]
symbol = "BTCUSDT"
lower_price = 60000.0
upper_price = 70000.0
levels = 10
qty_per_level_base = 0.001

[risk]
starting_equity_quote = 10000.0

[bot]
dry_run = true

{overrides}
"#
        )
    }

    #[test]
    fn loads_minimal_config() {
        let toml = minimal_toml("");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.grid.symbol, "BTCUSDT");
        assert_eq!(cfg.grid.levels, 10);
        assert!(cfg.bot.dry_run);
    }

    #[test]
    fn rejects_zero_levels() {
        let toml = minimal_toml("").replace("levels = 10", "levels = 1");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_inverted_range() {
        let toml = minimal_toml("").replace("lower_price = 60000.0", "lower_price = 80000.0");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_empty_keys_in_live_mode() {
        let toml = minimal_toml("")
            .replace("dry_run = true", "dry_run = false")
            .replace("api_key = \"test_key\"", "api_key = \"\"");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn allows_empty_keys_in_dry_run() {
        let toml = minimal_toml("")
            .replace("api_key = \"test_key\"", "api_key = \"\"")
            .replace("api_secret = \"test_secret\"", "api_secret = \"\"");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn autopilot_skips_grid_validation() {
        let toml = minimal_toml("[autopilot]\nenabled = true")
            .replace("levels = 10", "levels = 0")
            .replace("qty_per_level_base = 0.001", "qty_per_level_base = 0.0");
        let cfg: BotConfig = toml::from_str(&toml).unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn defaults_are_sensible() {
        let settings = BotSettings::default();
        assert!(!settings.dry_run);
        assert_eq!(settings.max_retries, 3);
        assert_eq!(settings.circuit_breaker_threshold, 5);
    }
}
