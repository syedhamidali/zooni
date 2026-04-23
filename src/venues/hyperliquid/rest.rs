#![allow(dead_code)]
//! Hyperliquid REST skeleton.
//!
//! Read examples (no auth, POST /info):
//!   { "type": "meta" }
//!   { "type": "userState", "user": "0x..." }
//!   { "type": "l2Book", "coin": "BTC" }
//!
//! Write pattern (auth, POST /exchange):
//!   { "action": {...}, "nonce": <ms_ts>, "signature": {r, s, v}, "vaultAddress": null }
//! The `action` is hashed with Hyperliquid's EIP-712 domain + types and signed
//! with the agent wallet's private key.

use anyhow::{Result, bail};
use reqwest::Client as Http;

use super::{Config, REST_BASE, REST_BASE_TESTNET};

pub struct Client {
    http: Http,
    base: &'static str,
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        let base = if cfg.testnet {
            REST_BASE_TESTNET
        } else {
            REST_BASE
        };
        Self {
            http: Http::new(),
            base,
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

    /// POST /info { type: "meta" } — exchange metadata (universes, leverage caps, funding).
    pub async fn meta(&self) -> Result<()> {
        bail!("hyperliquid::meta not yet implemented")
    }

    /// POST /info { type: "userState", user: <addr> } — positions, margin, open orders.
    pub async fn user_state(&self, _address: &str) -> Result<()> {
        bail!("hyperliquid::user_state not yet implemented")
    }

    /// POST /info { type: "l2Book", coin: <coin> } — order book snapshot.
    pub async fn l2_book(&self, _coin: &str) -> Result<()> {
        bail!("hyperliquid::l2_book not yet implemented")
    }

    /// POST /exchange — place a limit order. Requires EIP-712 signing with the agent key.
    pub async fn place_order(
        &self,
        _coin: &str,
        _is_buy: bool,
        _size: &str,
        _limit_px: Option<&str>,
    ) -> Result<()> {
        bail!("hyperliquid::place_order not yet implemented (needs EIP-712 signer)")
    }

    /// POST /exchange — cancel by order id.
    pub async fn cancel_order(&self, _coin: &str, _order_id: u64) -> Result<()> {
        bail!("hyperliquid::cancel_order not yet implemented (needs EIP-712 signer)")
    }
}

/// Placeholder signature. Real impl will use `alloy-signer-local` to EIP-712 sign the action.
fn sign_action(
    _private_key_hex: &str,
    _action_json: &str,
    _nonce_ms: u64,
) -> Result<(String, String, u8)> {
    bail!("hyperliquid::sign_action not yet implemented (add alloy-signer-local dep)")
}
