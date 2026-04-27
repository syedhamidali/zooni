//! Integration tests for the risk engine.
//!
//! Simulates multi-market, multi-strategy scenarios end-to-end.

use zooni::risk::{Market, Reject, RiskConfig, RiskEngine};

fn engine_with_config(equity: f64, cfg: RiskConfig) -> RiskEngine {
    RiskEngine::new(cfg, equity).0
}

fn default_engine(equity: f64) -> RiskEngine {
    engine_with_config(equity, RiskConfig::default())
}

#[test]
fn multi_market_exposure_tracking() {
    let mut e = default_engine(100_000.0);

    // Trade crypto and NSE simultaneously
    e.on_fill(Market::Crypto, "BTCUSDT", "grid", 10_000.0);
    e.on_fill(Market::NseEquity, "RELIANCE", "dca", 5_000.0);

    // Both should be within limits
    assert!(
        e.pretrade_check(Market::Crypto, "ETHUSDT", "grid", 4_000.0)
            .is_ok()
    );
    assert!(
        e.pretrade_check(Market::NseEquity, "TCS", "dca", 5_000.0)
            .is_ok()
    );

    // But hitting crypto market limit (40% = 40k) should fail
    assert_eq!(
        e.pretrade_check(Market::Crypto, "SOLUSDT", "grid", 31_000.0),
        Err(Reject::ExceedsMarketExposure),
    );
}

#[test]
fn drawdown_recovery_allows_trading() {
    let mut e = default_engine(10_000.0);

    // Drop to drawdown limit
    e.set_equity(8_400.0); // 16% drawdown → rejected
    assert!(
        e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0)
            .is_err()
    );

    // Recover above drawdown
    e.set_equity(9_000.0); // 10% drawdown from 10k peak → ok
    assert!(
        e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0)
            .is_ok()
    );
}

#[test]
fn cascading_limits_checked_in_order() {
    let e = default_engine(10_000.0);
    e.trip_kill_switch();

    // Kill switch should be checked before any exposure limit
    assert_eq!(
        e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100_000.0),
        Err(Reject::KillSwitchTripped),
    );
}

#[test]
fn exposure_never_goes_negative() {
    let mut e = default_engine(10_000.0);
    e.on_fill(Market::Crypto, "BTCUSDT", "grid", 1_000.0);
    // Over-close: reduce by more than current exposure
    e.on_fill(Market::Crypto, "BTCUSDT", "grid", -5_000.0);
    // Exposure should be clamped to 0, not negative
    assert_eq!(e.exposure_total(), 0.0);
}

#[test]
fn tight_risk_config() {
    let cfg = RiskConfig {
        max_total_exposure: 0.10,
        max_per_market: 0.05,
        max_per_symbol: 0.03,
        max_per_strategy: 0.02,
        daily_loss_limit: 0.01,
        weekly_loss_limit: 0.03,
        max_drawdown: 0.05,
        max_consecutive_losses: 3,
        dry_run: false,
    };
    let e = engine_with_config(100_000.0, cfg);

    // 2% strategy cap = 2000
    assert!(
        e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 1_900.0)
            .is_ok()
    );
    assert_eq!(
        e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 2_100.0),
        Err(Reject::ExceedsStrategyExposure),
    );
}
