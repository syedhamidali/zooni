#![allow(dead_code)]
//! Slippage protection — guard against adverse fills during volatile moments.
//!
//! Provides pre-trade limit price calculation and post-trade fill verification.
//! Converts market orders to aggressive limit orders when configured, and rejects
//! fills that exceed the maximum acceptable slippage threshold.

use serde::Deserialize;

use crate::types::Side;

#[derive(Debug, Clone, Deserialize)]
pub struct SlippageConfig {
    #[serde(default = "default_max_slippage_pct")]
    pub max_slippage_pct: f64,
    #[serde(default = "default_prefer_limit_orders")]
    pub prefer_limit_orders: bool,
    #[serde(default = "default_limit_offset_pct")]
    pub limit_offset_pct: f64,
}

fn default_max_slippage_pct() -> f64 {
    0.005
}
fn default_prefer_limit_orders() -> bool {
    true
}
fn default_limit_offset_pct() -> f64 {
    0.001
}

impl Default for SlippageConfig {
    fn default() -> Self {
        Self {
            max_slippage_pct: default_max_slippage_pct(),
            prefer_limit_orders: default_prefer_limit_orders(),
            limit_offset_pct: default_limit_offset_pct(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SlippageResult {
    pub slippage_pct: f64,
    pub is_acceptable: bool,
    pub overpay_amount: f64,
}

#[derive(Debug, Clone)]
pub struct SlippageGuard {
    config: SlippageConfig,
}

impl SlippageGuard {
    pub fn new(config: SlippageConfig) -> Self {
        Self { config }
    }

    pub fn check_fill_price(
        &self,
        expected_price: f64,
        fill_price: f64,
        side: Side,
    ) -> SlippageResult {
        let slippage_pct = match side {
            Side::Buy => (fill_price - expected_price) / expected_price,
            Side::Sell => (expected_price - fill_price) / expected_price,
        };

        let is_acceptable = slippage_pct <= self.config.max_slippage_pct;
        let overpay_amount = if slippage_pct > 0.0 {
            slippage_pct * expected_price
        } else {
            0.0
        };

        SlippageResult {
            slippage_pct,
            is_acceptable,
            overpay_amount,
        }
    }

    pub fn aggressive_limit_price(&self, current_price: f64, side: Side) -> f64 {
        match side {
            Side::Buy => current_price * (1.0 + self.config.limit_offset_pct),
            Side::Sell => current_price * (1.0 - self.config.limit_offset_pct),
        }
    }

    pub fn max_acceptable_price(&self, reference_price: f64, side: Side) -> f64 {
        match side {
            Side::Buy => reference_price * (1.0 + self.config.max_slippage_pct),
            Side::Sell => reference_price * (1.0 - self.config.max_slippage_pct),
        }
    }

    pub fn should_use_limit(&self) -> bool {
        self.config.prefer_limit_orders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> SlippageGuard {
        SlippageGuard::new(SlippageConfig::default())
    }

    #[test]
    fn buy_acceptable_slippage() {
        let g = guard();
        // 0.2% slippage on a buy — within the 0.5% limit
        let r = g.check_fill_price(100.0, 100.20, Side::Buy);
        assert!((r.slippage_pct - 0.002).abs() < 1e-9);
        assert!(r.is_acceptable);
        assert!(r.overpay_amount > 0.0);
    }

    #[test]
    fn buy_excessive_slippage() {
        let g = guard();
        // 1% slippage on a buy — exceeds the 0.5% limit
        let r = g.check_fill_price(100.0, 101.0, Side::Buy);
        assert!((r.slippage_pct - 0.01).abs() < 1e-9);
        assert!(!r.is_acceptable);
    }

    #[test]
    fn sell_acceptable_slippage() {
        let g = guard();
        // 0.3% slippage on a sell — within the 0.5% limit
        let r = g.check_fill_price(100.0, 99.70, Side::Sell);
        assert!((r.slippage_pct - 0.003).abs() < 1e-9);
        assert!(r.is_acceptable);
    }

    #[test]
    fn sell_excessive_slippage() {
        let g = guard();
        // 0.8% slippage on a sell — exceeds the 0.5% limit
        let r = g.check_fill_price(100.0, 99.20, Side::Sell);
        assert!((r.slippage_pct - 0.008).abs() < 1e-9);
        assert!(!r.is_acceptable);
    }

    #[test]
    fn aggressive_limit_price_buy() {
        let g = guard();
        let price = g.aggressive_limit_price(1000.0, Side::Buy);
        assert!((price - 1001.0).abs() < 1e-9); // 1000 * 1.001
    }

    #[test]
    fn aggressive_limit_price_sell() {
        let g = guard();
        let price = g.aggressive_limit_price(1000.0, Side::Sell);
        assert!((price - 999.0).abs() < 1e-9); // 1000 * 0.999
    }

    #[test]
    fn max_acceptable_price_buy() {
        let g = guard();
        let price = g.max_acceptable_price(1000.0, Side::Buy);
        assert!((price - 1005.0).abs() < 1e-9); // 1000 * 1.005
    }

    #[test]
    fn max_acceptable_price_sell() {
        let g = guard();
        let price = g.max_acceptable_price(1000.0, Side::Sell);
        assert!((price - 995.0).abs() < 1e-9); // 1000 * 0.995
    }

    #[test]
    fn zero_slippage() {
        let g = guard();
        let r = g.check_fill_price(50.0, 50.0, Side::Buy);
        assert!((r.slippage_pct).abs() < 1e-9);
        assert!(r.is_acceptable);
        assert!((r.overpay_amount).abs() < 1e-9);
    }

    #[test]
    fn negative_slippage_favorable_fill() {
        let g = guard();
        // Buy filled below expected — favorable
        let r = g.check_fill_price(100.0, 99.50, Side::Buy);
        assert!(r.slippage_pct < 0.0);
        assert!(r.is_acceptable);
        assert!((r.overpay_amount).abs() < 1e-9);

        // Sell filled above expected — also favorable
        let r = g.check_fill_price(100.0, 100.50, Side::Sell);
        assert!(r.slippage_pct < 0.0);
        assert!(r.is_acceptable);
        assert!((r.overpay_amount).abs() < 1e-9);
    }

    #[test]
    fn should_use_limit_respects_config() {
        let g = guard();
        assert!(g.should_use_limit());

        let g2 = SlippageGuard::new(SlippageConfig {
            prefer_limit_orders: false,
            ..Default::default()
        });
        assert!(!g2.should_use_limit());
    }
}
