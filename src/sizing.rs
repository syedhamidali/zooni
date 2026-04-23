//! Smart position sizing — Kelly criterion and volatility-adjusted sizing.
//!
//! Determines optimal qty_per_level_base and total capital allocation
//! based on account equity, volatility, and historical win rate.

use crate::regime::RegimeAnalysis;

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
/// We use fractional Kelly (typically 0.25-0.5) to be conservative.
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

/// Calculate recommended position sizing for a grid strategy.
///
/// Inputs:
/// - `equity`: total account equity in quote currency
/// - `analysis`: regime analysis from the scanner
/// - `grid_spacing_pct`: grid level spacing as percentage of price
/// - `num_levels`: number of grid levels
/// - `fee_rate`: trading fee rate (e.g., 0.001 for 0.1%)
/// - `historical_win_rate`: if available, historical round-trip completion rate
pub fn recommend(
    equity: f64,
    analysis: &RegimeAnalysis,
    grid_spacing_pct: f64,
    num_levels: u32,
    fee_rate: f64,
    historical_win_rate: Option<f64>,
) -> SizingRecommendation {
    // Base allocation: what fraction of equity to put into this grid
    let mut base_fraction = match analysis.regime {
        crate::regime::Regime::Ranging => 0.30,      // ideal conditions
        crate::regime::Regime::TrendingUp => 0.15,    // can work but risky
        crate::regime::Regime::TrendingDown => 0.10,  // dangerous, minimal
        crate::regime::Regime::Volatile => 0.05,      // stay mostly out
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
            // Use half-Kelly for safety
            let half_kelly = kelly * 0.5;
            // Blend with regime-based fraction
            base_fraction = base_fraction * 0.6 + half_kelly * 0.4;
        }
    }

    // Volatility adjustment — reduce size in high volatility
    if analysis.volatility_pct > 3.0 {
        base_fraction *= 0.5;
    } else if analysis.volatility_pct > 2.0 {
        base_fraction *= 0.75;
    }

    // Hard caps
    base_fraction = base_fraction.clamp(0.02, 0.40);

    let total_allocation = equity * base_fraction;

    // qty per level = total allocation / num_levels / current_price
    // But we don't have price here, so return in quote terms
    let quote_per_level = total_allocation / num_levels as f64;

    let reason = format!(
        "{}% of equity ({:.2} USDT). Regime: {} (conf: {:.0}%). Vol: {:.2}%/day.",
        (base_fraction * 100.0) as u32,
        total_allocation,
        analysis.regime,
        analysis.confidence * 100.0,
        analysis.volatility_pct,
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
}
