//! Venue integrations.
//!
//! Only `bybit` is fully wired for the grid bot. The rest are skeletons kept
//! compilable so they can be filled in without restructuring: method bodies
//! return `anyhow::bail!("not yet implemented: …")` so accidental calls fail
//! loudly instead of silently succeeding.

pub mod bybit;
pub mod hyperliquid;
pub mod interactive_brokers;
pub mod mcx;
pub mod zerodha;
