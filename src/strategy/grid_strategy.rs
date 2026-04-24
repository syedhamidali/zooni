//! Grid strategy as a Strategy trait impl for the orchestrator.
//!
//! Self-contained grid: tracks price movement across grid levels and emits
//! buy/sell orders when price crosses levels. Buys on downward crosses,
//! sells when price rises to the next level up. Profit = spacing * qty per cycle.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::Utc;

use super::{FillEvent, MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::grid::compute_levels;
use crate::market_hours;
use crate::regime::Regime;
use crate::types::Side;

#[derive(Debug, Clone)]
pub struct GridStrategyConfig {
    pub symbol: String,
    pub lower_price: f64,
    pub upper_price: f64,
    pub levels: usize,
    pub qty_per_level: f64,
    /// Cancel all buy orders when regime transitions from Ranging to Trending.
    pub cancel_buys_on_trend: bool,
    /// Automatically re-center the grid when price escapes the band by this fraction.
    /// Set to 0.0 to disable.
    pub auto_recenter: bool,
    /// Fraction beyond the upper/lower band that triggers re-centering (e.g. 0.15 = 15%).
    pub recenter_threshold_pct: f64,
    /// Only place new grid orders when the current price is within `vwap_threshold_pct`
    /// of VWAP. Reduces adverse selection on grid start.
    pub vwap_entry_filter: bool,
    /// Maximum fractional deviation from VWAP allowed before skipping order placement.
    /// E.g. 0.005 = within 0.5% of VWAP. Only used when `vwap_entry_filter = true`.
    pub vwap_threshold_pct: f64,
    /// Venue / exchange identifier for market-hours checking (e.g. "zerodha", "bybit").
    pub venue: String,
    /// When `true`, skip grid ticks outside market hours and cancel buy orders
    /// 15 minutes before close. For 24/7 venues this has no observable effect.
    pub respect_market_hours: bool,
}

impl Default for GridStrategyConfig {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            lower_price: 0.0,
            upper_price: 0.0,
            levels: 10,
            qty_per_level: 0.0,
            cancel_buys_on_trend: true,
            auto_recenter: false,
            recenter_threshold_pct: 0.15,
            vwap_entry_filter: false,
            vwap_threshold_pct: 0.005,
            venue: "bybit".to_string(),
            respect_market_hours: true,
        }
    }
}

pub struct GridStrategy {
    symbol: String,
    lower_price: f64,
    upper_price: f64,
    levels: Vec<f64>,
    qty_base: f64,
    last_price: Option<f64>,
    bought_at: HashSet<usize>,
    total_cycles: u64,
    /// Most recent regime (used to detect transitions for buy cancellation).
    last_regime: Option<Regime>,
    /// Cancel buy orders on trend breakout from ranging.
    cancel_buys_on_trend: bool,
    /// Enable automatic re-centering when price escapes grid band.
    auto_recenter: bool,
    /// Fraction beyond upper/lower band that triggers re-centering.
    recenter_threshold_pct: f64,
    /// Most recent ATR value for re-centering half-range calculation.
    last_atr: f64,
    /// Only place new orders when price is within `vwap_threshold_pct` of VWAP.
    vwap_entry_filter: bool,
    /// Maximum fractional deviation from VWAP to allow grid placement.
    vwap_threshold_pct: f64,
    /// Rolling price history for intra-strategy VWAP estimation (typical prices).
    vwap_prices: Vec<f64>,
    /// Rolling volume history paired with `vwap_prices`.
    vwap_volumes: Vec<f64>,
    /// Venue identifier for market-hours checking (e.g. "zerodha", "bybit").
    venue: String,
    /// Skip ticks outside market hours and cancel buy orders near close.
    respect_market_hours: bool,
    /// Track whether we already logged the "market closed" message this closed period.
    market_closed_logged: bool,
}

impl GridStrategy {
    pub fn new(config: GridStrategyConfig) -> Self {
        let levels = compute_levels(config.lower_price, config.upper_price, config.levels);
        Self {
            symbol: config.symbol,
            lower_price: config.lower_price,
            upper_price: config.upper_price,
            levels,
            qty_base: config.qty_per_level,
            last_price: None,
            bought_at: HashSet::new(),
            total_cycles: 0,
            last_regime: None,
            cancel_buys_on_trend: config.cancel_buys_on_trend,
            auto_recenter: config.auto_recenter,
            recenter_threshold_pct: config.recenter_threshold_pct,
            last_atr: 0.0,
            vwap_entry_filter: config.vwap_entry_filter,
            vwap_threshold_pct: config.vwap_threshold_pct,
            vwap_prices: Vec::new(),
            vwap_volumes: Vec::new(),
            venue: config.venue,
            respect_market_hours: config.respect_market_hours,
            market_closed_logged: false,
        }
    }

