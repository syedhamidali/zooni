//! DCA (Dollar-Cost Averaging) strategy — buy at regular intervals regardless of price.
//!
//! Simple but effective for long-term accumulation. Places a market buy every N ticks.
//! Optionally applies a 200 SMA bull-market filter to skip buys when price is below the
//! long-term moving average.

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
    price_history: Vec<f64>, // rolling price window for SMA
    sma_period: usize,       // SMA lookback (default 200)
    use_sma_filter: bool,    // whether to apply the bull-market filter
    buys_skipped: u64,       // count of buys skipped by the SMA filter
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
            price_history: Vec::new(),
            sma_period: 200,
            use_sma_filter: false,
            buys_skipped: 0,
        }
    }

    /// Creates a DCA strategy with the SMA bull-market filter enabled.
    pub fn with_sma_filter(
        symbol: String,
        buy_amount_quote: f64,
        interval_ticks: u64,
        sma_period: usize,
    ) -> Self {
        Self {
            symbol,
            buy_amount_quote,
            interval_ticks,
            tick_count: 0,
            total_bought: 0.0,
            total_spent: 0.0,
            price_history: Vec::new(),
            sma_period,
            use_sma_filter: true,
            buys_skipped: 0,
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

        // Record price and cap history to avoid unbounded growth
        self.price_history.push(snapshot.last);
        let max_len = self.sma_period + 10;
        if self.price_history.len() > max_len {
            let excess = self.price_history.len() - max_len;
            self.price_history.drain(..excess);
        }

        self.tick_count += 1;
        if self.tick_count >= self.interval_ticks {
            // SMA bull-market filter: skip buy when price is below the SMA
            if self.use_sma_filter {
                if let Some(sma_value) =
                    crate::indicators::sma(&self.price_history, self.sma_period)
                {
                    if snapshot.last < sma_value {
                        tracing::debug!(
                            price = snapshot.last,
                            sma = sma_value,
                            period = self.sma_period,
                            "DCA buy skipped: price below SMA (bear market filter)",
                        );
                        self.buys_skipped += 1;
                        self.tick_count = 0;
                        return Ok(vec![StrategyAction::None]);
                    }
                }
                // Insufficient data — fall through to buy normally
            }

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
            sma_filter_active = self.use_sma_filter,
            buys_skipped = self.buys_skipped,
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

    // --- SMA filter tests ---

    #[test]
    fn sma_filter_blocks_buy_in_bear_market() {
        // SMA period = 5 for testability. Feed prices that average higher than current.
        let mut dca = DcaStrategy::with_sma_filter("BTC".to_string(), 100.0, 1, 5);

        // Build up 5 prices so SMA has enough data: all at 200
        for _ in 0..5 {
            let _ = dca.on_tick(&snap(200.0), &[]).unwrap();
        }
        // SMA(5) = 200. Now tick with price 150 (below SMA) — should be blocked.
        let actions = dca.on_tick(&snap(150.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::None),
            "expected None when price below SMA, got {:?}",
            actions[0]
        );
        assert_eq!(dca.buys_skipped, 1);
    }

    #[test]
    fn sma_filter_allows_buy_in_bull_market() {
        let mut dca = DcaStrategy::with_sma_filter("BTC".to_string(), 100.0, 1, 5);

        // Build up 5 prices at 100
        for _ in 0..5 {
            let _ = dca.on_tick(&snap(100.0), &[]).unwrap();
        }
        // SMA(5) = 100. Now tick with price 150 (above SMA) — should buy.
        let actions = dca.on_tick(&snap(150.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::PlaceOrder { .. }),
            "expected PlaceOrder when price above SMA, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn sma_filter_disabled_always_buys() {
        let mut dca = DcaStrategy::new("BTC".to_string(), 100.0, 1);

        // Even with lots of data showing higher average, filter disabled means buy
        for _ in 0..5 {
            let _ = dca.on_tick(&snap(200.0), &[]).unwrap();
        }
        // Price at 50 (way below SMA) — should still buy because filter is off.
        let actions = dca.on_tick(&snap(50.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::PlaceOrder { .. }),
            "expected PlaceOrder when filter disabled, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn sma_filter_with_insufficient_data_buys() {
        // SMA period = 200 but we only feed a few prices — should buy normally.
        let mut dca = DcaStrategy::with_sma_filter("BTC".to_string(), 100.0, 1, 200);

        // Feed only 3 ticks (well below 200), price at 100
        for _ in 0..3 {
            let _ = dca.on_tick(&snap(100.0), &[]).unwrap();
        }
        // SMA(200) returns None with only 4 data points — should buy anyway.
        let actions = dca.on_tick(&snap(50.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::PlaceOrder { .. }),
            "expected PlaceOrder with insufficient SMA data, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn price_history_capped() {
        let period = 5;
        let mut dca = DcaStrategy::with_sma_filter("BTC".to_string(), 100.0, 1, period);

        // Feed many more ticks than sma_period + 10
        for i in 0..50 {
            let _ = dca.on_tick(&snap(100.0 + i as f64), &[]).unwrap();
        }
        // History should be capped at sma_period + 10
        assert!(
            dca.price_history.len() <= period + 10,
            "price_history length {} exceeds cap {}",
            dca.price_history.len(),
            period + 10,
        );
    }

    #[test]
    fn with_sma_filter_constructor() {
        let dca = DcaStrategy::with_sma_filter("ETH".to_string(), 50.0, 10, 200);
        assert!(dca.use_sma_filter);
        assert_eq!(dca.sma_period, 200);
        assert_eq!(dca.interval_ticks, 10);
        assert!((dca.buy_amount_quote - 50.0).abs() < f64::EPSILON);
        assert_eq!(dca.symbol, "ETH");
    }
}
