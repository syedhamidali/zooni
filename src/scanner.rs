//! Market scanner — finds the best trading pairs for grid strategy.
//!
//! Scans all Bybit spot pairs, filters by volume/liquidity, fetches klines,
//! runs regime analysis, and ranks by grid suitability.

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::regime::{self, Regime, RegimeAnalysis};
use crate::venues::bybit::rest::{Client, TickerFull};

/// Minimum 24h turnover in USDT to consider a pair.
const MIN_TURNOVER_USDT: f64 = 1_000_000.0;

/// Minimum spread quality (bid/ask within 0.2%).
const MAX_SPREAD_PCT: f64 = 0.2;

/// A scanned candidate with its analysis.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub symbol: String,
    pub last_price: f64,
    pub turnover_24h: f64,
    pub volume_24h: f64,
    pub spread_pct: f64,
    pub price_change_pct: f64,
    pub analysis: RegimeAnalysis,
    /// Composite score — higher is better for grid trading.
    pub score: f64,
}

/// Configuration for the scanner.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ScannerConfig {
    /// Minimum 24h turnover in USDT.
    #[serde(default = "default_min_turnover")]
    pub min_turnover: f64,
    /// Maximum spread percentage.
    #[serde(default = "default_max_spread")]
    pub max_spread_pct: f64,
    /// Quote currencies to scan (e.g., ["USDT", "USDC"]).
    #[serde(default = "default_quote_currencies")]
    pub quote_currencies: Vec<String>,
    /// Maximum number of results to return.
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// Kline interval for regime analysis.
    #[serde(default = "default_kline_interval")]
    pub kline_interval: String,
    /// Number of klines to fetch per symbol.
    #[serde(default = "default_kline_count")]
    pub kline_count: u32,
}

fn default_min_turnover() -> f64 {
    MIN_TURNOVER_USDT
}
fn default_max_spread() -> f64 {
    MAX_SPREAD_PCT
}
fn default_quote_currencies() -> Vec<String> {
    vec!["USDT".to_string()]
}
fn default_max_results() -> usize {
    10
}
fn default_kline_interval() -> String {
    "60".to_string()
}
fn default_kline_count() -> u32 {
    200
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            min_turnover: default_min_turnover(),
            max_spread_pct: default_max_spread(),
            quote_currencies: default_quote_currencies(),
            max_results: default_max_results(),
            kline_interval: default_kline_interval(),
            kline_count: default_kline_count(),
        }
    }
}

/// Pre-filter tickers by volume and quote currency.
fn pre_filter(tickers: &[TickerFull], cfg: &ScannerConfig) -> Vec<TickerFull> {
    tickers
        .iter()
        .filter(|t| {
            // Check quote currency
            let has_quote = cfg
                .quote_currencies
                .iter()
                .any(|q| t.symbol.ends_with(q.as_str()));
            if !has_quote {
                return false;
            }

            // Check turnover
            let turnover: f64 = t.turnover_24h.parse().unwrap_or(0.0);
            if turnover < cfg.min_turnover {
                return false;
            }

            // Check spread
            let bid: f64 = t.bid1_price.parse().unwrap_or(0.0);
            let ask: f64 = t.ask1_price.parse().unwrap_or(0.0);
            if bid <= 0.0 || ask <= 0.0 {
                return false;
            }
            let spread_pct = (ask - bid) / bid * 100.0;
            if spread_pct > cfg.max_spread_pct {
                return false;
            }

            // Filter out stablecoins trading against each other
            let stables = ["USDT", "USDC", "DAI", "BUSD", "TUSD", "FDUSD"];
            let base = cfg.quote_currencies.iter().fold(t.symbol.as_str(), |s, q| {
                s.strip_suffix(q.as_str()).unwrap_or(s)
            });
            if stables.contains(&base) {
                return false;
            }

            true
        })
        .cloned()
        .collect()
}

/// Calculate a composite score for grid suitability.
/// Higher = better for grid trading.
fn score_candidate(ticker: &TickerFull, analysis: &RegimeAnalysis) -> f64 {
    let mut score = 0.0;

    // Regime bonus
    match analysis.regime {
        Regime::Ranging => score += 40.0,
        Regime::TrendingUp => score += 10.0,
        Regime::TrendingDown => score += 5.0,
        Regime::Volatile => score -= 20.0,
    }

    // Confidence multiplier
    score *= 0.5 + analysis.confidence * 0.5;

    // Volume bonus (log scale, normalized)
    let turnover: f64 = ticker.turnover_24h.parse().unwrap_or(0.0);
    if turnover > 0.0 {
        score += (turnover.ln() - 13.0).max(0.0) * 2.0; // ln(1M) ≈ 13.8
    }

    // Tight spread bonus
    let bid: f64 = ticker.bid1_price.parse().unwrap_or(0.0);
    let ask: f64 = ticker.ask1_price.parse().unwrap_or(0.0);
    if bid > 0.0 && ask > 0.0 {
        let spread_pct = (ask - bid) / bid * 100.0;
        score += (0.1 - spread_pct).max(0.0) * 50.0; // bonus for very tight spreads
    }

    // Moderate volatility is ideal (1-3% ATR/price)
    if analysis.volatility_pct > 0.5 && analysis.volatility_pct < 3.0 {
        score += 15.0 - (analysis.volatility_pct - 1.5).abs() * 5.0;
    }

    // Low ADX bonus (range-bound)
    if analysis.adx < 20.0 {
        score += 10.0;
    }

    // RSI near 50 bonus (balanced)
    let rsi_distance = (analysis.rsi - 50.0).abs();
    score += (25.0 - rsi_distance).max(0.0) * 0.5;

    score
}

