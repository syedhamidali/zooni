//! Backtesting engine — replay historical price data through the grid strategy.
//!
//! Supports CSV input with columns: timestamp, open, high, low, close, volume.
//! Simulates fills when price crosses grid levels, tracks P&L, and reports stats.

use std::collections::HashMap;
use std::io::Write;

use anyhow::Context;

use crate::config::GridCfg;
use crate::grid::compute_levels;
use crate::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::pnl::PnlTracker;
use crate::regime;
use crate::strategy::{MarketSnapshot, OrderType, Strategy, StrategyAction};
use crate::types::Side;
use crate::venue::VenueKline as Kline;

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
    let levels = compute_levels(
        grid_cfg.lower_price,
        grid_cfg.upper_price,
        grid_cfg.levels as usize,
    );
    let grid_spacing = if levels.len() >= 2 {
        levels[1] - levels[0]
    } else {
        0.0
    };
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
        orders.insert(
            i,
            SimOrder {
                _level: i,
                side,
                price: level_price,
                qty: current_qty,
            },
        );
    }

    let mut prev_equity = equity;

    for (bar_idx, bar) in bars.iter().enumerate() {
        // Update ATR
        if adaptive_sizing && let Some(atr_val) = atr.update(bar.high, bar.low, bar.close) {
            // Adjust qty inversely proportional to volatility
            // Higher ATR = smaller position, lower ATR = larger position
            let base_atr = grid_spacing; // use grid spacing as reference
            if atr_val > 0.0 {
                current_qty = qty * (base_atr / atr_val).clamp(0.5, 2.0);
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
                    if level + 1 < levels.len() {
                        Some(level + 1)
                    } else {
                        None
                    }
                }
                Side::Sell => {
                    if level > 0 {
                        Some(level - 1)
                    } else {
                        None
                    }
                }
            };
            if let Some(nl) = new_level {
                orders.entry(nl).or_insert_with(|| {
                    let new_side = match side {
                        Side::Buy => Side::Sell,
                        Side::Sell => Side::Buy,
                    };
                    SimOrder {
                        _level: nl,
                        side: new_side,
                        price: levels[nl],
                        qty: current_qty,
                    }
                });
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
        let variance = daily_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
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
        let volume = cols
            .get(5)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        if open > 0.0 && close > 0.0 {
            bars.push(PriceBar {
                timestamp,
                open,
                high,
                low,
                close,
                volume,
            });
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
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
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

/// Fetch kline (candlestick) data from Bybit's public API without authentication.
/// Useful for backtesting with real historical data.
///
/// `symbol`: e.g. "BTCUSDT", "ETHUSDT"
/// `interval`: "1" (1m), "5", "15", "30", "60", "240", "D", "W"
/// `limit`: total number of bars to fetch; transparently paginates when > 1000.
pub async fn fetch_klines_public(
    symbol: &str,
    interval: &str,
    limit: usize,
) -> anyhow::Result<Vec<PriceBar>> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "retCode")]
        ret_code: i64,
        #[serde(rename = "retMsg")]
        ret_msg: String,
        result: Option<KlineResult>,
    }
    #[derive(Deserialize)]
    struct KlineResult {
        list: Vec<Vec<String>>,
    }

    let http = reqwest::Client::new();
    let base = "https://api.bybit.com";
    let max_per_page: usize = 1000;

    let mut all_bars: Vec<PriceBar> = Vec::with_capacity(limit);
    let mut remaining = limit;
    let mut end_ts: Option<u64> = None;

    while remaining > 0 {
        let page_limit = remaining.min(max_per_page);
        let limit_str = page_limit.to_string();
        let url = format!("{base}/v5/market/kline");

        let mut query: Vec<(&str, String)> = vec![
            ("category", "spot".to_string()),
            ("symbol", symbol.to_string()),
            ("interval", interval.to_string()),
            ("limit", limit_str),
        ];
        if let Some(end) = end_ts {
            query.push(("end", end.to_string()));
        }

        let res = http
            .get(&url)
            .query(&query)
            .send()
            .await
            .context("public klines request failed")?;

        let env: Envelope = res
            .json()
            .await
            .context("public klines JSON parse failed")?;

        if env.ret_code != 0 {
            return Err(anyhow::anyhow!(
                "public klines error {}: {}",
                env.ret_code,
                env.ret_msg
            ));
        }

        let raw = env.result.map(|r| r.list).unwrap_or_default();

        if raw.is_empty() {
            break; // no more data available
        }

        // Bybit returns newest first; parse and collect
        let mut page_bars: Vec<PriceBar> = raw
            .into_iter()
            .filter_map(|row| {
                if row.len() < 6 {
                    return None;
                }
                Some(PriceBar {
                    timestamp: row[0].parse().unwrap_or(0),
                    open: row[1].parse().unwrap_or(0.0),
                    high: row[2].parse().unwrap_or(0.0),
                    low: row[3].parse().unwrap_or(0.0),
                    close: row[4].parse().unwrap_or(0.0),
                    volume: row[5].parse().unwrap_or(0.0),
                })
            })
            .collect();

        // Find earliest timestamp for pagination (newest-first, so last element is oldest)
        let earliest = page_bars.last().map(|b| b.timestamp as u64).unwrap_or(0);

        // If we got fewer than requested, there's no more data
        let got = page_bars.len();
        all_bars.append(&mut page_bars);

        if got < page_limit {
            break;
        }

        remaining = remaining.saturating_sub(got);
        if remaining > 0 {
            // Set end to one millisecond before the earliest timestamp to avoid overlap
            end_ts = Some(earliest.saturating_sub(1));
        }
    }

    // all_bars is in newest-first order from each page concatenated; reverse to chronological
    all_bars.reverse();

    // Deduplicate by timestamp (page boundaries may overlap by 1)
    all_bars.dedup_by_key(|b| b.timestamp);

    // Trim to requested limit
    if all_bars.len() > limit {
        let start = all_bars.len() - limit;
        all_bars = all_bars.split_off(start);
    }

    Ok(all_bars)
}

