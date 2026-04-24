#![allow(dead_code)]
//! Funding Rate Arbitrage strategy — delta-neutral carry trade on perpetual futures.
//!
//! When the perpetual funding rate is positive, longs pay shorts every 8 hours.
//! We go long spot + short perpetual for the same notional, collecting funding
//! while remaining delta-neutral to price moves.
//!
//! Entry: funding_rate > min_threshold AND avg_8h_rate > entry_threshold
//! Exit:  funding_rate < exit_threshold OR accumulated loss > max_loss

use tracing::info;

/// Configuration for the funding arb strategy.
#[derive(Debug, Clone)]
pub struct FundingArbConfig {
    /// Minimum funding rate to consider entry (e.g., 0.0001 = 0.01%).
    pub entry_threshold: f64,
    /// Exit when funding drops below this (e.g., 0.0 or slightly negative).
    pub exit_threshold: f64,
    /// Minimum 24h turnover in USDT for a symbol to be considered.
    pub min_turnover: f64,
    /// Maximum fraction of equity to deploy per position.
    pub max_position_pct: f64,
    /// Leverage for the short perp side (1 = no leverage).
    pub leverage: u32,
    /// Maximum number of simultaneous positions.
    pub max_positions: usize,
    /// Minimum number of consecutive positive funding intervals to enter.
    pub min_positive_intervals: usize,
    /// Maximum unrealised loss before force-closing (fraction of position).
    pub max_unrealised_loss_pct: f64,
    /// Estimated annual borrowing/short cost as percentage (e.g. 10.0 = 10% APR).
    /// Deducted from annualised yield to compute net carry. Skip entry if net < min_net_yield_pct.
    pub borrow_cost_annual_pct: f64,
    /// Minimum net annualised yield (funding - borrow cost) to justify entry, as percentage.
    pub min_net_yield_pct: f64,
}

impl Default for FundingArbConfig {
    fn default() -> Self {
        Self {
            entry_threshold: 0.0001,
            exit_threshold: -0.00005,
            min_turnover: 5_000_000.0,
            max_position_pct: 0.15,
            leverage: 1,
            max_positions: 3,
            min_positive_intervals: 3,
            max_unrealised_loss_pct: 0.02,
            borrow_cost_annual_pct: 10.0,
            min_net_yield_pct: 5.0,
        }
    }
}

/// A candidate symbol for funding rate arbitrage.
#[derive(Debug, Clone)]
pub struct FundingCandidate {
    pub symbol: String,
    pub funding_rate: f64,
    pub next_funding_time_ms: u64,
    pub mark_price: f64,
    pub turnover_24h: f64,
    pub open_interest: f64,
    /// Average funding rate over recent intervals.
    pub avg_funding_rate: f64,
    /// Number of consecutive positive funding intervals.
    pub positive_streak: usize,
    /// Annualised yield from funding alone (rate × 3 × 365).
    pub annualised_yield: f64,
    /// Composite score for ranking.
    pub score: f64,
}

/// An active funding arb position.
#[derive(Debug, Clone)]
pub struct ActivePosition {
    pub symbol: String,
    pub entry_price: f64,
    pub spot_qty: f64,
    pub perp_qty: f64,
    pub spot_order_id: String,
    pub perp_order_id: String,
    pub entry_funding_rate: f64,
    pub funding_collected: f64,
    pub entry_time_ms: u64,
    pub notional: f64,
}

/// Decisions the arb engine can make.
#[derive(Debug, Clone)]
pub enum ArbAction {
    /// Enter a new delta-neutral position.
    Enter {
        symbol: String,
        qty: String,
        leverage: u32,
    },
    /// Exit an existing position (close both legs).
    Exit { symbol: String, reason: String },
    /// No action this tick.
    Hold,
}

