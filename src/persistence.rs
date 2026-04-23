//! SQLite-backed persistence for order state, fills, and P&L.
//!
//! Every placed order is journalled before the REST call returns.
//! On restart the bot loads surviving orders and reconciles against the exchange.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::types::Side;

/// One row in the `orders` table.
#[derive(Debug, Clone)]
pub struct OrderRow {
    pub order_id: String,
    pub order_link_id: String,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
    pub level_index: usize,
    pub status: OrderStatus,
    pub placed_at: DateTime<Utc>,
    pub filled_at: Option<DateTime<Utc>>,
    pub filled_qty: f64,
    pub filled_price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    Filled,
    Cancelled,
    PartiallyFilled,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Filled => "filled",
            Self::Cancelled => "cancelled",
            Self::PartiallyFilled => "partial",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "filled" => Self::Filled,
            "cancelled" => Self::Cancelled,
            "partial" => Self::PartiallyFilled,
            _ => Self::Open,
        }
    }
}

/// One row in the `fills` table — tracks realised P&L per fill.
#[derive(Debug, Clone)]
pub struct FillRow {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub qty: f64,
    pub notional: f64,
    pub realised_pnl: f64,
    pub filled_at: DateTime<Utc>,
}

/// One row in the `daily_pnl` table.
#[derive(Debug, Clone)]
pub struct DailyPnlRow {
    pub date: String, // "YYYY-MM-DD"
    pub realised_pnl: f64,
    pub fees: f64,
    pub net_pnl: f64,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).context("opening SQLite database")?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .context("setting pragmas")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory SQLite")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS orders (
                    order_id       TEXT PRIMARY KEY,
                    order_link_id  TEXT NOT NULL DEFAULT '',
                    symbol         TEXT NOT NULL,
                    side           TEXT NOT NULL,
                    price          REAL NOT NULL,
                    qty            REAL NOT NULL,
                    level_index    INTEGER NOT NULL,
                    status         TEXT NOT NULL DEFAULT 'open',
                    placed_at      TEXT NOT NULL,
                    filled_at      TEXT,
                    filled_qty     REAL NOT NULL DEFAULT 0,
                    filled_price   REAL NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS fills (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    order_id     TEXT NOT NULL,
                    symbol       TEXT NOT NULL,
                    side         TEXT NOT NULL,
                    price        REAL NOT NULL,
                    qty          REAL NOT NULL,
                    notional     REAL NOT NULL,
                    realised_pnl REAL NOT NULL DEFAULT 0,
                    filled_at    TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS daily_pnl (
                    date         TEXT PRIMARY KEY,
                    realised_pnl REAL NOT NULL DEFAULT 0,
                    fees         REAL NOT NULL DEFAULT 0,
                    net_pnl      REAL NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS bot_state (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_orders_symbol_status ON orders(symbol, status);
                CREATE INDEX IF NOT EXISTS idx_fills_symbol ON fills(symbol);
                CREATE INDEX IF NOT EXISTS idx_fills_filled_at ON fills(filled_at);
                ",
            )
            .context("running migrations")?;
        Ok(())
    }

    // -- Orders ---------------------------------------------------------------

    pub fn insert_order(&self, o: &OrderRow) -> Result<()> {
        let side_str = match o.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO orders
                 (order_id, order_link_id, symbol, side, price, qty, level_index, status, placed_at, filled_qty, filled_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    o.order_id,
                    o.order_link_id,
                    o.symbol,
                    side_str,
                    o.price,
                    o.qty,
                    o.level_index as i64,
                    o.status.as_str(),
                    o.placed_at.to_rfc3339(),
                    o.filled_qty,
                    o.filled_price,
                ],
            )
            .context("inserting order")?;
        Ok(())
    }

    pub fn mark_filled(&self, order_id: &str, filled_price: f64, filled_qty: f64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE orders SET status = 'filled', filled_at = ?1, filled_price = ?2, filled_qty = ?3 WHERE order_id = ?4",
                params![now, filled_price, filled_qty, order_id],
            )
            .context("marking order filled")?;
        Ok(())
    }

    pub fn mark_cancelled(&self, order_id: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE orders SET status = 'cancelled' WHERE order_id = ?1",
                params![order_id],
            )
            .context("marking order cancelled")?;
        Ok(())
    }

    pub fn open_orders(&self, symbol: &str) -> Result<Vec<OrderRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT order_id, order_link_id, symbol, side, price, qty, level_index, status, placed_at, filled_at, filled_qty, filled_price
             FROM orders WHERE symbol = ?1 AND status = 'open'",
        )?;
        let rows = stmt
            .query_map(params![symbol], |row| {
                let side_str: String = row.get(3)?;
                let status_str: String = row.get(7)?;
                let placed_str: String = row.get(8)?;
                let filled_str: Option<String> = row.get(9)?;
                Ok(OrderRow {
                    order_id: row.get(0)?,
                    order_link_id: row.get(1)?,
                    symbol: row.get(2)?,
                    side: if side_str == "buy" {
                        Side::Buy
                    } else {
                        Side::Sell
                    },
                    price: row.get(4)?,
                    qty: row.get(5)?,
                    level_index: row.get::<_, i64>(6)? as usize,
                    status: OrderStatus::from_str(&status_str),
                    placed_at: DateTime::parse_from_rfc3339(&placed_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    filled_at: filled_str.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    }),
                    filled_qty: row.get(10)?,
                    filled_price: row.get(11)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("reading open orders")?;
        Ok(rows)
    }

    // -- Fills ----------------------------------------------------------------

    pub fn insert_fill(&self, f: &FillRow) -> Result<()> {
        let side_str = match f.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        self.conn
            .execute(
                "INSERT INTO fills (order_id, symbol, side, price, qty, notional, realised_pnl, filled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    f.order_id,
                    f.symbol,
                    side_str,
                    f.price,
                    f.qty,
                    f.notional,
                    f.realised_pnl,
                    f.filled_at.to_rfc3339(),
                ],
            )
            .context("inserting fill")?;
        Ok(())
    }

    // -- P&L ------------------------------------------------------------------

    pub fn daily_pnl(&self, date: &str) -> Result<f64> {
        let pnl: f64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(realised_pnl), 0) FROM fills WHERE filled_at LIKE ?1",
                params![format!("{date}%")],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        Ok(pnl)
    }

    pub fn weekly_pnl(&self, week_start: &str, week_end: &str) -> Result<f64> {
        let pnl: f64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(realised_pnl), 0) FROM fills WHERE filled_at >= ?1 AND filled_at < ?2",
                params![week_start, week_end],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        Ok(pnl)
    }

    pub fn total_realised_pnl(&self, symbol: &str) -> Result<f64> {
        let pnl: f64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(realised_pnl), 0) FROM fills WHERE symbol = ?1",
                params![symbol],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        Ok(pnl)
    }

    pub fn total_fill_count(&self, symbol: &str) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fills WHERE symbol = ?1",
                params![symbol],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count)
    }

    // -- Bot state ------------------------------------------------------------

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO bot_state (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .context("setting bot state")?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM bot_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_order() {
        let db = Db::open_in_memory().unwrap();
        let o = OrderRow {
            order_id: "ord-1".into(),
            order_link_id: "link-1".into(),
            symbol: "BTCUSDT".into(),
            side: Side::Buy,
            price: 65000.0,
            qty: 0.001,
            level_index: 3,
            status: OrderStatus::Open,
            placed_at: Utc::now(),
            filled_at: None,
            filled_qty: 0.0,
            filled_price: 0.0,
        };
        db.insert_order(&o).unwrap();
        let orders = db.open_orders("BTCUSDT").unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, "ord-1");
        assert_eq!(orders[0].level_index, 3);

        db.mark_filled("ord-1", 65000.0, 0.001).unwrap();
        let orders = db.open_orders("BTCUSDT").unwrap();
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn fill_pnl_tracking() {
        let db = Db::open_in_memory().unwrap();
        let f = FillRow {
            order_id: "ord-1".into(),
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            price: 66000.0,
            qty: 0.001,
            notional: 66.0,
            realised_pnl: 1.0,
            filled_at: Utc::now(),
        };
        db.insert_fill(&f).unwrap();
        let pnl = db.total_realised_pnl("BTCUSDT").unwrap();
        assert!((pnl - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mark_cancelled_removes_from_open() {
        let db = Db::open_in_memory().unwrap();
        let o = OrderRow {
            order_id: "ord-cancel".into(),
            order_link_id: "link-c".into(),
            symbol: "ETHUSDT".into(),
            side: Side::Sell,
            price: 3500.0,
            qty: 0.1,
            level_index: 5,
            status: OrderStatus::Open,
            placed_at: Utc::now(),
            filled_at: None,
            filled_qty: 0.0,
            filled_price: 0.0,
        };
        db.insert_order(&o).unwrap();
        assert_eq!(db.open_orders("ETHUSDT").unwrap().len(), 1);

        db.mark_cancelled("ord-cancel").unwrap();
        assert_eq!(db.open_orders("ETHUSDT").unwrap().len(), 0);
    }

    #[test]
    fn multiple_fills_sum_pnl() {
        let db = Db::open_in_memory().unwrap();
        for (i, pnl) in [(1, 2.5), (2, -1.0), (3, 3.0)] {
            db.insert_fill(&FillRow {
                order_id: format!("ord-{i}"),
                symbol: "BTCUSDT".into(),
                side: Side::Sell,
                price: 65000.0,
                qty: 0.001,
                notional: 65.0,
                realised_pnl: pnl,
                filled_at: Utc::now(),
            })
            .unwrap();
        }
        let total = db.total_realised_pnl("BTCUSDT").unwrap();
        assert!((total - 4.5).abs() < 1e-9);
    }

    #[test]
    fn fill_count_tracks_fills() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.total_fill_count("BTCUSDT").unwrap(), 0);

        for i in 0..5 {
            db.insert_fill(&FillRow {
                order_id: format!("fill-{i}"),
                symbol: "BTCUSDT".into(),
                side: Side::Buy,
                price: 65000.0,
                qty: 0.001,
                notional: 65.0,
                realised_pnl: 0.0,
                filled_at: Utc::now(),
            })
            .unwrap();
        }
        assert_eq!(db.total_fill_count("BTCUSDT").unwrap(), 5);
        assert_eq!(db.total_fill_count("ETHUSDT").unwrap(), 0);
    }

    #[test]
    fn bot_state_round_trip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_state("missing").unwrap(), None);

        db.set_state("last_symbol", "BTCUSDT").unwrap();
        assert_eq!(db.get_state("last_symbol").unwrap(), Some("BTCUSDT".into()));

        db.set_state("last_symbol", "ETHUSDT").unwrap();
        assert_eq!(db.get_state("last_symbol").unwrap(), Some("ETHUSDT".into()));
    }

    #[test]
    fn order_status_from_str_round_trip() {
        let statuses = [
            OrderStatus::Open,
            OrderStatus::Filled,
            OrderStatus::Cancelled,
            OrderStatus::PartiallyFilled,
        ];
        for s in &statuses {
            assert_eq!(OrderStatus::from_str(s.as_str()), *s);
        }
    }

    #[test]
    fn order_status_unknown_defaults_to_open() {
        assert_eq!(OrderStatus::from_str("garbage"), OrderStatus::Open);
    }

    #[test]
    fn open_orders_scoped_by_symbol() {
        let db = Db::open_in_memory().unwrap();
        let make_order = |id: &str, sym: &str| OrderRow {
            order_id: id.into(),
            order_link_id: "".into(),
            symbol: sym.into(),
            side: Side::Buy,
            price: 100.0,
            qty: 1.0,
            level_index: 0,
            status: OrderStatus::Open,
            placed_at: Utc::now(),
            filled_at: None,
            filled_qty: 0.0,
            filled_price: 0.0,
        };
        db.insert_order(&make_order("a", "BTCUSDT")).unwrap();
        db.insert_order(&make_order("b", "BTCUSDT")).unwrap();
        db.insert_order(&make_order("c", "ETHUSDT")).unwrap();

        assert_eq!(db.open_orders("BTCUSDT").unwrap().len(), 2);
        assert_eq!(db.open_orders("ETHUSDT").unwrap().len(), 1);
        assert_eq!(db.open_orders("SOLUSDT").unwrap().len(), 0);
    }
}
