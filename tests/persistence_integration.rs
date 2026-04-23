//! Integration tests for SQLite persistence layer.
//!
//! Uses in-memory database — no files created.

use chrono::Utc;
use zooni::persistence::{Db, FillRow, OrderRow, OrderStatus};
use zooni::types::Side;

fn make_order(id: &str, symbol: &str, side: Side, price: f64, level: usize) -> OrderRow {
    OrderRow {
        order_id: id.into(),
        order_link_id: format!("link-{id}"),
        symbol: symbol.into(),
        side,
        price,
        qty: 0.001,
        level_index: level,
        status: OrderStatus::Open,
        placed_at: Utc::now(),
        filled_at: None,
        filled_qty: 0.0,
        filled_price: 0.0,
    }
}

fn make_fill(id: &str, symbol: &str, side: Side, price: f64, pnl: f64) -> FillRow {
    FillRow {
        order_id: id.into(),
        symbol: symbol.into(),
        side,
        price,
        qty: 0.001,
        notional: price * 0.001,
        realised_pnl: pnl,
        filled_at: Utc::now(),
    }
}

#[test]
fn full_order_lifecycle() {
    let db = Db::open_in_memory().unwrap();

    // Place 5 buy orders
    for i in 0..5 {
        let o = make_order(
            &format!("buy-{i}"),
            "BTCUSDT",
            Side::Buy,
            60000.0 + i as f64 * 1000.0,
            i,
        );
        db.insert_order(&o).unwrap();
    }

    let open = db.open_orders("BTCUSDT").unwrap();
    assert_eq!(open.len(), 5);

    // Fill 2
    db.mark_filled("buy-0", 60000.0, 0.001).unwrap();
    db.mark_filled("buy-1", 61000.0, 0.001).unwrap();

    let open = db.open_orders("BTCUSDT").unwrap();
    assert_eq!(open.len(), 3);

    // Cancel 1
    db.mark_cancelled("buy-2").unwrap();
    let open = db.open_orders("BTCUSDT").unwrap();
    assert_eq!(open.len(), 2);
}

#[test]
fn pnl_aggregation_across_fills() {
    let db = Db::open_in_memory().unwrap();

    // Mix of winning and losing fills
    let fills = vec![
        make_fill("f1", "BTCUSDT", Side::Sell, 66000.0, 5.0),
        make_fill("f2", "BTCUSDT", Side::Sell, 67000.0, 8.0),
        make_fill("f3", "BTCUSDT", Side::Sell, 64000.0, -3.0),
        make_fill("f4", "ETHUSDT", Side::Sell, 3500.0, 2.0),
    ];
    for f in &fills {
        db.insert_fill(f).unwrap();
    }

    let btc_pnl = db.total_realised_pnl("BTCUSDT").unwrap();
    assert!((btc_pnl - 10.0).abs() < 1e-9, "BTC PnL: {btc_pnl}");

    let eth_pnl = db.total_realised_pnl("ETHUSDT").unwrap();
    assert!((eth_pnl - 2.0).abs() < 1e-9, "ETH PnL: {eth_pnl}");

    let btc_count = db.total_fill_count("BTCUSDT").unwrap();
    assert_eq!(btc_count, 3);

    let eth_count = db.total_fill_count("ETHUSDT").unwrap();
    assert_eq!(eth_count, 1);
}

#[test]
fn bot_state_persistence() {
    let db = Db::open_in_memory().unwrap();

    // No state initially
    assert_eq!(db.get_state("active_symbol").unwrap(), None);

    // Set and retrieve
    db.set_state("active_symbol", "BTCUSDT").unwrap();
    db.set_state("grid_lower", "60000").unwrap();
    assert_eq!(
        db.get_state("active_symbol").unwrap(),
        Some("BTCUSDT".into())
    );
    assert_eq!(db.get_state("grid_lower").unwrap(), Some("60000".into()));

    // Overwrite
    db.set_state("active_symbol", "ETHUSDT").unwrap();
    assert_eq!(
        db.get_state("active_symbol").unwrap(),
        Some("ETHUSDT".into())
    );
}

#[test]
fn concurrent_symbols_are_isolated() {
    let db = Db::open_in_memory().unwrap();

    db.insert_order(&make_order("btc-1", "BTCUSDT", Side::Buy, 65000.0, 0))
        .unwrap();
    db.insert_order(&make_order("btc-2", "BTCUSDT", Side::Buy, 64000.0, 1))
        .unwrap();
    db.insert_order(&make_order("eth-1", "ETHUSDT", Side::Buy, 3400.0, 0))
        .unwrap();

    assert_eq!(db.open_orders("BTCUSDT").unwrap().len(), 2);
    assert_eq!(db.open_orders("ETHUSDT").unwrap().len(), 1);
    assert_eq!(db.open_orders("SOLUSDT").unwrap().len(), 0);

    // Filling BTC order doesn't affect ETH
    db.mark_filled("btc-1", 65000.0, 0.001).unwrap();
    assert_eq!(db.open_orders("BTCUSDT").unwrap().len(), 1);
    assert_eq!(db.open_orders("ETHUSDT").unwrap().len(), 1);
}
