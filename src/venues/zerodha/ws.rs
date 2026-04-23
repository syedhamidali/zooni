#![allow(dead_code)]
//! Kite Ticker WebSocket skeleton — **binary protocol**, not JSON.
//!
//! URL: `wss://ws.kite.trade?api_key=<k>&access_token=<t>`
//!
//! Control messages (client → server) are JSON:
//!   {"a":"subscribe","v":[instrument_token, ...]}
//!   {"a":"unsubscribe","v":[...]}
//!   {"a":"mode","v":["ltp"|"quote"|"full", [instrument_token, ...]]}
//!
//! Tick frames (server → client) are binary:
//!   [u16 num_packets] then N packets, each prefixed with [u16 length].
//!   Packet layouts by mode (all big-endian, prices in paise for equity):
//!     LTP    =   8 bytes — instrument_token:u32, last_price:u32
//!     QUOTE  =  44 bytes — + OHLC, volume, avg, change
//!     FULL   = 184 bytes — + bid/ask L5, last_trade_time, oi, oi_day_hi/lo
//!
//! Heartbeat: server sends a 1-byte frame every ~1s; if no data for >5s → reconnect.

use anyhow::{Result, bail};

use super::Config;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Ltp,
    Quote,
    Full,
}

pub struct Ticker {
    cfg: Config,
}

impl Ticker {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub async fn connect(&mut self) -> Result<()> {
        bail!("zerodha::ticker::connect not yet implemented")
    }

    pub async fn subscribe(&mut self, _tokens: &[u32], _mode: Mode) -> Result<()> {
        bail!("zerodha::ticker::subscribe not yet implemented")
    }

    pub async fn unsubscribe(&mut self, _tokens: &[u32]) -> Result<()> {
        bail!("zerodha::ticker::unsubscribe not yet implemented")
    }
}

/// Parse a single binary tick packet of `bytes.len()` bytes. All integers big-endian.
fn parse_packet(_bytes: &[u8]) -> Result<()> {
    bail!("zerodha::ticker::parse_packet not yet implemented")
}
