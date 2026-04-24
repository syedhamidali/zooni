//! DCA (Dollar-Cost Averaging) strategy — buy at regular intervals, sell on profit targets.
//!
//! Accumulates via market buys every N ticks. Optionally applies a 200 SMA bull-market
//! filter to skip buys when price is below the long-term average. Sells holdings when
//! price rises above a take-profit threshold or drops below a trailing stop.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{FillEvent, MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::types::Side;

/// Serializable snapshot of DCA state for persistence across restarts.
#[derive(Serialize, Deserialize)]
pub struct DcaSnapshot {
    pub tick_count: u64,
    pub total_bought: f64,
    pub total_spent: f64,
    pub price_history: Vec<f64>,
    pub buys_skipped: u64,
    pub peak_since_entry: f64,
    pub cycles_completed: u64,
}

pub struct DcaStrategy {
    symbol: String,
    buy_amount_quote: f64,
    interval_ticks: u64,
    tick_count: u64,
    total_bought: f64,
    total_spent: f64,
    price_history: Vec<f64>,
    sma_period: usize,
    use_sma_filter: bool,
    buys_skipped: u64,
    peak_since_entry: f64,
    take_profit_pct: f64,
    trailing_stop_pct: f64,
    cycles_completed: u64,
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
            peak_since_entry: 0.0,
            take_profit_pct: 0.05,
            trailing_stop_pct: 0.03,
            cycles_completed: 0,
        }
    }

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
            peak_since_entry: 0.0,
            take_profit_pct: 0.05,
            trailing_stop_pct: 0.03,
            cycles_completed: 0,
        }
    }

    pub fn with_exits(
        symbol: String,
        buy_amount_quote: f64,
        interval_ticks: u64,
        sma_period: usize,
        use_sma_filter: bool,
        take_profit_pct: f64,
        trailing_stop_pct: f64,
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
            use_sma_filter,
            buys_skipped: 0,
            peak_since_entry: 0.0,
            take_profit_pct,
            trailing_stop_pct,
            cycles_completed: 0,
        }
    }

    fn avg_entry(&self) -> f64 {
        if self.total_bought > 0.0 {
            self.total_spent / self.total_bought
        } else {
            0.0
        }
    }

    fn reset_position(&mut self) {
        self.total_bought = 0.0;
        self.total_spent = 0.0;
        self.peak_since_entry = 0.0;
        self.cycles_completed += 1;
    }

    /// Serialize key state for persistence.
    pub fn snapshot(&self) -> DcaSnapshot {
        DcaSnapshot {
            tick_count: self.tick_count,
            total_bought: self.total_bought,
            total_spent: self.total_spent,
            price_history: self.price_history.clone(),
            buys_skipped: self.buys_skipped,
            peak_since_entry: self.peak_since_entry,
            cycles_completed: self.cycles_completed,
        }
    }

    /// Restore state from a snapshot.
    pub fn restore(&mut self, snap: &DcaSnapshot) {
        self.tick_count = snap.tick_count;
        self.total_bought = snap.total_bought;
        self.total_spent = snap.total_spent;
        self.price_history = snap.price_history.clone();
        self.buys_skipped = snap.buys_skipped;
        self.peak_since_entry = snap.peak_since_entry;
        self.cycles_completed = snap.cycles_completed;
    }
}