/// Scan all Bybit spot pairs and return ranked candidates for grid trading.
pub async fn scan(client: &Client, cfg: &ScannerConfig) -> Result<Vec<ScanResult>> {
    info!("Starting market scan...");

    // Step 1: Fetch all tickers
    let tickers = client.all_tickers_spot().await?;
    info!(total = tickers.len(), "Fetched spot tickers");

    // Step 2: Pre-filter by volume, spread, and quote currency
    let candidates = pre_filter(&tickers, cfg);
    info!(
        candidates = candidates.len(),
        "After pre-filtering (min turnover: ${}, max spread: {}%)",
        cfg.min_turnover,
        cfg.max_spread_pct,
    );

    // Step 3: Fetch klines and analyze regime for each candidate
    let mut results = Vec::new();

    for ticker in &candidates {
        debug!(symbol = %ticker.symbol, "Analyzing...");

        match client
            .klines(&ticker.symbol, &cfg.kline_interval, cfg.kline_count)
            .await
        {
            Ok(klines) => {
                let venue_klines: Vec<crate::venue::VenueKline> =
                    klines.into_iter().map(Into::into).collect();
                if let Some(analysis) = regime::analyze(&venue_klines) {
                    let last_price: f64 = ticker.last_price.parse().unwrap_or(0.0);
                    let turnover: f64 = ticker.turnover_24h.parse().unwrap_or(0.0);
                    let volume: f64 = ticker.volume_24h.parse().unwrap_or(0.0);
                    let change_pct: f64 = ticker.price_24h_pcnt.parse().unwrap_or(0.0) * 100.0;

                    let bid: f64 = ticker.bid1_price.parse().unwrap_or(0.0);
                    let ask: f64 = ticker.ask1_price.parse().unwrap_or(0.0);
                    let spread_pct = if bid > 0.0 {
                        (ask - bid) / bid * 100.0
                    } else {
                        0.0
                    };

                    let score = score_candidate(ticker, &analysis);

                    results.push(ScanResult {
                        symbol: ticker.symbol.clone(),
                        last_price,
                        turnover_24h: turnover,
                        volume_24h: volume,
                        spread_pct,
                        price_change_pct: change_pct,
                        analysis,
                        score,
                    });
                }
            }
            Err(e) => {
                warn!(symbol = %ticker.symbol, err = %e, "Failed to fetch klines, skipping");
            }
        }

        // Small delay to respect rate limits
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Step 4: Sort by score (highest first)
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Truncate to max_results
    results.truncate(cfg.max_results);

    info!(results = results.len(), "Scan complete. Top candidates:");
    for (i, r) in results.iter().enumerate() {
        info!(
            rank = i + 1,
            symbol = %r.symbol,
            regime = %r.analysis.regime,
            score = format!("{:.1}", r.score),
            adx = format!("{:.1}", r.analysis.adx),
            rsi = format!("{:.1}", r.analysis.rsi),
            vol_pct = format!("{:.2}%", r.analysis.volatility_pct),
            turnover = format!("${:.0}", r.turnover_24h),
            "Candidate",
        );
    }

    Ok(results)
}

/// Format scan results as a human-readable report (for Telegram, TUI, etc.).
pub fn format_report(results: &[ScanResult]) -> String {
    if results.is_empty() {
        return "No suitable trading pairs found.".to_string();
    }

    let mut report = String::from("📊 *Market Scan Results*\n\n");
    for (i, r) in results.iter().enumerate() {
        let grid_icon = if r.analysis.grid_suitable {
            "✅"
        } else {
            "⚠️"
        };
        report.push_str(&format!(
            "{}. *{}* {} | Score: {:.0}\n   Regime: {} | ADX: {:.1} | RSI: {:.1}\n   Vol: {:.2}% | Spread: {:.3}%\n   Price: {:.4} | 24h: {:+.1}%\n",
            i + 1,
            r.symbol,
            grid_icon,
            r.score,
            r.analysis.regime,
            r.analysis.adx,
            r.analysis.rsi,
            r.analysis.volatility_pct,
            r.spread_pct,
            r.last_price,
            r.price_change_pct,
        ));
        if let Some((lower, upper)) = r.analysis.suggested_range {
            report.push_str(&format!(
                "   Suggested range: {:.4} - {:.4}\n",
                lower, upper
            ));
        }
        if let Some(levels) = r.analysis.suggested_levels {
            report.push_str(&format!("   Suggested levels: {}\n", levels));
        }
        report.push('\n');
    }
    report
}
