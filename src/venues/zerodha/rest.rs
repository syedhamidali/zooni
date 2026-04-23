#![allow(dead_code)]
//! Kite Connect REST skeleton.
//!
//! Auth header on every authenticated call:
//!   Authorization: token <api_key>:<access_token>
//!
//! First endpoints to fill in:
//!   POST /session/token               — exchange request_token → access_token
//!   GET  /user/profile                — sanity check
//!   GET  /user/margins                — available funds
//!   GET  /portfolio/positions         — net + day positions
//!   GET  /portfolio/holdings          — long-term holdings
//!   GET  /instruments[/EXCHANGE]      — instrument dump (CSV, daily)
//!   POST /orders/regular              — place order (form-encoded body)
//!   PUT  /orders/regular/:order_id    — modify
//!   DELETE /orders/regular/:order_id  — cancel
//!   GET  /orders                      — order book for the day
//!   GET  /trades                      — trades for the day
//!   GET  /quote[/ltp | /ohlc]         — quotes (up to 500 instruments per call)

use anyhow::{Result, bail};
use reqwest::Client as Http;

use super::{Config, REST_BASE};
use crate::types::Side;

pub struct Client {
    http: Http,
    base: &'static str,
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        Self {
            http: Http::new(),
            base: REST_BASE,
            cfg,
        }
    }

    pub fn base_url(&self) -> &str {
        self.base
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn http(&self) -> &Http {
        &self.http
    }

    /// POST /session/token — body contains api_key, request_token, checksum.
    /// Checksum = hex(SHA256(api_key + request_token + api_secret)).
    pub async fn exchange_request_token(&self, _request_token: &str) -> Result<String> {
        bail!("zerodha::exchange_request_token not yet implemented")
    }

    pub async fn profile(&self) -> Result<()> {
        bail!("zerodha::profile not yet implemented")
    }

    pub async fn margins(&self) -> Result<()> {
        bail!("zerodha::margins not yet implemented")
    }

    pub async fn positions(&self) -> Result<()> {
        bail!("zerodha::positions not yet implemented")
    }

    pub async fn holdings(&self) -> Result<()> {
        bail!("zerodha::holdings not yet implemented")
    }

    pub async fn instruments(&self, _exchange: Option<&str>) -> Result<()> {
        bail!("zerodha::instruments not yet implemented")
    }

    pub async fn place_order(
        &self,
        _tradingsymbol: &str,
        _exchange: &str,
        _product: &str,
        _side: Side,
        _kind: crate::types::OrderKind,
        _quantity: u32,
        _price: Option<f64>,
    ) -> Result<String> {
        bail!("zerodha::place_order not yet implemented")
    }

    pub async fn cancel_order(&self, _order_id: &str) -> Result<()> {
        bail!("zerodha::cancel_order not yet implemented")
    }

    pub async fn ltp(&self, _instruments: &[&str]) -> Result<()> {
        bail!("zerodha::ltp not yet implemented")
    }
}

/// SHA256 concatenation checksum required by POST /session/token.
fn session_checksum(_api_key: &str, _request_token: &str, _api_secret: &str) -> Result<String> {
    bail!("zerodha::session_checksum not yet implemented")
}
