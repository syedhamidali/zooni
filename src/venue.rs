//! Venue abstraction — unified interface over Bybit, Zerodha, etc.
//!
//! Uses enum dispatch (not trait objects) to avoid async-trait boxing overhead.
//! Each runner accepts a `Venue` and calls its methods; the enum delegates to
//! the concrete venue client internally.

use anyhow::{Result, anyhow};

use crate::types::{OrderKind, Side};
use crate::venues::{bybit, yahoo, zerodha};

// ---------------------------------------------------------------------------
// Unified types — every venue maps its native types into these
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VenueTicker {
    pub symbol: String,
    pub last_price: f64,
    pub bid_price: Option<f64>,
    pub ask_price: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct VenueOrderResult {
    pub order_id: String,
}

#[derive(Debug, Clone)]
pub struct VenueOpenOrder {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
    pub filled_qty: f64,
}

#[derive(Debug, Clone)]
pub struct VenueBalance {
    pub total_equity: f64,
    pub available: f64,
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct VenueInstrument {
    pub symbol: String,
    pub tick_size: f64,
    pub min_qty: f64,
    pub lot_size: f64,
}

#[derive(Debug, Clone)]
pub struct VenueKline {
    pub timestamp: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

// ---------------------------------------------------------------------------
// Conversions from venue-specific types
// ---------------------------------------------------------------------------

impl From<bybit::rest::Kline> for VenueKline {
    fn from(k: bybit::rest::Kline) -> Self {
        Self {
            timestamp: k.timestamp,
            open: k.open,
            high: k.high,
            low: k.low,
            close: k.close,
            volume: k.volume,
        }
    }
}

// ---------------------------------------------------------------------------
// Venue enum
// ---------------------------------------------------------------------------

pub enum Venue {
    Bybit {
        client: bybit::rest::Client,
        account_type: String,
    },
    Zerodha {
        client: zerodha::rest::Client,
        exchange: String,
        product: String,
        yahoo: tokio::sync::Mutex<yahoo::YahooClient>,
    },
}

impl Venue {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bybit { .. } => "Bybit",
            Self::Zerodha { .. } => "Zerodha",
        }
    }

    pub fn base_url(&self) -> &str {
        match self {
            Self::Bybit { client, .. } => client.base_url(),
            Self::Zerodha { client, .. } => client.base_url(),
        }
    }

    // -- Market data --------------------------------------------------------

    pub async fn ticker(&self, symbol: &str) -> Result<VenueTicker> {
        match self {
            Self::Bybit { client, .. } => {
                let t = client.ticker_spot(symbol).await?;
                Ok(VenueTicker {
                    symbol: t.symbol,
                    last_price: t.last_price.parse().unwrap_or(0.0),
                    bid_price: t.bid1_price.parse().ok(),
                    ask_price: t.ask1_price.parse().ok(),
                })
            }
            Self::Zerodha {
                client, exchange, ..
            } => {
                let key = format!("{exchange}:{symbol}");
                let mut ltp = client.ltp(&[&key]).await?;
                let data = ltp
                    .remove(&key)
                    .ok_or_else(|| anyhow!("no LTP data for {key}"))?;
                Ok(VenueTicker {
                    symbol: symbol.to_string(),
                    last_price: data.last_price,
                    bid_price: None,
                    ask_price: None,
                })
            }
        }
    }

    pub async fn ticker_with_depth(&self, symbol: &str) -> Result<VenueTicker> {
        match self {
            Self::Bybit { .. } => self.ticker(symbol).await,
            Self::Zerodha {
                client, exchange, ..
            } => {
                let key = format!("{exchange}:{symbol}");
                let mut quotes = client.quote(&[&key]).await?;
                let q = quotes
                    .remove(&key)
                    .ok_or_else(|| anyhow!("no quote data for {key}"))?;
                Ok(VenueTicker {
                    symbol: symbol.to_string(),
                    last_price: q.last_price,
                    bid_price: None,
                    ask_price: None,
                })
            }
        }
    }

    pub async fn instrument_info(&self, symbol: &str) -> Result<VenueInstrument> {
        match self {
            Self::Bybit { client, .. } => {
                let info = client.instrument_info_spot(symbol).await?;
                Ok(VenueInstrument {
                    symbol: info.symbol,
                    tick_size: info.price_filter.tick_size.parse().unwrap_or(0.01),
                    min_qty: info.lot_size_filter.min_order_qty.parse().unwrap_or(0.0),
                    lot_size: info.lot_size_filter.base_precision.parse().unwrap_or(1.0),
                })
            }
            Self::Zerodha {
                client, exchange, ..
            } => {
                let instruments = client.instruments(Some(exchange)).await?;
                let inst = instruments
                    .into_iter()
                    .find(|i| i.tradingsymbol == symbol)
                    .ok_or_else(|| anyhow!("instrument {symbol} not found on {exchange}"))?;
                Ok(VenueInstrument {
                    symbol: inst.tradingsymbol,
                    tick_size: inst.tick_size,
                    min_qty: 1.0,
                    lot_size: inst.lot_size as f64,
                })
            }
        }
    }

    pub async fn klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<VenueKline>> {
        match self {
            Self::Bybit { client, .. } => {
                let klines = client.klines(symbol, interval, limit).await?;
                Ok(klines
                    .into_iter()
                    .map(|k| VenueKline {
                        timestamp: k.timestamp,
                        open: k.open,
                        high: k.high,
                        low: k.low,
                        close: k.close,
                        volume: k.volume,
                    })
                    .collect())
            }
            Self::Zerodha {
                exchange, yahoo, ..
            } => {
                let yahoo_sym = match exchange.as_str() {
                    "BSE" => yahoo::bse_symbol(symbol),
                    _ => yahoo::nse_symbol(symbol),
                };
                let yahoo_interval = yahoo::map_interval(interval);
                let days = match limit {
                    0..=50 => 30,
                    51..=200 => 180,
                    _ => 365,
                };
                let mut yc = yahoo.lock().await;
                yc.klines_days(&yahoo_sym, yahoo_interval, days).await
            }
        }
    }

    // -- Account ------------------------------------------------------------

    pub async fn wallet_balance(&self) -> Result<VenueBalance> {
        match self {
            Self::Bybit {
                client,
                account_type,
            } => {
                let accounts = client.wallet_balance(account_type).await?;
                let mut total = 0.0f64;
                let mut available = 0.0f64;
                for acct in &accounts {
                    for coin in &acct.coin {
                        if coin.coin == "USDT" || coin.coin == "USD" {
                            total += coin.wallet_balance.parse::<f64>().unwrap_or(0.0);
                            available += coin.available_to_withdraw.parse::<f64>().unwrap_or(0.0);
                        }
                    }
                }
                Ok(VenueBalance {
                    total_equity: total,
                    available,
                    currency: "USDT".to_string(),
                })
            }
            Self::Zerodha { client, .. } => {
                let margins = client.margins().await?;
                let (total, avail) = if let Some(eq) = &margins.equity {
                    (eq.net, eq.available.cash + eq.available.collateral)
                } else {
                    (0.0, 0.0)
                };
                Ok(VenueBalance {
                    total_equity: total,
                    available: avail,
                    currency: "INR".to_string(),
                })
            }
        }
    }

    // -- Order management ---------------------------------------------------

    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        kind: OrderKind,
        qty: &str,
        price: Option<&str>,
        tag: Option<&str>,
        post_only: bool,
    ) -> Result<VenueOrderResult> {
        match self {
            Self::Bybit { client, .. } => {
                let order_type = match kind {
                    OrderKind::Limit => "Limit",
                    OrderKind::Market => "Market",
                };
                let result = client
                    .place_spot_order(symbol, side, order_type, qty, price, tag, post_only)
                    .await?;
                Ok(VenueOrderResult {
                    order_id: result.order_id,
                })
            }
            Self::Zerodha {
                client,
                exchange,
                product,
                ..
            } => {
                let quantity: u32 = qty.parse::<f64>().map(|v| v as u32).unwrap_or(0);
                let price_f = price.and_then(|p| p.parse::<f64>().ok());
                let order_id = client
                    .place_order(symbol, exchange, product, side, kind, quantity, price_f)
                    .await?;
                Ok(VenueOrderResult { order_id })
            }
        }
    }

    pub async fn cancel_order(&self, symbol: &str, order_id: &str) -> Result<()> {
        match self {
            Self::Bybit { client, .. } => {
                client.cancel_spot_order(symbol, order_id).await?;
                Ok(())
            }
            Self::Zerodha { client, .. } => client.cancel_order(order_id).await,
        }
    }

    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<()> {
        match self {
            Self::Bybit { client, .. } => client.cancel_all_spot(symbol).await,
            Self::Zerodha { client, .. } => {
                let orders = client.orders().await?;
                for order in orders {
                    if order.tradingsymbol == symbol
                        && (order.status == "OPEN"
                            || order.status == "TRIGGER PENDING"
                            || order.status == "OPEN PENDING")
                    {
                        let _ = client
                            .cancel_order_variety(&order.variety, &order.order_id)
                            .await;
                    }
                }
                Ok(())
            }
        }
    }

    pub async fn open_orders(&self, symbol: &str) -> Result<Vec<VenueOpenOrder>> {
        match self {
            Self::Bybit { client, .. } => {
                let orders = client.open_orders_spot(symbol).await?;
                Ok(orders
                    .into_iter()
                    .map(|o| {
                        let side = if o.side == "Buy" {
                            Side::Buy
                        } else {
                            Side::Sell
                        };
                        VenueOpenOrder {
                            order_id: o.order_id,
                            symbol: o.symbol,
                            side,
                            price: o.price.parse().unwrap_or(0.0),
                            qty: o.qty.parse().unwrap_or(0.0),
                            filled_qty: o.cum_exec_qty.parse().unwrap_or(0.0),
                        }
                    })
                    .collect())
            }
            Self::Zerodha { client, .. } => {
                let orders = client.orders().await?;
                Ok(orders
                    .into_iter()
                    .filter(|o| {
                        o.tradingsymbol == symbol
                            && (o.status == "OPEN" || o.status == "TRIGGER PENDING")
                    })
                    .map(|o| {
                        let side = if o.transaction_type == "BUY" {
                            Side::Buy
                        } else {
                            Side::Sell
                        };
                        VenueOpenOrder {
                            order_id: o.order_id,
                            symbol: o.tradingsymbol,
                            side,
                            price: o.price,
                            qty: o.quantity as f64,
                            filled_qty: o.filled_quantity as f64,
                        }
                    })
                    .collect())
            }
        }
    }

    // -- Zerodha-specific ---------------------------------------------------

    pub fn zerodha_client_mut(&mut self) -> Option<&mut zerodha::rest::Client> {
        match self {
            Self::Zerodha { client, .. } => Some(client),
            _ => None,
        }
    }

    pub fn zerodha_login_url(&self) -> Option<String> {
        match self {
            Self::Zerodha { client, .. } => client.config().login_url(),
            _ => None,
        }
    }

    // -- Bybit-specific passthrough for features only Bybit has -------------

    pub fn bybit_client(&self) -> Option<&bybit::rest::Client> {
        match self {
            Self::Bybit { client, .. } => Some(client),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl Venue {
    pub fn new_bybit(cfg: bybit::Config, account_type: String) -> Self {
        Self::Bybit {
            client: bybit::rest::Client::new(cfg),
            account_type,
        }
    }

    pub fn new_zerodha(cfg: zerodha::Config, exchange: String, product: String) -> Self {
        Self::Zerodha {
            client: zerodha::rest::Client::new(cfg),
            exchange,
            product,
            yahoo: tokio::sync::Mutex::new(yahoo::YahooClient::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_name() {
        let bybit_cfg = bybit::Config {
            testnet: true,
            credentials: None,
            recv_window_ms: 5000,
        };
        let v = Venue::new_bybit(bybit_cfg, "UNIFIED".to_string());
        assert_eq!(v.name(), "Bybit");
    }

    #[test]
    fn zerodha_login_url_works() {
        let cfg = zerodha::Config {
            credentials: Some(zerodha::Credentials {
                api_key: "test_key".to_string(),
                api_secret: "test_secret".to_string(),
                access_token: None,
            }),
        };
        let v = Venue::new_zerodha(cfg, "NSE".to_string(), "CNC".to_string());
        let url = v.zerodha_login_url().unwrap();
        assert!(url.contains("test_key"));
        assert!(url.contains("kite.zerodha.com"));
    }

    #[test]
    fn bybit_client_accessor() {
        let cfg = bybit::Config {
            testnet: true,
            credentials: None,
            recv_window_ms: 5000,
        };
        let v = Venue::new_bybit(cfg, "UNIFIED".to_string());
        assert!(v.bybit_client().is_some());
        assert!(v.zerodha_login_url().is_none());
    }

    #[test]
    fn zerodha_client_accessor() {
        let cfg = zerodha::Config::default();
        let mut v = Venue::new_zerodha(cfg, "NSE".to_string(), "CNC".to_string());
        assert!(v.zerodha_client_mut().is_some());
        assert!(v.bybit_client().is_none());
    }
}
