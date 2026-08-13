use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::proto::confidence::flags::resolver::v1::events::FallthroughAssignment;
use crate::proto::confidence::flags::resolver::v1::resolve_token_v1::AssignedFlag;
use crate::FlagToApply;

const FNV64_INIT: u64 = 0xCBF2_9CE4_8422_2325;
const FNV64_PRIME: u64 = 0x1000_0000_01B3;
const CLEANUP_INTERVAL: usize = 128;

/// Identity hasher for pre-hashed u64 keys — avoids double-hashing in the HashMap.
#[derive(Default)]
struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _bytes: &[u8]) {}
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }
}

type IdentityBuildHasher = BuildHasherDefault<IdentityHasher>;

/// Streaming FNV-1a hasher — feeds bytes directly without allocating a Vec.
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(FNV64_INIT)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(FNV64_PRIME);
        }
    }

    fn separator(&mut self) {
        self.0 = self.0.wrapping_mul(FNV64_PRIME);
    }

    fn finish(self) -> u64 {
        self.0
    }
}

pub fn compute_dedup_hash(assigned: &AssignedFlag) -> u64 {
    let mut h = Fnv1a::new();
    h.write(assigned.flag.as_bytes());
    h.separator();
    h.write(assigned.targeting_key.as_bytes());
    h.separator();
    h.write(assigned.assignment_id.as_bytes());
    h.separator();
    h.write(&assigned.reason.to_le_bytes());
    h.finish()
}

#[derive(Default)]
pub struct AppliedFlagRef<'a> {
    pub flag: &'a str,
    pub targeting_key: &'a str,
    pub assignment_id: &'a str,
    pub rule: &'a str,
    pub variant: &'a str,
    pub segment: &'a str,
    pub reason: i32,
    pub fallthrough_assignments: &'a [FallthroughAssignment],
}

pub fn compute_applied_flag_dedup_hash(applied: &AppliedFlagRef<'_>) -> u64 {
    let mut h = Fnv1a::new();
    h.write(applied.flag.as_bytes());
    h.separator();
    h.write(applied.targeting_key.as_bytes());
    h.separator();
    h.write(applied.assignment_id.as_bytes());
    h.separator();
    h.write(&applied.reason.to_le_bytes());
    h.finish()
}

pub struct ApplyDedup {
    seen: HashMap<u64, i64, IdentityBuildHasher>,
    ttl_seconds: i64,
    max_entries: usize,
    ops_since_cleanup: usize,
}

impl ApplyDedup {
    pub fn new(ttl_seconds: i64, max_entries: usize) -> Self {
        Self {
            seen: HashMap::with_hasher(IdentityBuildHasher::default()),
            ttl_seconds,
            max_entries,
            ops_since_cleanup: 0,
        }
    }

    /// Returns `true` for each flag that should be logged (not a duplicate).
    /// The caller uses the returned mask to decide which flags to forward.
    pub fn filter_duplicates(&mut self, flags: &[FlagToApply], now_seconds: i64) -> DedupResult {
        self.maybe_cleanup(now_seconds);

        let mut keep = DedupResult::new(flags.len());
        for (i, fta) in flags.iter().enumerate() {
            let hash = compute_dedup_hash(&fta.assigned_flag);
            if let Some(&ts) = self.seen.get(&hash) {
                if now_seconds.saturating_sub(ts) < self.ttl_seconds {
                    continue;
                }
            }
            if self.seen.len() < self.max_entries {
                self.seen.insert(hash, now_seconds);
            }
            keep.mark(i);
        }
        keep
    }

    fn maybe_cleanup(&mut self, now_seconds: i64) {
        self.ops_since_cleanup = self.ops_since_cleanup.saturating_add(1);
        if self.ops_since_cleanup < CLEANUP_INTERVAL {
            return;
        }
        self.ops_since_cleanup = 0;
        let ttl = self.ttl_seconds;
        self.seen
            .retain(|_, ts| now_seconds.saturating_sub(*ts) < ttl);
    }
}

/// Bitmask of which flags in a slice survived dedup.
/// Avoids cloning `FlagToApply` — the caller reads from the original slice.
pub struct DedupResult {
    mask: u64,
    count: usize,
}

