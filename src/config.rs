//! TOML config loader for the trading bot.
//!
//! Pass the config path as the first CLI argument. Never commit real creds;
//! see `config.example.toml` for the shape.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::alerts::AlertConfig;
use crate::orchestrator::OrchestratorConfig;
use crate::scanner::ScannerConfig;
use crate::slippage::SlippageConfig;
use crate::tax::TaxConfig;
use crate::telegram::TelegramConfig;

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
    pub coindcx: Option<CoinDCXCfg>,
    #[serde(default)]
    pub funding_arb: Option<FundingArbCfg>,
    #[serde(default)]
    pub tax: TaxConfig,
    #[serde(default)]
    pub slippage: SlippageConfig,
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub sizing: SizingCfg,
}

impl BotConfig {
    /// Build a `Venue` from this config.
    /// When `[zerodha]` is present and `venue_override` is "zerodha", use Zerodha;
    /// otherwise default to Bybit.
    pub fn build_venue(&self, venue_override: Option<&str>) -> crate::venue::Venue {
        let use_zerodha = venue_override
            .map(|v| v.eq_ignore_ascii_case("zerodha") || v.eq_ignore_ascii_case("kite"))
            .unwrap_or(false);
        let use_coindcx = venue_override
            .map(|v| v.eq_ignore_ascii_case("coindcx"))
            .unwrap_or(false);

        if use_coindcx {
            let ccfg = self
                .coindcx
                .clone()
                .expect("[coindcx] config section required when --venue coindcx");
            crate::venue::Venue::new_coindcx(ccfg.into_venue_config())
        } else if use_zerodha {
            let zcfg = self
                .zerodha
                .clone()
                .expect("[zerodha] config section required when --venue zerodha");
            let kite_cfg = zcfg.into_venue_config();
            let exchange = zcfg_exchange(&self.zerodha);
            let product = zcfg_product(&self.zerodha);
            crate::venue::Venue::new_zerodha(kite_cfg, exchange, product)
        } else {
            let bybit_cfg = crate::venues::bybit::Config {
                testnet: self.bybit.testnet,
                credentials: Some(crate::venues::bybit::Credentials {
                    api_key: self.bybit.api_key.clone(),
                    api_secret: self.bybit.api_secret.clone(),
                }),
                recv_window_ms: self.bybit.recv_window_ms,
            };
            crate::venue::Venue::new_bybit(bybit_cfg, self.bybit.account_type.clone())
        }
    }
}

fn zcfg_exchange(z: &Option<ZerodhaCfg>) -> String {
    z.as_ref()
        .and_then(|c| c.exchange.clone())
        .unwrap_or_else(|| "NSE".to_string())
}

fn zcfg_product(z: &Option<ZerodhaCfg>) -> String {
    z.as_ref()
        .and_then(|c| c.product.clone())
        .unwrap_or_else(|| "CNC".to_string())
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
    /// Path to a file containing the access token (written by `zooni-kite-login`).
    /// If set and the file exists, overrides `access_token`.
    #[serde(default)]
    pub access_token_file: Option<String>,
    /// Exchange code: NSE, BSE, NFO, MCX, etc. Default: NSE.
    #[serde(default)]
    pub exchange: Option<String>,
    /// Product code: CNC (delivery), MIS (intraday), NRML (F&O carry). Default: CNC.
    #[serde(default)]
    pub product: Option<String>,
}

impl ZerodhaCfg {
    pub fn into_venue_config(self) -> crate::venues::zerodha::Config {
        let mut token = self.access_token;
        if let Some(ref path) = self.access_token_file {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let trimmed = contents.trim().to_string();
                if !trimmed.is_empty() {
                    token = Some(trimmed);
                }
            }
        }
        crate::venues::zerodha::Config {
            credentials: Some(crate::venues::zerodha::Credentials {
                api_key: self.api_key,
                api_secret: self.api_secret,
                access_token: token,
            }),
        }
    }

    pub fn resolved_access_token(&self) -> Option<String> {
        if let Some(ref path) = self.access_token_file {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let trimmed = contents.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        self.access_token.clone()
    }
}

/// CoinDCX credentials. Optional �� only needed for Indian crypto trading.
#[derive(Debug, Deserialize, Clone)]
pub struct CoinDCXCfg {
    pub api_key: String,
    pub api_secret: String,
}

