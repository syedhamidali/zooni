//! Integration tests for the grid trading engine.
//!
//! These simulate a full grid lifecycle: setup → initial placement → fills →
//! replacements → edge cases. No network calls.

use zooni::config::GridCfg;
use zooni::grid::{self, Grid};
use zooni::types::Side;

fn test_grid() -> Grid {
    let cfg = GridCfg {
        symbol: "BTCUSDT".into(),
        lower_price: 60000.0,
        upper_price: 70000.0,
        levels: 11, // every 1000 USDT
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
    Grid::new(&cfg, "0.01".into(), "0.00001".into(), 0.00001, 5.0)
}

#[test]
fn full_grid_lifecycle() {
    let g = test_grid();

    // Mid at 65000 → 5 buys below, 5 sells above, skip exact mid
    let plan = g.plan_initial(65000.0);
    let buys: Vec<_> = plan.iter().filter(|p| p.side == Side::Buy).collect();
    let sells: Vec<_> = plan.iter().filter(|p| p.side == Side::Sell).collect();
    assert_eq!(buys.len(), 5);
    assert_eq!(sells.len(), 5);

    // All buy prices should be < 65000
    for b in &buys {
        let price: f64 = b.price_str.parse().unwrap();
        assert!(price < 65000.0, "Buy at {price} should be below mid");
    }
    // All sell prices should be > 65000
    for s in &sells {
        let price: f64 = s.price_str.parse().unwrap();
        assert!(price > 65000.0, "Sell at {price} should be above mid");
    }
}

#[test]
fn replacement_chain_buy_up_sell_down() {
    let g = test_grid();
    // Level 3 (63000) buy filled → should place sell at level 4 (64000)
    let rep = g.plan_replacement(3, Side::Buy).unwrap();
    assert_eq!(rep.side, Side::Sell);
    assert_eq!(rep.level_index, 4);

    // That sell at level 4 fills → should place buy at level 3
    let rep2 = g.plan_replacement(4, Side::Sell).unwrap();
    assert_eq!(rep2.side, Side::Buy);
    assert_eq!(rep2.level_index, 3);
}

#[test]
fn replacement_at_boundaries_returns_none() {
    let g = test_grid();
    // Sell fill at level 0 → can't buy at level -1
    assert!(g.plan_replacement(0, Side::Sell).is_none());
    // Buy fill at top level → can't sell above
    assert!(g.plan_replacement(10, Side::Buy).is_none());
}

#[test]
fn all_levels_are_monotonically_increasing() {
    let levels = grid::compute_levels(100.0, 200.0, 50);
    for w in levels.windows(2) {
        assert!(
            w[1] > w[0],
            "Levels must be increasing: {} > {}",
            w[1],
            w[0]
        );
    }
}

#[test]
fn level_spacing_is_uniform() {
    let levels = grid::compute_levels(100.0, 200.0, 11);
    let step = levels[1] - levels[0];
    for w in levels.windows(2) {
        let diff = w[1] - w[0];
        assert!(
            (diff - step).abs() < 1e-10,
            "Spacing should be uniform: {diff} vs {step}"
        );
    }
}

#[test]
fn qty_respects_base_precision() {
    let cfg = GridCfg {
        symbol: "ETHUSDT".into(),
        lower_price: 3000.0,
        upper_price: 3500.0,
        levels: 6,
        qty_per_level_base: 0.123456789,
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
    let g = Grid::new(&cfg, "0.01".into(), "0.001".into(), 0.001, 5.0);
    let plan = g.plan_initial(3250.0);
    for p in &plan {
        let parts: Vec<&str> = p.qty_str.split('.').collect();
        if parts.len() == 2 {
            // Precision of "0.001" is 3 decimal places
            assert!(parts[1].len() <= 3, "Too many decimals: {}", p.qty_str);
        }
    }
}

#[test]
fn format_to_increment_precision() {
    assert_eq!(grid::format_to_increment(123.456, "0.01"), "123.45");
    assert_eq!(grid::format_to_increment(123.456, "0.1"), "123.4");
    assert_eq!(grid::format_to_increment(123.999, "1"), "123");
    assert_eq!(grid::format_to_increment(0.0001234, "0.00001"), "0.00012");
}
