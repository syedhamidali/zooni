#![allow(dead_code)]
//! MCX — Multi Commodity Exchange of India.
//!
//! **MCX is not a separate API venue.** It's accessed through Indian brokers
//! (Zerodha, Upstox, Angel One, etc.) using exchange code `MCX` in their order
//! endpoints. When you're ready to trade MCX, route orders via
//! `crate::venues::zerodha::rest::Client::place_order(..., exchange = "MCX", ...)`.
//!
//! This module holds MCX-specific constants (session hours, common instruments,
//! product codes, lot-size notes) so strategy code doesn't have to hard-code them.
//!
//! Session: MCX non-agri commodities run ~09:00 – 23:30 IST Mon–Fri (agri shorter).
//! Window logic lives in `crate::market_hours::Session::Mcx`. Holidays follow the
//! MCX calendar, which mostly overlaps NSE's; verify annually.
//!
//! Typical Zerodha product code for MCX: `NRML` (carry-forward), `MIS` (intraday).

/// Common MCX underlyings (futures trading symbols on Kite look like `GOLD26APRFUT`).
pub mod instruments {
    pub const GOLD: &str = "GOLD";
    pub const SILVER: &str = "SILVER";
    pub const CRUDEOIL: &str = "CRUDEOIL";
    pub const NATURALGAS: &str = "NATURALGAS";
    pub const COPPER: &str = "COPPER";
    pub const ZINC: &str = "ZINC";
    pub const LEAD: &str = "LEAD";
    pub const ALUMINIUM: &str = "ALUMINIUM";
    pub const NICKEL: &str = "NICKEL";
}

/// Approximate lot sizes (contract specs — reconfirm against MCX current spec sheet before trading).
pub mod lot_size {
    pub const GOLD_KG: u32 = 1;           // 1 kg per GOLD 1kg contract
    pub const GOLDM_G: u32 = 100;         // 100 g per GOLDM contract
    pub const SILVER_KG: u32 = 30;        // 30 kg per SILVER contract
    pub const CRUDEOIL_BBL: u32 = 100;    // 100 barrels per CRUDEOIL contract
    pub const NATURALGAS_MMBTU: u32 = 1250;
}

/// Kite exchange code to use when placing MCX orders.
pub const KITE_EXCHANGE_CODE: &str = "MCX";
