#![allow(dead_code)]
//! Bybit V5 API.
//!
//! Docs: <https://bybit-exchange.github.io/docs/v5/intro>
//! Auth: HMAC-SHA256. Headers: `X-BAPI-API-KEY`, `X-BAPI-TIMESTAMP`,
//!       `X-BAPI-RECV-WINDOW`, `X-BAPI-SIGN`.
//! Sign payload: `timestamp + api_key + recv_window + (query_string | body_json)`.
//!
//! Rate limits: per-endpoint, published in response headers. Trading endpoints
//! are under a separate 600-orders/5s bucket.
//!
//! Whitelist the VPS IP in the API key panel once credentials are provisioned.

pub mod rest;
pub mod signer;
pub mod ws;

pub const REST_BASE: &str = "https://api.bybit.com";
pub const REST_BASE_TESTNET: &str = "https://api-testnet.bybit.com";
pub const WS_PUBLIC_LINEAR: &str = "wss://stream.bybit.com/v5/public/linear";
pub const WS_PUBLIC_SPOT: &str = "wss://stream.bybit.com/v5/public/spot";
pub const WS_PUBLIC_SPOT_TESTNET: &str = "wss://stream-testnet.bybit.com/v5/public/spot";
pub const WS_PUBLIC_LINEAR_TESTNET: &str = "wss://stream-testnet.bybit.com/v5/public/linear";
pub const WS_PRIVATE: &str = "wss://stream.bybit.com/v5/private";
pub const WS_PRIVATE_TESTNET: &str = "wss://stream-testnet.bybit.com/v5/private";

#[derive(Debug, Clone)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub credentials: Option<Credentials>,
    pub testnet: bool,
    pub recv_window_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { credentials: None, testnet: false, recv_window_ms: 5_000 }
    }
}
