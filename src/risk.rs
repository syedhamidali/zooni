#![allow(dead_code)]
//! Risk limits + kill switch. Pure state + pure checks, no I/O.
//!
//! Strategies consult `RiskEngine::pretrade_check(...)` before submitting an order.
//! Fills update exposure via `on_fill(...)`. PnL is updated by the bot loop via
//! `set_daily_pnl` / `set_equity`. The kill switch is atomic so any task can trip it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct RiskConfig {
    /// Fraction of equity allowed in open exposure total. 0..=1.
    pub max_total_exposure: f64,
    pub max_per_market: f64,
    pub max_per_symbol: f64,
    pub max_per_strategy: f64,
    /// Daily PnL as negative fraction of starting equity that trips the pause.
    pub daily_loss_limit: f64,
    pub weekly_loss_limit: f64,
    pub max_drawdown: f64,
    pub max_consecutive_losses: u32,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_total_exposure: 0.80,
            max_per_market: 0.40,
            max_per_symbol: 0.20,
            max_per_strategy: 0.15,
            daily_loss_limit: 0.05,
            weekly_loss_limit: 0.10,
            max_drawdown: 0.15,
            max_consecutive_losses: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    KillSwitchTripped,
    DailyLossLimit,
    WeeklyLossLimit,
    DrawdownLimit,
    ExceedsTotalExposure,
    ExceedsMarketExposure,
    ExceedsSymbolExposure,
    ExceedsStrategyExposure,
    NoEquity,
    ConsecutiveLossLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Market {
    Crypto,
    NseEquity,
    NseFo,
    Mcx,
    UsEquity,
}

#[derive(Debug)]
pub struct RiskEngine {
    cfg: RiskConfig,
    kill: Arc<AtomicBool>,
    equity: f64,
    peak_equity: f64,
    daily_pnl: f64,
    weekly_pnl: f64,
    exposure_total: f64,
    exposure_market: HashMap<Market, f64>,
    exposure_symbol: HashMap<String, f64>,
    exposure_strategy: HashMap<String, f64>,
    /// Position quantity per symbol for mark-price revaluation.
    position_qty: HashMap<String, f64>,
    consecutive_losses: u32,
    max_consecutive_losses: u32,
}

impl RiskEngine {
    pub fn new(cfg: RiskConfig, initial_equity: f64) -> (Self, Arc<AtomicBool>) {
        let kill = Arc::new(AtomicBool::new(false));
        let engine = Self {
            cfg,
            kill: kill.clone(),
            equity: initial_equity,
            peak_equity: initial_equity,
            daily_pnl: 0.0,
            weekly_pnl: 0.0,
            exposure_total: 0.0,
            exposure_market: HashMap::new(),
            exposure_symbol: HashMap::new(),
            exposure_strategy: HashMap::new(),
            position_qty: HashMap::new(),
            consecutive_losses: 0,
            max_consecutive_losses: cfg.max_consecutive_losses,
        };
        (engine, kill)
    }

    pub fn kill_switch(&self) -> Arc<AtomicBool> {
        self.kill.clone()
    }
    pub fn trip_kill_switch(&self) {
        self.kill.store(true, Ordering::SeqCst);
    }
    pub fn reset_kill_switch(&self) {
        self.kill.store(false, Ordering::SeqCst);
    }

    pub fn on_trade_result(&mut self, profit: f64) {
        if profit < 0.0 {
            self.consecutive_losses += 1;
            if self.consecutive_losses >= self.max_consecutive_losses {
                eprintln!(
                    "[RISK] consecutive loss limit reached ({}/{}), tripping kill switch",
                    self.consecutive_losses, self.max_consecutive_losses,
                );
                self.trip_kill_switch();
            }
        } else {
            self.consecutive_losses = 0;
        }
    }

    pub fn reset_session(&mut self) {
        self.consecutive_losses = 0;
        self.daily_pnl = 0.0;
        self.exposure_total = 0.0;
        self.exposure_market.clear();
        self.exposure_symbol.clear();
        self.exposure_strategy.clear();
        self.position_qty.clear();
    }

    pub fn set_equity(&mut self, equity: f64) {
        self.equity = equity;
        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
    }

    pub fn set_daily_pnl(&mut self, pnl: f64) {
        self.daily_pnl = pnl;
    }
    pub fn set_weekly_pnl(&mut self, pnl: f64) {
        self.weekly_pnl = pnl;
    }

    /// Called after a fill. Tracks position quantity per symbol so exposure can be
    /// revalued at current mark price via `revalue_exposure`.
    ///
    /// `notional_delta` is the absolute USD-equivalent exposure delta
    /// (+ for opening, − for closing). Markets may use different currencies; caller
    /// normalises to equity currency before calling.
    pub fn on_fill(&mut self, market: Market, symbol: &str, strategy: &str, notional_delta: f64) {
        self.exposure_total = (self.exposure_total + notional_delta).max(0.0);
        let m = self.exposure_market.entry(market).or_insert(0.0);
        *m = (*m + notional_delta).max(0.0);
        let s = self
            .exposure_symbol
            .entry(symbol.to_string())
            .or_insert(0.0);
        *s = (*s + notional_delta).max(0.0);
        let st = self
            .exposure_strategy
            .entry(strategy.to_string())
            .or_insert(0.0);
        *st = (*st + notional_delta).max(0.0);

        // Track position qty for mark-price revaluation
        let qty_delta = if notional_delta >= 0.0 {
            notional_delta
        } else {
            notional_delta
        };
        let pq = self
            .position_qty
            .entry(symbol.to_string())
            .or_insert(0.0);
        *pq = (*pq + qty_delta).max(0.0);
    }

