//! Backtesting engine — replay historical price data through the grid strategy.
//!
//! Supports CSV input with columns: timestamp, open, high, low, close, volume.
//! Simulates fills when price crosses grid levels, tracks P&L, and reports stats.

use std::collections::HashMap;

use crate::config::GridCfg;
use crate::grid::compute_levels;
use crate::types::Side;

/// One candle / tick of price data.
#[derive(Debug, Clone)]
pub struct PriceBar {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Result of a backtest run.
#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub total_pnl: f64,
    pub total_fills: usize,
    pub buy_fills: usize,
    pub sell_fills: usize,
    pub max_drawdown: f64,
    pub sharpe_ratio: f64,
    pub win_rate: f64,
    pub avg_profit_per_cycle: f64,
    pub peak_equity: f64,
    pub final_equity: f64,
    pub bars_processed: usize,
    pub holding_value: f64,
    /// Per-day P&L snapshots for equity curve.
    pub equity_curve: Vec<(i64, f64)>,
    /// All fills for analysis.
    pub fills: Vec<BacktestFill>,
}

#[derive(Debug, Clone)]
pub struct BacktestFill {
    pub timestamp: i64,
    pub level: usize,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
    pub pnl: f64,
}

/// Simulated open order on the grid.
#[derive(Debug, Clone)]
struct SimOrder {
    _level: usize,
    side: Side,
    price: f64,
    qty: f64,
}

/// ATR (Average True Range) calculator for adaptive sizing.
#[derive(Debug)]
pub struct AtrCalculator {
    period: usize,
    history: Vec<f64>,
    prev_close: Option<f64>,
}

impl AtrCalculator {
    pub fn new(period: usize) -> Self {
        Self {
            period,
            history: Vec::new(),
            prev_close: None,
        }
    }

    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = if let Some(pc) = self.prev_close {
            (high - low).max((high - pc).abs()).max((low - pc).abs())
        } else {
            high - low
        };
        self.prev_close = Some(close);
        self.history.push(tr);
        if self.history.len() > self.period {
            self.history.remove(0);
        }
        if self.history.len() >= self.period {
            Some(self.history.iter().sum::<f64>() / self.history.len() as f64)
        } else {
            None
        }
    }

    pub fn current(&self) -> Option<f64> {
        if self.history.len() >= self.period {
            Some(self.history.iter().sum::<f64>() / self.history.len() as f64)
        } else {
            None
        }
    }
}

