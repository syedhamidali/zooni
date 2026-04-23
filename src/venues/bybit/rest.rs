#![allow(dead_code)]
//! Bybit V5 signed REST client — spot category.
//!
//! Sign recipe (HMAC-SHA256 of `timestamp + api_key + recv_window + payload`):
//!   GET  → payload = query_string (we build it deterministically below)
//!   POST → payload = raw request body JSON
//!
//! Response envelope:
//!   { "retCode": 0, "retMsg": "OK", "result": {...}, "time": ... }
//! Anything with retCode != 0 is an error.

use anyhow::{Context, Result, anyhow};
use reqwest::Client as Http;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use super::signer::sign;
use super::{Config, REST_BASE, REST_BASE_TESTNET};
use crate::types::Side;

pub struct Client {
    http: Http,
    base: &'static str,
    cfg: Config,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(rename = "retCode")]
    ret_code: i64,
    #[serde(rename = "retMsg")]
    ret_msg: String,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct InstrumentInfo {
    pub symbol: String,
    #[serde(rename = "baseCoin")]
    pub base_coin: String,
    #[serde(rename = "quoteCoin")]
    pub quote_coin: String,
    #[serde(rename = "priceFilter")]
    pub price_filter: PriceFilter,
    #[serde(rename = "lotSizeFilter")]
    pub lot_size_filter: LotSizeFilter,
}

#[derive(Debug, Deserialize)]
pub struct PriceFilter {
    #[serde(rename = "tickSize")]
    pub tick_size: String,
    #[serde(rename = "minPrice", default)]
    pub min_price: String,
    #[serde(rename = "maxPrice", default)]
    pub max_price: String,
}

#[derive(Debug, Deserialize)]
pub struct LotSizeFilter {
    #[serde(rename = "basePrecision")]
    pub base_precision: String,
    #[serde(rename = "quotePrecision")]
    pub quote_precision: String,
    #[serde(rename = "minOrderQty")]
    pub min_order_qty: String,
    #[serde(rename = "maxOrderQty")]
    pub max_order_qty: String,
    #[serde(rename = "minOrderAmt", default)]
    pub min_order_amt: String,
}

#[derive(Debug, Deserialize)]
struct InstrumentsInfoResult {
    list: Vec<InstrumentInfo>,
}

#[derive(Debug, Deserialize)]
pub struct Ticker {
    pub symbol: String,
    #[serde(rename = "lastPrice")]
    pub last_price: String,
    #[serde(rename = "bid1Price", default)]
    pub bid1_price: String,
    #[serde(rename = "ask1Price", default)]
    pub ask1_price: String,
}

#[derive(Debug, Deserialize)]
struct TickersResult {
    list: Vec<Ticker>,
}

/// Full ticker data returned when fetching all tickers (includes volume, turnover, change%).
#[derive(Debug, Deserialize, Clone)]
pub struct TickerFull {
    pub symbol: String,
    #[serde(rename = "lastPrice")]
    pub last_price: String,
    #[serde(rename = "bid1Price", default)]
    pub bid1_price: String,
    #[serde(rename = "ask1Price", default)]
    pub ask1_price: String,
    #[serde(rename = "volume24h", default)]
    pub volume_24h: String,
    #[serde(rename = "turnover24h", default)]
    pub turnover_24h: String,
    #[serde(rename = "highPrice24h", default)]
    pub high_price_24h: String,
    #[serde(rename = "lowPrice24h", default)]
    pub low_price_24h: String,
    #[serde(rename = "price24hPcnt", default)]
    pub price_24h_pcnt: String,
}

#[derive(Debug, Deserialize)]
struct AllTickersResult {
    list: Vec<TickerFull>,
}

/// Parsed kline (candlestick) data.
#[derive(Debug, Clone)]
pub struct Kline {
    pub timestamp: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Deserialize)]
struct KlineResult {
    list: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CoinBalance {
    pub coin: String,
    #[serde(rename = "walletBalance")]
    pub wallet_balance: String,
    #[serde(rename = "availableToWithdraw", default)]
    pub available_to_withdraw: String,
}

#[derive(Debug, Deserialize)]
struct WalletBalanceResult {
    list: Vec<WalletBalanceAccount>,
}

#[derive(Debug, Deserialize)]
pub struct WalletBalanceAccount {
    #[serde(rename = "accountType")]
    pub account_type: String,
    pub coin: Vec<CoinBalance>,
}

#[derive(Debug, Serialize)]
struct PlaceOrderReq<'a> {
    category: &'a str,
    symbol: &'a str,
    side: &'a str,
    #[serde(rename = "orderType")]
    order_type: &'a str,
    qty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<String>,
    #[serde(rename = "timeInForce", skip_serializing_if = "Option::is_none")]
    time_in_force: Option<&'a str>,
    #[serde(rename = "orderLinkId", skip_serializing_if = "Option::is_none")]
    order_link_id: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct PlaceOrderResult {
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(rename = "orderLinkId", default)]
    pub order_link_id: String,
}

