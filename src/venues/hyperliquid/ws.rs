#![allow(dead_code)]
//! Hyperliquid WebSocket skeleton.
//!
//! URL: wss://api.hyperliquid.xyz/ws
//!
//! Subscribe shape (JSON):
//!   { "method": "subscribe", "subscription": { "type": "l2Book", "coin": "BTC" } }
//!   { "method": "subscribe", "subscription": { "type": "trades", "coin": "BTC" } }
//!   { "method": "subscribe", "subscription": { "type": "userFills", "user": "0x..." } }
//!
//! Keep-alive: `{ "method": "ping" }`; server replies `{ "channel": "pong" }`.
//! Market-data channels are public; user channels need a valid `user` (address).

use anyhow::{Result, bail};

use super::Config;

pub struct Stream {
    cfg: Config,
}

impl Stream {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub async fn subscribe_l2_book(&mut self, _coin: &str) -> Result<()> {
        bail!("hyperliquid::subscribe_l2_book not yet implemented")
    }

    pub async fn subscribe_trades(&mut self, _coin: &str) -> Result<()> {
        bail!("hyperliquid::subscribe_trades not yet implemented")
    }

    pub async fn subscribe_user_fills(&mut self, _address: &str) -> Result<()> {
        bail!("hyperliquid::subscribe_user_fills not yet implemented")
    }
}