/// Generate a correlated price series from a primary series.
/// `correlation` controls how tightly the two move together (0.0–1.0).
/// `scale` adjusts the price level relative to the primary.
pub fn generate_correlated_data(
    primary: &[PriceBar],
    correlation: f64,
    scale: f64,
    seed: u64,
) -> Vec<PriceBar> {
    let mut result = Vec::with_capacity(primary.len());
    let mut rng_state = seed;
    let corr = correlation.clamp(0.0, 1.0);
    let indep = (1.0 - corr * corr).sqrt();

    let mut price = primary[0].close * scale;

    for (i, bar) in primary.iter().enumerate() {
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let uniform = (rng_state >> 33) as f64 / (1u64 << 31) as f64;
        let noise = (uniform - 0.5) * 2.0;

        let primary_ret = if i > 0 {
            (bar.close - primary[i - 1].close) / primary[i - 1].close
        } else {
            0.0
        };

        let correlated_ret = corr * primary_ret + indep * noise * 0.015;
        let open = price;
        let close = (price * (1.0 + correlated_ret)).max(0.01);
        let spread = (close - open).abs();
        let high = open.max(close) + spread * 0.3;
        let low = (open.min(close) - spread * 0.3).max(0.01);

        result.push(PriceBar {
            timestamp: bar.timestamp,
            open,
            high,
            low,
            close,
            volume: bar.volume * 0.8,
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
    results.sort_by(|a, b| {
        b.3.total_pnl
            .partial_cmp(&a.3.total_pnl)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Result of an orchestrator backtest.
#[derive(Debug, Clone)]
pub struct OrchestratorBacktestResult {
    pub total_pnl: f64,
    pub total_fills: usize,
    pub bars_processed: usize,
    pub max_drawdown: f64,
    pub sharpe_ratio: f64,
    pub final_equity: f64,
    pub peak_equity: f64,
    pub per_strategy: Vec<StrategyBacktestResult>,
    pub allocation_history: Vec<(usize, HashMap<String, f64>)>,
    pub equity_curve: Vec<(usize, f64)>,
}

#[derive(Debug, Clone)]
pub struct StrategyBacktestResult {
    pub name: String,
    pub pnl: f64,
    pub fills: usize,
    pub win_rate: f64,
    pub final_allocation: f64,
}

/// Simulated pending order for the orchestrator backtest.
#[derive(Debug, Clone)]
struct PendingOrder {
    strategy: String,
    side: Side,
    price: f64,
    qty: f64,
    order_type: OrderType,
    bar_placed: usize,
}

/// Run a multi-strategy orchestrator backtest on historical data.
pub fn run_orchestrator_backtest(
    strategies: Vec<Box<dyn Strategy>>,
    orch_config: OrchestratorConfig,
    bars: &[PriceBar],
    starting_equity: f64,
) -> OrchestratorBacktestResult {
    run_orchestrator_backtest_with_data(strategies, orch_config, bars, None, starting_equity)
}

/// Run a multi-strategy orchestrator backtest with optional real secondary-symbol data.
/// When `secondary` is `Some((symbol, bars))`, those bars feed the pairs strategy
/// instead of synthetic correlated data.
pub fn run_orchestrator_backtest_with_data(
    strategies: Vec<Box<dyn Strategy>>,
    orch_config: OrchestratorConfig,
    bars: &[PriceBar],
    secondary: Option<(&str, &[PriceBar])>,
    starting_equity: f64,
) -> OrchestratorBacktestResult {
    let empty = OrchestratorBacktestResult {
        total_pnl: 0.0,
        total_fills: 0,
        bars_processed: 0,
        max_drawdown: 0.0,
        sharpe_ratio: 0.0,
        final_equity: starting_equity,
        peak_equity: starting_equity,
        per_strategy: Vec::new(),
        allocation_history: Vec::new(),
        equity_curve: Vec::new(),
    };

    if bars.is_empty() || strategies.is_empty() {
        return empty;
    }

    let strategy_names: Vec<String> = strategies.iter().map(|s| s.name().to_string()).collect();

    // Extract pairs config before moving orch_config into Orchestrator
    let has_pairs = orch_config.strategies.contains(&"pairs".to_string());
    let pairs_sym_a = orch_config.pairs.symbol_a.clone();
    let pairs_sym_b = orch_config.pairs.symbol_b.clone();

    let mut orchestrator = Orchestrator::new(strategies, orch_config);
    let mut pnl_tracker = PnlTracker::new();
    let mut pending_orders: Vec<PendingOrder> = Vec::new();
    let mut equity = starting_equity;
    let mut peak_equity = starting_equity;
    let mut max_drawdown: f64 = 0.0;
    let mut total_fills: usize = 0;
    let mut daily_returns: Vec<f64> = Vec::new();
    let mut prev_equity = equity;
    let mut equity_curve: Vec<(usize, f64)> = Vec::new();
    let mut allocation_history: Vec<(usize, HashMap<String, f64>)> = Vec::new();

    let sym = "BTCUSDT";
    // Use provided secondary data, or fall back to synthetic correlated data for pairs
    let owned_secondary: Vec<PriceBar>;
    let secondary_resolved: Option<(&str, &[PriceBar])> =
        if let Some((sec_sym, sec_bars)) = secondary {
            Some((sec_sym, sec_bars))
        } else if has_pairs {
            owned_secondary = generate_correlated_data(bars, 0.85, 0.05, 137);
            Some((pairs_sym_a.as_str(), &owned_secondary))
        } else {
            None
        };

    // Build kline history for regime detection
    let mut kline_buf: Vec<Kline> = Vec::new();

    // Start
    let initial_snap = bar_to_snapshot(sym, &bars[0]);
    let _ = orchestrator.on_start(&initial_snap);

    for (bar_idx, bar) in bars.iter().enumerate() {
        let snapshot = bar_to_snapshot(sym, bar);

        // Build kline history
        kline_buf.push(Kline {
            timestamp: bar.timestamp as u64,
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume,
        });
        if kline_buf.len() > 200 {
            kline_buf.remove(0);
        }

        // Check pending orders for fills
        let mut fills_this_bar = Vec::new();
        pending_orders.retain(|order| {
            let filled = match order.order_type {
                OrderType::Market => true,
                OrderType::Limit => match order.side {
                    Side::Buy => bar.low <= order.price,
                    Side::Sell => bar.high >= order.price,
                },
            };
            if filled {
                let fill_price = match order.order_type {
                    OrderType::Market => bar.close,
                    OrderType::Limit => order.price,
                };
                fills_this_bar.push((order.strategy.clone(), order.side, fill_price, order.qty));
                false
            } else {
                bar_idx - order.bar_placed < 50 // expire stale orders after 50 bars
            }
        });

        // Process fills
        let mut fill_events = Vec::new();
        for (strat, side, price, qty) in &fills_this_bar {
            let pnl = pnl_tracker.record_fill(strat, *side, *price, *qty);
            orchestrator.record_fill(strat, pnl);
            total_fills += 1;

            match side {
                Side::Buy => equity -= price * qty,
                Side::Sell => equity += price * qty,
            }

            fill_events.push(crate::strategy::FillEvent {
                order_id: format!("bt-{}-{}", bar_idx, total_fills),
                symbol: sym.to_string(),
                side: *side,
                price: *price,
                qty: *qty,
            });
        }

        // Regime detection every 60 bars
        let current_regime = if bar_idx % 60 == 0 && kline_buf.len() >= 50 {
            regime::analyze(&kline_buf).map(|a| a.regime)
        } else {
            orchestrator.current_regime()
        };

        // Tick orchestrator — primary symbol (symbol_b for pairs, main for others)
        let primary_snap = if has_pairs && pairs_sym_b == sym {
            snapshot.clone()
        } else if has_pairs {
            let mut s = snapshot.clone();
            s.symbol = pairs_sym_b.clone();
            s
        } else {
            snapshot.clone()
        };
        let tagged_actions = match orchestrator.on_tick(&primary_snap, &fill_events, current_regime)
        {
            Ok(actions) => actions,
            Err(_) => continue,
        };

        // Tick with secondary symbol for pairs strategy
        let secondary_actions = if let Some((sec_sym, sec_bars)) = &secondary_resolved {
            if bar_idx < sec_bars.len() {
                let sec_snap = bar_to_snapshot(sec_sym, &sec_bars[bar_idx]);
                orchestrator
                    .on_tick(&sec_snap, &[], current_regime)
                    .unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Collect new orders from both ticks
        let all_tagged_actions = tagged_actions.into_iter().chain(secondary_actions);
        for ta in all_tagged_actions {
            if let StrategyAction::PlaceOrder {
                side,
                price,
                qty,
                order_type,
                ..
            } = &ta.action
            {
                if *qty > 1e-12 {
                    pending_orders.push(PendingOrder {
                        strategy: ta.strategy_name.clone(),
                        side: *side,
                        price: *price,
                        qty: *qty,
                        order_type: *order_type,
                        bar_placed: bar_idx,
                    });
                }
            }
        }

        // Mark-to-market equity
        let holdings_value = pnl_tracker.total_unrealised_pnl(bar.close);
        let mtm_equity = equity
            + holdings_value
            + strategy_names
                .iter()
                .map(|n| pnl_tracker.holdings(n) * bar.close)
                .sum::<f64>();

        if mtm_equity > peak_equity {
            peak_equity = mtm_equity;
        }
        let dd = if peak_equity > 0.0 {
            (peak_equity - mtm_equity) / peak_equity
        } else {
            0.0
        };
        if dd > max_drawdown {
            max_drawdown = dd;
        }

        // Periodic snapshots
        if bar_idx % 24 == 0 || bar_idx == bars.len() - 1 {
            let ret = if prev_equity > 0.0 {
                (mtm_equity - prev_equity) / prev_equity
            } else {
                0.0
            };
            daily_returns.push(ret);
            prev_equity = mtm_equity;
            equity_curve.push((bar_idx, mtm_equity));
        }
        if bar_idx % 100 == 0 {
            allocation_history.push((bar_idx, orchestrator.allocations()));
        }
    }

    // Sharpe ratio
    let sharpe_ratio = if daily_returns.len() > 1 {
        let n = daily_returns.len() as f64;
        let mean = daily_returns.iter().sum::<f64>() / n;
        let variance = daily_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0);
        let std_dev = variance.sqrt();
        if std_dev > 0.0 {
            (mean / std_dev) * (252.0_f64).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Per-strategy results
    let final_allocs = orchestrator.allocations();
    let per_strategy = strategy_names
        .iter()
        .map(|name| {
            let perf = orchestrator.performance(name);
            StrategyBacktestResult {
                name: name.clone(),
                pnl: pnl_tracker.realised_pnl(name),
                fills: perf.map(|p| p.trades as usize).unwrap_or(0),
                win_rate: pnl_tracker.win_rate(name),
                final_allocation: *final_allocs.get(name).unwrap_or(&0.0),
            }
        })
        .collect();

    let _ = orchestrator.on_shutdown();

    OrchestratorBacktestResult {
        total_pnl: pnl_tracker.total_realised_pnl(),
        total_fills,
        bars_processed: bars.len(),
        max_drawdown,
        sharpe_ratio,
        final_equity: equity,
        peak_equity,
        per_strategy,
        allocation_history,
        equity_curve,
    }
}

/// Result from one walk-forward window.
#[derive(Debug, Clone)]
pub struct WalkForwardWindow {
    pub window_idx: usize,
    pub train_bars: usize,
    pub test_bars: usize,
    pub train_result: OrchestratorBacktestResult,
    pub test_result: OrchestratorBacktestResult,
}

/// Aggregated walk-forward results.
#[derive(Debug, Clone)]
pub struct WalkForwardResult {
    pub windows: Vec<WalkForwardWindow>,
    pub total_test_pnl: f64,
    pub total_test_fills: usize,
    pub avg_test_sharpe: f64,
    pub avg_test_drawdown: f64,
    pub avg_test_win_rate: f64,
}

/// Walk-forward backtest: slide a window over the data, train on the first portion
/// and validate on the rest. This detects overfitting and gives realistic performance
/// estimates. `strategy_builder` is called for each window to create fresh strategies.
pub fn run_walk_forward<F>(
    strategy_builder: F,
    orch_config: &OrchestratorConfig,
    bars: &[PriceBar],
    starting_equity: f64,
    window_size: usize,
    train_pct: f64,
) -> WalkForwardResult
where
    F: Fn() -> Vec<Box<dyn Strategy>>,
{
    let mut windows = Vec::new();
    let step = window_size / 2; // 50% overlap between windows
    let train_size = (window_size as f64 * train_pct) as usize;

    let mut idx = 0;
    let mut window_idx = 0;

    while idx + window_size <= bars.len() {
        let window = &bars[idx..idx + window_size];
        let train_bars = &window[..train_size];
        let test_bars = &window[train_size..];

        let train_result = run_orchestrator_backtest(
            strategy_builder(),
            orch_config.clone(),
            train_bars,
            starting_equity,
        );

        let test_result = run_orchestrator_backtest(
            strategy_builder(),
            orch_config.clone(),
            test_bars,
            starting_equity,
        );

        windows.push(WalkForwardWindow {
            window_idx,
            train_bars: train_bars.len(),
            test_bars: test_bars.len(),
            train_result,
            test_result,
        });

        idx += step;
        window_idx += 1;
    }

    let n = windows.len().max(1) as f64;
    let total_test_pnl: f64 = windows.iter().map(|w| w.test_result.total_pnl).sum();
    let total_test_fills: usize = windows.iter().map(|w| w.test_result.total_fills).sum();
    let avg_test_sharpe = windows
        .iter()
        .map(|w| w.test_result.sharpe_ratio)
        .sum::<f64>()
        / n;
    let avg_test_drawdown = windows
        .iter()
        .map(|w| w.test_result.max_drawdown)
        .sum::<f64>()
        / n;

    let avg_test_win_rate = if !windows.is_empty() {
        let total_wins: f64 = windows
            .iter()
            .flat_map(|w| &w.test_result.per_strategy)
            .map(|s| s.win_rate * s.fills as f64)
            .sum();
        let total_trades: f64 = windows
            .iter()
            .flat_map(|w| &w.test_result.per_strategy)
            .map(|s| s.fills as f64)
            .sum();
        if total_trades > 0.0 {
            total_wins / total_trades
        } else {
            0.0
        }
    } else {
        0.0
    };

    WalkForwardResult {
        windows,
        total_test_pnl,
        total_test_fills,
        avg_test_sharpe,
        avg_test_drawdown,
        avg_test_win_rate,
    }
}

fn bar_to_snapshot(symbol: &str, bar: &PriceBar) -> MarketSnapshot {
    MarketSnapshot {
        symbol: symbol.to_string(),
        bid: bar.close - (bar.high - bar.low) * 0.01,
        ask: bar.close + (bar.high - bar.low) * 0.01,
        last: bar.close,
        volume_24h: bar.volume,
    }
}

/// Export orchestrator backtest results to a CSV file.
///
/// Columns: `bar`, `equity`, then one `<name>_alloc` column per strategy.
/// Allocation values are interpolated using last-known values from the
/// (typically every-100-bar) allocation history.
pub fn export_orchestrator_csv(
    result: &OrchestratorBacktestResult,
    path: &str,
) -> anyhow::Result<()> {
    let mut file =
        std::fs::File::create(path).with_context(|| format!("creating export file {path}"))?;

    // Collect strategy names from per_strategy results
    let strat_names: Vec<&str> = result
        .per_strategy
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    // Write header
    write!(file, "bar,equity")?;
    for name in &strat_names {
        write!(file, ",{}_alloc", name)?;
    }
    writeln!(file)?;

    // Build a sorted copy of allocation history for lookup
    let alloc_hist: &[(usize, HashMap<String, f64>)] = &result.allocation_history;

    for &(bar, equity) in &result.equity_curve {
        write!(file, "{},{:.2}", bar, equity)?;

        // Find the most recent allocation snapshot at or before this bar
        let snapshot = alloc_hist
            .iter()
            .rev()
            .find(|(b, _)| *b <= bar)
            .map(|(_, allocs)| allocs);

        for name in &strat_names {
            let alloc = snapshot.and_then(|a| a.get(*name)).copied().unwrap_or(0.0);
            write!(file, ",{:.3}", alloc)?;
        }
        writeln!(file)?;
    }

    Ok(())
}

/// Export grid backtest results to CSV files.
///
/// Writes an equity-curve CSV to `path` with columns `bar,equity`, and a
/// fills CSV to a sibling file with `_fills` inserted before the extension
/// (e.g. `results.csv` -> `results_fills.csv`) with columns
/// `timestamp,side,level,price,qty,pnl`.
pub fn export_grid_csv(result: &BacktestResult, path: &str) -> anyhow::Result<()> {
    // --- equity curve ---
    let mut file =
        std::fs::File::create(path).with_context(|| format!("creating export file {path}"))?;
    writeln!(file, "bar,equity")?;
    for &(ts, equity) in &result.equity_curve {
        writeln!(file, "{},{:.2}", ts, equity)?;
    }

    // --- fills ---
    let fills_path = if let Some(dot) = path.rfind('.') {
        format!("{}_fills{}", &path[..dot], &path[dot..])
    } else {
        format!("{}_fills", path)
    };

    let mut fills_file = std::fs::File::create(&fills_path)
        .with_context(|| format!("creating fills file {fills_path}"))?;
    writeln!(fills_file, "timestamp,side,level,price,qty,pnl")?;
    for f in &result.fills {
        let side_str = match f.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        writeln!(
            fills_file,
            "{},{},{},{:.2},{:.6},{:.4}",
            f.timestamp, side_str, f.level, f.price, f.qty, f.pnl,
        )?;
    }

    Ok(())
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

    // -- Orchestrator backtest tests --

    fn make_test_strategies() -> Vec<Box<dyn Strategy>> {
        use crate::strategy::dca::DcaStrategy;
        use crate::strategy::momentum::{MomentumConfig, MomentumStrategy};

        vec![
            Box::new(DcaStrategy::with_sma_filter("BTCUSDT".into(), 100.0, 5, 20)),
            Box::new(MomentumStrategy::new(MomentumConfig {
                symbol: "BTCUSDT".into(),
                ..MomentumConfig::default()
            })),
        ]
    }

    #[test]
    fn orchestrator_backtest_runs() {
        let bars = generate_synthetic_data(65000.0, 500, 0.02, 42);
        let result = run_orchestrator_backtest(
            make_test_strategies(),
            OrchestratorConfig::default(),
            &bars,
            10000.0,
        );
        assert_eq!(result.bars_processed, 500);
        assert_eq!(result.per_strategy.len(), 2);
    }

    #[test]
    fn orchestrator_backtest_tracks_allocations() {
        let bars = generate_synthetic_data(65000.0, 500, 0.02, 77);
        let result = run_orchestrator_backtest(
            make_test_strategies(),
            OrchestratorConfig::default(),
            &bars,
            10000.0,
        );
        assert!(
            !result.allocation_history.is_empty(),
            "should track allocation snapshots"
        );
        assert!(!result.equity_curve.is_empty(), "should track equity curve");
    }

    #[test]
    fn orchestrator_backtest_empty_bars() {
        let result = run_orchestrator_backtest(
            make_test_strategies(),
            OrchestratorConfig::default(),
            &[],
            10000.0,
        );
        assert_eq!(result.bars_processed, 0);
        assert!((result.final_equity - 10000.0).abs() < 1e-9);
    }

    #[test]
    fn orchestrator_backtest_per_strategy_results() {
        let bars = generate_synthetic_data(65000.0, 300, 0.02, 123);
        let result = run_orchestrator_backtest(
            make_test_strategies(),
            OrchestratorConfig {
                rebalance_interval: 50,
                strategy_max_drawdown: 100.0, // disable risk guard for this test
                ..Default::default()
            },
            &bars,
            10000.0,
        );
        for sr in &result.per_strategy {
            assert!(
                sr.final_allocation > 0.0,
                "{} should have non-zero allocation",
                sr.name
            );
        }
    }
}
