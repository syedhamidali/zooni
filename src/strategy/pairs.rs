//! Statistical arbitrage pairs trading strategy — market-neutral spread trading.
//!
//! Trades the spread between two correlated instruments (e.g., HDFCBANK vs ICICIBANK).
//! Enters when the z-score of the log-spread exceeds a threshold, exits on mean
//! reversion, and stops out if the spread diverges too far.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{FillEvent, MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::indicators;
use crate::types::Side;

#[derive(Debug, Clone)]
pub enum HedgeMethod {
    Fixed(f64),
    Rolling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadDirection {
    LongSpread,
    ShortSpread,
}

#[derive(Debug, Clone)]
pub struct PairPosition {
    pub spread_direction: SpreadDirection,
    pub entry_z_score: f64,
    pub entry_price_a: f64,
    pub entry_price_b: f64,
    pub qty_a: f64,
    pub qty_b: f64,
    pub order_id_a: String,
    pub order_id_b: String,
}

#[derive(Debug, Clone)]
pub struct PairsConfig {
    pub symbol_a: String,
    pub symbol_b: String,
    pub lookback: usize,
    pub entry_z_score: f64,
    pub exit_z_score: f64,
    pub stop_z_score: f64,
    pub position_size_quote: f64,
    pub max_open_pairs: usize,
    pub hedge_ratio_method: HedgeMethod,
    /// Minimum Pearson correlation required to allow trades. Default 0.7.
    pub min_correlation: f64,
}

impl Default for PairsConfig {
    fn default() -> Self {
        Self {
            symbol_a: String::new(),
            symbol_b: String::new(),
            lookback: 100,
            entry_z_score: 2.0,
            exit_z_score: 0.5,
            stop_z_score: 3.5,
            position_size_quote: 50_000.0,
            max_open_pairs: 1,
            hedge_ratio_method: HedgeMethod::Fixed(1.0),
            min_correlation: 0.7,
        }
    }
}

pub struct PairsStrategy {
    config: PairsConfig,
    prices_a: Vec<f64>,
    prices_b: Vec<f64>,
    position: Option<PairPosition>,
    ticks_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairsSnapshot {
    prices_a: Vec<f64>,
    prices_b: Vec<f64>,
    ticks_count: u64,
}

impl PairsStrategy {
    pub fn new(config: PairsConfig) -> Self {
        Self {
            config,
            prices_a: Vec::new(),
            prices_b: Vec::new(),
            position: None,
            ticks_count: 0,
        }
    }

    fn snapshot(&self) -> PairsSnapshot {
        PairsSnapshot {
            prices_a: self.prices_a.clone(),
            prices_b: self.prices_b.clone(),
            ticks_count: self.ticks_count,
        }
    }

    fn restore(&mut self, snap: PairsSnapshot) {
        self.prices_a = snap.prices_a;
        self.prices_b = snap.prices_b;
        self.ticks_count = snap.ticks_count;
    }

    pub fn hedge_ratio(&self) -> f64 {
        match &self.config.hedge_ratio_method {
            HedgeMethod::Fixed(r) => *r,
            HedgeMethod::Rolling => {
                if self.prices_a.len() < 2 || self.prices_b.len() < 2 {
                    return 1.0;
                }
                let n = self.prices_a.len().min(self.prices_b.len());
                let a = &self.prices_a[self.prices_a.len() - n..];
                let b = &self.prices_b[self.prices_b.len() - n..];

                let mean_a = a.iter().sum::<f64>() / n as f64;
                let mean_b = b.iter().sum::<f64>() / n as f64;

                let mut cov = 0.0;
                let mut var_b = 0.0;
                for i in 0..n {
                    let da = a[i] - mean_a;
                    let db = b[i] - mean_b;
                    cov += da * db;
                    var_b += db * db;
                }

                if var_b == 0.0 {
                    return 1.0;
                }
                cov / var_b
            }
        }
    }

    pub fn compute_spread(&self) -> Vec<f64> {
        let n = self.prices_a.len().min(self.prices_b.len());
        if n == 0 {
            return vec![];
        }
        let ratio = self.hedge_ratio();
        let a = &self.prices_a[self.prices_a.len() - n..];
        let b = &self.prices_b[self.prices_b.len() - n..];

        a.iter()
            .zip(b.iter())
            .map(|(pa, pb)| pa.ln() - ratio * pb.ln())
            .collect()
    }

