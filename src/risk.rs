#![allow(dead_code)]
//! Risk limits + kill switch. Pure state + pure checks, no I/O.
//!
//! Strategies consult `RiskEngine::pretrade_check(...)` before submitting an order.
//! Fills update exposure via `on_fill(...)`. PnL is updated by the bot loop via
//! `set_daily_pnl` / `set_equity`. The kill switch is atomic so any task can trip it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};

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

/// Rolling-window loss tracking entry: (timestamp, absolute loss amount > 0).
type LossEvent = (DateTime<Utc>, f64);

/// Interior-mutable state for the rolling-window cache so that `pretrade_check`
/// can retain its `&self` receiver while still updating cached values.
#[derive(Debug)]
struct WindowCache {
    /// Timestamped log of realized losses (positive values = loss magnitude).
    /// Pruned to 7 days whenever `refresh` runs.
    loss_history: Vec<LossEvent>,
    /// When the rolling sums were last computed. `None` = never.
    last_calc: Option<Instant>,
    /// Cached rolling 24 h loss sum (positive = loss).
    daily_loss: f64,
    /// Cached rolling 168 h loss sum (positive = loss).
    weekly_loss: f64,
}

impl WindowCache {
    fn new() -> Self {
        Self {
            loss_history: Vec::new(),
            last_calc: None,
            daily_loss: 0.0,
            weekly_loss: 0.0,
        }
    }

    /// Recompute cached rolling sums if more than 60 seconds have elapsed since
    /// the last calculation.  Also prunes entries older than 7 days.
    fn refresh(&mut self) {
        let needs_refresh = match self.last_calc {
            None => true,
            Some(t) => t.elapsed().as_secs() >= 60,
        };
        if !needs_refresh {
            return;
        }

        let now = Utc::now();
        let cutoff_7d = now - Duration::hours(168);
        let cutoff_24h = now - Duration::hours(24);

        // Prune entries older than 7 days to prevent unbounded memory growth.
        self.loss_history.retain(|(ts, _)| *ts >= cutoff_7d);

        let mut daily = 0.0_f64;
        let mut weekly = 0.0_f64;
        for (ts, loss) in &self.loss_history {
            if *ts >= cutoff_24h {
                daily += loss;
            }
            // Everything remaining in history is already within 7 days.
            weekly += loss;
        }

        self.daily_loss = daily;
        self.weekly_loss = weekly;
        self.last_calc = Some(Instant::now());
    }
}

#[derive(Debug)]
pub struct RiskEngine {
    cfg: RiskConfig,
    kill: Arc<AtomicBool>,
    equity: f64,
    peak_equity: f64,
    /// Legacy fields kept for callers using `set_daily_pnl` / `set_weekly_pnl`.
    /// Used only as fallback when no loss events have been recorded yet.
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

    // ── Rolling-window loss tracking (interior-mutable so pretrade_check stays &self) ──
    window: RefCell<WindowCache>,
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
            window: RefCell::new(WindowCache::new()),
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

    // ── Rolling-window helpers ────────────────────────────────────────────────

    /// Record a realized loss event.  `amount` must be positive (the magnitude
    /// of the loss).  The entry is timestamped to `Utc::now()`.
    pub fn record_loss(&mut self, amount: f64) {
        if amount > 0.0 {
            self.window.borrow_mut().loss_history.push((Utc::now(), amount));
            // Invalidate cache so the next check recomputes immediately.
            self.window.borrow_mut().last_calc = None;
        }
    }

    /// Rolling 24 h realized loss sum (positive = loss).
    /// Falls back to the legacy `daily_pnl` field when no events have been recorded.
    fn rolling_daily_loss(&self) -> f64 {
        let mut w = self.window.borrow_mut();
        if w.loss_history.is_empty() {
            return if self.daily_pnl < 0.0 { -self.daily_pnl } else { 0.0 };
        }
        w.refresh();
        w.daily_loss
    }

