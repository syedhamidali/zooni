//! Telegram bot for remote control and monitoring.
//!
//! Commands:
//!   /status  — current state, equity, P&L
//!   /pnl     — daily/weekly/total P&L breakdown
//!   /grid    — grid state summary
//!   /pause   — trip kill switch
//!   /resume  — reset kill switch
//!   /fills   — recent fills
//!
//! Requires TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID env vars.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tracing::{error, info, warn};

/// Telegram bot configuration.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TelegramConfig {
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
    /// Polling interval for getUpdates (seconds).
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
}

fn default_poll() -> u64 { 5 }

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: None,
            chat_id: None,
            poll_secs: default_poll(),
        }
    }
}

pub struct TelegramBot {
    token: String,
    chat_id: String,
    http: reqwest::Client,
}

impl TelegramBot {
    pub fn new(token: String, chat_id: String) -> Self {
        Self {
            token,
            chat_id,
            http: reqwest::Client::new(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok()?;
        if token.is_empty() || chat_id.is_empty() {
            return None;
        }
        Some(Self::new(token, chat_id))
    }

    /// Send a message to the configured chat.
    pub async fn send(&self, text: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "Markdown",
        });
        self.http
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await?;
        Ok(())
    }

    /// Poll for commands and dispatch. Returns on error or when kill_switch is tripped.
    pub async fn poll_loop(
        &self,
        kill: Arc<AtomicBool>,
        poll_interval: Duration,
        status_fn: Arc<dyn Fn() -> String + Send + Sync>,
        pnl_fn: Arc<dyn Fn() -> String + Send + Sync>,
    ) {
        let mut offset: i64 = 0;
        info!("Telegram bot polling started");

        loop {
            if kill.load(Ordering::Relaxed) {
                break;
            }

            match self.get_updates(offset).await {
                Ok(updates) => {
                    for update in updates {
                        if let Some(new_offset) = update.update_id {
                            offset = new_offset + 1;
                        }
                        if let Some(text) = update.message_text() {
                            let response = match text.trim() {
                                "/status" | "/start" => status_fn(),
                                "/pnl" => pnl_fn(),
                                "/pause" => {
                                    kill.store(true, Ordering::SeqCst);
                                    "Kill switch tripped. Bot is pausing.".to_string()
                                }
                                "/help" => {
                                    "/status — current state\n\
                                     /pnl — P&L breakdown\n\
                                     /pause — stop trading\n\
                                     /help — this message"
                                        .to_string()
                                }
                                _ => format!("Unknown command: {text}\nTry /help"),
                            };
                            if let Err(e) = self.send(&response).await {
                                error!(err = %e, "failed to send Telegram response");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(err = %e, "Telegram getUpdates failed");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn get_updates(&self, offset: i64) -> Result<Vec<TgUpdate>> {
        let url = format!("https://api.telegram.org/bot{}/getUpdates", self.token);
        let body = serde_json::json!({
            "offset": offset,
            "timeout": 5,
            "allowed_updates": ["message"],
        });

        #[derive(serde::Deserialize)]
        struct Response {
            #[allow(dead_code)]
            ok: bool,
            result: Vec<TgUpdate>,
        }

        let resp: Response = self
            .http
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.result)
    }
}

#[derive(Debug, serde::Deserialize)]
struct TgUpdate {
    update_id: Option<i64>,
    message: Option<TgMessage>,
}

#[derive(Debug, serde::Deserialize)]
struct TgMessage {
    text: Option<String>,
}

impl TgUpdate {
    fn message_text(&self) -> Option<&str> {
        self.message.as_ref()?.text.as_deref()
    }
}
