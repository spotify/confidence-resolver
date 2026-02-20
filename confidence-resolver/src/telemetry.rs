use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::ResolveReason;


mod pb {
    pub use crate::proto::confidence::flags::resolver::v1::telemetry_data::{
        BucketSpan, ResolveLatency, ResolveRate, StateAge,
    };
    pub use crate::proto::confidence::flags::resolver::v1::TelemetryData;
}

/// Trait for types that have histogram configuration metadata
pub trait HistogramConfig {
    const MIN_VALUE: u32;
    const MAX_VALUE: u32;
    const BUCKET_COUNT: usize;
    const UNIT: &'static str;
}

include!(concat!(env!("OUT_DIR"), "/telemetry_config.rs"));

/// Number of reason variants (matches the Reason enum in the proto).
/// This is derived from the highest discriminant + 1.
const REASON_COUNT: usize = ResolveReason::Error as usize + 1;

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
    /// Create a histogram for a type that implements HistogramConfig.
    pub fn for_type<T: HistogramConfig>() -> Self {
        Self::new(T::MIN_VALUE, T::MAX_VALUE, T::BUCKET_COUNT)
    }

    /// Create a new histogram covering [min_value, max_value] with at most
    /// `max_buckets` buckets total, including one underflow and one overflow bucket.
    ///
    /// The ratio is derived as `(max_value / min_value)^(1 / (max_buckets - 3))`.
    pub fn new(min_value: u32, max_value: u32, max_buckets: usize) -> Self {
        assert!(max_buckets >= 3, "max_buckets must be at least 3");
        let closed_buckets = max_buckets.saturating_sub(2); // exclude underflow + overflow
        let ln_min = (min_value as f64).ln();
        let ln_max = (max_value as f64).ln();
        let denominator = closed_buckets.saturating_sub(1).max(1) as f64;
        let ln_ratio = (ln_max - ln_min) / denominator;
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
            let k = k.floor() as i32;
            let k = k.saturating_sub(self.min_exponent);
            let last = self.buckets.len().saturating_sub(1);
            (k as usize).saturating_add(1).clamp(1, last)
        };
        if let Some(bucket) = self.buckets.get(idx) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Snapshot the histogram into a proto `ResolveLatency` message.
    ///
    /// Bucket layout: index 0 = underflow, index 1..=n = regular buckets,
    /// index n+1 = overflow. Exponents are offset by `min_exponent`.
    fn snapshot(&self) -> Option<pb::ResolveLatency> {
        let sum = self.sum.load(Ordering::Relaxed);
        let count = self.count.load(Ordering::Relaxed);

        if count == 0 {
            return None;
        }

        let mut spans: Vec<pb::BucketSpan> = Vec::new();
        let mut current_span: Option<(i32, Vec<u32>)> = None;

        for (i, bucket) in self.buckets.iter().enumerate() {
            let v = bucket.load(Ordering::Relaxed);
            let exponent = self.min_exponent.saturating_add(i as i32).saturating_sub(1);
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
            resolve_latency: Histogram::for_type::<pb::ResolveLatency>(),
            resolve_rates: resolve_rates.into_boxed_slice(),
            last_state_update: AtomicU64::new(0),
        }
    }

    /// Record a resolve latency observation in microseconds.
    pub fn record_latency_us(&self, micros: u32) {
        self.resolve_latency.observe(micros);
    }

    /// Increment the resolve rate counter for the given reason.
    pub fn mark_resolve(&self, reason: ResolveReason ) {
        if let Some(counter) = self.resolve_rates.get(reason as usize) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Update the last known state timestamp (millisecond epoch).
    pub fn set_last_state_update(&self, epoch_ms: u64) {
        self.last_state_update.store(epoch_ms, Ordering::Relaxed);
    }

    /// Snapshot the current telemetry state into a [`TelemetryData`] proto message.
    ///
    /// Reads all counters without resetting them. This is not perfectly atomic across
    /// all fields, but each individual counter load is atomic — acceptable for metrics.
    pub fn snapshot(&self) -> pb::TelemetryData {
        let resolve_latency = self.resolve_latency.snapshot();

        let resolve_rate: Vec<pb::ResolveRate> = self
            .resolve_rates
            .iter()
            .enumerate()
            .filter_map(|(i, counter)| {
                let c = counter.load(Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_empty_returns_none() {
        let hist = Histogram::new(1, 1000, 10);
        assert!(hist.snapshot().is_none());
    }

    #[test]
    fn histogram_basic_observation() {
        let hist = Histogram::new(1, 1000, 10);
        hist.observe(100);
        hist.observe(200);
        hist.observe(300);

        let snapshot = hist.snapshot().expect("snapshot should be Some");
        assert_eq!(snapshot.count, 3);
        assert_eq!(snapshot.sum, 600);
        assert!(!snapshot.buckets.is_empty());
    }

    #[test]
    fn histogram_snapshot_does_not_reset() {
        let hist = Histogram::new(1, 1000, 10);
        hist.observe(100);
        hist.observe(200);

        let snap1 = hist.snapshot().expect("first snapshot");
        assert_eq!(snap1.count, 2);
        assert_eq!(snap1.sum, 300);

        // Second snapshot should show same values
        let snap2 = hist.snapshot().expect("second snapshot");
        assert_eq!(snap2.count, 2);
        assert_eq!(snap2.sum, 300);

        // Add more observations
        hist.observe(50);
        let snap3 = hist.snapshot().expect("third snapshot");
        assert_eq!(snap3.count, 3);
        assert_eq!(snap3.sum, 350);
    }

    #[test]
    fn histogram_underflow_bucket() {
        let hist = Histogram::new(100, 1000, 10);
        hist.observe(0); // underflow
        hist.observe(1); // underflow (< min_value)
        hist.observe(50); // underflow

        let snapshot = hist.snapshot().expect("snapshot");
        assert_eq!(snapshot.count, 3);

        // All observations should be in buckets (underflow bucket)
        let total_in_buckets: u32 = snapshot.buckets.iter().flat_map(|s| &s.counts).sum();
        assert_eq!(total_in_buckets, 3);
    }

    #[test]
    fn histogram_overflow_bucket() {
        let hist = Histogram::new(1, 100, 10);
        hist.observe(200); // overflow
        hist.observe(500); // overflow
        hist.observe(1000); // overflow

        let snapshot = hist.snapshot().expect("snapshot");
        assert_eq!(snapshot.count, 3);

        let total_in_buckets: u32 = snapshot.buckets.iter().flat_map(|s| &s.counts).sum();
        assert_eq!(total_in_buckets, 3);
    }

    #[test]
    fn histogram_bucket_spans() {
        let hist = Histogram::new(1, 1000, 20);

        // Create a pattern with gaps
        hist.observe(10);
        hist.observe(11);
        hist.observe(12);
        // gap
        hist.observe(100);
        hist.observe(101);

        let snapshot = hist.snapshot().expect("snapshot");

        // Should have multiple spans due to gaps
        assert!(snapshot.buckets.len() > 0);

        // Total count should match
        let total: u32 = snapshot.buckets.iter().flat_map(|s| &s.counts).sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn telemetry_new_is_empty() {
        let tel = Telemetry::new();
        let data = tel.snapshot();

        assert!(data.resolve_latency.is_none());
        assert!(data.resolve_rate.is_empty());
        assert!(data.state_age.is_none());
    }

    #[test]
    fn telemetry_record_latency() {
        let tel = Telemetry::new();
        tel.record_latency_us(1000);
        tel.record_latency_us(2000);
        tel.record_latency_us(3000);

        let data = tel.snapshot();
        let latency = data.resolve_latency.expect("latency should be present");

        assert_eq!(latency.count, 3);
        assert_eq!(latency.sum, 6000);
    }

    #[test]
    fn telemetry_mark_resolve() {
        let tel = Telemetry::new();
        tel.mark_resolve(ResolveReason::Match);
        tel.mark_resolve(ResolveReason::Match);
        tel.mark_resolve(ResolveReason::NoTreatmentMatch);

        let data = tel.snapshot();

        assert_eq!(data.resolve_rate.len(), 2);

        let match_rate = data.resolve_rate.iter()
            .find(|r| r.reason == ResolveReason::Match as i32)
            .expect("should have Match reason");
        assert_eq!(match_rate.count, 2);

        let no_treatment_rate = data.resolve_rate.iter()
            .find(|r| r.reason == ResolveReason::NoTreatmentMatch as i32)
            .expect("should have NoTreatmentMatch reason");
        assert_eq!(no_treatment_rate.count, 1);
    }

    #[test]
    fn telemetry_state_age() {
        let tel = Telemetry::new();

        // Initially no state age
        let data1 = tel.snapshot();
        assert!(data1.state_age.is_none());

        // Set state update
        tel.set_last_state_update(1234567890);
        let data2 = tel.snapshot();

        assert!(data2.state_age.is_some());
        assert_eq!(data2.state_age.unwrap().last_state_update, 1234567890);
    }

    #[test]
    fn telemetry_snapshot_does_not_reset() {
        let tel = Telemetry::new();

        tel.record_latency_us(100);
        tel.mark_resolve(ResolveReason::Match);
        tel.set_last_state_update(1000);

        // First snapshot
        let data1 = tel.snapshot();
        assert_eq!(data1.resolve_latency.as_ref().unwrap().count, 1);
        assert_eq!(data1.resolve_rate.len(), 1);
        assert_eq!(data1.state_age.as_ref().unwrap().last_state_update, 1000);

        // Second snapshot should show same values
        let data2 = tel.snapshot();
        assert_eq!(data2.resolve_latency.as_ref().unwrap().count, 1);
        assert_eq!(data2.resolve_rate.len(), 1);
        assert_eq!(data2.state_age.as_ref().unwrap().last_state_update, 1000);

        // Add more data
        tel.record_latency_us(200);
        tel.mark_resolve(ResolveReason::NoTreatmentMatch);
        tel.set_last_state_update(2000);

        // Third snapshot should show accumulated values
        let data3 = tel.snapshot();
        assert_eq!(data3.resolve_latency.as_ref().unwrap().count, 2);
        assert_eq!(data3.resolve_rate.len(), 2);
        assert_eq!(data3.state_age.as_ref().unwrap().last_state_update, 2000);
    }

    #[test]
    fn telemetry_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let tel = Arc::new(Telemetry::new());
        let mut handles = vec![];

        // Spawn multiple threads recording metrics
        for i in 0..10 {
            let tel_clone = Arc::clone(&tel);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    tel_clone.record_latency_us(i * 10);
                    tel_clone.mark_resolve(ResolveReason::Match);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        let data = tel.snapshot();
        let latency = data.resolve_latency.expect("latency should be present");

        // 10 threads * 100 observations each
        assert_eq!(latency.count, 1000);

        let match_rate = data.resolve_rate.iter()
            .find(|r| r.reason == ResolveReason::Match as i32)
            .expect("should have Match reason");
        assert_eq!(match_rate.count, 1000);
    }
}
