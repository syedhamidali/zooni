//! Venue integrations.
//!
//! `bybit` is fully wired for the grid bot (crypto spot/perps).
//! `zerodha` is implemented for Indian equity, F&O, and MCX commodities
//! (requires daily access_token from the Kite login flow).
//! The rest are skeletons kept compilable so they can be filled in without
//! restructuring: method bodies return `anyhow::bail!("not yet implemented: …")`
//! so accidental calls fail loudly instead of silently succeeding.

pub mod bybit;
pub mod coindcx;
pub mod hyperliquid;
pub mod interactive_brokers;
pub mod mcx;
pub mod yahoo;
pub mod zerodha;
