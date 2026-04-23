//! Fixed-range grid strategy — pure logic, no I/O.
//!
//! - Place one order per grid line. Buys below the current mid, sells above.
//! - On a BUY fill at level `i` → place a SELL at level `i+1`.
//! - On a SELL fill at level `i` → place a BUY at level `i-1`.
//! - No auto-reinvest, no range re-anchor, no trailing logic — that's deliberate.
//!
//! All orchestration (placing/cancelling orders, polling fills, risk checks) lives
//! in `bot.rs`. This module exposes pure functions that `bot.rs` drives.

use std::collections::HashMap;

use crate::config::GridCfg;
use crate::types::Side;

/// Evenly spaced levels, inclusive of `lower` and `upper`, length = `n`.
pub fn compute_levels(lower: f64, upper: f64, n: usize) -> Vec<f64> {
    assert!(n >= 2, "grid needs at least 2 levels");
    let step = (upper - lower) / (n as f64 - 1.0);
    (0..n).map(|i| lower + step * i as f64).collect()
}

/// Number of significant decimal places in an increment string like "0.01" or "0.000001".
pub fn decimals_of(increment: &str) -> usize {
    increment
        .split_once('.')
        .map(|(_, frac)| frac.trim_end_matches('0').len())
        .unwrap_or(0)
}

/// Round `value` down to the nearest multiple of `increment`, formatted to match
/// the increment's decimal precision. Returns "0" shape on parse failure.
pub fn format_to_increment(value: f64, increment: &str) -> String {
    let inc: f64 = increment.parse().unwrap_or(0.0);
    if inc <= 0.0 {
        return format!("{value}");
    }
    let d = decimals_of(increment);
    let mult = (value / inc).floor();
    let rounded = mult * inc;
    format!("{rounded:.d$}")
}

#[derive(Debug, Clone)]
pub struct PlannedOrder {
    pub level_index: usize,
    pub side: Side,
    pub price_str: String,
    pub qty_str: String,
}

#[derive(Debug, Clone)]
pub struct LevelOrder {
    pub order_id: String,
    pub side: Side,
    pub price_str: String,
    pub qty_str: String,
}

#[derive(Debug)]
pub struct Grid {
    pub levels: Vec<f64>,
    pub qty_base: f64,
    pub tick_size: String,
    pub base_precision: String,
    pub min_order_qty: f64,
    pub min_order_amt_quote: f64,
    /// level_index → currently open order at that level (if any).
    pub orders: HashMap<usize, LevelOrder>,
}

impl Grid {
    pub fn new(
        cfg: &GridCfg,
        tick_size: String,
        base_precision: String,
        min_order_qty: f64,
        min_order_amt_quote: f64,
    ) -> Self {
        let levels = compute_levels(cfg.lower_price, cfg.upper_price, cfg.levels as usize);
        Self {
            levels,
            qty_base: cfg.qty_per_level_base,
            tick_size,
            base_precision,
            min_order_qty,
            min_order_amt_quote,
            orders: HashMap::new(),
        }
    }

    /// Plan the initial placement given the current mid price.
    /// Buys on levels strictly below mid, sells on levels strictly above mid.
    pub fn plan_initial(&self, mid: f64) -> Vec<PlannedOrder> {
        let qty_str = format_to_increment(self.qty_base, &self.base_precision);
        self.levels
            .iter()
            .enumerate()
            .filter_map(|(i, &price)| {
                let side = if price < mid {
                    Side::Buy
                } else if price > mid {
                    Side::Sell
                } else {
                    return None;
                };
                Some(PlannedOrder {
                    level_index: i,
                    side,
                    price_str: format_to_increment(price, &self.tick_size),
                    qty_str: qty_str.clone(),
                })
            })
            .collect()
    }

