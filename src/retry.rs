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
    let mut x = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