/// Run a backtest of the grid strategy on historical data.
pub fn run_backtest(
    grid_cfg: &GridCfg,
    bars: &[PriceBar],
    starting_equity: f64,
    _tick_size: &str,
    _base_precision: &str,
    adaptive_sizing: bool,
) -> BacktestResult {
    let levels = compute_levels(grid_cfg.lower_price, grid_cfg.upper_price, grid_cfg.levels as usize);
    let grid_spacing = if levels.len() >= 2 { levels[1] - levels[0] } else { 0.0 };
    let qty = grid_cfg.qty_per_level_base;

    let mut equity = starting_equity;
    let mut peak_equity = equity;
    let mut max_drawdown: f64 = 0.0;
    let mut total_pnl = 0.0;
    let mut buy_fills = 0usize;
    let mut sell_fills = 0usize;
    let mut winning_cycles = 0usize;
    let mut total_cycles = 0usize;
    let mut holdings_base = 0.0_f64; // net base asset held
    let mut avg_buy_price = 0.0_f64;
    let mut daily_returns: Vec<f64> = Vec::new();
    let mut equity_curve: Vec<(i64, f64)> = Vec::new();
    let mut all_fills: Vec<BacktestFill> = Vec::new();

    // Active orders: level -> SimOrder
    let mut orders: HashMap<usize, SimOrder> = HashMap::new();

    // ATR for adaptive sizing
    let mut atr = AtrCalculator::new(14);
    let mut current_qty = qty;

    if bars.is_empty() {
        return BacktestResult {
            total_pnl: 0.0,
            total_fills: 0,
            buy_fills: 0,
            sell_fills: 0,
            max_drawdown: 0.0,
            sharpe_ratio: 0.0,
            win_rate: 0.0,
            avg_profit_per_cycle: 0.0,
            peak_equity: equity,
            final_equity: equity,
            bars_processed: 0,
            holding_value: 0.0,
            equity_curve: Vec::new(),
            fills: Vec::new(),
        };
    }

    // Place initial grid around first bar's close
    let initial_mid = bars[0].close;
    for (i, &level_price) in levels.iter().enumerate() {
        let side = if level_price < initial_mid {
            Side::Buy
        } else if level_price > initial_mid {
            Side::Sell
        } else {
            continue;
        };
        orders.insert(i, SimOrder {
            _level: i,
            side,
            price: level_price,
            qty: current_qty,
        });
    }

    let mut prev_equity = equity;

    for (bar_idx, bar) in bars.iter().enumerate() {
        // Update ATR
        if adaptive_sizing {
            if let Some(atr_val) = atr.update(bar.high, bar.low, bar.close) {
                // Adjust qty inversely proportional to volatility
                // Higher ATR = smaller position, lower ATR = larger position
                let base_atr = grid_spacing; // use grid spacing as reference
                if atr_val > 0.0 {
                    current_qty = qty * (base_atr / atr_val).clamp(0.5, 2.0);
                }
            }
        }

        // Check which orders would have filled during this bar
        let mut filled_levels: Vec<(usize, Side, f64)> = Vec::new();

        for (&level, order) in orders.iter() {
            let would_fill = match order.side {
                Side::Buy => bar.low <= order.price,
                Side::Sell => bar.high >= order.price,
            };
            if would_fill {
                filled_levels.push((level, order.side, order.price));
            }
        }

        // Process fills
        for (level, side, fill_price) in filled_levels {
            let fill_qty = orders.get(&level).map(|o| o.qty).unwrap_or(current_qty);
            orders.remove(&level);

            let pnl = match side {
                Side::Buy => {
                    buy_fills += 1;
                    holdings_base += fill_qty;
                    // Update weighted average buy price
                    if holdings_base > 0.0 {
                        avg_buy_price = ((avg_buy_price * (holdings_base - fill_qty))
                            + (fill_price * fill_qty))
                            / holdings_base;
                    }
                    equity -= fill_price * fill_qty; // spend cash
                    0.0 // no realized PnL on buy
                }
                Side::Sell => {
                    sell_fills += 1;
                    let cycle_pnl = if holdings_base >= fill_qty {
                        (fill_price - avg_buy_price) * fill_qty
                    } else {
                        grid_spacing * fill_qty // approximate
                    };
                    holdings_base = (holdings_base - fill_qty).max(0.0);
                    equity += fill_price * fill_qty; // receive cash
                    total_cycles += 1;
                    if cycle_pnl > 0.0 {
                        winning_cycles += 1;
                    }
                    total_pnl += cycle_pnl;
                    cycle_pnl
                }
            };

            all_fills.push(BacktestFill {
                timestamp: bar.timestamp,
                level,
                side,
                price: fill_price,
                qty: fill_qty,
                pnl,
            });

            // Place replacement order
            let new_level = match side {
                Side::Buy => {
                    if level + 1 < levels.len() { Some(level + 1) } else { None }
                }
                Side::Sell => {
                    if level > 0 { Some(level - 1) } else { None }
                }
            };
            if let Some(nl) = new_level {
                if !orders.contains_key(&nl) {
                    let new_side = match side {
                        Side::Buy => Side::Sell,
                        Side::Sell => Side::Buy,
                    };
                    orders.insert(nl, SimOrder {
                        _level: nl,
                        side: new_side,
                        price: levels[nl],
                        qty: current_qty,
                    });
                }
            }
        }

        // Track equity (cash + holdings mark-to-market)
        let mark_to_market = equity + holdings_base * bar.close;
        if mark_to_market > peak_equity {
            peak_equity = mark_to_market;
        }
        let dd = if peak_equity > 0.0 {
            (peak_equity - mark_to_market) / peak_equity
        } else {
            0.0
        };
        if dd > max_drawdown {
            max_drawdown = dd;
        }

        // Daily return for Sharpe calculation
        if bar_idx % 24 == 0 || bar_idx == bars.len() - 1 {
            let ret = if prev_equity > 0.0 {
                (mark_to_market - prev_equity) / prev_equity
            } else {
                0.0
            };
            daily_returns.push(ret);
            prev_equity = mark_to_market;
            equity_curve.push((bar.timestamp, mark_to_market));
        }
    }

    let holding_value = holdings_base * bars.last().map(|b| b.close).unwrap_or(0.0);
    let final_equity = equity + holding_value;

    // Sharpe ratio (annualized, assuming daily returns)
    let sharpe_ratio = if daily_returns.len() > 1 {
        let mean = daily_returns.iter().sum::<f64>() / daily_returns.len() as f64;
        let variance = daily_returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
            / (daily_returns.len() - 1) as f64;
        let std_dev = variance.sqrt();
        if std_dev > 0.0 {
            (mean / std_dev) * (252.0_f64).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };

    let win_rate = if total_cycles > 0 {
        winning_cycles as f64 / total_cycles as f64
    } else {
        0.0
    };
    let avg_profit = if total_cycles > 0 {
        total_pnl / total_cycles as f64
    } else {
        0.0
    };

    BacktestResult {
        total_pnl,
        total_fills: buy_fills + sell_fills,
        buy_fills,
        sell_fills,
        max_drawdown,
        sharpe_ratio,
        win_rate,
        avg_profit_per_cycle: avg_profit,
        peak_equity,
        final_equity,
        bars_processed: bars.len(),
        holding_value,
        equity_curve,
        fills: all_fills,
    }
}

/// Parse CSV price data. Expected columns: timestamp,open,high,low,close,volume
pub fn parse_csv(data: &str) -> Vec<PriceBar> {
    let mut bars = Vec::new();
    for line in data.lines().skip(1) {
        // skip header
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        let timestamp = cols[0].trim().parse::<i64>().unwrap_or(0);
        let open = cols[1].trim().parse::<f64>().unwrap_or(0.0);
        let high = cols[2].trim().parse::<f64>().unwrap_or(0.0);
        let low = cols[3].trim().parse::<f64>().unwrap_or(0.0);
        let close = cols[4].trim().parse::<f64>().unwrap_or(0.0);
        let volume = cols.get(5).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        if open > 0.0 && close > 0.0 {
            bars.push(PriceBar { timestamp, open, high, low, close, volume });
        }
    }
    bars
}

/// Generate synthetic random-walk price data for testing.
pub fn generate_synthetic_data(
    start_price: f64,
    bars: usize,
    volatility: f64,
    seed: u64,
) -> Vec<PriceBar> {
    let mut result = Vec::with_capacity(bars);
    let mut price = start_price;
    let mut rng_state = seed;

    for i in 0..bars {
        // Simple LCG PRNG
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let uniform = (rng_state >> 33) as f64 / (1u64 << 31) as f64; // 0..1
        let change = (uniform - 0.5) * 2.0 * volatility * price;

        let open = price;
        let close = (price + change).max(1.0);
        let high = open.max(close) * (1.0 + volatility * 0.3);
        let low = open.min(close) * (1.0 - volatility * 0.3);
        let volume = 100.0 + uniform * 900.0;

        result.push(PriceBar {
            timestamp: i as i64,
            open,
            high: high.max(open).max(close),
            low: low.min(open).min(close).max(0.01),
            close,
            volume,
        });
        price = close;
    }
    result
}

/// Optimize grid parameters by testing multiple configurations.
pub fn optimize_grid(
    bars: &[PriceBar],
    starting_equity: f64,
    symbol: &str,
    level_range: std::ops::Range<u32>,
    level_step: u32,
) -> Vec<(u32, f64, f64, BacktestResult)> {
    let price_min = bars.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    let price_max = bars.iter().map(|b| b.high).fold(0.0_f64, f64::max);
    let price_range = price_max - price_min;

    let mut results = Vec::new();

    // Test different grid ranges (tight vs wide) and level counts
    for range_pct in [0.5, 0.7, 0.9, 1.0] {
        let lower = price_min + price_range * (1.0 - range_pct) / 2.0;
        let upper = price_max - price_range * (1.0 - range_pct) / 2.0;

        let mut n = level_range.start;
        while n < level_range.end {
            let qty = starting_equity * 0.02 / ((lower + upper) / 2.0); // ~2% per level
            let cfg = GridCfg {
                symbol: symbol.to_string(),
                lower_price: lower,
                upper_price: upper,
                levels: n,
                qty_per_level_base: qty,
                poll_secs: 3,
                cancel_existing_on_start: true,
                auto_rebalance: false,
                trailing_profit_threshold: 0.0,
            };
            let result = run_backtest(&cfg, bars, starting_equity, "0.01", "0.000001", false);
            results.push((n, lower, upper, result));
            n += level_step;
        }
    }

    // Sort by total P&L descending
    results.sort_by(|a, b| b.3.total_pnl.partial_cmp(&a.3.total_pnl).unwrap_or(std::cmp::Ordering::Equal));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_backtest_produces_fills() {
        let bars = generate_synthetic_data(65000.0, 1000, 0.02, 42);
        let cfg = GridCfg {
            symbol: "BTCUSDT".to_string(),
            lower_price: 55000.0,
            upper_price: 75000.0,
            levels: 20,
            qty_per_level_base: 0.001,
            poll_secs: 3,
            cancel_existing_on_start: true,
            auto_rebalance: false,
            trailing_profit_threshold: 0.0,
        };
        let result = run_backtest(&cfg, &bars, 10000.0, "0.01", "0.000001", false);
        assert!(result.total_fills > 0, "should have at least one fill");
        assert!(result.bars_processed == 1000);
    }

    #[test]
    fn atr_calculator_works() {
        let mut atr = AtrCalculator::new(3);
        assert!(atr.update(105.0, 95.0, 100.0).is_none());
        assert!(atr.update(110.0, 98.0, 105.0).is_none());
        let val = atr.update(108.0, 100.0, 103.0);
        assert!(val.is_some());
        assert!(val.unwrap() > 0.0);
    }

    #[test]
    fn csv_parsing() {
        let csv = "timestamp,open,high,low,close,volume\n\
                   1000,100.0,105.0,95.0,102.0,500\n\
                   2000,102.0,108.0,99.0,106.0,600\n";
        let bars = parse_csv(csv);
        assert_eq!(bars.len(), 2);
        assert!((bars[0].open - 100.0).abs() < 1e-9);
    }

    #[test]
    fn adaptive_sizing_backtest() {
        let bars = generate_synthetic_data(65000.0, 500, 0.02, 99);
        let cfg = GridCfg {
            symbol: "BTCUSDT".to_string(),
            lower_price: 55000.0,
            upper_price: 75000.0,
            levels: 10,
            qty_per_level_base: 0.001,
            poll_secs: 3,
            cancel_existing_on_start: true,
            auto_rebalance: false,
            trailing_profit_threshold: 0.0,
        };
        let result = run_backtest(&cfg, &bars, 10000.0, "0.01", "0.000001", true);
        assert!(result.bars_processed == 500);
    }
}