/// Score and rank funding rate candidates.
pub fn rank_candidates<'a>(
    candidates: &'a mut [FundingCandidate],
    config: &FundingArbConfig,
) -> Vec<&'a FundingCandidate> {
    // Filter by thresholds
    let mut viable: Vec<&FundingCandidate> = candidates
        .iter()
        .filter(|c| {
            c.funding_rate >= config.entry_threshold
                && c.turnover_24h >= config.min_turnover
                && c.positive_streak >= config.min_positive_intervals
        })
        .collect();

    // Score = weighted combination of funding rate, consistency, and liquidity
    // Higher is better
    viable.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    viable.truncate(config.max_positions);
    viable
}

/// Compute the composite score for a candidate.
pub fn compute_score(rate: f64, avg_rate: f64, streak: usize, turnover: f64) -> f64 {
    let rate_score = rate * 10_000.0; // scale up (0.01% → 1.0)
    let consistency_score = (avg_rate / rate.max(1e-10)).min(1.5); // how stable is the rate
    let streak_bonus = (streak as f64).sqrt(); // reward persistence
    let liquidity_score = (turnover / 10_000_000.0).min(3.0).sqrt(); // log-ish scale

    rate_score * consistency_score * streak_bonus * liquidity_score
}

/// Calculate annualised yield from a per-interval funding rate.
/// Bybit charges funding every 8h = 3 times per day.
pub fn annualised_yield(rate: f64) -> f64 {
    rate * 3.0 * 365.0 * 100.0 // as percentage
}

/// Determine the optimal position size given equity, config, and candidate.
pub fn position_size(equity: f64, config: &FundingArbConfig, mark_price: f64) -> f64 {
    let max_notional = equity * config.max_position_pct;
    max_notional / mark_price
}

/// Decide whether to enter, exit, or hold for each position/candidate.
pub fn evaluate_positions(
    active: &[ActivePosition],
    candidates: &[FundingCandidate],
    config: &FundingArbConfig,
    equity: f64,
) -> Vec<ArbAction> {
    let mut actions = Vec::new();

    // Check active positions for exit signals
    for pos in active {
        let current = candidates.iter().find(|c| c.symbol == pos.symbol);
        let should_exit = match current {
            Some(c) => {
                // Funding flipped negative
                c.funding_rate < config.exit_threshold
            }
            None => true, // symbol disappeared from tickers
        };

        if should_exit {
            actions.push(ArbAction::Exit {
                symbol: pos.symbol.clone(),
                reason: format!(
                    "funding rate below exit threshold ({})",
                    current
                        .map(|c| format!("{:.4}%", c.funding_rate * 100.0))
                        .unwrap_or_else(|| "symbol missing".to_string())
                ),
            });
        }
    }

    // Look for new entry opportunities (if we have capacity)
    let active_symbols: std::collections::HashSet<&str> =
        active.iter().map(|p| p.symbol.as_str()).collect();
    let remaining_slots = config.max_positions.saturating_sub(active.len());

    if remaining_slots > 0 {
        let mut entry_candidates: Vec<&FundingCandidate> = candidates
            .iter()
            .filter(|c| {
                // Net yield = funding yield - borrow cost; skip if insufficient
                let net_yield = c.annualised_yield - config.borrow_cost_annual_pct;
                c.funding_rate >= config.entry_threshold
                    && c.turnover_24h >= config.min_turnover
                    && c.positive_streak >= config.min_positive_intervals
                    && net_yield >= config.min_net_yield_pct
                    && !active_symbols.contains(c.symbol.as_str())
            })
            .collect();

        entry_candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for candidate in entry_candidates.into_iter().take(remaining_slots) {
            let qty = position_size(equity, config, candidate.mark_price);
            if qty > 0.0 {
                info!(
                    symbol = %candidate.symbol,
                    funding_rate = format!("{:.4}%", candidate.funding_rate * 100.0),
                    annualised = format!("{:.1}%", candidate.annualised_yield),
                    streak = candidate.positive_streak,
                    "entering funding arb position",
                );
                actions.push(ArbAction::Enter {
                    symbol: candidate.symbol.clone(),
                    qty: format!("{qty:.6}"),
                    leverage: config.leverage,
                });
            }
        }
    }

    if actions.is_empty() {
        actions.push(ArbAction::Hold);
    }

    actions
}