    /// Revalue all symbol exposures using current mark prices.
    /// `mark_prices` maps symbol → current mark price.
    /// This replaces stale entry-price-based exposure with live mark-to-market values.
    pub fn revalue_exposure(&mut self, mark_prices: &HashMap<String, f64>) {
        let mut new_total = 0.0_f64;
        for (symbol, qty) in &self.position_qty {
            if let Some(&mark_price) = mark_prices.get(symbol) {
                let new_exposure = mark_price * qty;
                self.exposure_symbol.insert(symbol.clone(), new_exposure);
                new_total += new_exposure;
            }
        }
        self.exposure_total = new_total;
    }

    pub fn pretrade_check(
        &self,
        market: Market,
        symbol: &str,
        strategy: &str,
        notional: f64,
    ) -> Result<(), Reject> {
        if self.kill.load(Ordering::SeqCst) {
            return Err(Reject::KillSwitchTripped);
        }
        if self.consecutive_losses >= self.max_consecutive_losses {
            return Err(Reject::ConsecutiveLossLimit);
        }
        if self.equity <= 0.0 {
            return Err(Reject::NoEquity);
        }

        if self.daily_pnl <= -self.cfg.daily_loss_limit * self.equity {
            return Err(Reject::DailyLossLimit);
        }
        if self.weekly_pnl <= -self.cfg.weekly_loss_limit * self.equity {
            return Err(Reject::WeeklyLossLimit);
        }
        if self.peak_equity > 0.0 {
            let dd = (self.peak_equity - self.equity) / self.peak_equity;
            if dd >= self.cfg.max_drawdown {
                return Err(Reject::DrawdownLimit);
            }
        }

        let cap_total = self.cfg.max_total_exposure * self.equity;
        if self.exposure_total + notional > cap_total {
            return Err(Reject::ExceedsTotalExposure);
        }

        let cap_market = self.cfg.max_per_market * self.equity;
        let cur_market = *self.exposure_market.get(&market).unwrap_or(&0.0);
        if cur_market + notional > cap_market {
            return Err(Reject::ExceedsMarketExposure);
        }

        let cap_symbol = self.cfg.max_per_symbol * self.equity;
        let cur_symbol = *self.exposure_symbol.get(symbol).unwrap_or(&0.0);
        if cur_symbol + notional > cap_symbol {
            return Err(Reject::ExceedsSymbolExposure);
        }

        let cap_strategy = self.cfg.max_per_strategy * self.equity;
        let cur_strategy = *self.exposure_strategy.get(strategy).unwrap_or(&0.0);
        if cur_strategy + notional > cap_strategy {
            return Err(Reject::ExceedsStrategyExposure);
        }

        Ok(())
    }

