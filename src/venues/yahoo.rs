//! Yahoo Finance historical data — free OHLCV for any market.
//!
//! Used as the kline source when the primary venue (e.g., Zerodha Personal)
//! doesn't include historical data APIs.
//!
//! Symbol format: `RELIANCE.NS` (NSE), `RELIANCE.BO` (BSE), `BTCUSD` (crypto).
//!
//! Yahoo requires a cookie + crumb auth flow:
//!   1. GET `https://fc.yahoo.com` → captures `set-cookie`
//!   2. GET `.../v1/test/getcrumb` → plain-text crumb
//!   3. Append `&crumb=...` to chart requests

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::venue::VenueKline;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub struct YahooClient {
    http: reqwest::Client,
    crumb: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChartResponse {
    chart: ChartResult,
}

#[derive(Debug, Deserialize)]
struct ChartResult {
    result: Option<Vec<ChartData>>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChartData {
    timestamp: Vec<i64>,
    indicators: Indicators,
}

#[derive(Debug, Deserialize)]
struct Indicators {
    quote: Vec<QuoteData>,
}

#[derive(Debug, Deserialize)]
struct QuoteData {
    open: Vec<Option<f64>>,
    high: Vec<Option<f64>>,
    low: Vec<Option<f64>>,
    close: Vec<Option<f64>>,
    volume: Vec<Option<u64>>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

impl YahooClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .cookie_store(true)
            .user_agent(USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("yahoo http client build");
        Self { http, crumb: None }
    }

    async fn ensure_crumb(&mut self) -> Result<&str> {
        if self.crumb.is_some() {
            return Ok(self.crumb.as_ref().unwrap());
        }

        self.http
            .get("https://fc.yahoo.com")
            .send()
            .await
            .context("yahoo cookie fetch failed")?;

        let crumb = self
            .http
            .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
            .send()
            .await
            .context("yahoo crumb fetch failed")?
            .text()
            .await
            .context("yahoo crumb read failed")?;

        if crumb.is_empty() || crumb.len() > 100 {
            return Err(anyhow!(
                "invalid crumb from yahoo: {:?}",
                &crumb[..crumb.len().min(50)]
            ));
        }

        self.crumb = Some(crumb);
        Ok(self.crumb.as_ref().unwrap())
    }

    /// Fetch historical klines using a range string (e.g., "6mo", "1y", "max").
    pub async fn klines_range(
        &mut self,
        symbol: &str,
        interval: &str,
        range: &str,
    ) -> Result<Vec<VenueKline>> {
        let crumb = self.ensure_crumb().await?.to_string();
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}\
             ?range={range}&interval={interval}&crumb={crumb}"
        );
        self.fetch_chart(&url).await
    }

    /// Fetch historical klines between two Unix timestamps (seconds).
    pub async fn klines_period(
        &mut self,
        symbol: &str,
        interval: &str,
        period1: i64,
        period2: i64,
    ) -> Result<Vec<VenueKline>> {
        let crumb = self.ensure_crumb().await?.to_string();
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}\
             ?period1={period1}&period2={period2}&interval={interval}&crumb={crumb}"
        );
        self.fetch_chart(&url).await
    }

    /// Fetch the last N days of daily klines.
    pub async fn klines_days(
        &mut self,
        symbol: &str,
        interval: &str,
        days: u32,
    ) -> Result<Vec<VenueKline>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let start = now - (days as i64 * 86400);
        self.klines_period(symbol, interval, start, now).await
    }

    async fn fetch_chart(&self, url: &str) -> Result<Vec<VenueKline>> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .context("yahoo chart request failed")?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("yahoo auth failed (401) — crumb may be expired"));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!("yahoo rate limited (429) — back off and retry"));
        }
        if !status.is_success() {
            return Err(anyhow!("yahoo chart failed with status {}", status));
        }

        let chart: ChartResponse = resp.json().await.context("yahoo chart parse failed")?;

        if let Some(err) = chart.chart.error {
            return Err(anyhow!("yahoo chart error: {}", err));
        }

        let data = chart
            .chart
            .result
            .and_then(|mut r| {
                if r.is_empty() {
                    None
                } else {
                    Some(r.remove(0))
                }
            })
            .ok_or_else(|| anyhow!("yahoo chart: empty result"))?;

        let quote = data
            .indicators
            .quote
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("yahoo chart: no quote data"))?;

        let len = data.timestamp.len();
        let mut klines = Vec::with_capacity(len);

        for i in 0..len {
            let open = match quote.open.get(i).copied().flatten() {
                Some(v) => v,
                None => continue,
            };
            let high = quote.high.get(i).copied().flatten().unwrap_or(open);
            let low = quote.low.get(i).copied().flatten().unwrap_or(open);
            let close = quote.close.get(i).copied().flatten().unwrap_or(open);
            let volume = quote.volume.get(i).copied().flatten().unwrap_or(0) as f64;

            klines.push(VenueKline {
                timestamp: data.timestamp[i] as u64,
                open,
                high,
                low,
                close,
                volume,
            });
        }

        Ok(klines)
    }

    /// Invalidate the cached crumb (call on 401 before retrying).
    pub fn clear_crumb(&mut self) {
        self.crumb = None;
    }
}

