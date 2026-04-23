#![allow(dead_code)]
//! Bybit public WebSocket — verified working against `linear` stream.
//! Subscribe pattern: `{"op": "subscribe", "args": ["tickers.BTCUSDT"]}`.
//! Keep-alive: client-sent `{"op": "ping"}` every 20s.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::WS_PUBLIC_LINEAR;

const PING_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Debug, Deserialize)]
struct TickerMsg {
    #[allow(dead_code)]
    topic: String,
    #[serde(rename = "type")]
    update_type: String,
    #[allow(dead_code)]
    ts: i64,
    data: TickerData,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct TickerData {
    symbol: Option<String>,
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(rename = "bid1Price")]
    bid1_price: Option<String>,
    #[serde(rename = "ask1Price")]
    ask1_price: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "indexPrice")]
    index_price: Option<String>,
}

pub async fn run_public_ticker(symbol: &str) -> Result<()> {
    info!(url = WS_PUBLIC_LINEAR, symbol, "connecting to Bybit linear public WS");
    let (mut ws, _resp) = connect_async(WS_PUBLIC_LINEAR).await.context("ws connect failed")?;
    info!("connected");

    let sub = serde_json::json!({
        "op": "subscribe",
        "args": [format!("tickers.{symbol}")],
    });
    ws.send(Message::Text(sub.to_string())).await.context("subscribe send failed")?;
    info!(symbol, "subscribe sent");

    let mut ping_tick = interval(PING_INTERVAL);
    ping_tick.tick().await;

    loop {
        tokio::select! {
            _ = ping_tick.tick() => {
                let ping = serde_json::json!({"op": "ping"});
                if let Err(e) = ws.send(Message::Text(ping.to_string())).await {
                    error!(err = %e, "ping send failed, exiting");
                    break;
                }
                debug!("ping sent");
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(text))) => handle_text(&text),
                Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                Some(Ok(Message::Close(frame))) => { warn!(?frame, "server closed"); break; }
                Some(Ok(_)) => {}
                Some(Err(e)) => { error!(err = %e, "ws error, exiting"); break; }
                None => { warn!("ws stream ended"); break; }
            }
        }
    }
    Ok(())
}

fn handle_text(text: &str) {
    if let Ok(msg) = serde_json::from_str::<TickerMsg>(text) {
        let d = &msg.data;
        info!(
            symbol = d.symbol.as_deref().unwrap_or("?"),
            kind = %msg.update_type,
            last = d.last_price.as_deref().unwrap_or("-"),
            bid = d.bid1_price.as_deref().unwrap_or("-"),
            ask = d.ask1_price.as_deref().unwrap_or("-"),
            mark = d.mark_price.as_deref().unwrap_or("-"),
            index = d.index_price.as_deref().unwrap_or("-"),
            "tick"
        );
    } else {
        debug!(raw = text, "control or unknown msg");
    }
}
