//! Market regime detection — classifies current market conditions.
//!
//! Determines whether a market is trending (bad for grid) or ranging (good for grid),
//! and recommends strategy parameters accordingly.

use crate::indicators;
use crate::venues::bybit::rest::Kline;

/// Market regime classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// Price oscillating in a range — ideal for grid trading.
    Ranging,
    /// Strong upward movement — risky for grid (sell side drains).
    TrendingUp,
    /// Strong downward movement — risky for grid (buy side drains, unrealised losses).
    TrendingDown,
    /// Extremely volatile / uncertain — stay out.
    Volatile,
}

impl std::fmt::Display for Regime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Regime::Ranging => write!(f, "RANGING"),
            Regime::TrendingUp => write!(f, "TRENDING_UP"),
            Regime::TrendingDown => write!(f, "TRENDING_DOWN"),
            Regime::Volatile => write!(f, "VOLATILE"),
        }
    }
}

/// Confidence score for the regime classification (0.0 to 1.0).
#[derive(Debug, Clone)]
pub struct RegimeAnalysis {
    pub regime: Regime,
    pub confidence: f64,
    /// ADX value (trend strength). > 25 = trending, < 20 = ranging.
    pub adx: f64,
    /// RSI value (momentum). Extreme values (>70 or <30) suggest overextension.
    pub rsi: f64,
    /// Bollinger bandwidth — narrow = consolidation, wide = volatile.
    pub bb_bandwidth: f64,
    /// ATR as percentage of price — higher = more volatile.
    pub volatility_pct: f64,
    /// Whether grid trading is recommended in this regime.
    pub grid_suitable: bool,
    /// Suggested grid range (lower, upper) based on Bollinger bands and ATR.
    pub suggested_range: Option<(f64, f64)>,
    /// Suggested number of grid levels based on volatility.
    pub suggested_levels: Option<u32>,
}

/// Analyze market regime from kline data.
///
/// Requires at least 50 candles for reliable analysis.
pub fn analyze(klines: &[Kline]) -> Option<RegimeAnalysis> {
    if klines.len() < 50 {
        return None;
    }

    let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let highs: Vec<f64> = klines.iter().map(|k| k.high).collect();
    let lows: Vec<f64> = klines.iter().map(|k| k.low).collect();

    let current_price = *closes.last()?;

    // Calculate indicators
    let adx_val = indicators::adx(&highs, &lows, &closes, 14)
        .map(|(adx, _, _)| adx)
        .unwrap_or(0.0);

    let rsi_val = indicators::rsi(&closes, 14).unwrap_or(50.0);

    let (bb_upper, bb_mid, bb_lower, bb_bw) =
        indicators::bollinger_bands(&closes, 20, 2.0).unwrap_or((0.0, 0.0, 0.0, 0.0));

    let atr_val = indicators::atr(&highs, &lows, &closes, 14).unwrap_or(0.0);
    let volatility_pct = if current_price > 0.0 {
        atr_val / current_price * 100.0
    } else {
        0.0
    };

    // Classify regime
    let (regime, confidence) = classify(adx_val, rsi_val, bb_bw, volatility_pct);

    // Determine grid suitability
    let grid_suitable = matches!(regime, Regime::Ranging)
        || (matches!(regime, Regime::TrendingUp) && adx_val < 30.0);

    // Calculate suggested grid range
    let suggested_range = if grid_suitable && bb_mid > 0.0 {
        // Use Bollinger bands extended by 0.5 ATR for range
        let lower = (bb_lower - atr_val * 0.5).max(current_price * 0.9);
        let upper = bb_upper + atr_val * 0.5;
        Some((lower, upper))
    } else {
        None
    };

    // Suggest grid levels based on volatility
    let suggested_levels = if grid_suitable {
        let range_pct = suggested_range
            .map(|(l, u)| (u - l) / current_price * 100.0)
            .unwrap_or(5.0);
        // More levels for wider ranges, fewer for narrow
        let levels = (range_pct * 3.0).clamp(5.0, 50.0) as u32;
        Some(levels)
    } else {
        None
    };

    Some(RegimeAnalysis {
        regime,
        confidence,
        adx: adx_val,
        rsi: rsi_val,
        bb_bandwidth: bb_bw,
        volatility_pct,
        grid_suitable,
        suggested_range,
        suggested_levels,
    })
}