/// Convert an NSE trading symbol (e.g., "RELIANCE") to Yahoo format ("RELIANCE.NS").
pub fn nse_symbol(trading_symbol: &str) -> String {
    if trading_symbol.contains('.') {
        trading_symbol.to_string()
    } else {
        format!("{trading_symbol}.NS")
    }
}

/// Convert a BSE trading symbol to Yahoo format ("RELIANCE.BO").
pub fn bse_symbol(trading_symbol: &str) -> String {
    if trading_symbol.contains('.') {
        trading_symbol.to_string()
    } else {
        format!("{trading_symbol}.BO")
    }
}

/// Map a Kite interval string (e.g., "15" for 15 min) to Yahoo interval.
pub fn map_interval(kite_interval: &str) -> &str {
    match kite_interval {
        "1" => "1m",
        "5" => "5m",
        "15" => "15m",
        "30" => "30m",
        "60" => "1h",
        "D" | "1d" | "day" => "1d",
        "W" | "1wk" | "week" => "1wk",
        "M" | "1mo" | "month" => "1mo",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nse_symbol_conversion() {
        assert_eq!(nse_symbol("RELIANCE"), "RELIANCE.NS");
        assert_eq!(nse_symbol("HDFCBANK"), "HDFCBANK.NS");
        assert_eq!(nse_symbol("TCS.NS"), "TCS.NS");
    }

    #[test]
    fn bse_symbol_conversion() {
        assert_eq!(bse_symbol("RELIANCE"), "RELIANCE.BO");
        assert_eq!(bse_symbol("TCS.BO"), "TCS.BO");
    }

    #[test]
    fn interval_mapping() {
        assert_eq!(map_interval("15"), "15m");
        assert_eq!(map_interval("60"), "1h");
        assert_eq!(map_interval("D"), "1d");
        assert_eq!(map_interval("1d"), "1d");
        assert_eq!(map_interval("W"), "1wk");
    }

    #[test]
    fn parse_chart_response() {
        let json = r#"{
            "chart": {
                "result": [{
                    "timestamp": [1700000000, 1700086400, 1700172800],
                    "indicators": {
                        "quote": [{
                            "open": [2500.0, 2510.0, 2520.0],
                            "high": [2550.0, 2560.0, 2570.0],
                            "low": [2490.0, 2500.0, 2510.0],
                            "close": [2540.0, 2555.0, 2565.0],
                            "volume": [1000000, 1200000, 900000]
                        }]
                    }
                }],
                "error": null
            }
        }"#;

        let resp: ChartResponse = serde_json::from_str(json).unwrap();
        let result = resp.chart.result.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].timestamp.len(), 3);
        assert_eq!(result[0].indicators.quote[0].close[0], Some(2540.0));
    }

    #[test]
    fn parse_chart_with_nulls() {
        let json = r#"{
            "chart": {
                "result": [{
                    "timestamp": [1700000000, 1700086400],
                    "indicators": {
                        "quote": [{
                            "open": [2500.0, null],
                            "high": [2550.0, null],
                            "low": [2490.0, null],
                            "close": [2540.0, null],
                            "volume": [1000000, null]
                        }]
                    }
                }],
                "error": null
            }
        }"#;

        let resp: ChartResponse = serde_json::from_str(json).unwrap();
        let data = &resp.chart.result.unwrap()[0];
        assert_eq!(data.indicators.quote[0].open[1], None);
    }

    #[test]
    fn parse_chart_error() {
        let json = r#"{
            "chart": {
                "result": null,
                "error": {"code": "Not Found", "description": "No data found"}
            }
        }"#;

        let resp: ChartResponse = serde_json::from_str(json).unwrap();
        assert!(resp.chart.error.is_some());
        assert!(resp.chart.result.is_none());
    }
}
