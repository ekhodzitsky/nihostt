//! Per-IP token-bucket rate limiter.
//!
//! Disabled by default; enable via `--rate-limit-per-minute N`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
    refill_per_minute: u32,
    burst: u32,
}

struct TokenBucket {
    tokens: f64,
    last_update: Instant,
}

impl RateLimiter {
    pub fn new(refill_per_minute: u32, burst: u32) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            refill_per_minute,
            burst,
        }
    }

    pub fn check(&self, ip: IpAddr) -> bool {
        if self.refill_per_minute == 0 {
            return true;
        }

        let now = Instant::now();
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = buckets.entry(ip).or_insert_with(|| TokenBucket {
            tokens: self.burst as f64,
            last_update: now,
        });

        let elapsed = now.duration_since(entry.last_update).as_secs_f64();
        entry.tokens += elapsed * (self.refill_per_minute as f64 / 60.0);
        entry.tokens = entry.tokens.min(self.burst as f64);
        entry.last_update = now;

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_refill_disables_rate_limit() {
        let limiter = RateLimiter::new(0, 0);
        let ip = "127.0.0.1".parse().expect("valid ip");

        for _ in 0..10 {
            assert!(limiter.check(ip));
        }
    }

    #[test]
    fn burst_is_consumed_per_ip() {
        let limiter = RateLimiter::new(1, 2);
        let ip = "127.0.0.1".parse().expect("valid ip");

        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(!limiter.check(ip));
    }
}