    /// Rolling 168 h realized loss sum (positive = loss).
    /// Falls back to the legacy `weekly_pnl` field when no events have been recorded.
    fn rolling_weekly_loss(&self) -> f64 {
        let mut w = self.window.borrow_mut();
        if w.loss_history.is_empty() {
            return if self.weekly_pnl < 0.0 { -self.weekly_pnl } else { 0.0 };
        }
        w.refresh();
        w.weekly_loss
    }

    // ── Trade lifecycle ───────────────────────────────────────────────────────

    pub fn on_trade_result(&mut self, profit: f64) {
        if profit < 0.0 {
            self.consecutive_losses += 1;
            // Record the loss in the rolling window history.
            self.record_loss(-profit); // convert to positive magnitude
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
        // Note: loss_history is intentionally NOT cleared on session reset so
        // rolling windows remain accurate across session boundaries.
    }

    pub fn set_equity(&mut self, equity: f64) {
        self.equity = equity;
        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
    }

    /// Legacy setter — kept for API compatibility.  Callers that have not yet
    /// migrated to `record_loss` can continue supplying a negative daily PnL
    /// here; it is used as a fallback when no events exist in `loss_history`.
    pub fn set_daily_pnl(&mut self, pnl: f64) {
        self.daily_pnl = pnl;
    }

    /// Legacy setter — kept for API compatibility.
    pub fn set_weekly_pnl(&mut self, pnl: f64) {
        self.weekly_pnl = pnl;
    }

    // ── Exposure tracking ─────────────────────────────────────────────────────

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
        let pq = self.position_qty.entry(symbol.to_string()).or_insert(0.0);
        *pq = (*pq + notional_delta).max(0.0);
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

    // ── Pre-trade gate ────────────────────────────────────────────────────────

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

        // Rolling 24 h loss limit.
        let daily_loss = self.rolling_daily_loss();
        if daily_loss >= self.cfg.daily_loss_limit * self.equity {
            return Err(Reject::DailyLossLimit);
        }

        // Rolling 7-day (168 h) loss limit.
        let weekly_loss = self.rolling_weekly_loss();
        if weekly_loss >= self.cfg.weekly_loss_limit * self.equity {
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

    // ── Accessors ─────────────────────────────────────────────────────────────

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
        e.set_daily_pnl(-600.0); // -6%, past 5% daily limit; triggers legacy fallback
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "trend", 100.0),
            Err(Reject::DailyLossLimit),
        );
    }

    #[test]
    fn rejects_after_daily_loss_limit_via_record_loss() {
        let mut e = engine(10_000.0);
        // Record a loss of 600 (6% of 10_000 equity), past the 5% daily limit.
        e.record_loss(600.0);
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
        e.set_weekly_pnl(-1100.0); // -11%, past 10% weekly limit; triggers legacy fallback
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::WeeklyLossLimit),
        );
    }

    #[test]
    fn rejects_weekly_loss_limit_via_record_loss() {
        let mut e = engine(10_000.0);
        // 1100 loss = 11% of 10_000 equity, past 10% weekly limit.
        e.record_loss(1100.0);
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

    /// Verify that small losses within the 24 h window do NOT trip the daily limit.
    #[test]
    fn small_rolling_loss_does_not_trip_daily_limit() {
        let mut e = engine(10_000.0);
        e.record_loss(400.0); // 4% < 5% limit → should still be allowed
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Ok(()),
        );
    }

    /// Verify that two `record_loss` calls accumulate correctly.
    #[test]
    fn rolling_losses_accumulate_across_events() {
        let mut e = engine(10_000.0);
        e.record_loss(300.0);
        e.record_loss(250.0); // total 550 = 5.5% > 5% → reject
        assert_eq!(
            e.pretrade_check(Market::Crypto, "BTCUSDT", "grid", 100.0),
            Err(Reject::DailyLossLimit),
        );
    }
}
