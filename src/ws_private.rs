//! Bybit private WebSocket — real-time order/fill notifications.
//!
//! Subscribes to `order` topic for fill events. Auth via HMAC-SHA256.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::types::Side;
use crate::venues::bybit;

const PING_INTERVAL: Duration = Duration::from_secs(20);

/// Events emitted by the private WebSocket.
#[derive(Debug, Clone)]
pub enum WsEvent {
    OrderFilled {
        order_id: String,
        order_link_id: String,
        symbol: String,
        side: Side,
        price: f64,
        qty: f64,
        cum_exec_qty: f64,
        status: String,
    },
    OrderCancelled {
        order_id: String,
        symbol: String,
    },
    Connected,
    Disconnected,
}

#[derive(Debug, Deserialize)]
struct WsOrderMsg {
    topic: String,
    data: Vec<WsOrderData>,
}

#[derive(Debug, Deserialize)]
struct WsOrderData {
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "orderLinkId", default)]
    order_link_id: String,
    symbol: String,
    side: String,
    #[serde(rename = "avgPrice", default)]
    avg_price: String,
    #[serde(rename = "cumExecQty", default)]
    cum_exec_qty: String,
    qty: String,
    #[serde(rename = "orderStatus")]
    order_status: String,
}

/// Start the private WebSocket listener. Sends events on the returned channel.
/// Automatically reconnects on disconnection.
pub async fn start(
    api_key: String,
    api_secret: String,
    testnet: bool,
    tx: mpsc::UnboundedSender<WsEvent>,
) {
    loop {
        match run_once(&api_key, &api_secret, testnet, &tx).await {
            Ok(()) => {
                info!("private WS closed normally, reconnecting in 5s");
            }
            Err(e) => {
                warn!(err = %e, "private WS error, reconnecting in 5s");
            }
        }
        let _ = tx.send(WsEvent::Disconnected);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_once(
    api_key: &str,
    api_secret: &str,
    testnet: bool,
    tx: &mpsc::UnboundedSender<WsEvent>,
) -> Result<()> {
    let url = if testnet {
        bybit::WS_PRIVATE_TESTNET
    } else {
        bybit::WS_PRIVATE
    };

    info!(url, "connecting to Bybit private WS");
    let (mut ws, _) = connect_async(url).await.context("ws connect failed")?;
    info!("private WS connected");

    // Authenticate
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64 + 10_000) // 10s expiry
        .unwrap_or(0);
    let sign_payload = format!("GET/realtime{expires}");
    let signature = {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).unwrap();
        mac.update(sign_payload.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    };

    let auth = serde_json::json!({
        "op": "auth",
        "args": [api_key, expires, signature],
    });
    ws.send(Message::Text(auth.to_string())).await.context("auth send failed")?;
    debug!("auth sent, waiting for response");

    // Subscribe to order updates
    let sub = serde_json::json!({
        "op": "subscribe",
        "args": ["order"],
    });
    ws.send(Message::Text(sub.to_string())).await.context("subscribe send failed")?;
    info!("subscribed to order updates");
    let _ = tx.send(WsEvent::Connected);

    let mut ping_tick = interval(PING_INTERVAL);
    ping_tick.tick().await;

    loop {
        tokio::select! {
            _ = ping_tick.tick() => {
                let ping = serde_json::json!({"op": "ping"});
                if let Err(e) = ws.send(Message::Text(ping.to_string())).await {
                    error!(err = %e, "ping send failed");
                    break;
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    handle_message(&text, tx);
                }
                Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                Some(Ok(Message::Close(frame))) => {
                    warn!(?frame, "server closed private WS");
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    error!(err = %e, "private WS error");
                    break;
                }
                None => {
                    warn!("private WS stream ended");
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_message(text: &str, tx: &mpsc::UnboundedSender<WsEvent>) {
    if let Ok(msg) = serde_json::from_str::<WsOrderMsg>(text) {
        if msg.topic == "order" {
            for d in &msg.data {
                let side = if d.side == "Buy" { Side::Buy } else { Side::Sell };
                let price = d.avg_price.parse::<f64>().unwrap_or(0.0);
                let qty = d.qty.parse::<f64>().unwrap_or(0.0);
                let cum_exec_qty = d.cum_exec_qty.parse::<f64>().unwrap_or(0.0);

                match d.order_status.as_str() {
                    "Filled" | "PartiallyFilled" => {
                        info!(
                            order_id = %d.order_id,
                            symbol = %d.symbol,
                            side = %d.side,
                            status = %d.order_status,
                            price,
                            cum_exec_qty,
                            "[WS] fill event",
                        );
                        let _ = tx.send(WsEvent::OrderFilled {
                            order_id: d.order_id.clone(),
                            order_link_id: d.order_link_id.clone(),
                            symbol: d.symbol.clone(),
                            side,
                            price,
                            qty,
                            cum_exec_qty,
                            status: d.order_status.clone(),
                        });
                    }
                    "Cancelled" => {
                        debug!(order_id = %d.order_id, "[WS] order cancelled");
                        let _ = tx.send(WsEvent::OrderCancelled {
                            order_id: d.order_id.clone(),
                            symbol: d.symbol.clone(),
                        });
                    }
                    _ => {
                        debug!(order_id = %d.order_id, status = %d.order_status, "[WS] order update");
                    }
                }
            }
        }
    } else {
        debug!(raw = text, "control/auth msg");
    }
}
