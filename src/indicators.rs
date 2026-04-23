//! Technical indicators for market analysis.
//!
//! All indicators operate on slices of f64 (typically close prices or OHLCV data).
//! They return None when insufficient data is available.

/// Simple Moving Average over `period` values.
pub fn sma(data: &[f64], period: usize) -> Option<f64> {
    if data.len() < period || period == 0 {
        return None;
    }
    let sum: f64 = data[data.len() - period..].iter().sum();
    Some(sum / period as f64)
}

/// Exponential Moving Average. Returns the full EMA series for the given data.
pub fn ema_series(data: &[f64], period: usize) -> Vec<f64> {
    if data.is_empty() || period == 0 {
        return vec![];
    }
    let k = 2.0 / (period as f64 + 1.0);
    let mut result = Vec::with_capacity(data.len());
    result.push(data[0]);
    for i in 1..data.len() {
        let prev = result[i - 1];
        result.push(data[i] * k + prev * (1.0 - k));
    }
    result
}

/// Relative Strength Index (RSI) over `period` bars.
/// Returns a value between 0 and 100.
pub fn rsi(closes: &[f64], period: usize) -> Option<f64> {
    if closes.len() < period + 1 || period == 0 {
        return None;
    }

    let mut avg_gain = 0.0;
    let mut avg_loss = 0.0;

    // Initial averages
    for i in 1..=period {
        let change =
            closes[closes.len() - period - 1 + i] - closes[closes.len() - period - 1 + i - 1];
        if change > 0.0 {
            avg_gain += change;
        } else {
            avg_loss += change.abs();
        }
    }
    avg_gain /= period as f64;
    avg_loss /= period as f64;

    if avg_loss == 0.0 {
        return Some(100.0);
    }

    let rs = avg_gain / avg_loss;
    Some(100.0 - 100.0 / (1.0 + rs))
}