impl Strategy for DcaStrategy {
    fn name(&self) -> &'static str {
        "dca"
    }

    fn on_start(&mut self, _snapshot: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
        Ok(vec![])
    }

    fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        fills: &[FillEvent],
    ) -> Result<Vec<StrategyAction>> {
        if snapshot.symbol != self.symbol {
            return Ok(vec![StrategyAction::None]);
        }

        for f in fills {
            if f.side == Side::Buy {
                self.total_bought += f.qty;
                self.total_spent += f.price * f.qty;
            } else if f.side == Side::Sell {
                self.total_bought = (self.total_bought - f.qty).max(0.0);
                self.total_spent = self.avg_entry() * self.total_bought;
            }
        }

        self.price_history.push(snapshot.last);
        let max_len = self.sma_period + 10;
        if self.price_history.len() > max_len {
            let excess = self.price_history.len() - max_len;
            self.price_history.drain(..excess);
        }

        // Exit logic: check take-profit and trailing stop
        if self.total_bought > 1e-12 {
            let price = snapshot.last;
            let avg = self.avg_entry();

            if price > self.peak_since_entry {
                self.peak_since_entry = price;
            }

            let profit_pct = (price - avg) / avg;

            // Take profit: sell when price is above avg entry by take_profit_pct
            if profit_pct >= self.take_profit_pct {
                let qty = self.total_bought;
                self.reset_position();
                return Ok(vec![StrategyAction::PlaceOrder {
                    symbol: self.symbol.clone(),
                    side: Side::Sell,
                    price,
                    qty,
                    order_type: OrderType::Market,
                }]);
            }

            // Trailing stop: sell when price drops trailing_stop_pct from peak
            if self.peak_since_entry > avg && self.trailing_stop_pct > 0.0 {
                let drop_from_peak = (self.peak_since_entry - price) / self.peak_since_entry;
                if drop_from_peak >= self.trailing_stop_pct {
                    let qty = self.total_bought;
                    self.reset_position();
                    return Ok(vec![StrategyAction::PlaceOrder {
                        symbol: self.symbol.clone(),
                        side: Side::Sell,
                        price,
                        qty,
                        order_type: OrderType::Market,
                    }]);
                }
            }
        }

        // Buy logic — adjust interval based on volatility (ATR).
        // Higher volatility → buy less frequently; lower → buy more frequently.
        self.tick_count += 1;
        let effective_interval = if self.price_history.len() >= 15 {
            // Compute a simple ATR proxy using recent price range
            let recent = &self.price_history[self.price_history.len().saturating_sub(14)..];
            let avg_price = recent.iter().sum::<f64>() / recent.len() as f64;
            if avg_price > 0.0 {
                let volatility: f64 = recent
                    .windows(2)
                    .map(|w| ((w[1] - w[0]) / avg_price).abs())
                    .sum::<f64>()
                    / (recent.len() - 1) as f64;
                let base_vol = 0.01; // 1% baseline daily volatility
                let scale = (volatility / base_vol).clamp(0.5, 3.0);
                ((self.interval_ticks as f64 * scale) as u64).max(1)
            } else {
                self.interval_ticks
            }
        } else {
            self.interval_ticks
        };
        if self.tick_count >= effective_interval {
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
        let avg = self.avg_entry();
        tracing::info!(
            total_bought = self.total_bought,
            total_spent = self.total_spent,
            avg_price = avg,
            sma_filter_active = self.use_sma_filter,
            buys_skipped = self.buys_skipped,
            cycles_completed = self.cycles_completed,
            "DCA strategy shutdown summary",
        );
        Ok(vec![])
    }

    fn tunable_params(&self) -> HashMap<String, f64> {
        HashMap::from([
            ("take_profit_pct".into(), self.take_profit_pct),
            ("trailing_stop_pct".into(), self.trailing_stop_pct),
            ("interval_ticks".into(), self.interval_ticks as f64),
        ])
    }

    fn apply_params(&mut self, params: &HashMap<String, f64>) {
        if let Some(&v) = params.get("take_profit_pct") {
            self.take_profit_pct = v.clamp(0.01, 0.50);
        }
        if let Some(&v) = params.get("trailing_stop_pct") {
            self.trailing_stop_pct = v.clamp(0.005, 0.20);
        }
        if let Some(&v) = params.get("interval_ticks") {
            self.interval_ticks = (v as u64).clamp(1, 100);
        }
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&self.snapshot()).ok()
    }

    fn load_state(&mut self, json: &str) {
        if let Ok(snap) = serde_json::from_str::<DcaSnapshot>(json) {
            self.restore(&snap);
        }
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

        let actions = dca.on_tick(&s, &[]).unwrap();
        assert!(matches!(actions[0], StrategyAction::None));
        let actions = dca.on_tick(&s, &[]).unwrap();
        assert!(matches!(actions[0], StrategyAction::None));

        let actions = dca.on_tick(&s, &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, qty, .. } => {
                assert_eq!(*side, Side::Buy);
                assert!((qty - 0.002).abs() < 0.0001);
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
    }

    #[test]
    fn name_is_dca() {
        let dca = DcaStrategy::new("X".to_string(), 1.0, 1);
        assert_eq!(dca.name(), "dca");
    }

    #[test]
    fn sma_filter_blocks_buy_in_bear_market() {
        let mut dca = DcaStrategy::with_sma_filter("BTCUSDT".to_string(), 100.0, 1, 5);

        for _ in 0..5 {
            let _ = dca.on_tick(&snap(200.0), &[]).unwrap();
        }
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
        let mut dca = DcaStrategy::with_sma_filter("BTCUSDT".to_string(), 100.0, 1, 5);

        for _ in 0..5 {
            let _ = dca.on_tick(&snap(100.0), &[]).unwrap();
        }
        let actions = dca.on_tick(&snap(150.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::PlaceOrder { .. }),
            "expected PlaceOrder when price above SMA, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn sma_filter_disabled_always_buys() {
        let mut dca = DcaStrategy::new("BTCUSDT".to_string(), 100.0, 1);

        for _ in 0..5 {
            let _ = dca.on_tick(&snap(200.0), &[]).unwrap();
        }
        let actions = dca.on_tick(&snap(50.0), &[]).unwrap();
        assert!(
            matches!(actions[0], StrategyAction::PlaceOrder { .. }),
            "expected PlaceOrder when filter disabled, got {:?}",
            actions[0]
        );
    }

    #[test]
    fn sma_filter_with_insufficient_data_buys() {
        let mut dca = DcaStrategy::with_sma_filter("BTCUSDT".to_string(), 100.0, 1, 200);

        for _ in 0..3 {
            let _ = dca.on_tick(&snap(100.0), &[]).unwrap();
        }
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
        let mut dca = DcaStrategy::with_sma_filter("BTCUSDT".to_string(), 100.0, 1, period);

        for i in 0..50 {
            let _ = dca.on_tick(&snap(100.0 + i as f64), &[]).unwrap();
        }
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

    #[test]
    fn take_profit_sells_at_target() {
        let mut dca =
            DcaStrategy::with_exits("BTCUSDT".to_string(), 100.0, 1, 5, false, 0.05, 0.03);

        // Buy at 100
        let buy_fill = FillEvent {
            order_id: "b1".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            price: 100.0,
            qty: 1.0,
        };
        let _ = dca.on_tick(&snap(100.0), &[buy_fill]).unwrap();

        // Price rises 5% to 105 — should trigger take-profit sell
        let actions = dca.on_tick(&snap(105.0), &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, qty, .. } => {
                assert_eq!(*side, Side::Sell);
                assert!((qty - 1.0).abs() < 1e-9, "should sell full position");
            }
            other => panic!("expected sell on take-profit, got {:?}", other),
        }
        assert_eq!(dca.cycles_completed, 1);
    }

    #[test]
    fn trailing_stop_sells_on_drop() {
        let mut dca =
            DcaStrategy::with_exits("BTCUSDT".to_string(), 100.0, 1, 5, false, 0.20, 0.03);

        // Buy at 100
        let buy_fill = FillEvent {
            order_id: "b1".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            price: 100.0,
            qty: 1.0,
        };
        let _ = dca.on_tick(&snap(100.0), &[buy_fill]).unwrap();

        // Price rises to 110 (new peak, above avg entry)
        let _ = dca.on_tick(&snap(110.0), &[]).unwrap();
        assert!((dca.peak_since_entry - 110.0).abs() < 1e-9);

        // Price drops >3% from peak to 106.0 — should trigger trailing stop
        let actions = dca.on_tick(&snap(106.0), &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, .. } => {
                assert_eq!(*side, Side::Sell, "should sell on trailing stop");
            }
            other => panic!("expected sell on trailing stop, got {:?}", other),
        }
    }

    #[test]
    fn no_sell_when_no_position() {
        let mut dca =
            DcaStrategy::with_exits("BTCUSDT".to_string(), 100.0, 1, 5, false, 0.05, 0.03);

        // No buys — should just buy normally
        let actions = dca.on_tick(&snap(100.0), &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, .. } => {
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected buy, got {:?}", other),
        }
    }

    #[test]
    fn resumes_buying_after_sell_cycle() {
        let mut dca =
            DcaStrategy::with_exits("BTCUSDT".to_string(), 100.0, 1, 5, false, 0.05, 0.03);

        // Cycle 1: buy then take profit
        let buy = FillEvent {
            order_id: "b1".to_string(),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            price: 100.0,
            qty: 1.0,
        };
        let _ = dca.on_tick(&snap(100.0), &[buy]).unwrap();
        let _ = dca.on_tick(&snap(106.0), &[]).unwrap(); // sells (take-profit)
        assert_eq!(dca.cycles_completed, 1);
        assert!(dca.total_bought < 1e-12);

        // Next tick: should buy again (new cycle)
        let actions = dca.on_tick(&snap(106.0), &[]).unwrap();
        match &actions[0] {
            StrategyAction::PlaceOrder { side, .. } => {
                assert_eq!(*side, Side::Buy, "should resume buying after sell");
            }
            other => panic!("expected buy after cycle reset, got {:?}", other),
        }
    }
}
