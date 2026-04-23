//! Client-side rate limiter — token bucket algorithm.
//!
//! Bybit allows 600 orders per 5 seconds on the trading endpoint.
//! This module ensures we never exceed that.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::debug;

pub struct RateLimiter {
    inner: Mutex<Bucket>,
}

struct Bucket {
    capacity: u32,
    tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a rate limiter with `capacity` tokens that refill at `per_seconds` interval.
    /// E.g., `new(600, 5)` = 600 requests per 5 seconds.
    pub fn new(capacity: u32, per_seconds: u64) -> Self {
        let refill_rate = capacity as f64 / per_seconds as f64;
        Self {
            inner: Mutex::new(Bucket {
                capacity,
                tokens: capacity as f64,
                refill_rate,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Try to acquire a token. Returns Ok(()) if allowed, or the Duration to wait.
    pub fn try_acquire(&self) -> Result<(), Duration> {
        let mut b = self.inner.lock().unwrap();
        b.refill();
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            let wait = Duration::from_secs_f64((1.0 - b.tokens) / b.refill_rate);
            Err(wait)
        }
    }

    /// Acquire a token, sleeping if necessary.
    pub async fn acquire(&self) {
        loop {
            match self.try_acquire() {
                Ok(()) => return,
                Err(wait) => {
                    debug!(wait_ms = wait.as_millis() as u64, "rate limit: waiting");
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }

    pub fn available_tokens(&self) -> u32 {
        let mut b = self.inner.lock().unwrap();
        b.refill();
        b.tokens as u32
    }
}

impl Bucket {
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity as f64);
        self.last_refill = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_rate_limiting() {
        let rl = RateLimiter::new(3, 1); // 3 per second
        assert!(rl.try_acquire().is_ok());
        assert!(rl.try_acquire().is_ok());
        assert!(rl.try_acquire().is_ok());
        // 4th should fail (no tokens left)
        assert!(rl.try_acquire().is_err());
    }

    #[test]
    fn tokens_show_count() {
        let rl = RateLimiter::new(10, 1);
        assert_eq!(rl.available_tokens(), 10);
        rl.try_acquire().unwrap();
        assert_eq!(rl.available_tokens(), 9);
    }
}
