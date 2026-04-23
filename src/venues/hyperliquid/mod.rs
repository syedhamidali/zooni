#![allow(dead_code)]
//! Hyperliquid API — skeleton.
//!
//! Docs: <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api>
//! Official Rust SDK (reference): `hyperliquid-rust-sdk`.
//!
//! Auth model: **EIP-712 typed-data signing** with an Ethereum-compatible private key.
//! There is no API key — the wallet *is* the account. Secure deployment on a VPS uses
//! the **Agent Wallet / API Wallet** pattern: a separate key that can place and cancel
//! orders on behalf of a master wallet but **cannot withdraw**.
//!
//! Endpoints:
//!   POST /info      — unauthenticated reads (meta, user state, L2 book, candles, funding)
//!   POST /exchange  — authenticated writes; action payload is EIP-712 signed.
//!
//! WS: `wss://api.hyperliquid.xyz/ws` — JSON subscriptions; user channels require `user`.
//!
//! Future dep needed for real signing: `alloy-signer-local` (or `ethers-signers`).
//! Not added to `Cargo.toml` yet so the skeleton has zero extra weight.

pub mod rest;
pub mod ws;

pub const REST_BASE: &str = "https://api.hyperliquid.xyz";
pub const REST_BASE_TESTNET: &str = "https://api.hyperliquid-testnet.xyz";
pub const WS_URL: &str = "wss://api.hyperliquid.xyz/ws";
pub const WS_URL_TESTNET: &str = "wss://api.hyperliquid-testnet.xyz/ws";

/// Agent (API) wallet used for signing trading actions. Kept separate from the
/// master wallet so a VPS compromise can't drain funds.
#[derive(Debug, Clone)]
pub struct AgentKey {
    /// Hex-encoded private key. Handle with care — treat as a credential.
    pub private_key_hex: String,
    /// Master wallet address this agent is authorised to act for.
    pub master_address: String,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub agent: Option<AgentKey>,
    pub testnet: bool,
}
