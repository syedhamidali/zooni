//! Strategy trait — the lifecycle interface every strategy implements.
//!
//! Strategies are pluggable: implement `Strategy` and register via the bot config.

pub mod dca;
pub mod funding_arb;

use anyhow::Result;

use crate::types::Side;

/// An action a strategy wants the bot to execute.
#[derive(Debug, Clone)]
pub enum StrategyAction {
    PlaceOrder {
        symbol: String,
        side: Side,
        price: f64,
        qty: f64,
        order_type: OrderType,
    },
    CancelOrder {
        order_id: String,
    },
    CancelAll {
        symbol: String,
    },
    /// Trip the kill switch (risk event).
    TripKillSwitch {
        reason: String,
    },
    /// No action needed this tick.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Limit,
    Market,
}

/// Market data snapshot provided to the strategy each tick.
#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub last: f64,
    pub volume_24h: f64,
}

/// The common lifecycle of a trading strategy.
pub trait Strategy: Send {
    /// Stable identifier for logging, risk tagging, and persistence keys.
    fn name(&self) -> &'static str;

    /// One-shot startup: initialise state. Returns initial actions.
    fn on_start(&mut self, snapshot: &MarketSnapshot) -> Result<Vec<StrategyAction>>;

    /// Periodic tick: react to market state and fills. Returns actions.
    fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        fills_since_last: &[FillEvent],
    ) -> Result<Vec<StrategyAction>>;

    /// Shutdown: return cleanup actions (usually CancelAll).
    fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>>;
}

/// A fill event reported to the strategy.
#[derive(Debug, Clone)]
pub struct FillEvent {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
}
