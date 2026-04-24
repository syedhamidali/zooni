#![allow(dead_code)]
//! Zerodha Kite Connect v3 — Indian equity + F&O + MCX commodities.
//!
//! Docs: <https://kite.trade/docs/connect/v3/>
//!
//! Auth flow (daily rotation):
//!   1. User opens the Kite login URL with `api_key` → logs in (password + TOTP)
//!      → redirected with `request_token` on the query string.
//!   2. Server exchanges `api_key + request_token + api_secret` via POST /session/token
//!      (with sha256 checksum) → receives `access_token`.
//!   3. `access_token` is valid **until ~06:00 IST next day** — must be refreshed daily.
//!   4. REST auth header: `Authorization: token <api_key>:<access_token>`.
//!
//! The daily login cannot be fully automated: Kite requires interactive TOTP input
//! (Zerodha policy). Pragmatic options on a VPS:
//!   - Headless browser + TOTP generator (fragile; TOS-adjacent)
//!   - Manual login on laptop each morning → paste token into a secure endpoint
//!   - A local login helper that POSTs the fresh access_token to the VPS
//!
//! Ticker WebSocket uses a **binary protocol** (not JSON) — length-prefixed packets,
//! big-endian integers, prices in paise for equity. See `ws.rs` for layout.
//!
//! Instruments are identified by numeric `instrument_token` from the daily instrument
//! CSV dump at <https://api.kite.trade/instruments>.
//!
//! MCX is accessed through the same Kite API — use exchange code "MCX" and the
//! appropriate product (NRML for carry-forward, MIS for intraday). See `crate::venues::mcx`
//! for session hours and instrument notes.
//!
//! Rate limits: ~10 req/s for most endpoints, 1 req/s for historical data.

pub mod rest;
pub mod ws;

pub const REST_BASE: &str = "https://api.kite.trade";
pub const LOGIN_URL_TEMPLATE: &str = "https://kite.zerodha.com/connect/login?v=3&api_key={api_key}";
pub const WS_URL: &str = "wss://ws.kite.trade";
pub const INSTRUMENTS_DUMP_URL: &str = "https://api.kite.trade/instruments";

/// Kite product codes (cash equity + derivatives).
pub mod product {
    pub const CNC: &str = "CNC"; // cash-and-carry (equity delivery)
    pub const MIS: &str = "MIS"; // intraday
    pub const NRML: &str = "NRML"; // F&O or commodity carry-forward
    pub const MTF: &str = "MTF"; // margin trading facility
}

/// Kite exchange codes.
pub mod exchange {
    pub const NSE: &str = "NSE";
    pub const BSE: &str = "BSE";
    pub const NFO: &str = "NFO"; // NSE F&O
    pub const BFO: &str = "BFO"; // BSE F&O
    pub const CDS: &str = "CDS"; // currency derivatives
    pub const MCX: &str = "MCX"; // commodity derivatives
}

/// Kite order variety codes.
pub mod variety {
    pub const REGULAR: &str = "regular";
    pub const AMO: &str = "amo"; // after-market order
    pub const CO: &str = "co"; // cover order
    pub const ICEBERG: &str = "iceberg";
    pub const AUCTION: &str = "auction";
}

/// Kite validity codes.
pub mod validity {
    pub const DAY: &str = "DAY";
    pub const IOC: &str = "IOC";
    pub const TTL: &str = "TTL"; // minutes-validity (for AMO/iceberg)
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
    pub access_token: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub credentials: Option<Credentials>,
}

impl Config {
    pub fn set_access_token(&mut self, token: String) {
        if let Some(ref mut creds) = self.credentials {
            creds.access_token = Some(token);
        }
    }

    pub fn login_url(&self) -> Option<String> {
        self.credentials
            .as_ref()
            .map(|c| LOGIN_URL_TEMPLATE.replace("{api_key}", &c.api_key))
    }
}
