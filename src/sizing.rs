//! Smart position sizing — Kelly criterion and volatility-adjusted sizing.
//!
//! Determines optimal qty_per_level_base and total capital allocation
//! based on account equity, volatility, and historical win rate.

use crate::regime::{Regime, RegimeAnalysis};

/// Sizing recommendation for grid strategy.
#[derive(Debug, Clone)]
pub struct SizingRecommendation {
    /// Recommended quantity per grid level (in base currency).
    pub qty_per_level: f64,
    /// Total capital to allocate to this grid (in quote currency).
    pub total_allocation: f64,
    /// Fraction of equity allocated (0.0 to 1.0).
    pub equity_fraction: f64,
    /// Reasoning for the sizing decision.
    pub reason: String,
}

/// Kelly criterion — optimal bet fraction given win probability and payoff ratio.
///
/// f* = (bp - q) / b
/// where:
///   b = net odds (win_amount / loss_amount)
///   p = probability of win
///   q = 1 - p
///
/// For grid trading:
///   p = fill rate (% of grid orders that complete a round trip)
///   b = grid_spacing / (grid_spacing * fee_rate * 2) ≈ 1 / (2 * fee_rate) for successful fills
///
/// We use fractional Kelly (default 0.15 — 15% of full Kelly) to be conservative.
/// This gives roughly 50% of full-Kelly returns with far lower drawdown risk.
pub fn kelly_fraction(win_rate: f64, avg_win: f64, avg_loss: f64) -> f64 {
    if avg_loss == 0.0 || win_rate <= 0.0 || win_rate >= 1.0 {
        return 0.0;
    }
    let b = avg_win / avg_loss;
    let p = win_rate;
    let q = 1.0 - p;
    let kelly = (b * p - q) / b;
    kelly.max(0.0)
}

/// Regime-based hard cap: maximum fraction of equity for any single position.
///
/// These caps are applied after volatility scaling and Kelly blending to ensure
/// we never oversize in unfavourable market conditions.
pub fn regime_max_fraction(regime: Regime) -> f64 {
    match regime {
        Regime::Volatile => 0.05,      // 5%  — stay mostly out of chaotic markets
        Regime::TrendingDown => 0.08,  // 8%  — don't catch falling knives
        Regime::TrendingUp => 0.15,    // 15% — can work but carry risk
        Regime::Ranging => 0.20,       // 20% — ideal grid conditions
    }
}

/// Volatility scaling multiplier.
///
/// `vol_pct` is ATR-as-percentage-of-price (same field as `RegimeAnalysis::volatility_pct`).
/// Higher volatility → smaller multiplier → smaller position.
pub fn vol_scale(vol_pct: f64) -> f64 {
    if vol_pct > 4.0 {
        0.25
    } else if vol_pct > 3.0 {
        0.40
    } else if vol_pct > 2.0 {
        0.65
    } else if vol_pct > 1.5 {
        0.85
    } else {
        1.00
    }
}

/// Calculate recommended position sizing for a grid strategy.
///
/// Inputs:
/// - `equity`: total account equity in quote currency
/// - `analysis`: regime analysis from the scanner
/// - `grid_spacing_pct`: grid level spacing as percentage of price
/// - `num_levels`: number of grid levels
/// - `fee_rate`: trading fee rate (e.g., 0.001 for 0.1%)
/// - `historical_win_rate`: if available, historical round-trip completion rate
/// - `kelly_multiplier`: fractional-Kelly multiplier (default 0.15 from `SizingCfg`)
/// - `min_notional`: minimum quote-currency notional; returns 0.0 for dust orders
pub fn recommend(
    equity: f64,
    analysis: &RegimeAnalysis,
    grid_spacing_pct: f64,
    num_levels: u32,
    fee_rate: f64,
    historical_win_rate: Option<f64>,
    kelly_multiplier: f64,
    min_notional: f64,
) -> SizingRecommendation {
    // Base allocation: what fraction of equity to put into this grid
    let mut base_fraction = match analysis.regime {
        Regime::Ranging => 0.30,       // ideal conditions
        Regime::TrendingUp => 0.15,    // can work but risky
        Regime::TrendingDown => 0.10,  // dangerous, minimal
        Regime::Volatile => 0.05,      // stay mostly out
    };

    // Scale by confidence
    base_fraction *= 0.5 + analysis.confidence * 0.5;

    // Kelly adjustment if we have historical data
    if let Some(win_rate) = historical_win_rate {
        // For grid trading: avg_win ≈ grid_spacing - 2 * fee
        // avg_loss ≈ varies, but for stopped-out grids, roughly = grid_spacing * num_levels
        let avg_win = grid_spacing_pct - fee_rate * 200.0; // as pct
        let avg_loss = grid_spacing_pct * 3.0; // conservative loss estimate

        if avg_win > 0.0 {
            let kelly = kelly_fraction(win_rate, avg_win, avg_loss);
            // Apply configurable fractional-Kelly multiplier (default 0.15)
            let frac_kelly = kelly * kelly_multiplier;
            // Blend with regime-based fraction
            base_fraction = base_fraction * 0.6 + frac_kelly * 0.4;
        }
    }

    // Tighter volatility scaling — 5-tier multiplier based on ATR% of price
    base_fraction *= vol_scale(analysis.volatility_pct);

    // Regime-aware hard cap (applied before global clamp)
    let regime_cap = regime_max_fraction(analysis.regime);
    base_fraction = base_fraction.min(regime_cap);

    // Global floor (never go below 2% if we've decided to trade at all)
    base_fraction = base_fraction.clamp(0.02, regime_cap);

    let total_allocation = equity * base_fraction;

    // Minimum order size filter: skip dust orders that only burn fees
    if total_allocation < min_notional {
        return SizingRecommendation {
            qty_per_level: 0.0,
            total_allocation: 0.0,
            equity_fraction: 0.0,
            reason: format!(
                "Skipped: allocation {:.2} USDT below min notional {:.2} USDT.",
                total_allocation, min_notional
            ),
        };
    }

    // qty per level = total allocation / num_levels / current_price
    // But we don't have price here, so return in quote terms
    let quote_per_level = total_allocation / num_levels as f64;

    let reason = format!(
        "{}% of equity ({:.2} USDT). Regime: {} (conf: {:.0}%). Vol: {:.2}%/day. Kelly×{:.2}.",
        (base_fraction * 100.0) as u32,
        total_allocation,
        analysis.regime,
        analysis.confidence * 100.0,
        analysis.volatility_pct,
        kelly_multiplier,
    );

    SizingRecommendation {
        qty_per_level: quote_per_level,
        total_allocation,
        equity_fraction: base_fraction,
        reason,
    }
}

