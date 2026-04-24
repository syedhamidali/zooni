#![allow(dead_code)]
//! Lightweight order-timeout tracker. Tracks pending orders and reports
//! which ones have exceeded their allowed fill time so the caller can cancel them.

use std::time::{Duration, Instant};

pub struct OrderTimeout {
    pub order_id: String,
    pub submitted_at: Instant,
    pub timeout: Duration,
    /// Target quantity for this order.
    pub target_qty: f64,
    /// Cumulative quantity filled so far.
    pub filled_qty: f64,
}

pub struct OrderManager {
    pending: Vec<OrderTimeout>,
    default_timeout: Duration,
}

impl OrderManager {
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            pending: Vec::new(),
            default_timeout,
        }
    }

    pub fn track(&mut self, order_id: String) {
        self.pending.push(OrderTimeout {
            order_id,
            submitted_at: Instant::now(),
            timeout: self.default_timeout,
            target_qty: 0.0,
            filled_qty: 0.0,
        });
    }

    pub fn track_with_qty(&mut self, order_id: String, target_qty: f64) {
        self.pending.push(OrderTimeout {
            order_id,
            submitted_at: Instant::now(),
            timeout: self.default_timeout,
            target_qty,
            filled_qty: 0.0,
        });
    }

    pub fn track_with_timeout(&mut self, order_id: String, timeout: Duration) {
        self.pending.push(OrderTimeout {
            order_id,
            submitted_at: Instant::now(),
            timeout,
            target_qty: 0.0,
            filled_qty: 0.0,
        });
    }

    /// Record a partial or full fill. Returns true if the order is now fully filled
    /// (filled_qty >= target_qty). A fully filled order is automatically removed.
    pub fn record_fill(&mut self, order_id: &str, qty: f64) -> bool {
        let mut fully_filled = false;
        if let Some(o) = self.pending.iter_mut().find(|o| o.order_id == order_id) {
            o.filled_qty += qty;
            if o.target_qty > 0.0 && o.filled_qty >= o.target_qty - 1e-12 {
                fully_filled = true;
            }
        }
        if fully_filled {
            self.pending.retain(|o| o.order_id != order_id);
        }
        fully_filled
    }

    /// Get the filled quantity for a pending order.
    pub fn filled_qty(&self, order_id: &str) -> f64 {
        self.pending
            .iter()
            .find(|o| o.order_id == order_id)
            .map(|o| o.filled_qty)
            .unwrap_or(0.0)
    }

    /// Check if an order is fully filled (filled_qty >= target_qty).
    pub fn is_fully_filled(&self, order_id: &str) -> bool {
        // If it's not in pending, it was already removed (fully filled or cancelled)
        !self.pending.iter().any(|o| o.order_id == order_id)
    }

    pub fn filled(&mut self, order_id: &str) {
        self.pending.retain(|o| o.order_id != order_id);
    }

    pub fn expired_orders(&mut self) -> Vec<String> {
        let now = Instant::now();
        let mut expired = Vec::new();
        self.pending.retain(|o| {
            if now.duration_since(o.submitted_at) >= o.timeout {
                expired.push(o.order_id.clone());
                false
            } else {
                true
            }
        });
        expired
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_and_fill_removes_order() {
        let mut mgr = OrderManager::new(Duration::from_secs(5));
        mgr.track("ORD-1".into());
        assert_eq!(mgr.pending_count(), 1);
        mgr.filled("ORD-1");
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn expired_orders_returns_timed_out() {
        let mut mgr = OrderManager::new(Duration::from_secs(5));
        // Manually insert an order that was submitted far in the past.
        mgr.pending.push(OrderTimeout {
            order_id: "OLD-1".into(),
            submitted_at: Instant::now() - Duration::from_secs(10),
            timeout: Duration::from_secs(5),
            target_qty: 0.0,
            filled_qty: 0.0,
        });
        mgr.track("NEW-1".into());

        let expired = mgr.expired_orders();
        assert_eq!(expired, vec!["OLD-1".to_string()]);
        assert_eq!(mgr.pending_count(), 1); // NEW-1 still pending
    }

    #[test]
    fn pending_count() {
        let mut mgr = OrderManager::new(Duration::from_secs(5));
        assert_eq!(mgr.pending_count(), 0);
        mgr.track("A".into());
        mgr.track("B".into());
        assert_eq!(mgr.pending_count(), 2);
        mgr.filled("A");
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn custom_timeout() {
        let mut mgr = OrderManager::new(Duration::from_secs(60));
        // Insert with a custom short timeout that is already expired.
        mgr.pending.push(OrderTimeout {
            order_id: "FAST-1".into(),
            submitted_at: Instant::now() - Duration::from_millis(200),
            timeout: Duration::from_millis(100),
            target_qty: 0.0,
            filled_qty: 0.0,
        });

        let expired = mgr.expired_orders();
        assert_eq!(expired, vec!["FAST-1".to_string()]);
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn fill_nonexistent_is_noop() {
        let mut mgr = OrderManager::new(Duration::from_secs(5));
        mgr.track("A".into());
        mgr.filled("DOES-NOT-EXIST"); // should not panic
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn partial_fill_tracking() {
        let mut mgr = OrderManager::new(Duration::from_secs(60));
        mgr.track_with_qty("ORD-1".into(), 10.0);
        assert_eq!(mgr.pending_count(), 1);

        // Partial fill: 3 of 10
        let full = mgr.record_fill("ORD-1", 3.0);
        assert!(!full, "should not be fully filled yet");
        assert_eq!(mgr.pending_count(), 1);
        assert!((mgr.filled_qty("ORD-1") - 3.0).abs() < 1e-9);

        // Another partial: 5 of 10 (total 8)
        let full = mgr.record_fill("ORD-1", 5.0);
        assert!(!full);
        assert!((mgr.filled_qty("ORD-1") - 8.0).abs() < 1e-9);

        // Final fill: 2 of 10 (total 10) → fully filled, removed
        let full = mgr.record_fill("ORD-1", 2.0);
        assert!(full, "should be fully filled now");
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn is_fully_filled_after_removal() {
        let mut mgr = OrderManager::new(Duration::from_secs(5));
        mgr.track_with_qty("ORD-1".into(), 1.0);
        assert!(!mgr.is_fully_filled("ORD-1"));
        mgr.record_fill("ORD-1", 1.0);
        assert!(mgr.is_fully_filled("ORD-1"));
    }
}
