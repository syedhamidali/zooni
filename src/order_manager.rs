#![allow(dead_code)]
//! Lightweight order-timeout tracker. Tracks pending orders and reports
//! which ones have exceeded their allowed fill time so the caller can cancel them.

use std::time::{Duration, Instant};

pub struct OrderTimeout {
    pub order_id: String,
    pub submitted_at: Instant,
    pub timeout: Duration,
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
        });
    }

    pub fn track_with_timeout(&mut self, order_id: String, timeout: Duration) {
        self.pending.push(OrderTimeout {
            order_id,
            submitted_at: Instant::now(),
            timeout,
        });
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
}
