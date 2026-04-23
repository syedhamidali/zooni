#![allow(dead_code)]
//! IB TWS/Gateway client skeleton.
//!
//! Connection flow (IB proprietary protocol):
//!   1. TCP connect to host:port.
//!   2. Server sends a handshake; client sends server version handshake back.
//!   3. Client sends a `startAPI` message with clientId and optionalCapabilities.
//!   4. Server sends managedAccounts + nextValidId.
//!   5. Now you can send requests (reqContractDetails, placeOrder, reqMktData, etc.)
//!      and receive callbacks on a persistent reader task.
//!
//! We'll wrap all of this behind high-level methods when implementing — the protocol
//! is length-prefixed UTF-8 text, with numeric message IDs. A dedicated crate is
//! strongly preferred over a hand-roll.

use anyhow::{bail, Result};

use super::Config;
use crate::types::{OrderKind, Side};

pub struct Client {
    cfg: Config,
}

impl Client {
    pub fn new(cfg: Config) -> Self { Self { cfg } }
    pub fn config(&self) -> &Config { &self.cfg }

    /// TCP + handshake + startAPI; returns once `nextValidId` has been received.
    pub async fn connect(&mut self) -> Result<()> {
        bail!("interactive_brokers::connect not yet implemented")
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        bail!("interactive_brokers::disconnect not yet implemented")
    }

    /// reqAccountSummary for NetLiquidation, BuyingPower, TotalCashValue.
    pub async fn account_summary(&self) -> Result<()> {
        bail!("interactive_brokers::account_summary not yet implemented")
    }

    /// reqPositions — streams positions on an event channel.
    pub async fn positions(&self) -> Result<()> {
        bail!("interactive_brokers::positions not yet implemented")
    }

    /// placeOrder for a US stock contract (e.g., AAPL on NASDAQ, USD).
    pub async fn place_us_stock_order(
        &self,
        _symbol: &str,
        _side: Side,
        _kind: OrderKind,
        _quantity: u32,
        _limit_price: Option<f64>,
    ) -> Result<i32> {
        bail!("interactive_brokers::place_us_stock_order not yet implemented")
    }

    pub async fn cancel_order(&self, _order_id: i32) -> Result<()> {
        bail!("interactive_brokers::cancel_order not yet implemented")
    }

    /// Cancel every open order on the connected account.
    pub async fn global_cancel(&self) -> Result<()> {
        bail!("interactive_brokers::global_cancel not yet implemented")
    }
}