    /// Update the regime known to this strategy. Returns any actions triggered by a transition.
    ///
    /// When regime changes from Ranging -> TrendingUp/TrendingDown and `cancel_buys_on_trend`
    /// is enabled, all open buy orders are cancelled to prevent accumulating into a trend.
    pub fn apply_regime(&mut self, regime: Regime, atr: f64) -> Vec<StrategyAction> {
        self.last_atr = atr;
        let prev = self.last_regime.take();
        self.last_regime = Some(regime);

        if self.cancel_buys_on_trend {
            let was_ranging = matches!(prev, Some(Regime::Ranging));
            let now_trending = matches!(regime, Regime::TrendingUp | Regime::TrendingDown);
            if was_ranging && now_trending {
                tracing::info!(
                    "Trend breakout detected, cancelling buy orders to prevent accumulating into trend"
                );
                return vec![StrategyAction::CancelAll {
                    symbol: self.symbol.clone(),
                }];
            }
        }

        vec![]
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

        // Market hours guard
        if self.respect_market_hours {
            let now = Utc::now();
            let venue = self.venue.as_str();

            if !market_hours::is_market_open(venue, now) {
                if !self.market_closed_logged {
                    tracing::info!(venue, "Market closed for {venue}, skipping grid tick");
                    self.market_closed_logged = true;
                }
                return Ok(vec![StrategyAction::None]);
            }

            self.market_closed_logged = false;

            // Cancel all buy orders 15 minutes before close (MIS intraday risk).
            if let Some(mins) = market_hours::minutes_until_close(venue, now) {
                if (0..=15).contains(&mins) {
                    tracing::info!(
                        venue,
                        mins,
                        "Market closing in {mins}min -- cancelling buy orders"
                    );
                    return Ok(vec![StrategyAction::CancelAll {
                        symbol: self.symbol.clone(),
                    }]);
                }
            }
        }

        let price = snapshot.last;

        // Accumulate price/volume history for VWAP estimation.
        const VWAP_WINDOW: usize = 100;
        self.vwap_prices.push(price);
        self.vwap_volumes.push(snapshot.volume_24h);
        if self.vwap_prices.len() > VWAP_WINDOW {
            self.vwap_prices.remove(0);
            self.vwap_volumes.remove(0);
        }

        // VWAP entry filter
        if self.vwap_entry_filter {
            if let Some(vwap_val) = crate::indicators::vwap(
                &self.vwap_prices,
                &self.vwap_prices,
                &self.vwap_prices,
                &self.vwap_volumes,
            ) {
                let deviation_pct = (price - vwap_val).abs() / vwap_val;
                if deviation_pct > self.vwap_threshold_pct {
                    tracing::debug!(
                        "Skipping grid placement: price {:.2} is {:.2}% from VWAP {:.2}",
                        price,
                        deviation_pct * 100.0,
                        vwap_val
                    );
                    self.last_price = Some(price);
                    return Ok(vec![StrategyAction::None]);
                }
            }
        }

        let mut actions = Vec::new();

        // Auto re-centering
        if self.auto_recenter && self.recenter_threshold_pct > 0.0 {
            let threshold = self.recenter_threshold_pct;
            let price_above = price > self.upper_price * (1.0 + threshold);
            let price_below = price < self.lower_price * (1.0 - threshold);

            if price_above || price_below {
                let old_lower = self.lower_price;
                let old_upper = self.upper_price;

                let half_range = if self.last_atr > 0.0 {
                    2.0 * self.last_atr
                } else {
                    (old_upper - old_lower) / 2.0
                };
                let new_lower = (price - half_range).max(0.01);
                let new_upper = price + half_range;

                tracing::info!(
                    symbol = %self.symbol,
                    old_lower,
                    old_upper,
                    new_lower,
                    new_upper,
                    current_price = price,
                    "grid auto re-center: price escaped band, rebuilding grid around current price",
                );

                self.lower_price = new_lower;
                self.upper_price = new_upper;
                let n_levels = self.levels.len();
                self.levels = compute_levels(new_lower, new_upper, n_levels);
                self.bought_at.clear();

                actions.push(StrategyAction::CancelAll {
                    symbol: self.symbol.clone(),
                });

                for (i, &level) in self.levels.iter().enumerate() {
                    let side = if level < price {
                        Side::Buy
                    } else if level > price {
                        Side::Sell
                    } else {
                        continue;
                    };
                    if side == Side::Buy {
                        self.bought_at.insert(i);
                    }
                    actions.push(StrategyAction::PlaceOrder {
                        symbol: self.symbol.clone(),
                        side,
                        price: level,
                        qty: self.qty_base,
                        order_type: OrderType::Limit,
                    });
                }

                self.last_price = Some(price);
                return Ok(actions);
            }
        }

        if let Some(last) = self.last_price {
            for (i, &level) in self.levels.iter().enumerate() {
                // Price crossed DOWN through a level -> buy
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

                // Price crossed UP through a level -> sell if we hold at level below
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
            cancel_buys_on_trend: false,
            auto_recenter: false,
            recenter_threshold_pct: 0.15,
            vwap_entry_filter: false,
            vwap_threshold_pct: 0.005,
            venue: "bybit".into(),
            respect_market_hours: false,
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
        g.on_start(&snap(105.0)).unwrap();
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
        g.on_start(&snap(105.0)).unwrap();
        g.on_tick(&snap(103.0), &[]).unwrap();
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
        g.on_tick(&snap(103.0), &[]).unwrap();
        g.on_tick(&snap(103.5), &[]).unwrap();
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
        g.on_tick(&snap(103.0), &[]).unwrap();
        assert!(g.bought_at.contains(&2));
        g.on_tick(&snap(107.0), &[]).unwrap();
        assert!(!g.bought_at.contains(&2));
        assert_eq!(g.total_cycles, 1);
    }

    #[test]
    fn multiple_levels_crossed() {
        let mut g = make_grid();
        g.on_start(&snap(109.0)).unwrap();
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