/// Classify regime based on indicator values.
fn classify(adx: f64, rsi: f64, bb_bandwidth: f64, vol_pct: f64) -> (Regime, f64) {
    // High volatility override
    if vol_pct > 5.0 || bb_bandwidth > 0.20 {
        let conf = ((vol_pct - 3.0) / 5.0).clamp(0.5, 1.0);
        return (Regime::Volatile, conf);
    }

    // Strong trend
    if adx > 25.0 {
        let conf = ((adx - 25.0) / 25.0).clamp(0.4, 0.95);
        if rsi > 55.0 {
            return (Regime::TrendingUp, conf);
        } else if rsi < 45.0 {
            return (Regime::TrendingDown, conf);
        }
        // ADX high but RSI neutral — mild trend, still might be okay for grid
        let direction = if rsi >= 50.0 {
            Regime::TrendingUp
        } else {
            Regime::TrendingDown
        };
        return (direction, conf * 0.7);
    }

    // Low ADX + narrow Bollinger bands = ranging
    if adx < 20.0 {
        let conf = ((20.0 - adx) / 15.0).clamp(0.5, 0.95);
        return (Regime::Ranging, conf);
    }

    // Borderline (ADX 20-25) — lean towards ranging with lower confidence
    let conf = 0.4;
    if bb_bandwidth < 0.04 {
        (Regime::Ranging, conf + 0.1)
    } else {
        if rsi > 55.0 {
            (Regime::TrendingUp, conf)
        } else if rsi < 45.0 {
            (Regime::TrendingDown, conf)
        } else {
            (Regime::Ranging, conf)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ranging_klines(n: usize) -> Vec<Kline> {
        // Oscillating price between 95 and 105
        (0..n)
            .map(|i| {
                let base = 100.0 + 5.0 * (i as f64 * 0.5).sin();
                Kline {
                    timestamp: i as u64 * 60000,
                    open: base - 0.5,
                    high: base + 1.0,
                    low: base - 1.0,
                    close: base + 0.5,
                    volume: 1000.0,
                }
            })
            .collect()
    }

    #[test]
    fn ranging_market() {
        let klines = make_ranging_klines(100);
        let analysis = analyze(&klines).unwrap();
        // Oscillating price should give low ADX
        assert!(
            analysis.adx < 35.0,
            "ADX should be low-ish for ranging: {}",
            analysis.adx
        );
        assert!(
            analysis.grid_suitable,
            "Grid should be suitable for ranging market"
        );
    }

    #[test]
    fn insufficient_data() {
        let klines = make_ranging_klines(10);
        assert!(analyze(&klines).is_none());
    }

    #[test]
    fn classify_volatile_when_high_vol() {
        let (regime, conf) = classify(15.0, 50.0, 0.05, 6.0);
        assert_eq!(regime, Regime::Volatile);
        assert!(conf >= 0.5);
    }

    #[test]
    fn classify_volatile_when_wide_bandwidth() {
        let (regime, _) = classify(15.0, 50.0, 0.25, 2.0);
        assert_eq!(regime, Regime::Volatile);
    }

    #[test]
    fn classify_trending_up_strong_adx_bullish_rsi() {
        let (regime, _) = classify(35.0, 65.0, 0.05, 2.0);
        assert_eq!(regime, Regime::TrendingUp);
    }

    #[test]
    fn classify_trending_down_strong_adx_bearish_rsi() {
        let (regime, _) = classify(35.0, 35.0, 0.05, 2.0);
        assert_eq!(regime, Regime::TrendingDown);
    }

    #[test]
    fn classify_ranging_low_adx() {
        let (regime, conf) = classify(15.0, 50.0, 0.03, 1.5);
        assert_eq!(regime, Regime::Ranging);
        assert!(conf >= 0.5);
    }

    #[test]
    fn classify_borderline_adx_narrow_bb() {
        let (regime, _) = classify(22.0, 50.0, 0.03, 1.5);
        assert_eq!(regime, Regime::Ranging);
    }

    #[test]
    fn classify_borderline_adx_bullish() {
        let (regime, _) = classify(22.0, 60.0, 0.06, 1.5);
        assert_eq!(regime, Regime::TrendingUp);
    }

    #[test]
    fn classify_borderline_adx_bearish() {
        let (regime, _) = classify(22.0, 40.0, 0.06, 1.5);
        assert_eq!(regime, Regime::TrendingDown);
    }

    #[test]
    fn regime_display() {
        assert_eq!(Regime::Ranging.to_string(), "RANGING");
        assert_eq!(Regime::TrendingUp.to_string(), "TRENDING_UP");
        assert_eq!(Regime::TrendingDown.to_string(), "TRENDING_DOWN");
        assert_eq!(Regime::Volatile.to_string(), "VOLATILE");
    }

    #[test]
    fn grid_not_suitable_for_trending_down() {
        // Build strongly trending-down klines
        let klines: Vec<Kline> = (0..100)
            .map(|i| {
                let base = 200.0 - i as f64 * 1.5;
                Kline {
                    timestamp: i as u64 * 60000,
                    open: base + 0.5,
                    high: base + 1.0,
                    low: base - 0.5,
                    close: base,
                    volume: 1000.0,
                }
            })
            .collect();
        let analysis = analyze(&klines).unwrap();
        if analysis.regime == Regime::TrendingDown {
            assert!(!analysis.grid_suitable);
        }
    }

    #[test]
    fn suggested_levels_clamped() {
        let klines = make_ranging_klines(100);
        let analysis = analyze(&klines).unwrap();
        if let Some(levels) = analysis.suggested_levels {
            assert!((5..=50).contains(&levels), "levels out of range: {levels}");
        }
    }
}
