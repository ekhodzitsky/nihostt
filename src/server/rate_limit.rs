//! Per-IP token-bucket rate limiter.
//!
//! Disabled by default; enable via `--rate-limit-per-minute N`.

use dashmap::DashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    buckets: Arc<DashMap<IpAddr, TokenBucket>>,
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
            buckets: Arc::new(DashMap::new()),
            refill_per_minute,
            burst,
        }
    }

    pub fn check(&self, ip: IpAddr) -> bool {
        if self.refill_per_minute == 0 {
            return true;
        }

        let now = Instant::now();
        let mut entry = self.buckets.entry(ip).or_insert_with(|| TokenBucket {
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