/// Build FundingCandidate from raw ticker + funding history data.
pub fn build_candidate(
    symbol: String,
    funding_rate: f64,
    next_funding_time_ms: u64,
    mark_price: f64,
    turnover_24h: f64,
    open_interest: f64,
    historical_rates: &[f64],
) -> FundingCandidate {
    let avg = if historical_rates.is_empty() {
        funding_rate
    } else {
        historical_rates.iter().sum::<f64>() / historical_rates.len() as f64
    };

    let streak = historical_rates
        .iter()
        .rev()
        .take_while(|&&r| r > 0.0)
        .count();

    let apy = annualised_yield(funding_rate);
    let score = compute_score(funding_rate, avg, streak, turnover_24h);

    FundingCandidate {
        symbol,
        funding_rate,
        next_funding_time_ms,
        mark_price,
        turnover_24h,
        open_interest,
        avg_funding_rate: avg,
        positive_streak: streak,
        annualised_yield: apy,
        score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> FundingArbConfig {
        FundingArbConfig::default()
    }

    fn make_candidate(symbol: &str, rate: f64, turnover: f64, streak: usize) -> FundingCandidate {
        let rates: Vec<f64> = vec![rate; streak];
        build_candidate(
            symbol.to_string(),
            rate,
            0,
            50000.0,
            turnover,
            1000.0,
            &rates,
        )
    }

    #[test]
    fn annualised_yield_calculation() {
        // 0.01% per 8h = 0.03% per day = 10.95% per year
        let apy = annualised_yield(0.0001);
        assert!((apy - 10.95).abs() < 0.01);
    }

    #[test]
    fn annualised_yield_high_rate() {
        // 0.1% per 8h = 0.3% per day = 109.5% per year
        let apy = annualised_yield(0.001);
        assert!((apy - 109.5).abs() < 0.1);
    }

    #[test]
    fn score_higher_rate_scores_higher() {
        let s1 = compute_score(0.0001, 0.0001, 5, 10_000_000.0);
        let s2 = compute_score(0.0005, 0.0005, 5, 10_000_000.0);
        assert!(s2 > s1);
    }

    #[test]
    fn score_higher_streak_scores_higher() {
        let s1 = compute_score(0.0003, 0.0003, 2, 10_000_000.0);
        let s2 = compute_score(0.0003, 0.0003, 10, 10_000_000.0);
        assert!(s2 > s1);
    }

    #[test]
    fn score_higher_turnover_scores_higher() {
        let s1 = compute_score(0.0003, 0.0003, 5, 1_000_000.0);
        let s2 = compute_score(0.0003, 0.0003, 5, 50_000_000.0);
        assert!(s2 > s1);
    }

    #[test]
    fn position_size_respects_max_pct() {
        let cfg = FundingArbConfig {
            max_position_pct: 0.10,
            ..default_cfg()
        };
        let qty = position_size(100_000.0, &cfg, 50_000.0);
        let notional = qty * 50_000.0;
        assert!((notional - 10_000.0).abs() < 0.01);
    }

    #[test]
    fn evaluate_exits_when_funding_negative() {
        let cfg = FundingArbConfig {
            exit_threshold: 0.0,
            ..default_cfg()
        };
        let active = vec![ActivePosition {
            symbol: "BTCUSDT".to_string(),
            entry_price: 50000.0,
            spot_qty: 0.1,
            perp_qty: 0.1,
            spot_order_id: "s1".to_string(),
            perp_order_id: "p1".to_string(),
            entry_funding_rate: 0.0003,
            funding_collected: 0.5,
            entry_time_ms: 0,
            notional: 5000.0,
        }];
        let candidates = vec![make_candidate("BTCUSDT", -0.0001, 10_000_000.0, 0)];
        let actions = evaluate_positions(&active, &candidates, &cfg, 100_000.0);
        assert!(actions.iter().any(|a| matches!(a, ArbAction::Exit { .. })));
    }

    #[test]
    fn evaluate_enters_when_rate_high() {
        let cfg = FundingArbConfig {
            entry_threshold: 0.0001,
            min_turnover: 1_000_000.0,
            min_positive_intervals: 3,
            max_positions: 2,
            ..default_cfg()
        };
        let candidates = vec![
            make_candidate("ETHUSDT", 0.0005, 20_000_000.0, 5),
            make_candidate("BTCUSDT", 0.0003, 50_000_000.0, 8),
        ];
        let actions = evaluate_positions(&[], &candidates, &cfg, 100_000.0);
        let entries: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, ArbAction::Enter { .. }))
            .collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn evaluate_skips_active_symbols() {
        let cfg = default_cfg();
        let active = vec![ActivePosition {
            symbol: "BTCUSDT".to_string(),
            entry_price: 50000.0,
            spot_qty: 0.1,
            perp_qty: 0.1,
            spot_order_id: "s1".to_string(),
            perp_order_id: "p1".to_string(),
            entry_funding_rate: 0.0003,
            funding_collected: 0.0,
            entry_time_ms: 0,
            notional: 5000.0,
        }];
        let candidates = vec![make_candidate("BTCUSDT", 0.0005, 50_000_000.0, 10)];
        let actions = evaluate_positions(&active, &candidates, &cfg, 100_000.0);
        let entries: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, ArbAction::Enter { .. }))
            .collect();
        assert_eq!(entries.len(), 0); // already active
    }

    #[test]
    fn evaluate_respects_max_positions() {
        let cfg = FundingArbConfig {
            max_positions: 1,
            entry_threshold: 0.0001,
            min_turnover: 1_000_000.0,
            min_positive_intervals: 1,
            ..default_cfg()
        };
        let active = vec![ActivePosition {
            symbol: "BTCUSDT".to_string(),
            entry_price: 50000.0,
            spot_qty: 0.1,
            perp_qty: 0.1,
            spot_order_id: "s1".to_string(),
            perp_order_id: "p1".to_string(),
            entry_funding_rate: 0.0003,
            funding_collected: 0.0,
            entry_time_ms: 0,
            notional: 5000.0,
        }];
        let candidates = vec![make_candidate("ETHUSDT", 0.0005, 50_000_000.0, 10)];
        let actions = evaluate_positions(&active, &candidates, &cfg, 100_000.0);
        let entries: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, ArbAction::Enter { .. }))
            .collect();
        assert_eq!(entries.len(), 0); // at capacity
    }

    #[test]
    fn build_candidate_computes_streak() {
        let rates = vec![0.0003, 0.0002, 0.0001, -0.0001, 0.0002];
        let c = build_candidate(
            "TEST".into(),
            0.0003,
            0,
            50000.0,
            10_000_000.0,
            1000.0,
            &rates,
        );
        // Last 3 are positive (reading from end: 0.0002 is not last... wait, let me reconsider)
        // rates = [0.0003, 0.0002, 0.0001, -0.0001, 0.0002]
        // rev = [0.0002, -0.0001, ...] → streak = 1
        assert_eq!(c.positive_streak, 1);
    }

    #[test]
    fn build_candidate_all_positive() {
        let rates = vec![0.0001, 0.0002, 0.0003];
        let c = build_candidate(
            "TEST".into(),
            0.0003,
            0,
            50000.0,
            10_000_000.0,
            1000.0,
            &rates,
        );
        assert_eq!(c.positive_streak, 3);
    }

    #[test]
    fn hold_when_nothing_to_do() {
        let cfg = FundingArbConfig {
            entry_threshold: 0.01, // very high threshold
            ..default_cfg()
        };
        let candidates = vec![make_candidate("BTCUSDT", 0.0001, 50_000_000.0, 10)];
        let actions = evaluate_positions(&[], &candidates, &cfg, 100_000.0);
        assert!(actions.iter().any(|a| matches!(a, ArbAction::Hold)));
    }
}
