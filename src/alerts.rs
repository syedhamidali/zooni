//! Notifications via webhook (Slack, Discord, generic HTTP).
//!
//! Fire-and-forget — alerting failures never block trading logic.

use anyhow::Result;
use serde::Serialize;
use tracing::{error, info};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AlertConfig {
    /// Webhook URL (Slack incoming webhook, Discord webhook, or any HTTP POST endpoint).
    pub webhook_url: Option<String>,
    /// Send an alert on every fill (can be noisy — disable for high-frequency grids).
    #[serde(default)]
    pub on_fill: bool,
    /// Alert on risk limit breach.
    #[serde(default = "default_true")]
    pub on_risk_breach: bool,
    /// Alert on bot startup / shutdown.
    #[serde(default = "default_true")]
    pub on_lifecycle: bool,
    /// Alert on errors (API failures, circuit breaker trips).
    #[serde(default = "default_true")]
    pub on_error: bool,
    /// Periodic P&L summary interval in seconds (0 = disabled).
    #[serde(default)]
    pub pnl_summary_interval_secs: u64,
}

fn default_true() -> bool { true }

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            webhook_url: None,
            on_fill: false,
            on_risk_breach: true,
            on_lifecycle: true,
            on_error: true,
            pnl_summary_interval_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

impl AlertLevel {
    fn emoji(self) -> &'static str {
        match self {
            Self::Info => "ℹ️",
            Self::Warning => "⚠️",
            Self::Critical => "🚨",
        }
    }
}

#[derive(Debug, Serialize)]
struct SlackPayload {
    text: String,
}

pub struct Alerter {
    config: AlertConfig,
    http: reqwest::Client,
    bot_name: String,
}

impl Alerter {
    pub fn new(config: AlertConfig, bot_name: String) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            bot_name,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.webhook_url.is_some()
    }

    /// Send an alert. Fire-and-forget — errors are logged but never propagated.
    pub async fn send(&self, level: AlertLevel, category: &str, message: &str) {
        // Check category filter
        match category {
            "fill" if !self.config.on_fill => return,
            "risk" if !self.config.on_risk_breach => return,
            "lifecycle" if !self.config.on_lifecycle => return,
            "error" if !self.config.on_error => return,
            _ => {}
        }

        let url = match &self.config.webhook_url {
            Some(u) => u,
            None => return,
        };

        let text = format!(
            "{} *[{}]* `{}` — {}",
            level.emoji(),
            self.bot_name,
            category,
            message
        );

        info!(category, level = ?level, "alert: {}", message);

        if let Err(e) = self.post_webhook(url, &text).await {
            error!(err = %e, "failed to send alert webhook");
        }
    }

    async fn post_webhook(&self, url: &str, text: &str) -> Result<()> {
        let payload = SlackPayload { text: text.to_string() };
        self.http
            .post(url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;
        Ok(())
    }

    // Convenience methods

    pub async fn fill(&self, symbol: &str, side: &str, price: f64, qty: f64, pnl: f64) {
        self.send(
            AlertLevel::Info,
            "fill",
            &format!("{symbol} {side} {qty:.6} @ {price:.2} | PnL: {pnl:+.2}"),
        )
        .await;
    }

    pub async fn risk_breach(&self, reason: &str) {
        self.send(AlertLevel::Critical, "risk", reason).await;
    }

    pub async fn lifecycle(&self, msg: &str) {
        self.send(AlertLevel::Info, "lifecycle", msg).await;
    }

    pub async fn error(&self, msg: &str) {
        self.send(AlertLevel::Warning, "error", msg).await;
    }

    pub async fn pnl_summary(
        &self,
        symbol: &str,
        daily_pnl: f64,
        total_pnl: f64,
        fills: i64,
        equity: f64,
    ) {
        self.send(
            AlertLevel::Info,
            "pnl",
            &format!(
                "{symbol} daily: {daily_pnl:+.2} | total: {total_pnl:+.2} | fills: {fills} | equity: {equity:.2}"
            ),
        )
        .await;
    }
}