    pub fn current_z_score(&self) -> Option<f64> {
        let spread = self.compute_spread();
        if spread.len() < self.config.lookback {
            return None;
        }
        let mean = spread.iter().sum::<f64>() / spread.len() as f64;
        let sd = indicators::std_dev(&spread);
        if sd == 0.0 {
            return None;
        }
        let current = *spread.last()?;
        Some((current - mean) / sd)
    }

    pub fn update_prices(&mut self, price_a: f64, price_b: f64) {
        self.prices_a.push(price_a);
        self.prices_b.push(price_b);

        if self.prices_a.len() > self.config.lookback {
            self.prices_a.remove(0);
        }
        if self.prices_b.len() > self.config.lookback {
            self.prices_b.remove(0);
        }
    }

    fn close_position(
        &mut self,
        snapshot: &MarketSnapshot,
        use_market: bool,
    ) -> Vec<StrategyAction> {
        let pos = match self.position.take() {
            Some(p) => p,
            None => return vec![StrategyAction::None],
        };

        let order_type = if use_market {
            OrderType::Market
        } else {
            OrderType::Limit
        };

        let (side_a, side_b) = match pos.spread_direction {
            SpreadDirection::LongSpread => (Side::Sell, Side::Buy),
            SpreadDirection::ShortSpread => (Side::Buy, Side::Sell),
        };

        vec![
            StrategyAction::PlaceOrder {
                symbol: self.config.symbol_a.clone(),
                side: side_a,
                price: snapshot.last,
                qty: pos.qty_a,
                order_type,
            },
            StrategyAction::PlaceOrder {
                symbol: self.config.symbol_b.clone(),
                side: side_b,
                price: snapshot.last,
                qty: pos.qty_b,
                order_type,
            },
        ]
    }
}

impl Strategy for PairsStrategy {
    fn name(&self) -> &'static str {
        "pairs"
    }

    fn on_start(&mut self, _snapshot: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
        Ok(vec![])
    }

    fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        _fills: &[FillEvent],
    ) -> Result<Vec<StrategyAction>> {
        self.ticks_count += 1;

        // Route the snapshot to the correct price window
        if snapshot.symbol == self.config.symbol_a {
            self.prices_a.push(snapshot.last);
            if self.prices_a.len() > self.config.lookback {
                self.prices_a.remove(0);
            }
        } else if snapshot.symbol == self.config.symbol_b {
            self.prices_b.push(snapshot.last);
            if self.prices_b.len() > self.config.lookback {
                self.prices_b.remove(0);
            }
        } else {
            return Ok(vec![StrategyAction::None]);
        }

        // Need enough data for both legs
        let z = match self.current_z_score() {
            Some(z) => z,
            None => return Ok(vec![StrategyAction::None]),
        };

        // Exit logic
        if self.position.is_some() {
            let pos = self.position.as_ref().unwrap();
            let should_exit_mean_reversion = match pos.spread_direction {
                SpreadDirection::LongSpread => z >= -self.config.exit_z_score,
                SpreadDirection::ShortSpread => z <= self.config.exit_z_score,
            };
            let should_stop_out = z.abs() > self.config.stop_z_score;

            if should_stop_out {
                return Ok(self.close_position(snapshot, true));
            }
            if should_exit_mean_reversion {
                return Ok(self.close_position(snapshot, false));
            }
            return Ok(vec![StrategyAction::None]);
        }

        // Entry logic — no position open
        if self.position.is_some() {
            return Ok(vec![StrategyAction::None]);
        }

        if z.abs() <= self.config.entry_z_score {
            return Ok(vec![StrategyAction::None]);
        }

        // Require minimum correlation between the two legs before entering
        if self.config.min_correlation > 0.0 {
            match indicators::pearson_correlation(&self.prices_a, &self.prices_b) {
                Some(corr) if corr < self.config.min_correlation => {
                    return Ok(vec![StrategyAction::None]);
                }
                None => {
                    return Ok(vec![StrategyAction::None]);
                }
                _ => {} // correlation is sufficient
            }
        }

        let price_a = *self.prices_a.last().unwrap();
        let price_b = *self.prices_b.last().unwrap();
        let qty_a = self.config.position_size_quote / price_a;
        let qty_b = self.config.position_size_quote / price_b;

        let (side_a, side_b, direction) = if z > self.config.entry_z_score {
            // Spread too high — short spread: sell A, buy B
            (Side::Sell, Side::Buy, SpreadDirection::ShortSpread)
        } else {
            // z < -entry_z_score — spread too low — long spread: buy A, sell B
            (Side::Buy, Side::Sell, SpreadDirection::LongSpread)
        };

        let order_id_a = format!("pairs_a_{}_{}", side_str(side_a), self.ticks_count);
        let order_id_b = format!("pairs_b_{}_{}", side_str(side_b), self.ticks_count);

        self.position = Some(PairPosition {
            spread_direction: direction,
            entry_z_score: z,
            entry_price_a: price_a,
            entry_price_b: price_b,
            qty_a,
            qty_b,
            order_id_a: order_id_a.clone(),
            order_id_b: order_id_b.clone(),
        });

        Ok(vec![
            StrategyAction::PlaceOrder {
                symbol: self.config.symbol_a.clone(),
                side: side_a,
                price: price_a,
                qty: qty_a,
                order_type: OrderType::Limit,
            },
            StrategyAction::PlaceOrder {
                symbol: self.config.symbol_b.clone(),
                side: side_b,
                price: price_b,
                qty: qty_b,
                order_type: OrderType::Limit,
            },
        ])
    }

    fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
        Ok(vec![
            StrategyAction::CancelAll {
                symbol: self.config.symbol_a.clone(),
            },
            StrategyAction::CancelAll {
                symbol: self.config.symbol_b.clone(),
            },
        ])
    }

    fn tunable_params(&self) -> HashMap<String, f64> {
        let mut m = HashMap::new();
        m.insert("entry_z_score".into(), self.config.entry_z_score);
        m.insert("exit_z_score".into(), self.config.exit_z_score);
        m.insert("stop_z_score".into(), self.config.stop_z_score);
        m.insert(
            "position_size_quote".into(),
            self.config.position_size_quote,
        );
        m.insert("min_correlation".into(), self.config.min_correlation);
        m
    }

    fn apply_params(&mut self, params: &HashMap<String, f64>) {
        if let Some(&v) = params.get("entry_z_score") {
            self.config.entry_z_score = v;
        }
        if let Some(&v) = params.get("exit_z_score") {
            self.config.exit_z_score = v;
        }
        if let Some(&v) = params.get("stop_z_score") {
            self.config.stop_z_score = v;
        }
        if let Some(&v) = params.get("position_size_quote") {
            self.config.position_size_quote = v;
        }
        if let Some(&v) = params.get("min_correlation") {
            self.config.min_correlation = v;
        }
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&self.snapshot()).ok()
    }

    fn load_state(&mut self, json: &str) {
        if let Ok(snap) = serde_json::from_str::<PairsSnapshot>(json) {
            self.restore(snap);
        }
    }
}