impl CoinDCXCfg {
    pub fn into_venue_config(self) -> crate::venues::coindcx::Config {
        crate::venues::coindcx::Config {
            credentials: Some(crate::venues::coindcx::Credentials {
                api_key: self.api_key,
                api_secret: self.api_secret,
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
    /// Estimated annual borrow cost (%) for the spot leg hedge.
    #[serde(default = "default_arb_borrow_cost")]
    pub borrow_cost_annual_pct: f64,
    /// Minimum net yield (%) after borrow costs to justify entry.
    #[serde(default = "default_arb_min_net_yield")]
    pub min_net_yield_pct: f64,
    /// How often to scan funding rates (seconds).
    #[serde(default = "default_arb_scan_interval")]
    pub scan_interval_secs: u64,
    /// EMA decay factor for funding rate smoothing (0 < decay ≤ 1). Default: 0.8.
    #[serde(default = "default_arb_funding_ema_decay")]
    pub funding_ema_decay: f64,
    /// Number of recent funding intervals to keep in the ring buffer. Default: 8.
    #[serde(default = "default_arb_funding_lookback")]
    pub funding_lookback: usize,
    /// Require a positive funding rate trend (rates increasing) before entry. Default: true.
    #[serde(default = "default_true")]
    pub require_positive_trend: bool,
    /// Number of consecutive below-threshold intervals required before exiting (graceful exit). Default: 2.
    #[serde(default = "default_arb_exit_confirmation")]
    pub exit_confirmation_intervals: u32,
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
fn default_arb_borrow_cost() -> f64 {
    10.0
}
fn default_arb_min_net_yield() -> f64 {
    5.0
}
fn default_arb_scan_interval() -> u64 {
    300
}
fn default_arb_funding_ema_decay() -> f64 {
    0.8
}
fn default_arb_funding_lookback() -> usize {
    8
}
fn default_arb_exit_confirmation() -> u32 {
    2
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
    /// Cancel all buy orders when regime transitions from Ranging to Trending,
    /// to prevent accumulating base asset into a directional move.
    #[serde(default = "default_true")]
    pub cancel_buys_on_trend: bool,
    /// Automatically re-center the grid if price escapes the range by
    /// `recenter_threshold_pct` beyond the upper or lower band.
    #[serde(default)]
    pub auto_recenter: bool,
    /// How far outside the grid band price must move before triggering auto-recenter
    /// (as a fraction of the band edge, e.g. 0.15 = 15%). Default: 0.15.
    #[serde(default = "default_recenter_threshold")]
    pub recenter_threshold_pct: f64,
    /// Use post-only (maker) orders for limit placements to pay maker fees (0.02%)
    /// instead of taker fees (0.1%). On PostOnly rejection, the bot adjusts price
    /// by one tick and retries once. If still rejected, skips the level this cycle.
    /// Default: true.
    #[serde(default = "default_true")]
    pub use_post_only: bool,
    /// Only start placing grid orders when the current price is within
    /// `vwap_threshold_pct` of VWAP. Reduces adverse selection on grid start.
    /// Default: false (filter disabled).
    #[serde(default)]
    pub vwap_entry_filter: bool,
    /// Maximum allowed fractional deviation from VWAP to start the grid.
    /// E.g. 0.005 = allow grid start if price is within 0.5% of VWAP.
    /// Only used when `vwap_entry_filter = true`. Default: 0.005.
    #[serde(default = "default_vwap_threshold_pct")]
    pub vwap_threshold_pct: f64,
    /// Venue / exchange identifier used to look up the trading-hours calendar.
    /// E.g. `"zerodha"`, `"NSE"`, `"bybit"`, `"crypto"`. Default: `"bybit"`.
    /// The value is matched case-insensitively in `market_hours::is_market_open`.
    #[serde(default = "default_venue")]
    pub venue: String,
    /// When `true`, the bot skips tick processing outside the venue's trading hours
    /// and cancels all open buy orders 15 minutes before market close.
    /// For 24/7 venues (Bybit, crypto) this has no observable effect.
    /// Default: `true`.
    #[serde(default = "default_true")]
    pub respect_market_hours: bool,
}

fn default_venue() -> String {
    "bybit".to_string()
}
fn default_recenter_threshold() -> f64 {
    0.15
}

fn default_vwap_threshold_pct() -> f64 {
    0.005
}

#[derive(Debug, Deserialize)]
pub struct RiskCfg {
    pub starting_equity_quote: f64,
    /// Trading fee rate per fill (maker/taker). Default 0.1% = 0.001.
    #[serde(default = "default_fee_rate")]
    pub fee_rate: f64,
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
    /// IANA timezone for daily/weekly loss reset (e.g. "Asia/Kolkata"). Default: "UTC".
    #[serde(default = "default_loss_reset_tz")]
    pub loss_reset_timezone: String,
}

fn default_loss_reset_tz() -> String {
    "UTC".to_string()
}

#[derive(Debug, Deserialize)]
pub struct BotSettings {
    /// Dry-run mode: simulate orders without hitting the exchange.
    #[serde(default)]
    pub dry_run: bool,
    /// Path for the SQLite database. Default: "zooni.db".
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
    /// When true, query the exchange for each DB-open order on startup and reconcile
    /// any fills or cancellations that occurred while the bot was offline.
    /// Default: true.
    #[serde(default = "default_true")]
    pub reconcile_on_startup: bool,
    /// How far back (in seconds) to look for exchange fills during startup reconciliation.
    /// Default: 86400 (24 hours).
    #[serde(default = "default_reconcile_lookback_secs")]
    pub reconcile_lookback_secs: u64,
    /// Optional password for the web dashboard. When set, the dashboard requires
    /// HTTP Basic Auth (username ignored, password must match) or a Bearer token.
    /// When `None`, the dashboard is publicly accessible.
    #[serde(default)]
    pub dashboard_password: Option<String>,
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
            reconcile_on_startup: true,
            reconcile_lookback_secs: default_reconcile_lookback_secs(),
            dashboard_password: None,
        }
    }
}

fn default_reconcile_lookback_secs() -> u64 {
    86400 // 24 hours
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
fn default_fee_rate() -> f64 {
    0.001
}

// ── Sizing config ────────────────────────────────────────────────────────────

/// Position-sizing tuning knobs.
#[derive(Debug, Deserialize, Clone)]
pub struct SizingCfg {
    /// Fractional-Kelly multiplier applied to the raw Kelly fraction.
    /// 0.15 (15% of full Kelly) gives ~50% of returns with far lower drawdown.
    #[serde(default = "default_kelly_fraction")]
    pub kelly_fraction: f64,
    /// Minimum notional order size in quote currency. Orders below this are skipped.
    #[serde(default = "default_min_order_notional")]
    pub min_order_notional: f64,
}

impl Default for SizingCfg {
    fn default() -> Self {
        Self {
            kelly_fraction: default_kelly_fraction(),
            min_order_notional: default_min_order_notional(),
        }
    }
}

fn default_kelly_fraction() -> f64 {
    0.15
}
fn default_min_order_notional() -> f64 {
    10.0
}
fn default_db_path() -> String {
    "zooni.db".to_string()
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
        anyhow::ensure!(
            self.risk.max_total_exposure > 0.0,
            "risk.max_total_exposure must be > 0"
        );
        anyhow::ensure!(
            self.risk.daily_loss_limit > 0.0,
            "risk.daily_loss_limit must be > 0"
        );
        anyhow::ensure!(
            self.risk.max_drawdown > 0.0 && self.risk.max_drawdown <= 1.0,
            "risk.max_drawdown must be in (0, 1]"
        );
        if let Some(ref fa) = self.funding_arb {
            anyhow::ensure!(
                fa.entry_threshold > 0.0,
                "funding_arb.entry_threshold must be > 0"
            );
            anyhow::ensure!(
                fa.max_positions > 0,
                "funding_arb.max_positions must be > 0"
            );
        }
        Ok(())
    }
}

// ── Optimal parameters (written by zooni-optimize, read at bot startup) ──────

/// Optimal grid parameters produced by the walk-forward optimizer.
/// Stored as `optimal_params.json` alongside the bot binary.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OptimalParams {
    /// Trading symbol these parameters apply to (e.g. "BTCUSDT").
    pub symbol: String,
    /// Number of grid levels.
    pub levels: u32,
    /// Grid spacing as a fraction of price (e.g. 0.008 = 0.8%).
    pub spacing_pct: f64,
    /// Absolute lower price boundary for the grid.
    pub lower_price: f64,
    /// Absolute upper price boundary for the grid.
    pub upper_price: f64,
    /// RFC-3339 timestamp when the file was generated.
    pub generated_at: String,
    /// Sharpe ratio achieved on the training window.
    pub train_sharpe: f64,
    /// Sharpe ratio achieved on the held-out test window.
    pub test_sharpe: f64,
}

/// Load optimal parameters from a JSON file written by `zooni-optimize`.
///
/// Returns `None` (and logs a warning) if the file does not exist or cannot
/// be parsed — the bot will then fall back to its TOML config values.
///
/// # Example
/// ```no_run
/// use zooni::config::load_optimal_params;
/// if let Some(p) = load_optimal_params("optimal_params.json") {
///     println!("Loaded optimal params: {} levels, lower={}", p.levels, p.lower_price);
/// }
/// ```
pub fn load_optimal_params(path: &str) -> Option<OptimalParams> {
    match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<OptimalParams>(&json) {
            Ok(params) => {
                tracing::info!(
                    path,
                    symbol = %params.symbol,
                    levels = params.levels,
                    lower_price = params.lower_price,
                    upper_price = params.upper_price,
                    test_sharpe = params.test_sharpe,
                    "Loaded optimal grid parameters from optimizer output"
                );
                Some(params)
            }
            Err(e) => {
                tracing::warn!(path, error = %e, "Failed to parse optimal_params.json — using config defaults");
                None
            }
        },
        Err(_) => {
            // File simply not present — this is the normal state before the first optimizer run.
            tracing::debug!(
                path,
                "optimal_params.json not found — using config defaults"
            );
            None
        }
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
