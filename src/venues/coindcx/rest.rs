//! CoinDCX REST client.
//!
//! Auth: HMAC-SHA256(compact_json_body, api_secret) → hex.
//! Headers: X-AUTH-APIKEY, X-AUTH-SIGNATURE, Content-Type: application/json.
//! All authenticated endpoints are POST.

use anyhow::{Context, Result, anyhow};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use reqwest::Client as Http;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::Duration;

use super::{Config, PUBLIC_BASE, REST_BASE};
use crate::types::Side;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Response / request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Ticker {
    pub market: String,
    pub last_price: String,
    pub bid: String,
    pub ask: String,
    pub high: String,
    pub low: String,
    pub volume: String,
    #[serde(default)]
    pub change_24_hour: String,
    pub timestamp: u64,
}

#[derive(Debug, Deserialize)]
pub struct MarketDetail {
    pub coindcx_name: String,
    pub pair: String,
    #[serde(default)]
    pub base_currency_short_name: String,
    #[serde(default)]
    pub target_currency_short_name: String,
    #[serde(default)]
    pub min_quantity: f64,
    #[serde(default)]
    pub max_quantity: f64,
    #[serde(default)]
    pub min_price: f64,
    #[serde(default)]
    pub max_price: f64,
    #[serde(default)]
    pub min_notional: f64,
    #[serde(default)]
    pub step: f64,
    #[serde(default)]
    pub order_types: Vec<String>,
    #[serde(default)]
    pub ecode: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub time: u64,
}

#[derive(Debug, Deserialize)]
pub struct Balance {
    pub currency: String,
    pub balance: String,
    pub locked_balance: String,
}

#[derive(Debug, Deserialize)]
pub struct OrderResponse {
    pub orders: Vec<OrderInfo>,
}

#[derive(Debug, Deserialize)]
pub struct OrderInfo {
    pub id: String,
    #[serde(default)]
    pub client_order_id: Option<String>,
    pub market: String,
    pub order_type: String,
    pub side: String,
    pub status: String,
    #[serde(default)]
    pub fee_amount: f64,
    #[serde(default)]
    pub total_quantity: f64,
    #[serde(default)]
    pub remaining_quantity: f64,
    #[serde(default)]
    pub avg_price: f64,
    #[serde(default)]
    pub price_per_unit: f64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ActiveOrder {
    pub id: String,
    #[serde(default)]
    pub client_order_id: Option<String>,
    pub market: String,
    pub order_type: String,
    pub side: String,
    pub status: String,
    #[serde(default)]
    pub total_quantity: f64,
    #[serde(default)]
    pub remaining_quantity: f64,
    #[serde(default)]
    pub price_per_unit: f64,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct Client {
    http: Http,
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        let http = Http::builder()
            .pool_max_idle_per_host(16)
            .tcp_keepalive(Some(Duration::from_secs(60)))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client build");
        Self { http, cfg }
    }

    pub fn base_url(&self) -> &str {
        REST_BASE
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    // -- Signing ------------------------------------------------------------

    fn sign(&self, body: &str) -> Result<(String, String)> {
        let creds = self
            .cfg
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow!("CoinDCX credentials not configured"))?;
        let mut mac =
            HmacSha256::new_from_slice(creds.api_secret.as_bytes()).expect("HMAC key size");
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        Ok((creds.api_key.clone(), sig))
    }

    async fn authed_post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &impl Serialize,
    ) -> Result<T> {
        let json_body = serde_json::to_string(body).context("serializing request body")?;
        let (api_key, signature) = self.sign(&json_body)?;
        let resp = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-AUTH-APIKEY", &api_key)
            .header("X-AUTH-SIGNATURE", &signature)
            .body(json_body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = resp.status();
        let text = resp.text().await.context("reading response body")?;
        if !status.is_success() {
            return Err(anyhow!("CoinDCX {url} HTTP {status}: {text}"));
        }
        serde_json::from_str(&text).with_context(|| format!("parsing response from {url}"))
    }

    // -- Public endpoints ---------------------------------------------------

    pub async fn tickers(&self) -> Result<Vec<Ticker>> {
        let url = format!("{PUBLIC_BASE}/exchange/ticker");
        let resp = self.http.get(&url).send().await.context("GET ticker")?;
        let text = resp.text().await?;
        serde_json::from_str(&text).context("parsing tickers")
    }

    pub async fn ticker_for(&self, symbol: &str) -> Result<Ticker> {
        let tickers = self.tickers().await?;
        tickers
            .into_iter()
            .find(|t| t.market == symbol)
            .ok_or_else(|| anyhow!("ticker not found for {symbol}"))
    }

    pub async fn markets_details(&self) -> Result<Vec<MarketDetail>> {
        let url = format!("{REST_BASE}/exchange/v1/markets_details");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("GET markets_details")?;
        let text = resp.text().await?;
        serde_json::from_str(&text).context("parsing markets_details")
    }

    pub async fn market_detail_for(&self, symbol: &str) -> Result<MarketDetail> {
        let details = self.markets_details().await?;
        details
            .into_iter()
            .find(|d| d.coindcx_name == symbol)
            .ok_or_else(|| anyhow!("market detail not found for {symbol}"))
    }

    pub async fn candles(&self, pair: &str, interval: &str, limit: u32) -> Result<Vec<Candle>> {
        let url = format!(
            "{PUBLIC_BASE}/market_data/candles?pair={pair}&interval={interval}&limit={limit}"
        );
        let resp = self.http.get(&url).send().await.context("GET candles")?;
        let text = resp.text().await?;
        serde_json::from_str(&text).context("parsing candles")
    }

    // -- Authenticated endpoints --------------------------------------------

    pub async fn balances(&self) -> Result<Vec<Balance>> {
        let url = format!("{REST_BASE}/exchange/v1/users/balances");
        let ts = timestamp_ms();
        let body = serde_json::json!({ "timestamp": ts });
        self.authed_post(&url, &body).await
    }

    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        order_type: &str,
        qty: f64,
        price: Option<f64>,
        client_order_id: Option<&str>,
    ) -> Result<OrderResponse> {
        let url = format!("{REST_BASE}/exchange/v1/orders/create");
        let ts = timestamp_ms();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let otype = match order_type {
            "Market" | "market" => "market_order",
            _ => "limit_order",
        };
        let mut body = serde_json::json!({
            "side": side_str,
            "order_type": otype,
            "market": symbol,
            "total_quantity": qty,
            "timestamp": ts,
        });
        if let Some(p) = price {
            body["price_per_unit"] = serde_json::json!(p);
        }
        if let Some(coid) = client_order_id {
            body["client_order_id"] = serde_json::json!(coid);
        }
        self.authed_post(&url, &body).await
    }