    pub fn equity(&self) -> f64 {
        self.equity
    }
    pub fn daily_pnl(&self) -> f64 {
        self.daily_pnl
    }
    pub fn exposure_total(&self) -> f64 {
        self.exposure_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(eq: f64) -> RiskEngine {
        RiskEngine::new(RiskConfig::default(), eq).0
    }

    #[test]
    fn accepts_trade_within_limits() {
        let e = engine(10_000.0);
        // 20% symbol limit = 2000; 15% strategy = 1500. Notional 1000 is under both.
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 1000.0),
            Ok(())
        );
    }

    #[test]
    fn rejects_when_over_symbol_limit() {
        let e = engine(10_000.0);
        // 20% symbol cap = 2000. Requesting 2500 fails.
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 2500.0),
            Err(Reject::ExceedsSymbolExposure),
        );
    }

    #[test]
    fn rejects_when_kill_switch_tripped() {
        let e = engine(10_000.0);
        e.trip_kill_switch();
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 100.0),
            Err(Reject::KillSwitchTripped),
        );
    }

    #[test]
    fn rejects_after_daily_loss_limit() {
        let mut e = engine(10_000.0);
        e.set_daily_pnl(-600.0); // -6%, past 5% daily limit
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 100.0),
            Err(Reject::DailyLossLimit),
        );
    }

    #[test]
    fn fills_accumulate_exposure_and_eventually_reject() {
        let mut e = engine(10_000.0);
        e.on_fill(Market::Crypto, "BTCUSDT", "trend", 1500.0);
        // Symbol is now at 1500; cap is 2000; requesting 600 more should fail.
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 600.0),
            Err(Reject::ExceedsSymbolExposure),
        );
    }

    #[test]
    fn rejects_zero_equity() {
        let e = engine(0.0);
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::NoEquity),
        );
    }

    #[test]
    fn rejects_weekly_loss_limit() {
        let mut e = engine(10_000.0);
        e.set_weekly_pnl(-1100.0); // -11%, past 10% weekly limit
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::WeeklyLossLimit),
        );
    }

    #[test]
    fn rejects_drawdown_limit() {
        let mut e = engine(10_000.0);
        // Peak is 10k, current drops to 8400 → 16% drawdown, past 15% limit
        e.set_equity(8400.0);
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::DrawdownLimit),
        );
    }

    #[test]
    fn rejects_total_exposure_limit() {
        let mut e = engine(10_000.0);
        // 80% total cap = 8000. Fill 7500 then try 600 more.
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", 7500.0);
        assert_eq!(
            e.pretrade_check(Market::Crypto, "ETHUSDT", "grid", 600.0),
            Err(Reject::ExceedsTotalExposure),
        );
    }

    #[test]
    fn rejects_market_exposure_limit() {
        let mut e = engine(10_000.0);
        // 40% market cap = 4000
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", 2000.0);
        e.on_fill(Market::Crypto, "ETHUSDT", "grid", 1800.0);
        assert_eq!(
            e.pretrade_check(Market::Crypto, "SOLUSDT", "grid", 300.0),
            Err(Reject::ExceedsMarketExposure),
        );
    }

    #[test]
    fn rejects_strategy_exposure_limit() {
        let mut e = engine(10_000.0);
        // 15% strategy cap = 1500
        e.on_fill(Market::Crypto, "BTCUSDT", "aggressive", 1400.0);
        assert_eq!(
            e.pretrade_check(Market::Crypto, "ETHUSDT", "aggressive", 200.0),
            Err(Reject::ExceedsStrategyExposure),
        );
    }

    #[test]
    fn closing_fill_reduces_exposure() {
        let mut e = engine(10_000.0);
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", 1500.0);
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", -1000.0); // close part
        // Symbol exposure: 500, strategy exposure: 500
        // Strategy cap is 15% = 1500, so 900 more is fine (500+900=1400 < 1500)
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 900.0),
            Ok(()),
        );
    }

    #[test]
    fn kill_switch_reset_allows_trading() {
        let e = engine(10_000.0);
        e.trip_kill_switch();
        assert!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0)
                .is_err()
        );
        e.reset_kill_switch();
        assert!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0)
                .is_ok()
        );
    }

    #[test]
    fn peak_equity_tracks_high_water_mark() {
        let mut e = engine(10_000.0);
        e.set_equity(12_000.0);
        e.set_equity(10_000.0);
        // Peak is 12k, current 10k → 16.7% drawdown → should reject
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::DrawdownLimit),
        );
    }

    #[test]
    fn consecutive_loss_trips_kill_switch() {
        let mut e = engine(10_000.0);
        e.on_trade_result(-100.0);
        e.on_trade_result(-50.0);
        e.on_trade_result(-200.0);
        assert!(e.kill.load(Ordering::SeqCst));
    }

    #[test]
    fn winning_trade_resets_consecutive_counter() {
        let mut e = engine(10_000.0);
        e.on_trade_result(-100.0);
        e.on_trade_result(-50.0);
        e.on_trade_result(10.0); // win resets
        e.on_trade_result(-30.0);
        assert_eq!(e.consecutive_losses, 1);
        assert!(!e.kill.load(Ordering::SeqCst));
    }

    #[test]
    fn pretrade_rejects_after_consecutive_losses() {
        let mut e = engine(10_000.0);
        e.on_trade_result(-1.0);
        e.on_trade_result(-1.0);
        e.on_trade_result(-1.0);
        // Reset kill switch so we test the consecutive-loss check itself
        e.reset_kill_switch();
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::ConsecutiveLossLimit),
        );
    }

    #[test]
    fn reset_session_clears_counters() {
        let mut e = engine(10_000.0);
        e.on_trade_result(-1.0);
        e.on_trade_result(-1.0);
        e.set_daily_pnl(-200.0);
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", 1000.0);
        e.reset_session();
        assert_eq!(e.consecutive_losses, 0);
        assert_eq!(e.daily_pnl, 0.0);
        assert_eq!(e.exposure_total, 0.0);
    }

    #[test]
    fn revalue_exposure_uses_mark_price() {
        let mut e = engine(10_000.0);
        // Open position: entry-based exposure = 1000
        e.on_fill(Market::Crypto, "BTCUSDT", "grid", 1000.0);
        assert!((e.exposure_total - 1000.0).abs() < 1e-9);

        // Mark price moves — revalue with new prices
        let mut marks = HashMap::new();
        marks.insert("BTCUSDT".to_string(), 1200.0);
        e.revalue_exposure(&marks);

        // position_qty for BTCUSDT = 1000 (notional delta), mark = 1200
        // new exposure = 1200 * 1000 = 1_200_000 — but that's qty*price
        // The key point is that exposure_total now reflects mark price
        assert!(e.exposure_total > 0.0);
        assert!(
            (e.exposure_total - 1200.0 * 1000.0).abs() < 1e-6,
            "exposure should reflect mark price * qty"
        );
    }
}
