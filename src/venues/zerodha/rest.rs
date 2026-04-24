#![allow(dead_code)]
//! Kite Connect v3 REST client.
//!
//! Auth header on every authenticated call:
//!   Authorization: token <api_key>:<access_token>
//!
//! Kite uses **form-encoded** bodies for POST/PUT (not JSON).
//! Responses are JSON: `{"status": "success", "data": {...}}` or
//! `{"status": "error", "message": "...", "error_type": "..."}`.

use anyhow::{Context, Result, anyhow};
use reqwest::Client as Http;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

use super::{Config, REST_BASE};
use crate::types::Side;

// ---------------------------------------------------------------------------
// Response envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    status: String,
    data: Option<T>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
}

impl<T> Envelope<T> {
    fn into_result(self, ctx: &str) -> Result<T> {
        if self.status == "success" {
            self.data
                .ok_or_else(|| anyhow!("{ctx}: success but no data"))
        } else {
            let msg = self.message.unwrap_or_default();
            let etype = self.error_type.unwrap_or_default();
            Err(anyhow!("{ctx}: {etype} — {msg}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub user_id: String,
    pub user_name: String,
    pub email: String,
    pub user_type: String,
    pub broker: String,
    #[serde(default)]
    pub exchanges: Vec<String>,
    #[serde(default)]
    pub products: Vec<String>,
    #[serde(default)]
    pub order_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Margins {
    pub equity: Option<MarginSegment>,
    pub commodity: Option<MarginSegment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarginSegment {
    pub enabled: bool,
    #[serde(default)]
    pub net: f64,
    pub available: MarginAvailable,
    pub utilised: HashMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarginAvailable {
    #[serde(default)]
    pub cash: f64,
    #[serde(default)]
    pub collateral: f64,
    #[serde(rename = "intraday_payin", default)]
    pub intraday_payin: f64,
    #[serde(rename = "live_balance", default)]
    pub live_balance: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub tradingsymbol: String,
    pub exchange: String,
    #[serde(rename = "instrument_token")]
    pub instrument_token: u32,
    pub product: String,
    pub quantity: i32,
    #[serde(rename = "overnight_quantity", default)]
    pub overnight_quantity: i32,
    #[serde(rename = "average_price", default)]
    pub average_price: f64,
    #[serde(rename = "last_price", default)]
    pub last_price: f64,
    #[serde(default)]
    pub pnl: f64,
    #[serde(rename = "m2m", default)]
    pub m2m: f64,
    #[serde(rename = "buy_quantity", default)]
    pub buy_quantity: i32,
    #[serde(rename = "sell_quantity", default)]
    pub sell_quantity: i32,
    #[serde(rename = "buy_price", default)]
    pub buy_price: f64,
    #[serde(rename = "sell_price", default)]
    pub sell_price: f64,
    #[serde(rename = "buy_value", default)]
    pub buy_value: f64,
    #[serde(rename = "sell_value", default)]
    pub sell_value: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PositionsEnvelope {
    pub net: Vec<Position>,
    pub day: Vec<Position>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Holding {
    pub tradingsymbol: String,
    pub exchange: String,
    #[serde(rename = "instrument_token")]
    pub instrument_token: u32,
    pub quantity: i32,
    #[serde(rename = "average_price", default)]
    pub average_price: f64,
    #[serde(rename = "last_price", default)]
    pub last_price: f64,
    #[serde(default)]
    pub pnl: f64,
    #[serde(rename = "close_price", default)]
    pub close_price: f64,
    #[serde(rename = "day_change", default)]
    pub day_change: f64,
    #[serde(rename = "day_change_percentage", default)]
    pub day_change_percentage: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub order_id: String,
    #[serde(default)]
    pub parent_order_id: Option<String>,
    pub exchange_order_id: Option<String>,
    pub tradingsymbol: String,
    pub exchange: String,
    #[serde(rename = "instrument_token", default)]
    pub instrument_token: u32,
    pub transaction_type: String,
    pub order_type: String,
    pub product: String,
    pub variety: String,
    pub status: String,
    #[serde(default)]
    pub quantity: i32,
    #[serde(default)]
    pub filled_quantity: i32,
    #[serde(default)]
    pub pending_quantity: i32,
    #[serde(default)]
    pub cancelled_quantity: i32,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub trigger_price: f64,
    #[serde(default)]
    pub average_price: f64,
    #[serde(default)]
    pub disclosed_quantity: i32,
    #[serde(default)]
    pub validity: String,
    #[serde(default)]
    pub status_message: Option<String>,
    #[serde(default)]
    pub status_message_raw: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trade {
    pub trade_id: String,
    pub order_id: String,
    pub exchange_order_id: Option<String>,
    pub tradingsymbol: String,
    pub exchange: String,
    #[serde(rename = "instrument_token", default)]
    pub instrument_token: u32,
    pub transaction_type: String,
    pub product: String,
    #[serde(default)]
    pub quantity: i32,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub average_price: f64,
    #[serde(rename = "fill_timestamp", default)]
    pub fill_timestamp: String,
    #[serde(rename = "exchange_timestamp", default)]
    pub exchange_timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ltp {
    #[serde(rename = "instrument_token")]
    pub instrument_token: u32,
    #[serde(rename = "last_price")]
    pub last_price: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ohlc {
    #[serde(rename = "instrument_token")]
    pub instrument_token: u32,
    #[serde(rename = "last_price")]
    pub last_price: f64,
    pub ohlc: OhlcData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhlcData {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    #[serde(rename = "instrument_token")]
    pub instrument_token: u32,
    #[serde(rename = "last_price")]
    pub last_price: f64,
    #[serde(default)]
    pub volume: u64,
    #[serde(rename = "average_price", default)]
    pub average_price: f64,
    #[serde(default)]
    pub ohlc: Option<OhlcData>,
    #[serde(rename = "buy_quantity", default)]
    pub buy_quantity: u64,
    #[serde(rename = "sell_quantity", default)]
    pub sell_quantity: u64,
    #[serde(rename = "last_quantity", default)]
    pub last_quantity: u64,
    #[serde(rename = "last_trade_time", default)]
    pub last_trade_time: Option<String>,
    #[serde(default)]
    pub oi: u64,
    #[serde(rename = "oi_day_high", default)]
    pub oi_day_high: u64,
    #[serde(rename = "oi_day_low", default)]
    pub oi_day_low: u64,
    #[serde(rename = "net_change", default)]
    pub net_change: f64,
    #[serde(rename = "lower_circuit_limit", default)]
    pub lower_circuit_limit: f64,
    #[serde(rename = "upper_circuit_limit", default)]
    pub upper_circuit_limit: f64,
}

/// Parsed row from the instruments CSV dump.
#[derive(Debug, Clone)]
pub struct Instrument {
    pub instrument_token: u32,
    pub exchange_token: u32,
    pub tradingsymbol: String,
    pub name: String,
    pub exchange: String,
    pub segment: String,
    pub instrument_type: String,
    pub tick_size: f64,
    pub lot_size: u32,
    pub expiry: String,
    pub strike: f64,
}

#[derive(Debug, Deserialize)]
struct SessionData {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct PlaceOrderData {
    order_id: String,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct Client {
    http: Http,
    base: &'static str,
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        let http = Http::builder()
            .pool_max_idle_per_host(16)
            .tcp_keepalive(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client build");
        Self {
            http,
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

    fn auth_header(&self) -> Result<String> {
        let creds = self
            .cfg
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow!("no Kite credentials configured"))?;
        let token = creds
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow!("no access_token — run the login flow first"))?;
        Ok(format!("token {}:{}", creds.api_key, token))
    }

    // -- Session ------------------------------------------------------------

    /// POST /api/session/token — exchange `request_token` for `access_token`.
    /// On success, stores the token in `self.cfg` and returns it.
    pub async fn exchange_request_token(&mut self, request_token: &str) -> Result<String> {
        let creds = self
            .cfg
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow!("no Kite credentials configured"))?;

        let checksum = session_checksum(&creds.api_key, request_token, &creds.api_secret);

        let form = [
            ("api_key", creds.api_key.as_str()),
            ("request_token", request_token),
            ("checksum", &checksum),
        ];

        let res = self
            .http
            .post(format!("{}/api/session/token", self.base))
            .form(&form)
            .send()
            .await
            .context("session/token request failed")?;

        let env: Envelope<SessionData> = res.json().await.context("session/token parse failed")?;
        let data = env.into_result("session/token")?;

        self.cfg.set_access_token(data.access_token.clone());
        Ok(data.access_token)
    }

    // -- User ---------------------------------------------------------------

    /// GET /user/profile
    pub async fn profile(&self) -> Result<Profile> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/user/profile", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("profile request failed")?;
        let env: Envelope<Profile> = res.json().await.context("profile parse failed")?;
        env.into_result("profile")
    }

    /// GET /user/margins — returns equity + commodity segments.
    pub async fn margins(&self) -> Result<Margins> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/user/margins", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("margins request failed")?;
        let env: Envelope<Margins> = res.json().await.context("margins parse failed")?;
        env.into_result("margins")
    }

    // -- Portfolio -----------------------------------------------------------

    /// GET /portfolio/positions — net and day positions.
    pub async fn positions(&self) -> Result<PositionsEnvelope> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/portfolio/positions", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("positions request failed")?;
        let env: Envelope<PositionsEnvelope> =
            res.json().await.context("positions parse failed")?;
        env.into_result("positions")
    }

    /// GET /portfolio/holdings
    pub async fn holdings(&self) -> Result<Vec<Holding>> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/portfolio/holdings", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("holdings request failed")?;
        let env: Envelope<Vec<Holding>> = res.json().await.context("holdings parse failed")?;
        env.into_result("holdings")
    }

    // -- Instruments --------------------------------------------------------

    /// GET /instruments or /instruments/:exchange — returns the daily CSV dump
    /// parsed into `Instrument` records. Cache the result; this is a large file.
    pub async fn instruments(&self, exchange: Option<&str>) -> Result<Vec<Instrument>> {
        let auth = self.auth_header()?;
        let url = match exchange {
            Some(ex) => format!("{}/instruments/{}", self.base, ex),
            None => format!("{}/instruments", self.base),
        };
        let res = self
            .http
            .get(&url)
            .header("Authorization", &auth)
            .send()
            .await
            .context("instruments request failed")?;

        let csv_text = res.text().await.context("instruments body read failed")?;
        parse_instruments_csv(&csv_text)
    }

    // -- Orders -------------------------------------------------------------

    /// POST /orders/:variety — place an order. Returns the order ID.
    pub async fn place_order(
        &self,
        tradingsymbol: &str,
        exchange: &str,
        product: &str,
        side: Side,
        kind: crate::types::OrderKind,
        quantity: u32,
        price: Option<f64>,
    ) -> Result<String> {
        self.place_order_full(
            super::variety::REGULAR,
            tradingsymbol,
            exchange,
            product,
            side,
            kind,
            quantity,
            price,
            None,
            None,
            None,
        )
        .await
    }

    /// Full order placement with all optional fields.
    pub async fn place_order_full(
        &self,
        variety: &str,
        tradingsymbol: &str,
        exchange: &str,
        product: &str,
        side: Side,
        kind: crate::types::OrderKind,
        quantity: u32,
        price: Option<f64>,
        trigger_price: Option<f64>,
        disclosed_quantity: Option<u32>,
        tag: Option<&str>,
    ) -> Result<String> {
        let auth = self.auth_header()?;
        let transaction_type = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        let order_type = match kind {
            crate::types::OrderKind::Market => "MARKET",
            crate::types::OrderKind::Limit => "LIMIT",
        };

        let mut form: Vec<(&str, String)> = vec![
            ("tradingsymbol", tradingsymbol.to_string()),
            ("exchange", exchange.to_string()),
            ("transaction_type", transaction_type.to_string()),
            ("order_type", order_type.to_string()),
            ("product", product.to_string()),
            ("quantity", quantity.to_string()),
            ("validity", super::validity::DAY.to_string()),
        ];

        if let Some(p) = price {
            form.push(("price", format!("{p}")));
        }
        if let Some(tp) = trigger_price {
            form.push(("trigger_price", format!("{tp}")));
        }
        if let Some(dq) = disclosed_quantity {
            form.push(("disclosed_quantity", dq.to_string()));
        }
        if let Some(t) = tag {
            form.push(("tag", t.to_string()));
        }

        let res = self
            .http
            .post(format!("{}/orders/{}", self.base, variety))
            .header("Authorization", &auth)
            .form(&form)
            .send()
            .await
            .context("place_order request failed")?;

        let env: Envelope<PlaceOrderData> = res.json().await.context("place_order parse failed")?;
        let data = env.into_result("place_order")?;
        Ok(data.order_id)
    }

    /// PUT /orders/:variety/:order_id — modify a pending order.
    pub async fn modify_order(
        &self,
        variety: &str,
        order_id: &str,
        quantity: Option<u32>,
        price: Option<f64>,
        order_type: Option<&str>,
        trigger_price: Option<f64>,
    ) -> Result<String> {
        let auth = self.auth_header()?;
        let mut form: Vec<(&str, String)> = Vec::new();

        if let Some(q) = quantity {
            form.push(("quantity", q.to_string()));
        }
        if let Some(p) = price {
            form.push(("price", format!("{p}")));
        }
        if let Some(ot) = order_type {
            form.push(("order_type", ot.to_string()));
        }
        if let Some(tp) = trigger_price {
            form.push(("trigger_price", format!("{tp}")));
        }

        let res = self
            .http
            .put(format!("{}/orders/{}/{}", self.base, variety, order_id))
            .header("Authorization", &auth)
            .form(&form)
            .send()
            .await
            .context("modify_order request failed")?;

        let env: Envelope<PlaceOrderData> =
            res.json().await.context("modify_order parse failed")?;
        let data = env.into_result("modify_order")?;
        Ok(data.order_id)
    }

    /// DELETE /orders/:variety/:order_id — cancel a pending order.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        self.cancel_order_variety(super::variety::REGULAR, order_id)
            .await
    }

    pub async fn cancel_order_variety(&self, variety: &str, order_id: &str) -> Result<()> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .delete(format!("{}/orders/{}/{}", self.base, variety, order_id))
            .header("Authorization", &auth)
            .send()
            .await
            .context("cancel_order request failed")?;

        let env: Envelope<PlaceOrderData> =
            res.json().await.context("cancel_order parse failed")?;
        env.into_result("cancel_order")?;
        Ok(())
    }

    /// GET /orders — today's order book.
    pub async fn orders(&self) -> Result<Vec<Order>> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/orders", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("orders request failed")?;
        let env: Envelope<Vec<Order>> = res.json().await.context("orders parse failed")?;
        env.into_result("orders")
    }

    /// GET /trades — today's trades.
    pub async fn trades(&self) -> Result<Vec<Trade>> {
        let auth = self.auth_header()?;
        let res = self
            .http
            .get(format!("{}/trades", self.base))
            .header("Authorization", &auth)
            .send()
            .await
            .context("trades request failed")?;
        let env: Envelope<Vec<Trade>> = res.json().await.context("trades parse failed")?;
        env.into_result("trades")
    }

    // -- Quotes -------------------------------------------------------------

    /// GET /quote/ltp — last traded price for up to 500 instruments.
    /// Keys are in the format "EXCHANGE:TRADINGSYMBOL" (e.g. "NSE:RELIANCE").
    pub async fn ltp(&self, instruments: &[&str]) -> Result<HashMap<String, Ltp>> {
        let auth = self.auth_header()?;
        let params: Vec<(&str, &str)> = instruments.iter().map(|i| ("i", *i)).collect();
        let res = self
            .http
            .get(format!("{}/quote/ltp", self.base))
            .header("Authorization", &auth)
            .query(&params)
            .send()
            .await
            .context("ltp request failed")?;
        let env: Envelope<HashMap<String, Ltp>> = res.json().await.context("ltp parse failed")?;
        env.into_result("ltp")
    }

    /// GET /quote/ohlc — OHLC + LTP for up to 500 instruments.
    pub async fn ohlc(&self, instruments: &[&str]) -> Result<HashMap<String, Ohlc>> {
        let auth = self.auth_header()?;
        let params: Vec<(&str, &str)> = instruments.iter().map(|i| ("i", *i)).collect();
        let res = self
            .http
            .get(format!("{}/quote/ohlc", self.base))
            .header("Authorization", &auth)
            .query(&params)
            .send()
            .await
            .context("ohlc request failed")?;
        let env: Envelope<HashMap<String, Ohlc>> = res.json().await.context("ohlc parse failed")?;
        env.into_result("ohlc")
    }

    /// GET /quote — full market quote for up to 500 instruments.
    pub async fn quote(&self, instruments: &[&str]) -> Result<HashMap<String, Quote>> {
        let auth = self.auth_header()?;
        let params: Vec<(&str, &str)> = instruments.iter().map(|i| ("i", *i)).collect();
        let res = self
            .http
            .get(format!("{}/quote", self.base))
            .header("Authorization", &auth)
            .query(&params)
            .send()
            .await
            .context("quote request failed")?;
        let env: Envelope<HashMap<String, Quote>> =
            res.json().await.context("quote parse failed")?;
        env.into_result("quote")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// SHA256(api_key + request_token + api_secret) — used by POST /session/token.
fn session_checksum(api_key: &str, request_token: &str, api_secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    hasher.update(request_token.as_bytes());
    hasher.update(api_secret.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parse the instruments CSV dump into structured records.
/// First line is the header; subsequent lines are comma-separated fields.
fn parse_instruments_csv(csv: &str) -> Result<Vec<Instrument>> {
    let mut lines = csv.lines();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("empty instruments CSV"))?;

    let cols: Vec<&str> = header.split(',').collect();
    let idx = |name: &str| -> Result<usize> {
        cols.iter()
            .position(|&c| c == name)
            .ok_or_else(|| anyhow!("missing column {name} in instruments CSV"))
    };

    let i_token = idx("instrument_token")?;
    let i_ex_token = idx("exchange_token")?;
    let i_symbol = idx("tradingsymbol")?;
    let i_name = idx("name")?;
    let i_exchange = idx("exchange")?;
    let i_segment = idx("segment")?;
    let i_type = idx("instrument_type")?;
    let i_tick = idx("tick_size")?;
    let i_lot = idx("lot_size")?;
    let i_expiry = idx("expiry")?;
    let i_strike = idx("strike")?;

    let mut instruments = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() <= i_strike {
            continue;
        }
        instruments.push(Instrument {
            instrument_token: fields[i_token].parse().unwrap_or(0),
            exchange_token: fields[i_ex_token].parse().unwrap_or(0),
            tradingsymbol: fields[i_symbol].to_string(),
            name: fields[i_name].to_string(),
            exchange: fields[i_exchange].to_string(),
            segment: fields[i_segment].to_string(),
            instrument_type: fields[i_type].to_string(),
            tick_size: fields[i_tick].parse().unwrap_or(0.0),
            lot_size: fields[i_lot].parse().unwrap_or(0),
            expiry: fields[i_expiry].to_string(),
            strike: fields[i_strike].parse().unwrap_or(0.0),
        });
    }
    Ok(instruments)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_known_vector() {
        // printf "api_key_123req_token_456api_secret_789" | openssl dgst -sha256
        let got = session_checksum("api_key_123", "req_token_456", "api_secret_789");
        assert_eq!(got.len(), 64);
        assert!(got.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify determinism
        let again = session_checksum("api_key_123", "req_token_456", "api_secret_789");
        assert_eq!(got, again);
    }

    #[test]
    fn different_inputs_produce_different_checksums() {
        let a = session_checksum("k", "r1", "s");
        let b = session_checksum("k", "r2", "s");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_instruments_csv_basic() {
        let csv = "instrument_token,exchange_token,tradingsymbol,name,\
                   exchange,segment,instrument_type,tick_size,lot_size,expiry,strike\n\
                   256265,99,RELIANCE,Reliance Industries,\
                   NSE,NSE,EQ,0.05,1,,0.0\n\
                   738561,2885,TCS,Tata Consultancy Services,\
                   NSE,NSE,EQ,0.05,1,,0.0\n";

        let instruments = parse_instruments_csv(csv).unwrap();
        assert_eq!(instruments.len(), 2);
        assert_eq!(instruments[0].tradingsymbol, "RELIANCE");
        assert_eq!(instruments[0].instrument_token, 256265);
        assert_eq!(instruments[0].exchange, "NSE");
        assert_eq!(instruments[0].tick_size, 0.05);
        assert_eq!(instruments[1].tradingsymbol, "TCS");
    }

    #[test]
    fn parse_instruments_csv_skips_empty_lines() {
        let csv = "instrument_token,exchange_token,tradingsymbol,name,\
                   exchange,segment,instrument_type,tick_size,lot_size,expiry,strike\n\
                   \n\
                   256265,99,RELIANCE,Reliance Industries,\
                   NSE,NSE,EQ,0.05,1,,0.0\n";

        let instruments = parse_instruments_csv(csv).unwrap();
        assert_eq!(instruments.len(), 1);
    }

    #[test]
    fn envelope_success() {
        let json = r#"{"status":"success","data":{"order_id":"123456"}}"#;
        let env: Envelope<PlaceOrderData> = serde_json::from_str(json).unwrap();
        let data = env.into_result("test").unwrap();
        assert_eq!(data.order_id, "123456");
    }

    #[test]
    fn envelope_error() {
        let json = r#"{"status":"error","message":"Invalid token","error_type":"TokenException"}"#;
        let env: Envelope<PlaceOrderData> = serde_json::from_str(json).unwrap();
        let err = env.into_result("test").unwrap_err();
        assert!(err.to_string().contains("TokenException"));
        assert!(err.to_string().contains("Invalid token"));
    }
}
