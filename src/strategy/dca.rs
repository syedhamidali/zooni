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