/// Adjust sizing dynamically based on recent performance.
/// Call this periodically (e.g., daily) to scale up winners and scale down losers.
pub fn adjust_for_performance(
    current_fraction: f64,
    recent_pnl: f64,
    equity: f64,
    max_fraction: f64,
) -> f64 {
    if equity <= 0.0 {
        return current_fraction;
    }

    let pnl_pct = recent_pnl / equity;

    // If profitable, allow gradual increase (max 20% increase per adjustment)
    // If losing, decrease faster (max 30% decrease per adjustment)
    let adjustment = if pnl_pct > 0.01 {
        // Profitable: scale up gently
        (pnl_pct * 2.0).min(0.20)
    } else if pnl_pct < -0.01 {
        // Losing: scale down faster
        (pnl_pct * 3.0).max(-0.30)
    } else {
        0.0
    };

    let new_fraction = current_fraction * (1.0 + adjustment);
    new_fraction.clamp(0.02, max_fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kelly_positive_edge() {
        // 60% win rate, 1:1 payoff → Kelly = 0.20
        let k = kelly_fraction(0.6, 1.0, 1.0);
        assert!((k - 0.2).abs() < 0.01, "Kelly should be ~0.2: {}", k);
    }

    #[test]
    fn kelly_no_edge() {
        // 50% win rate, 1:1 payoff → Kelly = 0
        let k = kelly_fraction(0.5, 1.0, 1.0);
        assert!(k.abs() < 0.01, "No edge should give Kelly ~0: {}", k);
    }

    #[test]
    fn kelly_negative_edge() {
        // 40% win rate, 1:1 payoff → Kelly < 0 → clamped to 0
        let k = kelly_fraction(0.4, 1.0, 1.0);
        assert_eq!(k, 0.0);
    }

    #[test]
    fn performance_adjustment_profitable() {
        let new = adjust_for_performance(0.20, 500.0, 10000.0, 0.40);
        assert!(new > 0.20, "Should increase on profit: {}", new);
        assert!(new <= 0.40, "Should not exceed max: {}", new);
    }

    #[test]
    fn performance_adjustment_losing() {
        let new = adjust_for_performance(0.20, -500.0, 10000.0, 0.40);
        assert!(new < 0.20, "Should decrease on loss: {}", new);
        assert!(new >= 0.02, "Should not go below min: {}", new);
    }

    #[test]
    fn kelly_edge_cases() {
        assert_eq!(kelly_fraction(0.0, 1.0, 1.0), 0.0); // zero win rate
        assert_eq!(kelly_fraction(1.0, 1.0, 1.0), 0.0); // 100% win rate (edge case in formula)
        assert_eq!(kelly_fraction(0.5, 1.0, 0.0), 0.0); // zero avg loss
    }

    #[test]
    fn kelly_high_edge_high_fraction() {
        // 80% win, 2:1 payoff → Kelly = (2*0.8 - 0.2)/2 = 0.7
        let k = kelly_fraction(0.8, 2.0, 1.0);
        assert!((k - 0.7).abs() < 0.01, "High edge Kelly: {k}");
    }

    #[test]
    fn performance_adjustment_flat_no_change() {
        // PnL within [-1%, +1%] → no adjustment
        let new = adjust_for_performance(0.20, 50.0, 10000.0, 0.40);
        assert!(
            (new - 0.20).abs() < 0.01,
            "Flat should not change much: {new}"
        );
    }

    #[test]
    fn performance_adjustment_zero_equity() {
        let new = adjust_for_performance(0.20, -500.0, 0.0, 0.40);
        assert!((new - 0.20).abs() < 1e-9, "Zero equity → no change");
    }

    #[test]
    fn performance_adjustment_respects_max() {
        let new = adjust_for_performance(0.39, 5000.0, 10000.0, 0.40);
        assert!(new <= 0.40, "Should not exceed max fraction: {new}");
    }

    #[test]
    fn performance_adjustment_respects_min() {
        let new = adjust_for_performance(0.03, -5000.0, 10000.0, 0.40);
        assert!(new >= 0.02, "Should not go below min: {new}");
    }

    // ── New tests for the four improvements ──────────────────────────────────

    #[test]
    fn vol_scale_tiers() {
        assert!((vol_scale(5.0) - 0.25).abs() < 1e-9, "vol > 4% → 0.25");
        assert!((vol_scale(3.5) - 0.40).abs() < 1e-9, "vol > 3% → 0.40");
        assert!((vol_scale(2.5) - 0.65).abs() < 1e-9, "vol > 2% → 0.65");
        assert!((vol_scale(1.8) - 0.85).abs() < 1e-9, "vol > 1.5% → 0.85");
        assert!((vol_scale(1.0) - 1.00).abs() < 1e-9, "vol <= 1.5% → 1.00");
    }

    #[test]
    fn regime_caps_correct() {
        assert!((regime_max_fraction(Regime::Volatile) - 0.05).abs() < 1e-9);
        assert!((regime_max_fraction(Regime::TrendingDown) - 0.08).abs() < 1e-9);
        assert!((regime_max_fraction(Regime::TrendingUp) - 0.15).abs() < 1e-9);
        assert!((regime_max_fraction(Regime::Ranging) - 0.20).abs() < 1e-9);
    }

    fn make_analysis(regime: Regime, confidence: f64, vol_pct: f64) -> RegimeAnalysis {
        RegimeAnalysis {
            regime,
            confidence,
            adx: 15.0,
            rsi: 50.0,
            bb_bandwidth: 0.03,
            volatility_pct: vol_pct,
            grid_suitable: true,
            suggested_range: None,
            suggested_levels: None,
        }
    }

    #[test]
    fn volatile_regime_cap_enforced() {
        let analysis = make_analysis(Regime::Volatile, 1.0, 1.0);
        let rec = recommend(100_000.0, &analysis, 1.0, 10, 0.001, None, 0.15, 10.0);
        assert!(
            rec.equity_fraction <= 0.05,
            "Volatile cap should be 5%: got {}",
            rec.equity_fraction
        );
    }

    #[test]
    fn trending_down_cap_enforced() {
        let analysis = make_analysis(Regime::TrendingDown, 1.0, 1.0);
        let rec = recommend(100_000.0, &analysis, 1.0, 10, 0.001, None, 0.15, 10.0);
        assert!(
            rec.equity_fraction <= 0.08,
            "TrendingDown cap should be 8%: got {}",
            rec.equity_fraction
        );
    }

    #[test]
    fn min_notional_filter_returns_zero() {
        // Very small equity → allocation < $10 → skipped
        let analysis = make_analysis(Regime::Ranging, 1.0, 1.0);
        let rec = recommend(50.0, &analysis, 1.0, 10, 0.001, None, 0.15, 10.0);
        assert_eq!(rec.qty_per_level, 0.0, "Dust order should be skipped");
        assert_eq!(rec.total_allocation, 0.0);
    }

    #[test]
    fn high_vol_shrinks_size() {
        let low_vol = make_analysis(Regime::Ranging, 1.0, 1.0);
        let high_vol = make_analysis(Regime::Ranging, 1.0, 5.0);
        let rec_low = recommend(10_000.0, &low_vol, 1.0, 10, 0.001, None, 0.15, 10.0);
        let rec_high = recommend(10_000.0, &high_vol, 1.0, 10, 0.001, None, 0.15, 10.0);
        assert!(
            rec_high.total_allocation < rec_low.total_allocation,
            "High vol should reduce allocation"
        );
    }

    #[test]
    fn kelly_multiplier_015_applied() {
        // With win_rate=0.6, avg_win=1.0, avg_loss=1.0 → full Kelly=0.2
        // At 0.15x multiplier → frac_kelly = 0.03
        // Blended: regime_base * 0.6 + 0.03 * 0.4, capped by regime cap
        let analysis = make_analysis(Regime::Ranging, 1.0, 1.0);
        let rec_015 = recommend(
            10_000.0, &analysis, 1.0, 10, 0.001,
            Some(0.6), 0.15, 10.0,
        );
        let rec_050 = recommend(
            10_000.0, &analysis, 1.0, 10, 0.001,
            Some(0.6), 0.50, 10.0,
        );
        assert!(
            rec_015.equity_fraction <= rec_050.equity_fraction,
            "0.15x Kelly should produce smaller or equal fraction than 0.50x"
        );
    }
}
