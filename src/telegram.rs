//! Telegram bot for remote control and monitoring.
//!
//! Commands:
//!   /status  — current state, equity, P&L
//!   /pnl     — daily/weekly/total P&L breakdown
//!   /alloc   — per-strategy allocations
//!   /regime  — current market regime
//!   /fills   — recent fill count
//!   /pause   — trip kill switch
//!   /resume  — reset kill switch
//!
//! Requires TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID env vars.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use tracing::{error, info, warn};

/// Telegram bot configuration.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TelegramConfig {
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
}

fn default_poll() -> u64 {
    5
}

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

/// Collection of callback functions the bot uses to query live state.
pub struct BotCallbacks {
    pub status: Arc<dyn Fn() -> String + Send + Sync>,
    pub pnl: Arc<dyn Fn() -> String + Send + Sync>,
    pub alloc: Arc<dyn Fn() -> String + Send + Sync>,
    pub regime: Arc<dyn Fn() -> String + Send + Sync>,
}

#[derive(serde::Deserialize)]
struct TgUpdate {
    update_id: Option<i64>,
    message: Option<TgMessage>,
}

#[derive(serde::Deserialize)]
struct TgMessage {
    text: Option<String>,
}

impl TgUpdate {
    fn message_text(&self) -> Option<&str> {
        self.message.as_ref()?.text.as_deref()
    }
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

    pub fn from_config(cfg: &TelegramConfig) -> Option<Self> {
        let token = cfg.bot_token.as_deref().filter(|s| !s.is_empty())?;
        let chat_id = cfg.chat_id.as_deref().filter(|s| !s.is_empty())?;
        Some(Self::new(token.to_string(), chat_id.to_string()))
    }

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

    /// Poll for commands and dispatch using the full callback set.
    ///
    /// If `alert_rx` is provided, outbound alert messages pushed by other tasks
    /// are drained and sent between poll cycles.
    pub async fn run(
        &self,
        kill: Arc<AtomicBool>,
        poll_interval: Duration,
        callbacks: BotCallbacks,
        mut alert_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
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
                                "/status" | "/start" => (callbacks.status)(),
                                "/pnl" => (callbacks.pnl)(),
                                "/alloc" | "/allocations" => (callbacks.alloc)(),
                                "/regime" => (callbacks.regime)(),
                                "/pause" => {
                                    kill.store(true, Ordering::SeqCst);
                                    "Kill switch tripped. Bot is pausing.".to_string()
                                }
                                "/resume" => {
                                    kill.store(false, Ordering::SeqCst);
                                    "Kill switch reset. Bot will resume.".to_string()
                                }
                                "/help" => HELP_TEXT.to_string(),
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

            // Drain proactive alert messages pushed by the main loop.
            while let Ok(msg) = alert_rx.try_recv() {
                if let Err(e) = self.send(&msg).await {
                    error!(err = %e, "failed to send Telegram alert");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Legacy poll loop for the grid bot (backwards-compatible).
    ///
    /// Creates a dummy alert channel internally so the grid bot path does not
    /// need to supply one.
    pub async fn poll_loop(
        &self,
        kill: Arc<AtomicBool>,
        poll_interval: Duration,
        status_fn: Arc<dyn Fn() -> String + Send + Sync>,
        pnl_fn: Arc<dyn Fn() -> String + Send + Sync>,
    ) {
        let callbacks = BotCallbacks {
            status: status_fn,
            pnl: pnl_fn,
            alloc: Arc::new(|| "Allocations not available in grid mode.".to_string()),
            regime: Arc::new(|| "Regime detection not available in grid mode.".to_string()),
        };
        // Dummy channel — no alerts in grid-bot mode.
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        self.run(kill, poll_interval, callbacks, rx).await;
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

const HELP_TEXT: &str = "\
/status — equity, P&L, uptime
/pnl — per-strategy P&L breakdown
/alloc — capital allocation per strategy
/regime — current market regime
/pause — stop trading (trip kill switch)
/resume — resume trading
/help — this message";

/// Format an orchestrator status message.
pub fn format_status(
    symbol: &str,
    equity: f64,
    total_pnl: f64,
    total_fills: u64,
    regime: &str,
    active_strategies: usize,
    uptime_secs: i64,
) -> String {
    let hours = uptime_secs / 3600;
    let mins = (uptime_secs % 3600) / 60;
    format!(
        "*{symbol} Orchestrator*\n\
         Equity: {equity:.2} USDT\n\
         P&L: {total_pnl:+.2} USDT\n\
         Fills: {total_fills}\n\
         Regime: {regime}\n\
         Strategies: {active_strategies} active\n\
         Uptime: {hours}h {mins}m",
    )
}

/// Format per-strategy P&L.
pub fn format_pnl(strategy_stats: &[(String, f64, u64, f64)]) -> String {
    let mut msg = "*Per-Strategy P&L*\n".to_string();
    for (name, pnl, trades, win_rate) in strategy_stats {
        msg.push_str(&format!(
            "\n`{name:<12}` {pnl:+.2} | {trades} trades | {wr:.0}% win",
            wr = win_rate * 100.0,
        ));
    }
    let total: f64 = strategy_stats.iter().map(|(_, p, _, _)| p).sum();
    msg.push_str(&format!("\n\nTotal: {total:+.2} USDT"));
    msg
}

/// Format allocation breakdown.
pub fn format_allocations(allocations: &HashMap<String, f64>) -> String {
    let mut msg = "*Capital Allocation*\n".to_string();
    let mut sorted: Vec<_> = allocations.iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (name, alloc) in &sorted {
        let bar_len = (*alloc * 20.0) as usize;
        let bar: String = "█".repeat(bar_len);
        msg.push_str(&format!(
            "\n`{name:<12}` {bar} {pct:.1}%",
            pct = *alloc * 100.0
        ));
    }
    msg
}

/// Format regime info.
pub fn format_regime(regime: &str, confidence: f64) -> String {
    format!(
        "*Market Regime*\n\
         Current: {regime}\n\
         Confidence: {confidence:.0}%",
    )
}