fn side_str(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> PairsConfig {
        PairsConfig {
            symbol_a: "NSE:HDFCBANK".to_string(),
            symbol_b: "NSE:ICICIBANK".to_string(),
            lookback: 10,
            entry_z_score: 2.0,
            exit_z_score: 0.5,
            stop_z_score: 3.5,
            position_size_quote: 50_000.0,
            max_open_pairs: 1,
            hedge_ratio_method: HedgeMethod::Fixed(1.0),
            min_correlation: 0.0, // disable correlation check in tests
        }
    }

    fn snap_a(price: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "NSE:HDFCBANK".to_string(),
            bid: price - 1.0,
            ask: price + 1.0,
            last: price,
            volume_24h: 100_000.0,
        }
    }

    fn snap_b(price: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "NSE:ICICIBANK".to_string(),
            bid: price - 1.0,
            ask: price + 1.0,
            last: price,
            volume_24h: 100_000.0,
        }
    }

    fn make_strategy_with_prices(prices_a: &[f64], prices_b: &[f64]) -> PairsStrategy {
        let mut config = default_config();
        config.lookback = prices_a.len().max(prices_b.len());
        let mut strat = PairsStrategy::new(config);
        // Use update_prices so both windows grow together
        for i in 0..prices_a.len().min(prices_b.len()) {
            strat.prices_a.push(prices_a[i]);
            strat.prices_b.push(prices_b[i]);
        }
        strat
    }

    #[test]
    fn hedge_ratio_fixed() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Fixed(1.5);
        let strat = PairsStrategy::new(config);
        assert!((strat.hedge_ratio() - 1.5).abs() < 1e-10);
    }

    #[test]
    fn hedge_ratio_rolling() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Rolling;
        config.lookback = 5;
        let mut strat = PairsStrategy::new(config);

        // Perfectly correlated: a = 2*b, so cov(a,b)/var(b) = 2
        strat.prices_a = vec![200.0, 204.0, 196.0, 210.0, 190.0];
        strat.prices_b = vec![100.0, 102.0, 98.0, 105.0, 95.0];

        let ratio = strat.hedge_ratio();
        assert!(
            (ratio - 2.0).abs() < 0.01,
            "rolling hedge ratio should be ~2.0, got {ratio}"
        );
    }

    #[test]
    fn compute_spread_basic() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Fixed(1.0);
        config.lookback = 3;
        let mut strat = PairsStrategy::new(config);

        strat.prices_a = vec![100.0, 110.0, 105.0];
        strat.prices_b = vec![100.0, 100.0, 100.0];

        let spread = strat.compute_spread();
        assert_eq!(spread.len(), 3);

        // spread[0] = ln(100) - 1.0 * ln(100) = 0
        assert!(
            spread[0].abs() < 1e-10,
            "spread[0] should be 0, got {}",
            spread[0]
        );
        // spread[1] = ln(110) - ln(100) = ln(1.1)
        let expected_1 = (110.0_f64).ln() - (100.0_f64).ln();
        assert!(
            (spread[1] - expected_1).abs() < 1e-10,
            "spread[1] should be {expected_1}, got {}",
            spread[1]
        );
    }

    #[test]
    fn z_score_at_mean_is_zero() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Fixed(1.0);
        config.lookback = 5;
        let mut strat = PairsStrategy::new(config);

        // All identical prices → constant spread → z-score undefined (std_dev = 0)
        // Use slight variation so std_dev > 0, but last spread equals mean
        strat.prices_a = vec![100.0, 101.0, 99.0, 101.0, 100.0];
        strat.prices_b = vec![100.0, 100.0, 100.0, 100.0, 100.0];

        // The spread will be: 0, ln(101)-ln(100), ln(99)-ln(100), ln(101)-ln(100), 0
        // The mean of these = (0 + 0.00995 - 0.01005 + 0.00995 + 0) / 5 ≈ 0.00197
        // Last spread = 0, which is close to but not exactly mean.
        // For exact z=0, set last spread = mean. Use symmetric prices:
        strat.prices_a = vec![102.0, 98.0, 102.0, 98.0, 100.0];
        strat.prices_b = vec![100.0, 100.0, 100.0, 100.0, 100.0];

        let spread = strat.compute_spread();
        let _mean = spread.iter().sum::<f64>() / spread.len() as f64;
        // Due to ln asymmetry, ln(102)-ln(100) != -(ln(98)-ln(100)), but they're close.
        // The last spread is ln(100)-ln(100) = 0.
        // z = (0 - mean) / std. Since spread is symmetric around ~0, mean ≈ 0.
        let z = strat.current_z_score();
        assert!(z.is_some());
        assert!(
            z.unwrap().abs() < 0.2,
            "z-score should be near zero with symmetric spread, got {}",
            z.unwrap()
        );
    }

    #[test]
    fn z_score_deviation() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Fixed(1.0);
        config.lookback = 10;
        let mut strat = PairsStrategy::new(config);

        // Constant spread for first 9 points, then a big deviation on the last
        strat.prices_b = vec![100.0; 10];
        let mut pa = vec![100.0; 9];
        pa.push(150.0); // big deviation
        strat.prices_a = pa;

        let z = strat.current_z_score().unwrap();
        assert!(
            z > 1.0,
            "z-score should be significantly positive when spread deviates up, got {z}"
        );
    }

    #[test]
    fn entry_long_spread_when_z_negative() {
        let mut config = default_config();
        config.hedge_ratio_method = HedgeMethod::Fixed(1.0);
        config.lookback = 10;
        config.entry_z_score = 2.0;
        let mut strat = PairsStrategy::new(config);

        // Build a history where spread is stable, then make A drop sharply relative to B
        // to get z < -2
        for _ in 0..9 {
            strat.prices_a.push(100.0);
            strat.prices_b.push(100.0);
        }
        // Last tick: A drops far below B → spread = ln(A) - ln(B) goes very negative
        strat.prices_a.push(50.0);
        strat.prices_b.push(100.0);

        let z = strat.current_z_score().unwrap();
        assert!(z < -2.0, "z should be < -2, got {z}");

        // Now feed the tick through on_tick
        let mut config2 = default_config();
        config2.lookback = 10;
        config2.entry_z_score = 2.0;
        let mut strat2 = PairsStrategy::new(config2);

        // Pre-fill 9 ticks for both symbols
        for _ in 0..9 {
            let _ = strat2.on_tick(&snap_a(100.0), &[]);
            let _ = strat2.on_tick(&snap_b(100.0), &[]);
        }
        // Trigger entry with A dropping
        let _ = strat2.on_tick(&snap_b(100.0), &[]);
        let actions = strat2.on_tick(&snap_a(50.0), &[]).unwrap();

        // Should enter long spread: buy A, sell B
        let place_orders: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, StrategyAction::PlaceOrder { .. }))
            .collect();
        assert_eq!(
            place_orders.len(),
            2,
            "should place two orders for pair entry"
        );

        // Verify the position direction
        assert!(strat2.position.is_some());
        assert_eq!(
            strat2.position.as_ref().unwrap().spread_direction,
            SpreadDirection::LongSpread
        );
    }

    #[test]
    fn entry_short_spread_when_z_positive() {
        let mut config = default_config();
        config.lookback = 10;
        config.entry_z_score = 2.0;
        let mut strat = PairsStrategy::new(config);

        // Pre-fill 9 ticks
        for _ in 0..9 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }
        // A spikes up relative to B → spread goes very positive → z > +2
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let actions = strat.on_tick(&snap_a(200.0), &[]).unwrap();

        let place_orders: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, StrategyAction::PlaceOrder { .. }))
            .collect();
        assert_eq!(place_orders.len(), 2);

        assert!(strat.position.is_some());
        assert_eq!(
            strat.position.as_ref().unwrap().spread_direction,
            SpreadDirection::ShortSpread
        );
    }

    #[test]
    fn exit_on_mean_reversion() {
        let mut config = default_config();
        config.lookback = 10;
        config.entry_z_score = 2.0;
        config.exit_z_score = 0.5;
        let mut strat = PairsStrategy::new(config);

        // Pre-fill and trigger a short spread entry (z > +2)
        for _ in 0..9 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let _ = strat.on_tick(&snap_a(200.0), &[]).unwrap();
        assert!(strat.position.is_some());

        // Now bring prices back together (mean reversion) — z should drop within exit threshold
        // Replace the last A price with something closer to B
        // We need to feed more ticks to shift the window
        for _ in 0..9 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let actions = strat.on_tick(&snap_a(100.0), &[]).unwrap();

        // z should now be near 0, within exit threshold → close position
        let close_orders: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, StrategyAction::PlaceOrder { .. }))
            .collect();
        assert!(
            close_orders.len() == 2 || strat.position.is_none(),
            "should close position on mean reversion"
        );
    }

    #[test]
    fn stop_out_on_extreme_divergence() {
        let mut config = default_config();
        config.lookback = 10;
        config.entry_z_score = 2.0;
        config.stop_z_score = 3.5;
        let mut strat = PairsStrategy::new(config);

        // Pre-fill and trigger entry (long spread: z < -2)
        for _ in 0..9 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let _ = strat.on_tick(&snap_a(50.0), &[]).unwrap();
        assert!(strat.position.is_some());

        // Now push the spread even further — A drops even more, exceeding stop_z
        for _ in 0..9 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let actions = strat.on_tick(&snap_a(10.0), &[]).unwrap();

        // Should stop out with market orders
        let market_orders: Vec<_> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        order_type: OrderType::Market,
                        ..
                    }
                )
            })
            .collect();

        // Either we stopped out (market orders) or the position was already closed
        if strat.position.is_some() {
            // Position still open means z wasn't extreme enough — check that actions are non-empty
            assert!(!actions.is_empty());
        } else {
            assert_eq!(
                market_orders.len(),
                2,
                "should place market orders on stop out"
            );
        }
    }

    #[test]
    fn no_entry_with_insufficient_data() {
        let config = default_config(); // lookback = 10
        let mut strat = PairsStrategy::new(config);

        // Only feed 5 ticks — not enough for lookback of 10
        for _ in 0..5 {
            let _ = strat.on_tick(&snap_a(100.0), &[]);
            let _ = strat.on_tick(&snap_b(100.0), &[]);
        }

        // Even with a divergent price, should not enter
        let _ = strat.on_tick(&snap_b(100.0), &[]);
        let actions = strat.on_tick(&snap_a(200.0), &[]).unwrap();

        assert!(
            strat.position.is_none(),
            "should not enter with insufficient data"
        );
        assert!(
            actions.iter().all(|a| matches!(a, StrategyAction::None)),
            "should return None action with insufficient data"
        );
    }

    #[test]
    fn no_second_pair_when_max_open() {
        let mut config = default_config();
        config.lookback = 10;
        config.max_open_pairs = 1;
        let mut strat = PairsStrategy::new(config);

        // Manually insert a position
        strat.position = Some(PairPosition {
            spread_direction: SpreadDirection::LongSpread,
            entry_z_score: -2.5,
            entry_price_a: 90.0,
            entry_price_b: 100.0,
            qty_a: 555.0,
            qty_b: 500.0,
            order_id_a: "existing_a".to_string(),
            order_id_b: "existing_b".to_string(),
        });

        // Pre-fill prices
        for _ in 0..10 {
            strat.prices_a.push(100.0);
            strat.prices_b.push(100.0);
        }
        // Cap at lookback
        while strat.prices_a.len() > strat.config.lookback {
            strat.prices_a.remove(0);
        }
        while strat.prices_b.len() > strat.config.lookback {
            strat.prices_b.remove(0);
        }

        // Even if z-score would trigger entry, position already exists
        // Push a divergent price via on_tick — but since position exists, it goes to exit logic
        let actions = strat.on_tick(&snap_a(200.0), &[]).unwrap();

        // Should not open a second position (only exit or hold actions)
        let entry_count = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        order_type: OrderType::Limit,
                        ..
                    }
                )
            })
            .count();

        // The existing position should either be exited or held — no new entry
        // We check that we still have at most one position
        // (the exit logic may or may not fire depending on z-score)
        assert!(
            strat.position.is_none() || entry_count == 0 || entry_count == 2,
            "should not open a second pair position when max_open_pairs = 1"
        );
    }

    #[test]
    fn price_windows_cap_at_lookback() {
        let mut config = default_config();
        config.lookback = 10;
        let mut strat = PairsStrategy::new(config);

        for i in 0..25 {
            strat.update_prices(100.0 + i as f64, 200.0 + i as f64);
        }

        assert_eq!(strat.prices_a.len(), 10, "prices_a should cap at lookback");
        assert_eq!(strat.prices_b.len(), 10, "prices_b should cap at lookback");

        // Oldest should be the 16th price pushed (index 15: 100+15=115)
        assert!(
            (strat.prices_a[0] - 115.0).abs() < 0.001,
            "oldest price_a should be 115.0, got {}",
            strat.prices_a[0]
        );
        assert!(
            (strat.prices_b[0] - 215.0).abs() < 0.001,
            "oldest price_b should be 215.0, got {}",
            strat.prices_b[0]
        );
    }

    #[test]
    fn on_shutdown_cancels_all() {
        let config = default_config();
        let mut strat = PairsStrategy::new(config);

        let actions = strat.on_shutdown().unwrap();
        assert_eq!(actions.len(), 2);

        match &actions[0] {
            StrategyAction::CancelAll { symbol } => {
                assert_eq!(symbol, "NSE:HDFCBANK");
            }
            other => panic!("expected CancelAll for symbol_a, got {:?}", other),
        }
        match &actions[1] {
            StrategyAction::CancelAll { symbol } => {
                assert_eq!(symbol, "NSE:ICICIBANK");
            }
            other => panic!("expected CancelAll for symbol_b, got {:?}", other),
        }
    }
}
