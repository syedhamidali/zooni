//! Benchmarks for hot-path functions.
//!
//! Run: cargo bench

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use zooni::config::GridCfg;
use zooni::grid;
use zooni::indicators;

// -- Grid engine benchmarks ---------------------------------------------------

fn bench_grid(c: &mut Criterion) {
    c.bench_function("compute_levels_20", |b| {
        b.iter(|| grid::compute_levels(black_box(60000.0), black_box(70000.0), 20))
    });

    c.bench_function("compute_levels_100", |b| {
        b.iter(|| grid::compute_levels(black_box(60000.0), black_box(70000.0), 100))
    });

    c.bench_function("format_to_increment", |b| {
        b.iter(|| grid::format_to_increment(black_box(65432.789), "0.01"))
    });

    let cfg = GridCfg {
        symbol: "BTCUSDT".into(),
        lower_price: 60000.0,
        upper_price: 70000.0,
        levels: 20,
        qty_per_level_base: 0.001,
        poll_secs: 5,
        cancel_existing_on_start: true,
        auto_rebalance: false,
        trailing_profit_threshold: 0.0,
        cancel_buys_on_trend: false,
        auto_recenter: false,
        recenter_threshold_pct: 0.15,
        use_post_only: true,
        vwap_entry_filter: false,
        vwap_threshold_pct: 0.005,
        venue: "bybit".to_string(),
        respect_market_hours: false,
    };
    let g = grid::Grid::new(&cfg, "0.01".into(), "0.00001".into(), 0.00001, 5.0);

    c.bench_function("plan_initial_20_levels", |b| {
        b.iter(|| g.plan_initial(black_box(65000.0)))
    });
}

// -- Indicator benchmarks -----------------------------------------------------

fn make_data(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 100.0 + 10.0 * (i as f64 * 0.1).sin())
        .collect()
}

fn make_ohlcv(n: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let closes = make_data(n);
    let highs: Vec<f64> = closes.iter().map(|c| c + 1.0).collect();
    let lows: Vec<f64> = closes.iter().map(|c| c - 1.0).collect();
    (highs, lows, closes)
}

fn bench_indicators(c: &mut Criterion) {
    let data = make_data(1000);
    let (h, l, cl) = make_ohlcv(1000);

    c.bench_function("sma_200_on_1000", |b| {
        b.iter(|| indicators::sma(black_box(&data), 200))
    });

    c.bench_function("ema_series_1000", |b| {
        b.iter(|| indicators::ema_series(black_box(&data), 20))
    });

    c.bench_function("rsi_14_on_1000", |b| {
        b.iter(|| indicators::rsi(black_box(&data), 14))
    });

    c.bench_function("atr_14_on_1000", |b| {
        b.iter(|| indicators::atr(black_box(&h), black_box(&l), black_box(&cl), 14))
    });

    c.bench_function("adx_14_on_1000", |b| {
        b.iter(|| indicators::adx(black_box(&h), black_box(&l), black_box(&cl), 14))
    });

    c.bench_function("bollinger_20_on_1000", |b| {
        b.iter(|| indicators::bollinger_bands(black_box(&data), 20, 2.0))
    });
}

criterion_group!(benches, bench_grid, bench_indicators);
criterion_main!(benches);
