//! Lock-free connection pool latency histogram for DarshanDB.
//!
//! Provides a `PoolStats` struct that tracks query latencies using atomic
//! counters in fixed-width histogram buckets. Designed for concurrent access
//! from multiple async tasks with zero contention (relaxed memory ordering
//! on non-critical stats counters).
//!
//! # Exposed via `/health`
//!
//! The `snapshot()` method returns a JSON object with:
//! - `connections.active` / `connections.idle` / `connections.max`
//! - `latency.avg_us` / `latency.p50_us` / `latency.p95_us` / `latency.p99_us`

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

/// Lock-free latency histogram using atomic counters.
pub struct PoolStats {
    /// Bucket boundaries in microseconds.
    bucket_boundaries_us: Vec<u64>,
    /// Atomic counters for each bucket (one extra for overflow).
    bucket_counts: Vec<AtomicU64>,
    /// Total observations.
    total_count: AtomicU64,
    /// Sum of all observed latencies in microseconds.
    total_sum_us: AtomicU64,
    /// Minimum observed latency in microseconds.
    min_us: AtomicU64,
    /// Maximum observed latency in microseconds.
    max_us: AtomicU64,
}

impl PoolStats {
    /// Create a new latency histogram with default bucket boundaries.
    ///
    /// Buckets: 100us, 500us, 1ms, 5ms, 10ms, 50ms, 100ms, 500ms, 1s.
    pub fn new() -> Self {
        let boundaries = vec![
            100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000,
        ];
        let bucket_counts = (0..=boundaries.len()).map(|_| AtomicU64::new(0)).collect();
        Self {
            bucket_boundaries_us: boundaries,
            bucket_counts,
            total_count: AtomicU64::new(0),
            total_sum_us: AtomicU64::new(0),
            min_us: AtomicU64::new(u64::MAX),
            max_us: AtomicU64::new(0),
        }
    }

    /// Record an observed latency duration.
    pub fn record(&self, duration: Duration) {
        let us = duration.as_micros() as u64;
        self.total_count.fetch_add(1, Ordering::Relaxed);
        self.total_sum_us.fetch_add(us, Ordering::Relaxed);

        // Update min (CAS loop).
        let mut current_min = self.min_us.load(Ordering::Relaxed);
        while us < current_min {
            match self.min_us.compare_exchange_weak(
                current_min,
                us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }

        // Update max (CAS loop).
        let mut current_max = self.max_us.load(Ordering::Relaxed);
        while us > current_max {
            match self.max_us.compare_exchange_weak(
                current_max,
                us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }

        // Find bucket and increment.
        let idx = self
            .bucket_boundaries_us
            .iter()
            .position(|&b| us <= b)
            .unwrap_or(self.bucket_boundaries_us.len());
        self.bucket_counts[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Compute a percentile (0.0 to 1.0) from the histogram buckets.
    /// Returns the upper boundary of the bucket containing the target percentile.
    fn percentile(&self, p: f64) -> u64 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let target = ((total as f64) * p).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, count) in self.bucket_counts.iter().enumerate() {
            cumulative += count.load(Ordering::Relaxed);
            if cumulative >= target {
                if i < self.bucket_boundaries_us.len() {
                    return self.bucket_boundaries_us[i];
                }
                return self.max_us.load(Ordering::Relaxed);
            }
        }
        self.max_us.load(Ordering::Relaxed)
    }

    /// Return a JSON snapshot of pool and latency statistics.
    pub fn snapshot(&self, pool: &PgPool) -> Value {
        let total = self.total_count.load(Ordering::Relaxed);
        let sum = self.total_sum_us.load(Ordering::Relaxed);
        let min = self.min_us.load(Ordering::Relaxed);
        let max = self.max_us.load(Ordering::Relaxed);
        let avg_us = if total > 0 { sum / total } else { 0 };

        serde_json::json!({
            "connections": {
                "active": pool.size() - pool.num_idle() as u32,
                "idle": pool.num_idle(),
                "max": pool.options().get_max_connections(),
                "min": pool.options().get_min_connections(),
            },
            "latency": {
                "total_queries": total,
                "avg_us": avg_us,
                "min_us": if min == u64::MAX { 0 } else { min },
                "max_us": max,
                "p50_us": self.percentile(0.50),
                "p95_us": self.percentile(0.95),
                "p99_us": self.percentile(0.99),
            }
        })
    }
}

impl Default for PoolStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_returns_zeros() {
        let stats = PoolStats::new();
        // Need a pool for snapshot, but we can test percentile directly.
        assert_eq!(stats.percentile(0.50), 0);
        assert_eq!(stats.percentile(0.95), 0);
        assert_eq!(stats.percentile(0.99), 0);
        assert_eq!(stats.total_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn record_updates_counts() {
        let stats = PoolStats::new();

        stats.record(Duration::from_micros(50)); // bucket 0 (<=100)
        stats.record(Duration::from_micros(200)); // bucket 1 (<=500)
        stats.record(Duration::from_millis(2)); // bucket 3 (<=5000)

        assert_eq!(stats.total_count.load(Ordering::Relaxed), 3);
        assert_eq!(stats.min_us.load(Ordering::Relaxed), 50);
        assert_eq!(stats.max_us.load(Ordering::Relaxed), 2000);
    }

    #[test]
    fn percentile_single_value() {
        let stats = PoolStats::new();
        stats.record(Duration::from_micros(300)); // bucket 1 (<=500)

        assert_eq!(stats.percentile(0.50), 500);
        assert_eq!(stats.percentile(0.95), 500);
        assert_eq!(stats.percentile(0.99), 500);
    }

    #[test]
    fn percentile_distribution() {
        let stats = PoolStats::new();

        // 50 observations in the 100us bucket.
        for _ in 0..50 {
            stats.record(Duration::from_micros(80));
        }
        // 40 observations in the 1ms bucket.
        for _ in 0..40 {
            stats.record(Duration::from_micros(800));
        }
        // 10 observations in the 10ms bucket.
        for _ in 0..10 {
            stats.record(Duration::from_millis(8));
        }

        // p50 should be in the 100us bucket (50th of 100 = bucket 0).
        assert_eq!(stats.percentile(0.50), 100);
        // p95: target=95, cumulative at bucket[2](<=1000)=90, next hit is bucket[4](<=10000)=100.
        assert_eq!(stats.percentile(0.95), 10_000);
        // p99: target=99, same bucket[4](<=10000) reaches 100.
        assert_eq!(stats.percentile(0.99), 10_000);
    }

    #[test]
    fn min_max_tracking() {
        let stats = PoolStats::new();
        stats.record(Duration::from_micros(500));
        stats.record(Duration::from_micros(100));
        stats.record(Duration::from_micros(1000));

        assert_eq!(stats.min_us.load(Ordering::Relaxed), 100);
        assert_eq!(stats.max_us.load(Ordering::Relaxed), 1000);
    }

    #[test]
    fn overflow_bucket() {
        let stats = PoolStats::new();
        // 2 seconds > 1_000_000us boundary = overflow bucket.
        stats.record(Duration::from_secs(2));

        assert_eq!(stats.total_count.load(Ordering::Relaxed), 1);
        // p50 should return max_us since it's in the overflow bucket.
        assert_eq!(stats.percentile(0.50), 2_000_000);
    }
}
