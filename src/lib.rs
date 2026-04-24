//! Trading bot library — shared modules for main bot and backtester.

pub mod alerts;
pub mod autopilot;
pub mod backtest;
pub mod bot;
pub mod config;
pub mod funding_arb;
pub mod grid;
pub mod indicators;
pub mod market_hours;
pub mod metrics;
pub mod persistence;
pub mod rate_limit;
pub mod regime;
pub mod retry;
pub mod risk;
pub mod scanner;
pub mod sizing;
pub mod strategy;
pub mod telegram;
pub mod tui;
pub mod types;
pub mod venues;
pub mod ws_private;