#[derive(Debug, Serialize)]
struct CancelOrderReq<'a> {
    category: &'a str,
    symbol: &'a str,
    #[serde(rename = "orderId", skip_serializing_if = "Option::is_none")]
    order_id: Option<&'a str>,
    #[serde(rename = "orderLinkId", skip_serializing_if = "Option::is_none")]
    order_link_id: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct CancelOrderResult {
    #[serde(rename = "orderId")]
    pub order_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OpenOrder {
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(rename = "orderLinkId", default)]
    pub order_link_id: String,
    pub symbol: String,
    pub side: String,
    #[serde(rename = "orderType")]
    pub order_type: String,
    pub price: String,
    pub qty: String,
    #[serde(rename = "cumExecQty", default)]
    pub cum_exec_qty: String,
    #[serde(rename = "orderStatus")]
    pub order_status: String,
}

#[derive(Debug, Deserialize)]
struct OpenOrdersResult {
    list: Vec<OpenOrder>,
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
    pub fn is_testnet(&self) -> bool {
        self.cfg.testnet
    }

    fn creds(&self) -> Result<(&str, &str, u64)> {
        let c = self
            .cfg
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow!("no Bybit credentials configured"))?;
        Ok((&c.api_key, &c.api_secret, self.cfg.recv_window_ms))
    }

    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    // -- Public (unauthenticated) --------------------------------------------

    pub async fn instrument_info_spot(&self, symbol: &str) -> Result<InstrumentInfo> {
        let url = format!("{}/v5/market/instruments-info", self.base);
        let res = self
            .http
            .get(&url)
            .query(&[("category", "spot"), ("symbol", symbol)])
            .send()
            .await
            .context("instruments-info request failed")?;
        let env: Envelope<InstrumentsInfoResult> =
            res.json().await.context("instruments-info parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!(
                "instruments-info error {}: {}",
                env.ret_code,
                env.ret_msg
            ));
        }
        env.result
            .and_then(|r| r.list.into_iter().next())
            .ok_or_else(|| anyhow!("instruments-info: empty result for {}", symbol))
    }

    /// Public spot ticker. Returns bid1/ask1/lastPrice strings.
    pub async fn ticker_spot(&self, symbol: &str) -> Result<Ticker> {
        let url = format!("{}/v5/market/tickers", self.base);
        let res = self
            .http
            .get(&url)
            .query(&[("category", "spot"), ("symbol", symbol)])
            .send()
            .await
            .context("tickers request failed")?;
        let env: Envelope<TickersResult> = res.json().await.context("tickers parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!("tickers error {}: {}", env.ret_code, env.ret_msg));
        }
        env.result
            .and_then(|r| r.list.into_iter().next())
            .ok_or_else(|| anyhow!("tickers: empty result for {}", symbol))
    }

    // -- Authenticated GET ---------------------------------------------------

    async fn signed_get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query_pairs: &[(&str, &str)],
    ) -> Result<T> {
        let (api_key, api_secret, recv_window) = self.creds()?;
        let timestamp = Self::timestamp_ms();

        // Build query string deterministically in the order we send it — Bybit signs what's on the wire.
        let query = query_pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = sign(api_secret, timestamp, api_key, recv_window, &query);
        let url = if query.is_empty() {
            format!("{}{}", self.base, path)
        } else {
            format!("{}{}?{}", self.base, path, query)
        };

        let res = self
            .http
            .get(&url)
            .header("X-BAPI-API-KEY", api_key)
            .header("X-BAPI-TIMESTAMP", timestamp.to_string())
            .header("X-BAPI-RECV-WINDOW", recv_window.to_string())
            .header("X-BAPI-SIGN", signature)
            .send()
            .await
            .context("signed GET send failed")?;

        let env: Envelope<T> = res.json().await.context("signed GET parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!("bybit error {}: {}", env.ret_code, env.ret_msg));
        }
        env.result
            .ok_or_else(|| anyhow!("bybit: empty result for {}", path))
    }

    // -- Authenticated POST --------------------------------------------------

    async fn signed_post<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let (api_key, api_secret, recv_window) = self.creds()?;
        let timestamp = Self::timestamp_ms();
        let body_str = serde_json::to_string(body).context("serialize POST body failed")?;
        let signature = sign(api_secret, timestamp, api_key, recv_window, &body_str);
        let url = format!("{}{}", self.base, path);

        let res = self
            .http
            .post(&url)
            .header("X-BAPI-API-KEY", api_key)
            .header("X-BAPI-TIMESTAMP", timestamp.to_string())
            .header("X-BAPI-RECV-WINDOW", recv_window.to_string())
            .header("X-BAPI-SIGN", signature)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await
            .context("signed POST send failed")?;

        let env: Envelope<T> = res.json().await.context("signed POST parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!("bybit error {}: {}", env.ret_code, env.ret_msg));
        }
        env.result
            .ok_or_else(|| anyhow!("bybit: empty result for {}", path))
    }

    // -- High-level methods --------------------------------------------------

    /// Wallet balance for an account type. Spot accounts on testnet are usually UNIFIED.
    pub async fn wallet_balance(&self, account_type: &str) -> Result<Vec<WalletBalanceAccount>> {
        let r: WalletBalanceResult = self
            .signed_get(
                "/v5/account/wallet-balance",
                &[("accountType", account_type)],
            )
            .await?;
        Ok(r.list)
    }

    /// Place a spot order. `price` is required for Limit orders, None for Market.
    pub async fn place_spot_order(
        &self,
        symbol: &str,
        side: Side,
        order_type: &str, // "Limit" | "Market"
        qty: &str,
        price: Option<&str>,
        order_link_id: Option<&str>,
    ) -> Result<PlaceOrderResult> {
        let side_str = match side {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        };
        let req = PlaceOrderReq {
            category: "spot",
            symbol,
            side: side_str,
            order_type,
            qty: qty.to_string(),
            price: price.map(|p| p.to_string()),
            time_in_force: if order_type == "Limit" {
                Some("GTC")
            } else {
                None
            },
            order_link_id,
        };
        self.signed_post("/v5/order/create", &req).await
    }

    pub async fn cancel_spot_order(
        &self,
        symbol: &str,
        order_id: &str,
    ) -> Result<CancelOrderResult> {
        let req = CancelOrderReq {
            category: "spot",
            symbol,
            order_id: Some(order_id),
            order_link_id: None,
        };
        self.signed_post("/v5/order/cancel", &req).await
    }

    pub async fn cancel_all_spot(&self, symbol: &str) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            category: &'a str,
            symbol: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {}
        let _: Resp = self
            .signed_post(
                "/v5/order/cancel-all",
                &Req {
                    category: "spot",
                    symbol,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn open_orders_spot(&self, symbol: &str) -> Result<Vec<OpenOrder>> {
        let r: OpenOrdersResult = self
            .signed_get(
                "/v5/order/realtime",
                &[("category", "spot"), ("symbol", symbol)],
            )
            .await?;
        Ok(r.list)
    }

    /// Fetch all spot tickers (no symbol filter). Used by the market scanner.
    pub async fn all_tickers_spot(&self) -> Result<Vec<TickerFull>> {
        let url = format!("{}/v5/market/tickers", self.base);
        let res = self
            .http
            .get(&url)
            .query(&[("category", "spot")])
            .send()
            .await
            .context("all tickers request failed")?;
        let env: Envelope<AllTickersResult> =
            res.json().await.context("all tickers parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!(
                "all tickers error {}: {}",
                env.ret_code,
                env.ret_msg
            ));
        }
        Ok(env.result.map(|r| r.list).unwrap_or_default())
    }

    /// Fetch kline (candlestick) data for a symbol.
    /// `interval`: "1" (1m), "5", "15", "30", "60", "240", "D", "W"
    /// `limit`: max 1000 per call.
    pub async fn klines(&self, symbol: &str, interval: &str, limit: u32) -> Result<Vec<Kline>> {
        let url = format!("{}/v5/market/kline", self.base);
        let limit_str = limit.to_string();
        let res = self
            .http
            .get(&url)
            .query(&[
                ("category", "spot"),
                ("symbol", symbol),
                ("interval", interval),
                ("limit", &limit_str),
            ])
            .send()
            .await
            .context("klines request failed")?;
        let env: Envelope<KlineResult> = res.json().await.context("klines parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!("klines error {}: {}", env.ret_code, env.ret_msg));
        }
        let raw = env.result.map(|r| r.list).unwrap_or_default();
        // Bybit returns newest first; reverse to chronological order
        let mut klines: Vec<Kline> = raw
            .into_iter()
            .map(|row| Kline {
                timestamp: row[0].parse().unwrap_or(0),
                open: row[1].parse().unwrap_or(0.0),
                high: row[2].parse().unwrap_or(0.0),
                low: row[3].parse().unwrap_or(0.0),
                close: row[4].parse().unwrap_or(0.0),
                volume: row[5].parse().unwrap_or(0.0),
            })
            .collect();
        klines.reverse();
        Ok(klines)
    }

    /// Fetch Bybit server time for clock-sync checks.
    pub async fn server_time(&self) -> Result<u64> {
        #[derive(Deserialize)]
        struct TimeResult {
            #[serde(rename = "timeSecond", default)]
            time_second: String,
            #[serde(rename = "timeNano", default)]
            time_nano: String,
        }
        let url = format!("{}/v5/market/time", self.base);
        let res = self
            .http
            .get(&url)
            .send()
            .await
            .context("server time request failed")?;
        let env: Envelope<TimeResult> = res.json().await.context("server time parse failed")?;
        if env.ret_code != 0 {
            return Err(anyhow!(
                "server time error {}: {}",
                env.ret_code,
                env.ret_msg
            ));
        }
        let result = env
            .result
            .ok_or_else(|| anyhow!("server time: empty result"))?;
        // timeSecond is epoch seconds as a string
        let secs: u64 = result.time_second.parse().unwrap_or(0);
        Ok(secs * 1000) // return as millis
    }
}