impl DedupResult {
    fn new(_len: usize) -> Self {
        Self { mask: 0, count: 0 }
    }

    fn mark(&mut self, idx: usize) {
        if idx < 64 {
            self.mask |= 1u64 << idx;
        }
        self.count = self.count.saturating_add(1);
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn kept_count(&self) -> usize {
        self.count
    }

    pub fn is_kept(&self, idx: usize) -> bool {
        if idx < 64 {
            self.mask & (1u64 << idx) != 0
        } else {
            true // beyond bitmask capacity → keep (safe fallback)
        }
    }

    /// Collect the kept flags by cloning only the survivors.
    pub fn collect(&self, flags: &[FlagToApply]) -> Vec<FlagToApply> {
        if self.count == flags.len() {
            return flags.to_vec();
        }
        let mut out = Vec::with_capacity(self.count);
        for (i, f) in flags.iter().enumerate() {
            if self.is_kept(i) {
                out.push(f.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assigned(flag: &str, targeting_key: &str, variant: &str) -> AssignedFlag {
        AssignedFlag {
            flag: flag.to_string(),
            targeting_key: targeting_key.to_string(),
            variant: variant.to_string(),
            ..Default::default()
        }
    }

    fn make_flag_to_apply(flag: &str, targeting_key: &str, variant: &str) -> FlagToApply {
        FlagToApply {
            assigned_flag: make_assigned(flag, targeting_key, variant),
            skew_adjusted_applied_time: Default::default(),
        }
    }

    #[test]
    fn dedup_filters_identical_flags() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        let first = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&flags, 1001);
        assert!(second.is_empty());
    }

    #[test]
    fn dedup_allows_after_ttl_expires() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        let first = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&flags, 1121);
        assert_eq!(second.kept_count(), 1);
    }

    #[test]
    fn dedup_allows_different_flags() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags_a = vec![make_flag_to_apply("flags/a", "user1", "on")];
        let flags_b = vec![make_flag_to_apply("flags/b", "user1", "on")];

        let first = dedup.filter_duplicates(&flags_a, 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&flags_b, 1000);
        assert_eq!(second.kept_count(), 1);
    }

    #[test]
    fn dedup_allows_same_flag_different_user() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags_u1 = vec![make_flag_to_apply("flags/a", "user1", "on")];
        let flags_u2 = vec![make_flag_to_apply("flags/a", "user2", "on")];

