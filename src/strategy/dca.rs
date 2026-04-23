//! DCA (Dollar-Cost Averaging) strategy — buy at regular intervals regardless of price.
//!
//! Simple but effective for long-term accumulation. Places a market buy every N ticks.

use anyhow::Result;

use super::{FillEvent, MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::types::Side;

pub struct DcaStrategy {
    symbol: String,
    buy_amount_quote: f64, // how much to buy each interval (in quote currency)
    interval_ticks: u64,   // buy every N ticks
    tick_count: u64,
    total_bought: f64,
    total_spent: f64,
}

impl DcaStrategy {
    pub fn new(symbol: String, buy_amount_quote: f64, interval_ticks: u64) -> Self {
        Self {
            symbol,
            buy_amount_quote,
            interval_ticks,
            tick_count: 0,
            total_bought: 0.0,
            total_spent: 0.0,
        }
    }
}

impl Strategy for DcaStrategy {
    fn name(&self) -> &'static str {
        "dca"
    }

    fn on_start(&mut self, _snapshot: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
        Ok(vec![]) // no action on start
    }

    fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        fills: &[FillEvent],
    ) -> Result<Vec<StrategyAction>> {
        // Track fills
        for f in fills {
            if f.side == Side::Buy {
                self.total_bought += f.qty;
                self.total_spent += f.price * f.qty;
            }
        }

        self.tick_count += 1;
        if self.tick_count >= self.interval_ticks {
            self.tick_count = 0;
            let qty = self.buy_amount_quote / snapshot.last;
            Ok(vec![StrategyAction::PlaceOrder {
                symbol: self.symbol.clone(),
                side: Side::Buy,
                price: snapshot.last,
                qty,
                order_type: OrderType::Market,
            }])
        } else {
            Ok(vec![StrategyAction::None])
        }
    }

    fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
        let avg = if self.total_bought > 0.0 {
            self.total_spent / self.total_bought
        } else {
            0.0
        };
        tracing::info!(
            total_bought = self.total_bought,
            total_spent = self.total_spent,
            avg_price = avg,
            "DCA strategy shutdown summary",
        );
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(price: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "BTCUSDT".to_string(),
            bid: price - 1.0,
            ask: price + 1.0,
            last: price,
            volume_24h: 1000.0,
        }
    }

    #[test]
    fn buys_at_correct_interval() {
        let mut dca = DcaStrategy::new("BTCUSDT".to_string(), 100.0, 3);
        let s = snap(50000.0);

        // Ticks 1 and 2 — no action
        let actions = dca.on_tick(&s, &[]).unwrap();
        assert!(matches!(actions[0], StrategyAction::None));
        let actions = dca.on_tick(&s, &[]).unwrap();
        assert!(matches!(actions[0], StrategyAction::None));

        // Tick 3 — should place a buy
        let actions = dca.on_tick(&s, &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, qty, .. } => {
                assert_eq!(*side, Side::Buy);
                assert!((qty - 0.002).abs() < 0.0001); // 100/50000
            }
            other => panic!("expected PlaceOrder, got {:?}", other),
        }
    }

    #[test]
    fn tracks_fills() {
        let mut dca = DcaStrategy::new("BTCUSDT".to_string(), 100.0, 1);
        let s = snap(50000.0);

        let fill = FillEvent {
            order_id: "123".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            price: 50000.0,
            qty: 0.002,
        };
        let _ = dca.on_tick(&s, &[fill]).unwrap();
        let _ = dca.on_shutdown().unwrap();
        // Just verify it doesn't panic — the shutdown logs the summary
    }

    #[test]
    fn name_is_dca() {
        let dca = DcaStrategy::new("X".to_string(), 1.0, 1);
        assert_eq!(dca.name(), "dca");
    }
}
