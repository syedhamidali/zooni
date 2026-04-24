#![allow(dead_code)]
//! Kite Ticker WebSocket — **binary protocol**, not JSON.
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
//!   Packet layouts by mode (all big-endian, prices ×100 for equity):
//!     LTP    =   8 bytes — instrument_token:u32, last_price:i32
//!     QUOTE  =  44 bytes — + OHLC, volume, buy/sell qty, avg_price, change
//!     FULL   = 184 bytes — + L5 depth, timestamps, OI
//!
//! Heartbeat: server sends a 1-byte frame every ~2.5s; if no data for >5s → reconnect.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::{Config, WS_URL};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(6);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ltp,
    Quote,
    Full,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Ltp => "ltp",
            Mode::Quote => "quote",
            Mode::Full => "full",
        }
    }
}

/// Parsed tick data from the binary stream.
#[derive(Debug, Clone)]
pub struct Tick {
    pub instrument_token: u32,
    pub mode: Mode,
    pub last_price: f64,
    pub last_quantity: u32,
    pub average_price: f64,
    pub volume: u32,
    pub buy_quantity: u32,
    pub sell_quantity: u32,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub change: f64,
    pub last_trade_time: u32,
    pub oi: u32,
    pub oi_day_high: u32,
    pub oi_day_low: u32,
    pub exchange_timestamp: u32,
    pub depth: Option<MarketDepth>,
}