    pub async fn cancel_order(&self, order_id: &str) -> Result<serde_json::Value> {
        let url = format!("{REST_BASE}/exchange/v1/orders/cancel");
        let ts = timestamp_ms();
        let body = serde_json::json!({
            "id": order_id,
            "timestamp": ts,
        });
        self.authed_post(&url, &body).await
    }

    pub async fn cancel_all(&self, symbol: &str) -> Result<serde_json::Value> {
        let url = format!("{REST_BASE}/exchange/v1/orders/cancel_all");
        let ts = timestamp_ms();
        let body = serde_json::json!({
            "market": symbol,
            "timestamp": ts,
        });
        self.authed_post(&url, &body).await
    }

    pub async fn active_orders(&self, symbol: &str) -> Result<Vec<ActiveOrder>> {
        let url = format!("{REST_BASE}/exchange/v1/orders/active_orders");
        let ts = timestamp_ms();
        let body = serde_json::json!({
            "market": symbol,
            "timestamp": ts,
        });
        self.authed_post(&url, &body).await
    }

    pub async fn order_status(&self, order_id: &str) -> Result<OrderInfo> {
        let url = format!("{REST_BASE}/exchange/v1/orders/status");
        let ts = timestamp_ms();
        let body = serde_json::json!({
            "id": order_id,
            "timestamp": ts,
        });
        self.authed_post(&url, &body).await
    }

    // -- Helpers for venue mapping ------------------------------------------

    /// Resolve the CoinDCX `pair` name (e.g. "B-BTC_USDT") from a coindcx_name.
    /// Falls back to guessing "B-{TARGET}_{BASE}" if no API call is desired.
    pub fn guess_pair(symbol: &str) -> String {
        if let Some(base) = symbol.strip_suffix("USDT") {
            format!("B-{base}_USDT")
        } else if let Some(base) = symbol.strip_suffix("INR") {
            format!("I-{base}_INR")
        } else {
            format!("B-{symbol}")
        }
    }
}

fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
