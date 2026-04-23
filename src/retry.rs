//! Exponential backoff retry with jitter + circuit breaker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use tracing::{info, warn};

/// Retry config.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// Execute `f` with exponential backoff. Returns the first success or the last error.
pub async fn with_retry<F, Fut, T>(policy: &RetryPolicy, op_name: &str, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=policy.max_retries {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt == policy.max_retries {
                    last_err = Some(e);
                    break;
                }
                let delay = backoff_delay(attempt, policy.base_delay, policy.max_delay);
                warn!(
                    op = op_name,
                    attempt = attempt + 1,
                    max = policy.max_retries,
                    delay_ms = delay.as_millis() as u64,
                    err = %e,
                    "retrying after error",
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
    Err(last_err.unwrap())
}

fn backoff_delay(attempt: u32, base: Duration, max: Duration) -> Duration {
    let exp = base.as_millis() as u64 * 2u64.pow(attempt);
    // Add ~25% jitter
    let jitter = exp / 4;
    let ms = exp + (simple_rand() % (jitter.max(1)));
    Duration::from_millis(ms.min(max.as_millis() as u64))
}

/// Simple deterministic-enough random using atomics; avoids pulling in `rand`.
fn simple_rand() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // xorshift-style mixing
    let mut x = n
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x
}

/// Circuit breaker — trips after `threshold` consecutive failures, auto-resets after `cooldown`.
pub struct CircuitBreaker {
    threshold: u32,
    cooldown: Duration,
    consecutive_failures: AtomicU64,
    last_failure_ms: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            threshold,
            cooldown,
            consecutive_failures: AtomicU64::new(0),
            last_failure_ms: AtomicU64::new(0),
        }
    }

    pub fn is_open(&self) -> bool {
        let failures = self.consecutive_failures.load(Ordering::Relaxed) as u32;
        if failures < self.threshold {
            return false;
        }
        // Check if cooldown has passed
        let last = self.last_failure_ms.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now.saturating_sub(last) > self.cooldown.as_millis() as u64 {
            // Cooldown passed, auto-reset
            self.consecutive_failures.store(0, Ordering::Relaxed);
            info!("circuit breaker reset after cooldown");
            return false;
        }
        true
    }

    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_failure_ms.store(now, Ordering::Relaxed);
        let count = self.consecutive_failures.load(Ordering::Relaxed);
        if count as u32 >= self.threshold {
            warn!(
                failures = count,
                cooldown_secs = self.cooldown.as_secs(),
                "circuit breaker OPEN — pausing API calls",
            );
        }
    }

    pub fn failure_count(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_breaker_stays_closed_under_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open());
        assert_eq!(cb.failure_count(), 2);
    }

    #[test]
    fn circuit_breaker_opens_at_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
        assert!(!cb.is_open());
    }

    #[test]
    fn backoff_increases_exponentially() {
        let d0 = backoff_delay(0, Duration::from_millis(100), Duration::from_secs(10));
        let d1 = backoff_delay(1, Duration::from_millis(100), Duration::from_secs(10));
        let d2 = backoff_delay(2, Duration::from_millis(100), Duration::from_secs(10));
        // d1 should be roughly 2x d0, d2 roughly 4x d0 (with jitter)
        assert!(d1 > d0, "d1={:?} should be > d0={:?}", d1, d0);
        assert!(d2 > d1, "d2={:?} should be > d1={:?}", d2, d1);
    }

    #[test]
    fn backoff_respects_max() {
        let d = backoff_delay(20, Duration::from_millis(100), Duration::from_secs(5));
        assert!(d <= Duration::from_secs(5));
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_try() {
        let policy = RetryPolicy {
            max_retries: 3,
            ..Default::default()
        };
        let result = with_retry(&policy, "test", || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_fails_after_max() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        };
        let result = with_retry(&policy, "test", || async {
            Err::<i32, _>(anyhow::anyhow!("always fails"))
        })
        .await;
        assert!(result.is_err());
    }
}
