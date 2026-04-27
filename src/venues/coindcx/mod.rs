#![allow(dead_code)]
//! CoinDCX — Indian crypto exchange.
//!
//! Docs: <https://docs.coindcx.com>
//! Auth: HMAC-SHA256 over compact JSON body.
//! Headers: `X-AUTH-APIKEY`, `X-AUTH-SIGNATURE`, `Content-Type: application/json`.
//!
//! All authenticated endpoints are POST (even reads like balances).
//! Market symbols use `coindcx_name` (e.g. "BTCUSDT") for REST,
//! but `pair` format (e.g. "B-BTC_USDT") for WebSocket channels.

pub mod rest;

pub const REST_BASE: &str = "https://api.coindcx.com";
pub const PUBLIC_BASE: &str = "https://public.coindcx.com";

#[derive(Debug, Clone)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub credentials: Option<Credentials>,
}