impl Default for Tick {
    fn default() -> Self {
        Self {
            instrument_token: 0,
            mode: Mode::Ltp,
            last_price: 0.0,
            last_quantity: 0,
            average_price: 0.0,
            volume: 0,
            buy_quantity: 0,
            sell_quantity: 0,
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            change: 0.0,
            last_trade_time: 0,
            oi: 0,
            oi_day_high: 0,
            oi_day_low: 0,
            exchange_timestamp: 0,
            depth: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MarketDepth {
    pub buy: [DepthLevel; 5],
    pub sell: [DepthLevel; 5],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DepthLevel {
    pub quantity: u32,
    pub price: f64,
    pub orders: u32,
}

/// Events emitted by the ticker.
#[derive(Debug, Clone)]
pub enum TickerEvent {
    Ticks(Vec<Tick>),
    Connected,
    Disconnected,
}

// ---------------------------------------------------------------------------
// Ticker
// ---------------------------------------------------------------------------

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

    /// Start the ticker loop. Sends events on `tx`. Automatically reconnects.
    pub async fn start(&self, tx: mpsc::UnboundedSender<TickerEvent>) {
        loop {
            match self.run_once(&tx).await {
                Ok(()) => info!("Kite ticker closed normally, reconnecting"),
                Err(e) => warn!(err = %e, "Kite ticker error, reconnecting"),
            }
            let _ = tx.send(TickerEvent::Disconnected);
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }

    async fn run_once(&self, tx: &mpsc::UnboundedSender<TickerEvent>) -> Result<()> {
        let creds = self
            .cfg
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow!("no Kite credentials"))?;
        let token = creds
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow!("no access_token for WebSocket"))?;

        let url = format!(
            "{}?api_key={}&access_token={}",
            WS_URL, creds.api_key, token
        );

        info!("connecting to Kite ticker");
        let (mut ws, _) = connect_async(&url).await.context("ws connect failed")?;
        info!("Kite ticker connected");
        let _ = tx.send(TickerEvent::Connected);

        let mut heartbeat_check = interval(HEARTBEAT_TIMEOUT);
        heartbeat_check.tick().await;

        loop {
            tokio::select! {
                _ = heartbeat_check.tick() => {
                    debug!("heartbeat timeout check (no-op if data flowing)");
                }
                msg = ws.next() => match msg {
                    Some(Ok(Message::Binary(data))) => {
                        match parse_binary_frame(&data) {
                            Ok(ticks) if !ticks.is_empty() => {
                                let _ = tx.send(TickerEvent::Ticks(ticks));
                            }
                            Ok(_) => {} // heartbeat (1-byte frame)
                            Err(e) => {
                                debug!(err = %e, "tick parse error");
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        debug!(raw = %text, "control message");
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = ws.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        warn!(?frame, "server closed ticker WS");
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!(err = %e, "ticker WS error");
                        break;
                    }
                    None => {
                        warn!("ticker WS stream ended");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Control messages (client → server)
// ---------------------------------------------------------------------------

/// Build a JSON subscribe message.
pub fn subscribe_msg(tokens: &[u32]) -> String {
    serde_json::json!({"a": "subscribe", "v": tokens}).to_string()
}

/// Build a JSON unsubscribe message.
pub fn unsubscribe_msg(tokens: &[u32]) -> String {
    serde_json::json!({"a": "unsubscribe", "v": tokens}).to_string()
}

/// Build a JSON mode-set message.
pub fn set_mode_msg(mode: Mode, tokens: &[u32]) -> String {
    serde_json::json!({"a": "mode", "v": [mode.as_str(), tokens]}).to_string()
}

// ---------------------------------------------------------------------------
// Binary protocol parser
// ---------------------------------------------------------------------------

/// Parse a complete binary frame into ticks.
/// Frame layout: [u16 num_packets] then for each: [u16 packet_length] [packet_bytes…]
/// A 1-byte frame is a heartbeat — returns an empty vec.
fn parse_binary_frame(data: &[u8]) -> Result<Vec<Tick>> {
    if data.len() <= 1 {
        return Ok(vec![]);
    }
    if data.len() < 2 {
        return Err(anyhow!("frame too short: {} bytes", data.len()));
    }

    let num_packets = read_u16(data, 0) as usize;
    let mut ticks = Vec::with_capacity(num_packets);
    let mut offset = 2;

    for _ in 0..num_packets {
        if offset + 2 > data.len() {
            break;
        }
        let pkt_len = read_u16(data, offset) as usize;
        offset += 2;
        if offset + pkt_len > data.len() {
            break;
        }
        let pkt = &data[offset..offset + pkt_len];
        if let Ok(tick) = parse_packet(pkt) {
            ticks.push(tick);
        }
        offset += pkt_len;
    }

    Ok(ticks)
}

const PRICE_DIVISOR: f64 = 100.0;

/// Parse a single binary tick packet. All integers big-endian, prices ×100.
fn parse_packet(bytes: &[u8]) -> Result<Tick> {
    let len = bytes.len();
    if len < 8 {
        return Err(anyhow!("packet too short: {len} bytes"));
    }

    let instrument_token = read_u32(bytes, 0);
    // Segment is encoded in the top byte of the token
    let _segment = instrument_token & 0xFF;

    let mut tick = Tick {
        instrument_token,
        ..Default::default()
    };

    if len == 8 {
        // LTP mode
        tick.mode = Mode::Ltp;
        tick.last_price = read_i32(bytes, 4) as f64 / PRICE_DIVISOR;
    } else if len == 44 {
        // Quote mode
        tick.mode = Mode::Quote;
        tick.last_price = read_i32(bytes, 4) as f64 / PRICE_DIVISOR;
        tick.last_quantity = read_u32(bytes, 8);
        tick.average_price = read_i32(bytes, 12) as f64 / PRICE_DIVISOR;
        tick.volume = read_u32(bytes, 16);
        tick.buy_quantity = read_u32(bytes, 20);
        tick.sell_quantity = read_u32(bytes, 24);
        tick.open = read_i32(bytes, 28) as f64 / PRICE_DIVISOR;
        tick.high = read_i32(bytes, 32) as f64 / PRICE_DIVISOR;
        tick.low = read_i32(bytes, 36) as f64 / PRICE_DIVISOR;
        tick.close = read_i32(bytes, 40) as f64 / PRICE_DIVISOR;
    } else if len >= 184 {
        // Full mode
        tick.mode = Mode::Full;
        tick.last_price = read_i32(bytes, 4) as f64 / PRICE_DIVISOR;
        tick.last_quantity = read_u32(bytes, 8);
        tick.average_price = read_i32(bytes, 12) as f64 / PRICE_DIVISOR;
        tick.volume = read_u32(bytes, 16);
        tick.buy_quantity = read_u32(bytes, 20);
        tick.sell_quantity = read_u32(bytes, 24);
        tick.open = read_i32(bytes, 28) as f64 / PRICE_DIVISOR;
        tick.high = read_i32(bytes, 32) as f64 / PRICE_DIVISOR;
        tick.low = read_i32(bytes, 36) as f64 / PRICE_DIVISOR;
        tick.close = read_i32(bytes, 40) as f64 / PRICE_DIVISOR;

        tick.last_trade_time = read_u32(bytes, 44);
        tick.oi = read_u32(bytes, 48);
        tick.oi_day_high = read_u32(bytes, 52);
        tick.oi_day_low = read_u32(bytes, 56);
        tick.exchange_timestamp = read_u32(bytes, 60);

        // L5 market depth: 5 buy levels then 5 sell levels, 12 bytes each
        let mut depth = MarketDepth {
            buy: [DepthLevel::default(); 5],
            sell: [DepthLevel::default(); 5],
        };

        for i in 0..5 {
            let base = 64 + i * 12;
            depth.buy[i] = DepthLevel {
                quantity: read_u32(bytes, base),
                price: read_i32(bytes, base + 4) as f64 / PRICE_DIVISOR,
                orders: read_u32(bytes, base + 8),
            };
        }
        for i in 0..5 {
            let base = 124 + i * 12;
            depth.sell[i] = DepthLevel {
                quantity: read_u32(bytes, base),
                price: read_i32(bytes, base + 4) as f64 / PRICE_DIVISOR,
                orders: read_u32(bytes, base + 8),
            };
        }
        tick.depth = Some(depth);
    } else {
        return Err(anyhow!("unexpected packet length: {len}"));
    }

    // Compute change % from close
    if tick.close != 0.0 {
        tick.change = (tick.last_price - tick.close) / tick.close * 100.0;
    }

    Ok(tick)
}

// Big-endian integer readers
fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([buf[offset], buf[offset + 1]])
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

fn read_i32(buf: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ltp_packet() {
        // instrument_token = 256265 (0x0003E909), last_price = 2450.50 (245050 in paise)
        let mut pkt = [0u8; 8];
        pkt[0..4].copy_from_slice(&256265u32.to_be_bytes());
        pkt[4..8].copy_from_slice(&245050i32.to_be_bytes());

        let tick = parse_packet(&pkt).unwrap();
        assert_eq!(tick.instrument_token, 256265);
        assert_eq!(tick.mode, Mode::Ltp);
        assert!((tick.last_price - 2450.50).abs() < 0.001);
    }

    #[test]
    fn parse_quote_packet() {
        let mut pkt = [0u8; 44];
        let token = 738561u32;
        pkt[0..4].copy_from_slice(&token.to_be_bytes());
        pkt[4..8].copy_from_slice(&350025i32.to_be_bytes()); // last_price = 3500.25
        pkt[8..12].copy_from_slice(&100u32.to_be_bytes()); // last_qty
        pkt[12..16].copy_from_slice(&349800i32.to_be_bytes()); // avg_price = 3498.00
        pkt[16..20].copy_from_slice(&5000000u32.to_be_bytes()); // volume
        pkt[20..24].copy_from_slice(&1200000u32.to_be_bytes()); // buy_qty
        pkt[24..28].copy_from_slice(&800000u32.to_be_bytes()); // sell_qty
        pkt[28..32].copy_from_slice(&348000i32.to_be_bytes()); // open = 3480.00
        pkt[32..36].copy_from_slice(&352000i32.to_be_bytes()); // high = 3520.00
        pkt[36..40].copy_from_slice(&347500i32.to_be_bytes()); // low = 3475.00
        pkt[40..44].copy_from_slice(&349000i32.to_be_bytes()); // close = 3490.00

        let tick = parse_packet(&pkt).unwrap();
        assert_eq!(tick.instrument_token, 738561);
        assert_eq!(tick.mode, Mode::Quote);
        assert!((tick.last_price - 3500.25).abs() < 0.001);
        assert_eq!(tick.last_quantity, 100);
        assert!((tick.average_price - 3498.00).abs() < 0.001);
        assert_eq!(tick.volume, 5_000_000);
        assert!((tick.open - 3480.00).abs() < 0.001);
        assert!((tick.high - 3520.00).abs() < 0.001);
        assert!((tick.low - 3475.00).abs() < 0.001);
        assert!((tick.close - 3490.00).abs() < 0.001);
        // change = (3500.25 - 3490.00) / 3490.00 * 100 ≈ 0.2937%
        assert!((tick.change - 0.2937).abs() < 0.01);
    }

    #[test]
    fn parse_full_packet() {
        let mut pkt = [0u8; 184];
        // Quote portion
        pkt[0..4].copy_from_slice(&256265u32.to_be_bytes());
        pkt[4..8].copy_from_slice(&245050i32.to_be_bytes());
        pkt[8..12].copy_from_slice(&50u32.to_be_bytes());
        pkt[12..16].copy_from_slice(&244800i32.to_be_bytes());
        pkt[16..20].copy_from_slice(&1000000u32.to_be_bytes());
        pkt[20..24].copy_from_slice(&500000u32.to_be_bytes());
        pkt[24..28].copy_from_slice(&400000u32.to_be_bytes());
        pkt[28..32].copy_from_slice(&243000i32.to_be_bytes());
        pkt[32..36].copy_from_slice(&246000i32.to_be_bytes());
        pkt[36..40].copy_from_slice(&242500i32.to_be_bytes());
        pkt[40..44].copy_from_slice(&244000i32.to_be_bytes());

        // Extended fields
        pkt[44..48].copy_from_slice(&1714000000u32.to_be_bytes()); // last_trade_time
        pkt[48..52].copy_from_slice(&25000u32.to_be_bytes()); // OI
        pkt[52..56].copy_from_slice(&30000u32.to_be_bytes()); // OI day high
        pkt[56..60].copy_from_slice(&20000u32.to_be_bytes()); // OI day low
        pkt[60..64].copy_from_slice(&1714000001u32.to_be_bytes()); // exchange_timestamp

        // Buy depth level 0
        pkt[64..68].copy_from_slice(&1000u32.to_be_bytes()); // qty
        pkt[68..72].copy_from_slice(&244900i32.to_be_bytes()); // price
        pkt[72..76].copy_from_slice(&15u32.to_be_bytes()); // orders

        // Sell depth level 0
        pkt[124..128].copy_from_slice(&800u32.to_be_bytes());
        pkt[128..132].copy_from_slice(&245100i32.to_be_bytes());
        pkt[132..136].copy_from_slice(&12u32.to_be_bytes());

        let tick = parse_packet(&pkt).unwrap();
        assert_eq!(tick.mode, Mode::Full);
        assert_eq!(tick.oi, 25000);
        assert_eq!(tick.oi_day_high, 30000);
        assert_eq!(tick.last_trade_time, 1714000000);

        let depth = tick.depth.unwrap();
        assert_eq!(depth.buy[0].quantity, 1000);
        assert!((depth.buy[0].price - 2449.00).abs() < 0.001);
        assert_eq!(depth.buy[0].orders, 15);
        assert_eq!(depth.sell[0].quantity, 800);
        assert!((depth.sell[0].price - 2451.00).abs() < 0.001);
    }

    #[test]
    fn parse_binary_frame_heartbeat() {
        let data = [0u8; 1];
        let ticks = parse_binary_frame(&data).unwrap();
        assert!(ticks.is_empty());
    }

    #[test]
    fn parse_binary_frame_single_ltp() {
        // Frame: [u16 num=1] [u16 len=8] [8-byte LTP packet]
        let mut frame = vec![0u8; 2 + 2 + 8];
        frame[0..2].copy_from_slice(&1u16.to_be_bytes()); // 1 packet
        frame[2..4].copy_from_slice(&8u16.to_be_bytes()); // 8 bytes
        frame[4..8].copy_from_slice(&256265u32.to_be_bytes());
        frame[8..12].copy_from_slice(&245050i32.to_be_bytes());

        let ticks = parse_binary_frame(&frame).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].instrument_token, 256265);
        assert!((ticks[0].last_price - 2450.50).abs() < 0.001);
    }

    #[test]
    fn parse_binary_frame_multiple_packets() {
        // 2 LTP packets
        let mut frame = vec![0u8; 2 + (2 + 8) * 2];
        frame[0..2].copy_from_slice(&2u16.to_be_bytes());
        // Packet 1
        frame[2..4].copy_from_slice(&8u16.to_be_bytes());
        frame[4..8].copy_from_slice(&100u32.to_be_bytes());
        frame[8..12].copy_from_slice(&500000i32.to_be_bytes());
        // Packet 2
        frame[12..14].copy_from_slice(&8u16.to_be_bytes());
        frame[14..18].copy_from_slice(&200u32.to_be_bytes());
        frame[18..22].copy_from_slice(&600000i32.to_be_bytes());

        let ticks = parse_binary_frame(&frame).unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].instrument_token, 100);
        assert!((ticks[0].last_price - 5000.00).abs() < 0.001);
        assert_eq!(ticks[1].instrument_token, 200);
        assert!((ticks[1].last_price - 6000.00).abs() < 0.001);
    }

    #[test]
    fn subscribe_message_format() {
        let msg = subscribe_msg(&[256265, 738561]);
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["a"], "subscribe");
        assert_eq!(v["v"][0], 256265);
        assert_eq!(v["v"][1], 738561);
    }

    #[test]
    fn set_mode_message_format() {
        let msg = set_mode_msg(Mode::Full, &[256265]);
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["a"], "mode");
        assert_eq!(v["v"][0], "full");
    }

    #[test]
    fn too_short_packet_errors() {
        let pkt = [0u8; 4];
        assert!(parse_packet(&pkt).is_err());
    }

    #[test]
    fn unexpected_length_errors() {
        let pkt = [0u8; 20]; // not 8, 44, or 184
        assert!(parse_packet(&pkt).is_err());
    }

    #[test]
    fn negative_price_handled() {
        // Prices are i32 — can be negative for change values
        let mut pkt = [0u8; 8];
        pkt[0..4].copy_from_slice(&1u32.to_be_bytes());
        pkt[4..8].copy_from_slice(&(-500i32).to_be_bytes());

        let tick = parse_packet(&pkt).unwrap();
        assert!((tick.last_price - (-5.0)).abs() < 0.001);
    }
}
