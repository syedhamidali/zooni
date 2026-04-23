#![allow(dead_code)]
//! Interactive Brokers (IB) — US stocks, via TWS or IB Gateway.
//!
//! Docs: <https://interactivebrokers.github.io/tws-api/>
//!
//! IB is structurally different from REST/WS exchanges:
//!   - You run **TWS** (desktop app) or **IB Gateway** (headless) locally — or on
//!     the VPS — listening on a TCP port (default 7497 paper, 7496 live).
//!   - The bot speaks IB's proprietary length-prefixed text protocol over that
//!     socket. There's no HTTPS API.
//!   - Account must be funded (typically ≥ $10k USD) before live orders are accepted.
//!   - Market data requires a subscription per exchange; US equities is a
//!     separate add-on.
//!
//! Pragmatic Rust crate options (do NOT add deps until implementing):
//!   - `ibapi` (rust-ibapi) — synchronous, actively maintained
//!   - `ib_tws_api` — async, older
//!
//! For the Singapore VPS + US stocks plan, either TWS/Gateway runs on the VPS itself
//! (GUI-less Gateway) or the bot connects to a remote gateway. Latency Singapore →
//! NYC ≈ 220ms — acceptable for bar-trigger strategies, unusable for HFT.
//!
//! Holiday + session handling lives in `crate::market_hours::Session::UsEquity`.

pub mod client;

/// Default TWS/Gateway ports.
pub mod ports {
    pub const TWS_LIVE: u16 = 7496;
    pub const TWS_PAPER: u16 = 7497;
    pub const GATEWAY_LIVE: u16 = 4001;
    pub const GATEWAY_PAPER: u16 = 4002;
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    /// Client ID, used by IB to multiplex multiple API clients on one gateway.
    pub client_id: i32,
    /// Paper account if true.
    pub paper: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: ports::TWS_PAPER,
            client_id: 1,
            paper: true,
        }
    }
}
