use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub use crate::proto::confidence::flags::resolver::v1::telemetry_data::resolve_rate::Reason;

mod pb {
    pub use super::Reason;
    pub use crate::proto::confidence::flags::resolver::v1::telemetry_data::{
        BucketSpan, ResolveLatency, ResolveRate, StateAge,
    };
    pub use crate::proto::confidence::flags::resolver::v1::TelemetryData;
}

/// Number of reason variants (matches the Reason enum in the proto).
const REASON_COUNT: usize = 21; // 0..=20

/// Lock-free exponential histogram for positive integer observations.
///
/// Bucket boundaries are at `ratio^k` where the ratio is derived from
/// `min_value`, `max_value` and `max_buckets`. The range [min_value, max_value]
/// is covered by `max_buckets` exponential buckets, plus one underflow bucket
/// for values < min_value and one overflow bucket for values > max_value.
pub struct Histogram {
    ln_ratio: f64,
    min_exponent: i32,
    sum: AtomicU32,
    count: AtomicU32,
    /// Layout: [underflow] [bucket 0 .. bucket n-1] [overflow]
    /// Total length: max_buckets + 2
    buckets: Box<[AtomicU32]>,
}

impl Histogram {
    /// Create a new histogram covering [min_value, max_value] with at most
    /// `max_buckets` buckets total, including one underflow and one overflow bucket.
    ///
    /// The ratio is derived as `(max_value / min_value)^(1 / (max_buckets - 3))`.
    pub fn new(min_value: u32, max_value: u32, max_buckets: usize) -> Self {
        let closed_buckets = max_buckets - 2; // exclude underflow + overflow
        let ln_min = (min_value as f64).ln();
        let ln_max = (max_value as f64).ln();
        let ln_ratio = (ln_max - ln_min) / (closed_buckets - 1) as f64;
        let min_exponent = (ln_min / ln_ratio).floor() as i32;
        let buckets: Vec<AtomicU32> = (0..max_buckets).map(|_| AtomicU32::new(0)).collect();
        Histogram {
            ln_ratio,
            min_exponent,
            sum: AtomicU32::new(0),
            count: AtomicU32::new(0),
            buckets: buckets.into_boxed_slice(),
        }
    }

    /// Record an observation.
    pub fn observe(&self, value: u32) {
        self.sum.fetch_add(value, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);

        let idx = if value == 0 {
            0 // underflow
        } else {
            let k = (value as f64).ln() / self.ln_ratio;
            let k = k.floor() as i32 - self.min_exponent;
            // +1 to skip the underflow bucket, clamped to [1, len-1] (overflow)
            (k as usize + 1).clamp(1, self.buckets.len() - 1)
        };
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Drain the histogram into a proto `ResolveLatency` message, resetting all counters.
    ///
    /// Bucket layout: index 0 = underflow, index 1..=n = regular buckets,
    /// index n+1 = overflow. Exponents are offset by `min_exponent`.
    fn snapshot(&self) -> Option<pb::ResolveLatency> {
        let sum = self.sum.swap(0, Ordering::Relaxed);
        let count = self.count.swap(0, Ordering::Relaxed);

        if count == 0 {
            return None;
        }

        let mut spans: Vec<pb::BucketSpan> = Vec::new();
        let mut current_span: Option<(i32, Vec<u32>)> = None;

        for i in 0..self.buckets.len() {
            let v = self.buckets[i].swap(0, Ordering::Relaxed);
            let exponent = self.min_exponent + i as i32 - 1;
            if v > 0 {
                match &mut current_span {
                    Some((_, counts)) => counts.push(v),
                    None => current_span = Some((exponent, vec![v])),
                }
            } else if let Some((offset, counts)) = current_span.take() {
                spans.push(pb::BucketSpan { offset, counts });
            }
        }
        if let Some((offset, counts)) = current_span {
            spans.push(pb::BucketSpan { offset, counts });
        }

        Some(pb::ResolveLatency {
            sum,
            count,
            buckets: spans,
        })
    }
}

/// Concurrent telemetry collector.
///
/// All methods are safe to call from multiple threads without locking.
/// Call [`Telemetry::snapshot`] to drain the current state
/// into a [`TelemetryData`] proto message.
pub struct Telemetry {
    resolve_latency: Histogram,

    /// Resolve rate counters, one per reason variant.
    resolve_rates: Box<[AtomicU32]>,

    /// Millisecond epoch of the last state update.
    last_state_update: AtomicU64,
}

impl Telemetry {
    pub fn new() -> Self {
        let resolve_rates: Vec<AtomicU32> = (0..REASON_COUNT).map(|_| AtomicU32::new(0)).collect();
        Telemetry {
            // 162 exponential buckets from 1 µs to 10s (10_000_000 µs), ratio ≈ 1.1
            resolve_latency: Histogram::new(1, 10_000_000, 162),
            resolve_rates: resolve_rates.into_boxed_slice(),
            last_state_update: AtomicU64::new(0),
        }
    }

    /// Record a resolve latency observation in microseconds.
    pub fn record_latency_us(&self, micros: u32) {
        self.resolve_latency.observe(micros);
    }

    /// Increment the resolve rate counter for the given reason.
    pub fn mark_resolve(&self, reason: pb::Reason) {
        let idx = reason as usize;
        if idx < REASON_COUNT {
            self.resolve_rates[idx].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Update the last known state timestamp (millisecond epoch).
    pub fn set_last_state_update(&self, epoch_ms: u64) {
        self.last_state_update.store(epoch_ms, Ordering::Relaxed);
    }

    /// Drain the current telemetry state into a [`TelemetryData`] proto message.
    ///
    /// Counters are reset to zero. This is not perfectly atomic across all fields,
    /// but each individual counter swap is atomic — acceptable for metrics.
    pub fn snapshot(&self) -> pb::TelemetryData {
        let resolve_latency = self.resolve_latency.snapshot();

        let resolve_rate: Vec<pb::ResolveRate> = self
            .resolve_rates
            .iter()
            .enumerate()
            .filter_map(|(i, counter)| {
                let c = counter.swap(0, Ordering::Relaxed);
                if c > 0 {
                    Some(pb::ResolveRate {
                        count: c,
                        reason: i as i32,
                    })
                } else {
                    None
                }
            })
            .collect();

        let last_update = self.last_state_update.load(Ordering::Relaxed);
        let state_age = if last_update > 0 {
            Some(pb::StateAge {
                last_state_update: last_update,
            })
        } else {
            None
        };

        pb::TelemetryData {
            sdk: None, // SDK info is set elsewhere
            resolve_latency,
            resolve_rate,
            state_age,
        }
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}