        let first = dedup.filter_duplicates(&flags_u1, 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&flags_u2, 1000);
        assert_eq!(second.kept_count(), 1);
    }

    #[test]
    fn dedup_allows_same_flag_different_assignment_id() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let mut f1 = make_flag_to_apply("flags/a", "user1", "on");
        f1.assigned_flag.assignment_id = "assign-v1".to_string();
        let mut f2 = make_flag_to_apply("flags/a", "user1", "off");
        f2.assigned_flag.assignment_id = "assign-v2".to_string();

        let first = dedup.filter_duplicates(&[f1], 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&[f2], 1000);
        assert_eq!(second.kept_count(), 1);
    }

    #[test]
    fn dedup_respects_max_entries() {
        let mut dedup = ApplyDedup::new(120, 2);

        let f1 = vec![make_flag_to_apply("flags/a", "user1", "on")];
        let f2 = vec![make_flag_to_apply("flags/b", "user1", "on")];
        let f3 = vec![make_flag_to_apply("flags/c", "user1", "on")];

        dedup.filter_duplicates(&f1, 1000);
        dedup.filter_duplicates(&f2, 1000);
        // at capacity, new entry not cached but still returned
        let result = dedup.filter_duplicates(&f3, 1000);
        assert_eq!(result.kept_count(), 1);
        // f3 was not cached, so it passes through again
        let result2 = dedup.filter_duplicates(&f3, 1001);
        assert_eq!(result2.kept_count(), 1);
    }

    #[test]
    fn cleanup_removes_expired_entries() {
        let mut dedup = ApplyDedup::new(10, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        dedup.filter_duplicates(&flags, 100);
        assert_eq!(dedup.seen.len(), 1);

        // force cleanup
        dedup.ops_since_cleanup = CLEANUP_INTERVAL - 1;
        dedup.maybe_cleanup(111);
        assert_eq!(dedup.seen.len(), 0);
    }

    #[test]
    fn hash_is_deterministic() {
        let a = make_assigned("flags/x", "user1", "on");
        let h1 = compute_dedup_hash(&a);
        let h2 = compute_dedup_hash(&a);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_differs_for_different_fields() {
        let a = make_assigned("flags/x", "user1", "on");
        let b = make_assigned("flags/y", "user1", "on");
        assert_ne!(compute_dedup_hash(&a), compute_dedup_hash(&b));

        let c = make_assigned("flags/x", "user1", "on");
        let d = make_assigned("flags/x", "user2", "on");
        assert_ne!(compute_dedup_hash(&c), compute_dedup_hash(&d));
    }

    #[test]
    fn mixed_batch_filters_only_duplicates() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags = vec![
            make_flag_to_apply("flags/a", "user1", "on"),
            make_flag_to_apply("flags/b", "user1", "off"),
        ];

        let first = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(first.kept_count(), 2);

        // only flag_a is repeated
        let flags2 = vec![
            make_flag_to_apply("flags/a", "user1", "on"),
            make_flag_to_apply("flags/c", "user1", "on"),
        ];
        let result = dedup.filter_duplicates(&flags2, 1001);
        assert_eq!(result.kept_count(), 1);
        assert!(!result.is_kept(0)); // flags/a is deduped
        assert!(result.is_kept(1)); // flags/c is new

        let collected = result.collect(&flags2);
        assert_eq!(collected.len(), 1);
        assert_eq!(collected.first().unwrap().assigned_flag.flag, "flags/c");
    }

    #[test]
    fn dedup_differentiates_by_reason() {
        let mut dedup = ApplyDedup::new(120, 1000);

        let mut f1 = make_flag_to_apply("flags/a", "user1", "");
        f1.assigned_flag.reason = 1; // NoSegmentMatch
        let mut f2 = make_flag_to_apply("flags/a", "user1", "");
        f2.assigned_flag.reason = 3; // FlagArchived

        let first = dedup.filter_duplicates(&[f1], 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&[f2], 1000);
        assert_eq!(
            second.kept_count(),
            1,
            "different reason should not be deduped"
        );
    }

    #[test]
    fn dedup_differentiates_by_assignment_id() {
        let mut dedup = ApplyDedup::new(120, 1000);

        let mut f1 = make_flag_to_apply("flags/a", "user1", "on");
        f1.assigned_flag.assignment_id = "assign-1".to_string();
        let mut f2 = make_flag_to_apply("flags/a", "user1", "on");
        f2.assigned_flag.assignment_id = "assign-2".to_string();

        let first = dedup.filter_duplicates(&[f1], 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&[f2], 1000);
        assert_eq!(
            second.kept_count(),
            1,
            "different assignment_id should not be deduped"
        );
    }

    #[test]
    fn hash_consistency_between_assigned_and_applied() {
        let assigned = AssignedFlag {
            flag: "flags/test".to_string(),
            targeting_key: "user-42".to_string(),
            assignment_id: "a-1".to_string(),
            reason: 0,
            ..Default::default()
        };
        let hash_assigned = compute_dedup_hash(&assigned);

        let applied_ref = AppliedFlagRef {
            flag: "flags/test",
            targeting_key: "user-42",
            assignment_id: "a-1",
            reason: 0,
            ..Default::default()
        };
        let hash_applied = compute_applied_flag_dedup_hash(&applied_ref);

        assert_eq!(
            hash_assigned, hash_applied,
            "same fields must produce same hash regardless of which function is used"
        );
    }

    #[test]
    fn dedup_with_assign_logger_integration() {
        use crate::assign_logger::AssignLogger;

        let mut dedup = ApplyDedup::new(120, 1000);
        let logger = AssignLogger::new();
        let client = crate::Client {
            account: crate::Account::new("test-account"),
            client_name: "test-client".to_string(),
            client_credential_name: "cred".to_string(),
            environments: vec![],
        };
        let sdk = None;

        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        // First resolve — should log
        let result = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(result.kept_count(), 1);
        let filtered = result.collect(&flags);
        logger.log_assigns("resolve-1", &filtered, &client, &sdk);

        // Second resolve — same assignment, should be deduped
        let result = dedup.filter_duplicates(&flags, 1001);
        assert!(result.is_empty());

        // Check only one event was logged
        let req = logger.checkpoint();
        assert_eq!(req.flag_assigned.len(), 1);
    }

    #[test]
    fn max_entries_frees_space_after_ttl_cleanup() {
        let mut dedup = ApplyDedup::new(10, 2);

        let f1 = vec![make_flag_to_apply("flags/a", "user1", "on")];
        let f2 = vec![make_flag_to_apply("flags/b", "user1", "on")];

        dedup.filter_duplicates(&f1, 100);
        dedup.filter_duplicates(&f2, 100);
        assert_eq!(dedup.seen.len(), 2);

        // trigger cleanup after TTL
        dedup.ops_since_cleanup = CLEANUP_INTERVAL - 1;
        let f3 = vec![make_flag_to_apply("flags/c", "user1", "on")];
        let result = dedup.filter_duplicates(&f3, 111);
        assert_eq!(result.kept_count(), 1);
        // old entries cleaned, new one inserted
        assert_eq!(dedup.seen.len(), 1);
    }

    #[test]
    fn rapid_fire_same_user_same_flag() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        let mut total_logged = 0;
        for t in 0..100 {
            total_logged += dedup.filter_duplicates(&flags, 1000 + t).kept_count();
        }
        assert_eq!(
            total_logged, 1,
            "100 rapid resolves should only produce 1 event"
        );
    }

    #[test]
    fn dedup_result_bitmask_works() {
        let flags = vec![
            make_flag_to_apply("flags/a", "user1", "on"),
            make_flag_to_apply("flags/b", "user1", "on"),
            make_flag_to_apply("flags/c", "user1", "on"),
        ];

        let mut dedup = ApplyDedup::new(120, 1000);

        // Pre-seed one flag
        dedup.filter_duplicates(&flags[0..1], 1000);

        // Now filter all three — only flags/a should be deduped
        let result = dedup.filter_duplicates(&flags, 1001);
        assert_eq!(result.kept_count(), 2);
        assert!(!result.is_kept(0)); // flags/a already seen
        assert!(result.is_kept(1)); // flags/b new
        assert!(result.is_kept(2)); // flags/c new
    }

    #[test]
    fn collect_returns_full_slice_when_all_kept() {
        let flags = vec![
            make_flag_to_apply("flags/a", "user1", "on"),
            make_flag_to_apply("flags/b", "user1", "off"),
        ];

        let mut dedup = ApplyDedup::new(120, 1000);
        let result = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(result.kept_count(), 2);

        let collected = result.collect(&flags);
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn perf_with_vs_without_dedup() {
        use crate::assign_logger::AssignLogger;
        use std::time::Instant;

        let client = crate::Client {
            account: crate::Account::new("test-account"),
            client_name: "test-client".to_string(),
            client_credential_name: "cred".to_string(),
            environments: vec![],
        };
        let sdk: Option<crate::proto::confidence::flags::resolver::v1::Sdk> = None;
        let iterations: u128 = 50_000;

        // 10 flags per resolve — realistic workload
        let flags: Vec<FlagToApply> = (0..10)
            .map(|i| make_flag_to_apply(&format!("flags/flag-{}", i), "user1", "on"))
            .collect();

        // ── Scenario 1: WITHOUT dedup (today's code path) ──
        // Every resolve logs all flags every time.
        let logger_no_dedup = AssignLogger::new();
        let t0 = Instant::now();
        for i in 0..iterations {
            logger_no_dedup.log_assigns(&format!("resolve-{}", i), &flags, &client, &sdk);
        }
        let without_dedup = t0.elapsed();
        let without_events = logger_no_dedup.checkpoint().flag_assigned.len();

        // ── Scenario 2: WITH dedup, repeated same user ──
        // Only the first resolve logs; the rest are filtered.
        let logger_dedup = AssignLogger::new();
        let mut dedup = ApplyDedup::new(120, 100_000);
        let t0 = Instant::now();
        for i in 0..iterations {
            let result = dedup.filter_duplicates(&flags, 1000 + i as i64);
            if !result.is_empty() {
                if result.kept_count() == flags.len() {
                    logger_dedup.log_assigns(&format!("resolve-{}", i), &flags, &client, &sdk);
                } else {
                    let filtered = result.collect(&flags);
                    logger_dedup.log_assigns(&format!("resolve-{}", i), &filtered, &client, &sdk);
                }
            }
        }
        let with_dedup_same_user = t0.elapsed();
        let dedup_events = logger_dedup.checkpoint().flag_assigned.len();

        // ── Scenario 3: WITH dedup, all unique users ──
        // Every resolve is a cache miss → all flags logged (worst-case overhead).
        let logger_unique = AssignLogger::new();
        let mut dedup2 = ApplyDedup::new(120, 200_000);
        let unique_flags: Vec<Vec<FlagToApply>> = (0..iterations)
            .map(|i| {
                (0..10)
                    .map(|j| {
                        make_flag_to_apply(
                            &format!("flags/flag-{}", j),
                            &format!("user-{}", i),
                            "on",
                        )
                    })
                    .collect()
            })
            .collect();
        let t0 = Instant::now();
        for (i, uflags) in unique_flags.iter().enumerate() {
            let result = dedup2.filter_duplicates(uflags, 1000 + i as i64);
            if !result.is_empty() {
                if result.kept_count() == uflags.len() {
                    logger_unique.log_assigns(&format!("resolve-{}", i), uflags, &client, &sdk);
                } else {
                    let filtered = result.collect(uflags);
                    logger_unique.log_assigns(&format!("resolve-{}", i), &filtered, &client, &sdk);
                }
            }
        }
        let with_dedup_unique = t0.elapsed();
        let unique_events = logger_unique.checkpoint().flag_assigned.len();

        eprintln!();
        eprintln!("╔══════════════════════════════════════════════════════════╗");
        eprintln!("║      Apply Dedup — Full log_assign Path Benchmark       ║");
        eprintln!(
            "║      {} iterations × 10 flags each{} ║",
            iterations,
            " ".repeat(16 - iterations.to_string().len())
        );
        eprintln!("╠══════════════════════════════════════════════════════════╣");
        eprintln!(
            "║  WITHOUT dedup:           {:>8} ns/iter  {:>6} events ║",
            without_dedup.as_nanos() / iterations,
            without_events,
        );
        eprintln!(
            "║  WITH dedup (same user):  {:>8} ns/iter  {:>6} events ║",
            with_dedup_same_user.as_nanos() / iterations,
            dedup_events,
        );
        eprintln!(
            "║  WITH dedup (unique):     {:>8} ns/iter  {:>6} events ║",
            with_dedup_unique.as_nanos() / iterations,
            unique_events,
        );
        eprintln!("╠══════════════════════════════════════════════════════════╣");

        let saved_pct = if without_dedup.as_nanos() > 0 {
            100u128.saturating_sub(
                with_dedup_same_user.as_nanos().saturating_mul(100) / without_dedup.as_nanos(),
            )
        } else {
            0
        };
        eprintln!(
            "║  Same-user speedup:                    {:>3}% faster       ║",
            saved_pct,
        );

        let overhead_ns = with_dedup_unique
            .as_nanos()
            .saturating_sub(without_dedup.as_nanos())
            / iterations;
        eprintln!(
            "║  Unique-user overhead:           {:>6} ns/resolve      ║",
            overhead_ns,
        );
        eprintln!("╚══════════════════════════════════════════════════════════╝");

        // In release builds, the dedup path with same user is significantly
        // faster because it skips proto event building. In debug builds the
        // unoptimized hash loop dominates, so we only assert in release.
        #[cfg(not(debug_assertions))]
        {
            assert!(
                with_dedup_same_user < without_dedup,
                "dedup same-user should be faster than no-dedup"
            );
            assert!(
                overhead_ns < 5_000,
                "unique-user overhead {} ns exceeds 5 µs",
                overhead_ns
            );
        }
    }
}
