//! Grid strategy as a Strategy trait impl for the orchestrator.
//!
//! Self-contained grid: tracks price movement across grid levels and emits
//! buy/sell orders when price crosses levels. Buys on downward crosses,
//! sells when price rises to the next level up. Profit = spacing * qty per cycle.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::{FillEvent, MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::grid::compute_levels;
use crate::types::Side;

#[derive(Debug, Clone)]
pub struct GridStrategyConfig {
    pub symbol: String,
    pub lower_price: f64,
    pub upper_price: f64,
    pub levels: usize,
    pub qty_per_level: f64,
}

pub struct GridStrategy {
    symbol: String,
    levels: Vec<f64>,
    qty_base: f64,
    last_price: Option<f64>,
    bought_at: HashSet<usize>,
    total_cycles: u64,
}

impl GridStrategy {
    pub fn new(config: GridStrategyConfig) -> Self {
        let levels = compute_levels(config.lower_price, config.upper_price, config.levels);
        Self {
            symbol: config.symbol,
            levels,
            qty_base: config.qty_per_level,
            last_price: None,
            bought_at: HashSet::new(),
            total_cycles: 0,
        }
    }
}

impl Strategy for GridStrategy {
    fn name(&self) -> &'static str {
        "grid"
    }

    fn on_start(&mut self, snapshot: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
        self.last_price = Some(snapshot.last);
        Ok(vec![])
    }

    fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        _fills: &[FillEvent],
    ) -> Result<Vec<StrategyAction>> {
        if snapshot.symbol != self.symbol {
            return Ok(vec![StrategyAction::None]);
        }

        let price = snapshot.last;
        let mut actions = Vec::new();

        if let Some(last) = self.last_price {
            for (i, &level) in self.levels.iter().enumerate() {
                // Price crossed DOWN through a level → buy
                if last > level && price <= level && !self.bought_at.contains(&i) {
                    actions.push(StrategyAction::PlaceOrder {
                        symbol: self.symbol.clone(),
                        side: Side::Buy,
                        price: level,
                        qty: self.qty_base,
                        order_type: OrderType::Limit,
                    });
                    self.bought_at.insert(i);
                }

                // Price crossed UP through a level → sell if we hold at level below
                if last < level && price >= level && i > 0 && self.bought_at.contains(&(i - 1)) {
                    actions.push(StrategyAction::PlaceOrder {
                        symbol: self.symbol.clone(),
                        side: Side::Sell,
                        price: level,
                        qty: self.qty_base,
                        order_type: OrderType::Limit,
                    });
                    self.bought_at.remove(&(i - 1));
                    self.total_cycles += 1;
                }
            }
        }

        self.last_price = Some(price);

        if actions.is_empty() {
            Ok(vec![StrategyAction::None])
        } else {
            Ok(actions)
        }
    }

    fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
        tracing::info!(
            held_levels = self.bought_at.len(),
            total_cycles = self.total_cycles,
            "grid strategy shutdown",
        );
        Ok(vec![StrategyAction::CancelAll {
            symbol: self.symbol.clone(),
        }])
    }

    fn tunable_params(&self) -> HashMap<String, f64> {
        HashMap::from([
            ("levels".into(), self.levels.len() as f64),
            ("qty_base".into(), self.qty_base),
        ])
    }

    fn apply_params(&mut self, params: &HashMap<String, f64>) {
        if let Some(&v) = params.get("qty_base") {
            self.qty_base = v.clamp(1e-8, 100.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grid() -> GridStrategy {
        GridStrategy::new(GridStrategyConfig {
            symbol: "BTCUSDT".into(),
            lower_price: 100.0,
            upper_price: 110.0,
            levels: 6,
            qty_per_level: 1.0,
        })
    }

    fn snap(price: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "BTCUSDT".into(),
            bid: price - 0.1,
            ask: price + 0.1,
            last: price,
            volume_24h: 1000.0,
        }
    }

    #[test]
    fn name_is_grid() {
        assert_eq!(make_grid().name(), "grid");
    }

    #[test]
    fn no_action_without_price_crossing() {
        let mut g = make_grid();
        g.on_start(&snap(105.0)).unwrap();
        let actions = g.on_tick(&snap(105.5), &[]).unwrap();
        assert!(matches!(actions[0], StrategyAction::None));
    }

    #[test]
    fn buy_on_downward_cross() {
        let mut g = make_grid();
        // Levels: 100, 102, 104, 106, 108, 110
        g.on_start(&snap(105.0)).unwrap();
        // Price drops from 105 to 103 → crosses level 104
        let actions = g.on_tick(&snap(103.0), &[]).unwrap();
        let buys: Vec<_> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        side: Side::Buy,
                        ..
                    }
                )
            })
            .collect();
        assert!(
            !buys.is_empty(),
            "should emit buy when price drops through level"
        );
    }

    #[test]
    fn sell_on_upward_cross_after_buy() {
        let mut g = make_grid();
        // levels: 100, 102, 104, 106, 108, 110
        g.on_start(&snap(105.0)).unwrap();
        // Drop to 103 → buys at 104
        g.on_tick(&snap(103.0), &[]).unwrap();
        // Rise to 107 → should sell at 106 (level above 104)
        let actions = g.on_tick(&snap(107.0), &[]).unwrap();
        let sells: Vec<_> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        side: Side::Sell,
                        ..
                    }
                )
            })
            .collect();
        assert!(
            !sells.is_empty(),
            "should emit sell after price rises past next level"
        );
    }

    #[test]
    fn no_double_buy_at_same_level() {
        let mut g = make_grid();
        g.on_start(&snap(105.0)).unwrap();
        g.on_tick(&snap(103.0), &[]).unwrap(); // buys at 104
        g.on_tick(&snap(103.5), &[]).unwrap(); // still below 104
        // No new buy — we already hold at level 2 (104)
        let actions = g.on_tick(&snap(103.0), &[]).unwrap();
        let buys: Vec<_> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        side: Side::Buy,
                        ..
                    }
                )
            })
            .collect();
        assert!(buys.is_empty(), "should not double-buy at same level");
    }

    #[test]
    fn complete_buy_sell_cycle() {
        let mut g = make_grid();
        g.on_start(&snap(105.0)).unwrap();
        // Buy at 104
        g.on_tick(&snap(103.0), &[]).unwrap();
        assert!(g.bought_at.contains(&2)); // level index 2 = 104
        // Sell at 106
        g.on_tick(&snap(107.0), &[]).unwrap();
        assert!(!g.bought_at.contains(&2));
        assert_eq!(g.total_cycles, 1);
    }

    #[test]
    fn multiple_levels_crossed() {
        let mut g = make_grid();
        g.on_start(&snap(109.0)).unwrap();
        // Big drop: 109 → 101 → crosses 108, 106, 104, 102
        let actions = g.on_tick(&snap(101.0), &[]).unwrap();
        let buys: Vec<_> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    StrategyAction::PlaceOrder {
                        side: Side::Buy,
                        ..
                    }
                )
            })
            .collect();
        assert!(
            buys.len() >= 3,
            "should buy at multiple levels: {}",
            buys.len()
        );
    }

    #[test]
    fn shutdown_cancels_all() {
        let mut g = make_grid();
        let actions = g.on_shutdown().unwrap();
        assert!(matches!(actions[0], StrategyAction::CancelAll { .. }));
    }
}