/// Average True Range (ATR) — measures volatility.
/// Requires high, low, close arrays of equal length.
pub fn atr(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Option<f64> {
    let n = highs.len().min(lows.len()).min(closes.len());
    if n < period + 1 || period == 0 {
        return None;
    }

    // Calculate true ranges
    let mut trs = Vec::with_capacity(n - 1);
    for i in 1..n {
        let hl = highs[i] - lows[i];
        let hc = (highs[i] - closes[i - 1]).abs();
        let lc = (lows[i] - closes[i - 1]).abs();
        trs.push(hl.max(hc).max(lc));
    }

    // Simple average of last `period` true ranges
    if trs.len() < period {
        return None;
    }
    let sum: f64 = trs[trs.len() - period..].iter().sum();
    Some(sum / period as f64)
}

/// Average Directional Index (ADX) — measures trend strength.
/// Returns (adx, +DI, -DI). ADX > 25 suggests trending, < 20 suggests ranging.
pub fn adx(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Option<(f64, f64, f64)> {
    let n = highs.len().min(lows.len()).min(closes.len());
    if n < period * 2 + 1 || period == 0 {
        return None;
    }

    let mut plus_dm = Vec::with_capacity(n - 1);
    let mut minus_dm = Vec::with_capacity(n - 1);
    let mut tr_vals = Vec::with_capacity(n - 1);

    for i in 1..n {
        let up = highs[i] - highs[i - 1];
        let down = lows[i - 1] - lows[i];

        if up > down && up > 0.0 {
            plus_dm.push(up);
        } else {
            plus_dm.push(0.0);
        }
        if down > up && down > 0.0 {
            minus_dm.push(down);
        } else {
            minus_dm.push(0.0);
        }

        let hl = highs[i] - lows[i];
        let hc = (highs[i] - closes[i - 1]).abs();
        let lc = (lows[i] - closes[i - 1]).abs();
        tr_vals.push(hl.max(hc).max(lc));
    }

    // Smooth with Wilder's method (EMA with alpha = 1/period)
    let smooth = |data: &[f64], p: usize| -> Vec<f64> {
        let mut result = Vec::with_capacity(data.len());
        let init: f64 = data[..p].iter().sum();
        result.push(init);
        for val in &data[p..] {
            let prev = result[result.len() - 1];
            result.push(prev - prev / p as f64 + val);
        }
        result
    };

    let smooth_tr = smooth(&tr_vals, period);
    let smooth_plus = smooth(&plus_dm, period);
    let smooth_minus = smooth(&minus_dm, period);

    let len = smooth_tr.len();
    if len == 0 {
        return None;
    }

    // Calculate DI values and DX
    let mut dx_vals = Vec::with_capacity(len);
    for i in 0..len {
        if smooth_tr[i] == 0.0 {
            dx_vals.push(0.0);
            continue;
        }
        let plus_di = 100.0 * smooth_plus[i] / smooth_tr[i];
        let minus_di = 100.0 * smooth_minus[i] / smooth_tr[i];
        let sum = plus_di + minus_di;
        if sum == 0.0 {
            dx_vals.push(0.0);
        } else {
            dx_vals.push(100.0 * (plus_di - minus_di).abs() / sum);
        }
    }

    if dx_vals.len() < period {
        return None;
    }

    // ADX = smoothed DX
    let adx_init: f64 = dx_vals[..period].iter().sum::<f64>() / period as f64;
    let mut adx_val = adx_init;
    for val in &dx_vals[period..] {
        adx_val = (adx_val * (period as f64 - 1.0) + val) / period as f64;
    }

    // Final +DI / -DI
    let last = len - 1;
    let plus_di = if smooth_tr[last] > 0.0 {
        100.0 * smooth_plus[last] / smooth_tr[last]
    } else {
        0.0
    };
    let minus_di = if smooth_tr[last] > 0.0 {
        100.0 * smooth_minus[last] / smooth_tr[last]
    } else {
        0.0
    };

    Some((adx_val, plus_di, minus_di))
}

/// Bollinger Bands — returns (upper, middle, lower, bandwidth).
/// Bandwidth = (upper - lower) / middle — narrow bands suggest consolidation.
pub fn bollinger_bands(
    closes: &[f64],
    period: usize,
    num_std: f64,
) -> Option<(f64, f64, f64, f64)> {
    if closes.len() < period || period == 0 {
        return None;
    }
    let window = &closes[closes.len() - period..];
    let mean = window.iter().sum::<f64>() / period as f64;
    let variance = window.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / period as f64;
    let std_dev = variance.sqrt();

    let upper = mean + num_std * std_dev;
    let lower = mean - num_std * std_dev;
    let bandwidth = if mean > 0.0 {
        (upper - lower) / mean
    } else {
        0.0
    };

    Some((upper, mean, lower, bandwidth))
}

/// Volume-Weighted Average Price (VWAP) over the given window.
pub fn vwap(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[f64]) -> Option<f64> {
    let n = highs
        .len()
        .min(lows.len())
        .min(closes.len())
        .min(volumes.len());
    if n == 0 {
        return None;
    }
    let mut cum_tp_vol = 0.0;
    let mut cum_vol = 0.0;
    for i in 0..n {
        let typical_price = (highs[i] + lows[i] + closes[i]) / 3.0;
        cum_tp_vol += typical_price * volumes[i];
        cum_vol += volumes[i];
    }
    if cum_vol == 0.0 {
        return None;
    }
    Some(cum_tp_vol / cum_vol)
}

/// Standard deviation of a slice.
pub fn std_dev(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mean = data.iter().sum::<f64>() / data.len() as f64;
    let variance = data.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / data.len() as f64;
    variance.sqrt()
}

/// Returns (mean, std_dev) for log returns — used for Sharpe ratio and volatility estimation.
pub fn log_return_stats(closes: &[f64]) -> Option<(f64, f64)> {
    if closes.len() < 2 {
        return None;
    }
    let returns: Vec<f64> = closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
    Some((mean, var.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sma_basic() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sma(&data, 3), Some(4.0)); // (3+4+5)/3
        assert_eq!(sma(&data, 5), Some(3.0)); // (1+2+3+4+5)/5
        assert_eq!(sma(&data, 6), None);
    }

    #[test]
    fn rsi_extreme() {
        // All gains → RSI = 100
        let data: Vec<f64> = (0..20).map(|i| 100.0 + i as f64).collect();
        let r = rsi(&data, 14).unwrap();
        assert!((r - 100.0).abs() < 0.01);
    }

    #[test]
    fn bollinger_symmetric() {
        let data = vec![100.0; 20];
        let (upper, mid, lower, bw) = bollinger_bands(&data, 20, 2.0).unwrap();
        assert!((upper - 100.0).abs() < 0.001);
        assert!((mid - 100.0).abs() < 0.001);
        assert!((lower - 100.0).abs() < 0.001);
        assert!(bw.abs() < 0.001);
    }

    #[test]
    fn atr_basic() {
        let highs = vec![10.0, 11.0, 12.0, 11.5, 12.5, 13.0];
        let lows = vec![9.0, 9.5, 10.0, 9.8, 10.5, 11.0];
        let closes = vec![9.5, 10.5, 11.0, 10.0, 12.0, 12.5];
        let a = atr(&highs, &lows, &closes, 3).unwrap();
        assert!(a > 0.0);
    }
}