    /// Given a fill at level `i` on side `filled_side`, return the replacement order, if any.
    pub fn plan_replacement(&self, filled_level: usize, filled_side: Side) -> Option<PlannedOrder> {
        let new_level = match filled_side {
            Side::Buy => {
                let n = filled_level.checked_add(1)?;
                if n >= self.levels.len() { return None; }
                n
            }
            Side::Sell => filled_level.checked_sub(1)?,
        };
        let new_side = match filled_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        };
        let price = self.levels[new_level];
        Some(PlannedOrder {
            level_index: new_level,
            side: new_side,
            price_str: format_to_increment(price, &self.tick_size),
            qty_str: format_to_increment(self.qty_base, &self.base_precision),
        })
    }

    /// Sanity check: each order must meet both min qty and min notional.
    pub fn validate_sizing(&self) -> Result<(), String> {
        if self.qty_base < self.min_order_qty {
            return Err(format!(
                "qty_per_level_base {} < exchange min_order_qty {}",
                self.qty_base, self.min_order_qty,
            ));
        }
        if self.min_order_amt_quote > 0.0 {
            let lowest = self.levels.first().copied().unwrap_or(0.0);
            let notional_at_lowest = self.qty_base * lowest;
            if notional_at_lowest < self.min_order_amt_quote {
                return Err(format!(
                    "notional {:.4} at lowest level {:.4} < min_order_amt {}",
                    notional_at_lowest, lowest, self.min_order_amt_quote,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_evenly_spaced_inclusive() {
        let lv = compute_levels(100.0, 200.0, 11);
        assert_eq!(lv.len(), 11);
        assert!((lv[0] - 100.0).abs() < 1e-9);
        assert!((lv[10] - 200.0).abs() < 1e-9);
        assert!((lv[5] - 150.0).abs() < 1e-9);
    }

    #[test]
    fn decimals_of_common_increments() {
        assert_eq!(decimals_of("0.01"), 2);
        assert_eq!(decimals_of("0.000001"), 6);
        assert_eq!(decimals_of("1"), 0);
        assert_eq!(decimals_of("0.1"), 1);
        assert_eq!(decimals_of("0.100"), 1);
    }

    #[test]
    fn format_to_increment_rounds_down() {
        assert_eq!(format_to_increment(123.456_789, "0.01"), "123.45");
        assert_eq!(format_to_increment(0.001_234_5, "0.000001"), "0.001234");
        assert_eq!(format_to_increment(12345.9, "1"), "12345");
    }

    fn test_grid() -> Grid {
        let cfg = GridCfg {
            symbol: "BTCUSDT".to_string(),
            lower_price: 100.0,
            upper_price: 200.0,
            levels: 11,
            qty_per_level_base: 0.001,
            poll_secs: 3,
            cancel_existing_on_start: true,
            auto_rebalance: false,
            trailing_profit_threshold: 0.0,
        };
        Grid::new(&cfg, "0.01".into(), "0.000001".into(), 0.0001, 5.0)
    }

    #[test]
    fn initial_plan_has_buys_below_and_sells_above() {
        let g = test_grid();
        let plan = g.plan_initial(150.0);
        // Levels: 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200
        // mid=150 excludes level 5. Buys at 0..=4, Sells at 6..=10.
        assert_eq!(plan.len(), 10);
        let buys: Vec<_> = plan.iter().filter(|p| p.side == Side::Buy).collect();
        let sells: Vec<_> = plan.iter().filter(|p| p.side == Side::Sell).collect();
        assert_eq!(buys.len(), 5);
        assert_eq!(sells.len(), 5);
        for b in &buys { assert!(g.levels[b.level_index] < 150.0); }
        for s in &sells { assert!(g.levels[s.level_index] > 150.0); }
    }

    #[test]
    fn replacement_buy_fill_places_sell_one_level_up() {
        let g = test_grid();
        let r = g.plan_replacement(3, Side::Buy).expect("replacement");
        assert_eq!(r.level_index, 4);
        assert_eq!(r.side, Side::Sell);
    }

    #[test]
    fn replacement_sell_fill_places_buy_one_level_down() {
        let g = test_grid();
        let r = g.plan_replacement(7, Side::Sell).expect("replacement");
        assert_eq!(r.level_index, 6);
        assert_eq!(r.side, Side::Buy);
    }

    #[test]
    fn replacement_at_edges_returns_none() {
        let g = test_grid();
        assert!(g.plan_replacement(g.levels.len() - 1, Side::Buy).is_none());
        assert!(g.plan_replacement(0, Side::Sell).is_none());
    }

    #[test]
    fn validate_sizing_catches_too_small_qty() {
        let cfg = GridCfg {
            symbol: "BTCUSDT".to_string(),
            lower_price: 100.0,
            upper_price: 200.0,
            levels: 11,
            qty_per_level_base: 0.00001,
            poll_secs: 3,
            cancel_existing_on_start: true,
            auto_rebalance: false,
            trailing_profit_threshold: 0.0,
        };
        let g = Grid::new(&cfg, "0.01".into(), "0.000001".into(), 0.0001, 5.0);
        assert!(g.validate_sizing().is_err());
    }
}
