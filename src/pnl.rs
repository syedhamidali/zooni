//! Per-strategy P&L attribution with proper entry/exit matching.
//!
//! Tracks positions per strategy using average cost basis. When a sell fill
//! arrives, computes realised P&L = (sell_price - avg_entry) * qty and
//! attributes it to the originating strategy.

use std::collections::HashMap;

use crate::types::Side;

#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub strategy: String,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
    pub realised_pnl: f64,
}

#[derive(Debug, Clone)]
struct Position {
    holdings: f64,
    avg_entry: f64,
    total_cost: f64,
    realised_pnl: f64,
    trade_count: u64,
    winning_trades: u64,
}

impl Position {
    fn new() -> Self {
        Self {
            holdings: 0.0,
            avg_entry: 0.0,
            total_cost: 0.0,
            realised_pnl: 0.0,
            trade_count: 0,
            winning_trades: 0,
        }
    }
}

#[derive(Debug)]
pub struct PnlTracker {
    positions: HashMap<String, Position>,
    /// Fee rate applied to each fill's notional value (e.g. 0.001 = 0.1%).
    fee_rate: f64,
    /// Total fees paid across all strategies.
    total_fees: f64,
}

impl PnlTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            fee_rate: 0.001, // 0.1% default maker/taker fee
            total_fees: 0.0,
        }
    }

    /// Create a tracker with a custom fee rate (e.g. 0.001 = 0.1%).
    pub fn with_fee_rate(fee_rate: f64) -> Self {
        Self {
            positions: HashMap::new(),
            fee_rate,
            total_fees: 0.0,
        }
    }

    /// Record a fill and return the realised P&L (net of fees; negative fee cost for buys).
    pub fn record_fill(&mut self, strategy: &str, side: Side, price: f64, qty: f64) -> f64 {
        let pos = self
            .positions
            .entry(strategy.to_string())
            .or_insert_with(Position::new);

        let notional = price * qty;
        let fee = notional * self.fee_rate;
        self.total_fees += fee;

        match side {
            Side::Buy => {
                // Include fee in cost basis so it's reflected when we sell
                let effective_cost = notional + fee;
                let new_cost = pos.total_cost + effective_cost;
                pos.holdings += qty;
                pos.total_cost = new_cost;
                pos.avg_entry = if pos.holdings > 0.0 {
                    new_cost / pos.holdings
                } else {
                    0.0
                };
                -fee // buys incur a fee cost
            }
            Side::Sell => {
                let sell_qty = qty.min(pos.holdings);
                if sell_qty <= 0.0 {
                    return 0.0;
                }
                let gross_pnl = (price - pos.avg_entry) * sell_qty;
                let pnl = gross_pnl - fee;
                pos.holdings -= sell_qty;
                pos.total_cost = pos.avg_entry * pos.holdings;
                pos.realised_pnl += pnl;
                pos.trade_count += 1;
                if pnl > 0.0 {
                    pos.winning_trades += 1;
                }
                pnl
            }
        }
    }

    pub fn total_fees(&self) -> f64 {
        self.total_fees
    }

    pub fn fee_rate(&self) -> f64 {
        self.fee_rate
    }

    pub fn realised_pnl(&self, strategy: &str) -> f64 {
        self.positions
            .get(strategy)
            .map(|p| p.realised_pnl)
            .unwrap_or(0.0)
    }

    pub fn total_realised_pnl(&self) -> f64 {
        self.positions.values().map(|p| p.realised_pnl).sum()
    }

    pub fn holdings(&self, strategy: &str) -> f64 {
        self.positions
            .get(strategy)
            .map(|p| p.holdings)
            .unwrap_or(0.0)
    }

    pub fn avg_entry(&self, strategy: &str) -> f64 {
        self.positions
            .get(strategy)
            .map(|p| p.avg_entry)
            .unwrap_or(0.0)
    }

    pub fn unrealised_pnl(&self, strategy: &str, current_price: f64) -> f64 {
        let pos = match self.positions.get(strategy) {
            Some(p) => p,
            None => return 0.0,
        };
        if pos.holdings <= 0.0 {
            return 0.0;
        }
        (current_price - pos.avg_entry) * pos.holdings
    }

    pub fn total_unrealised_pnl(&self, current_price: f64) -> f64 {
        self.positions
            .values()
            .filter(|p| p.holdings > 0.0)
            .map(|p| (current_price - p.avg_entry) * p.holdings)
            .sum()
    }

    pub fn win_rate(&self, strategy: &str) -> f64 {
        let pos = match self.positions.get(strategy) {
            Some(p) => p,
            None => return 0.0,
        };
        if pos.trade_count == 0 {
            return 0.0;
        }
        pos.winning_trades as f64 / pos.trade_count as f64
    }

    pub fn strategy_names(&self) -> Vec<&str> {
        self.positions.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for PnlTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PnlTracker {
    pub fn set_fee_rate(&mut self, rate: f64) {
        self.fee_rate = rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Use zero-fee tracker for legacy tests that check exact P&L amounts
    fn zero_fee_tracker() -> PnlTracker {
        PnlTracker::with_fee_rate(0.0)
    }

    #[test]
    fn buy_has_zero_realised_pnl() {
        let mut t = zero_fee_tracker();
        let pnl = t.record_fill("grid", Side::Buy, 100.0, 1.0);
        assert_eq!(pnl, 0.0);
        assert!((t.holdings("grid") - 1.0).abs() < 1e-9);
        assert!((t.avg_entry("grid") - 100.0).abs() < 1e-9);
    }

    #[test]
    fn sell_computes_pnl() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        let pnl = t.record_fill("grid", Side::Sell, 110.0, 1.0);
        assert!((pnl - 10.0).abs() < 1e-9);
        assert!((t.holdings("grid")).abs() < 1e-9);
        assert!((t.realised_pnl("grid") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn partial_sell() {
        let mut t = zero_fee_tracker();
        t.record_fill("dca", Side::Buy, 100.0, 2.0);
        let pnl = t.record_fill("dca", Side::Sell, 120.0, 1.0);
        assert!((pnl - 20.0).abs() < 1e-9);
        assert!((t.holdings("dca") - 1.0).abs() < 1e-9);
        assert!((t.avg_entry("dca") - 100.0).abs() < 1e-9);
    }

    #[test]
    fn multiple_buys_averaged() {
        let mut t = zero_fee_tracker();
        t.record_fill("mom", Side::Buy, 100.0, 1.0);
        t.record_fill("mom", Side::Buy, 120.0, 1.0);
        // avg entry = (100 + 120) / 2 = 110
        assert!((t.avg_entry("mom") - 110.0).abs() < 1e-9);
        let pnl = t.record_fill("mom", Side::Sell, 115.0, 2.0);
        // pnl = (115 - 110) * 2 = 10
        assert!((pnl - 10.0).abs() < 1e-9);
    }

    #[test]
    fn losing_trade() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        let pnl = t.record_fill("grid", Side::Sell, 90.0, 1.0);
        assert!((pnl - (-10.0)).abs() < 1e-9);
    }

    #[test]
    fn sell_without_holdings_returns_zero() {
        let mut t = zero_fee_tracker();
        let pnl = t.record_fill("grid", Side::Sell, 100.0, 1.0);
        assert_eq!(pnl, 0.0);
    }

    #[test]
    fn oversell_capped_at_holdings() {
        let mut t = zero_fee_tracker();
        t.record_fill("dca", Side::Buy, 100.0, 1.0);
        let pnl = t.record_fill("dca", Side::Sell, 110.0, 5.0);
        // Can only sell 1.0, not 5.0
        assert!((pnl - 10.0).abs() < 1e-9);
        assert!((t.holdings("dca")).abs() < 1e-9);
    }

    #[test]
    fn unrealised_pnl() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 2.0);
        let urpnl = t.unrealised_pnl("grid", 115.0);
        assert!((urpnl - 30.0).abs() < 1e-9);
    }

    #[test]
    fn total_pnl_across_strategies() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        t.record_fill("grid", Side::Sell, 110.0, 1.0);
        t.record_fill("dca", Side::Buy, 50.0, 2.0);
        t.record_fill("dca", Side::Sell, 55.0, 2.0);
        // grid: 10, dca: 10
        assert!((t.total_realised_pnl() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn win_rate_tracking() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        t.record_fill("grid", Side::Sell, 110.0, 1.0); // win
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        t.record_fill("grid", Side::Sell, 90.0, 1.0); // loss
        assert!((t.win_rate("grid") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn strategy_names_tracked() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        t.record_fill("dca", Side::Buy, 50.0, 1.0);
        let names = t.strategy_names();
        assert!(names.contains(&"grid"));
        assert!(names.contains(&"dca"));
    }

    #[test]
    fn total_unrealised_pnl() {
        let mut t = zero_fee_tracker();
        t.record_fill("grid", Side::Buy, 100.0, 1.0);
        t.record_fill("dca", Side::Buy, 200.0, 1.0);
        let total = t.total_unrealised_pnl(110.0);
        // grid: (110-100)*1 = 10, dca: (110-200)*1 = -90
        assert!((total - (-80.0)).abs() < 1e-9);
    }

    #[test]
    fn default_impl() {
        let t = PnlTracker::default();
        assert_eq!(t.total_realised_pnl(), 0.0);
        assert!((t.fee_rate() - 0.001).abs() < 1e-9);
    }

    #[test]
    fn fees_deducted_from_pnl() {
        let mut t = PnlTracker::with_fee_rate(0.001); // 0.1%
        // Buy 1 BTC at 100 → fee = 100 * 0.001 = 0.1
        let buy_pnl = t.record_fill("grid", Side::Buy, 100.0, 1.0);
        assert!((buy_pnl - (-0.1)).abs() < 1e-9, "buy should return -fee");
        // avg_entry includes fee: (100 + 0.1) / 1 = 100.1
        assert!((t.avg_entry("grid") - 100.1).abs() < 1e-9);

        // Sell 1 BTC at 110 → gross pnl = (110 - 100.1) * 1 = 9.9, fee = 0.11
        let sell_pnl = t.record_fill("grid", Side::Sell, 110.0, 1.0);
        let expected = 9.9 - 0.11; // 9.79
        assert!(
            (sell_pnl - expected).abs() < 1e-6,
            "sell pnl should be net of fees: got {sell_pnl}, expected {expected}"
        );
        assert!(t.total_fees() > 0.0);
    }

    #[test]
    fn custom_fee_rate() {
        let t = PnlTracker::with_fee_rate(0.0005);
        assert!((t.fee_rate() - 0.0005).abs() < 1e-9);
    }
}
