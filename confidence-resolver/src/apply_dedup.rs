use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::proto::confidence::flags::resolver::v1::events::FallthroughAssignment;
use crate::proto::confidence::flags::resolver::v1::resolve_token_v1::AssignedFlag;
use crate::FlagToApply;

const HASH_INIT: u64 = 0xCBF2_9CE4_8422_2325;
const HASH_PRIME: u64 = 0x1000_0000_01B3;
// Murmur3 fmix64 constants — used to avalanche each word before it enters
// the state, so structured keys ("user-1", "user-2", ...) can't cancel.
const MIX_C1: u64 = 0xFF51_AFD7_ED55_8CCD;
const MIX_C2: u64 = 0xC4CE_B9FE_1A85_EC53;

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

/// Streaming word-at-a-time hasher with murmur3-style per-word avalanche.
/// Strong mixing is required because hashes are used directly as map keys
/// (identity hasher) and inputs are highly structured.
struct DedupHasher(u64);

impl DedupHasher {
    fn new() -> Self {
        Self(HASH_INIT)
    }

    #[inline]
    fn mix(&mut self, word: u64) {
        let mut k = word.wrapping_mul(MIX_C1);
        k ^= k >> 33;
        k = k.wrapping_mul(MIX_C2);
        self.0 = (self.0 ^ k).rotate_left(27).wrapping_mul(HASH_PRIME);
    }

    fn write(&mut self, bytes: &[u8]) {
        let chunks = bytes.chunks_exact(8);
        let tail = chunks.remainder();
        for chunk in chunks {
            if let Ok(arr) = <[u8; 8]>::try_from(chunk) {
                self.mix(u64::from_le_bytes(arr));
            }
        }
        if !tail.is_empty() {
            let mut arr = [0u8; 8];
            for (dst, src) in arr.iter_mut().zip(tail) {
                *dst = *src;
            }
            // XOR in the tail length so "a" and "a\0" hash differently.
            self.mix(u64::from_le_bytes(arr).rotate_left(8) ^ tail.len() as u64);
        }
    }

    fn separator(&mut self) {
        self.0 = self.0.rotate_left(31).wrapping_mul(HASH_PRIME);
    }

    fn finish(self) -> u64 {
        // Final avalanche so the low bits (hashbrown bucket index) are as
        // well mixed as the high bits (hashbrown control bytes).
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(MIX_C1);
        h ^= h >> 33;
        h
    }
}

// Tier discriminators mixed in first so the two key schemes can never
// collide with each other.
const TIER_SLIM: u64 = 1;
const TIER_FULL: u64 = 2;

/// Hashes the apply identity.
///
/// Common case (non-empty `targeting_key` + `assignment_id`, no
/// fallthroughs): `(flag, targeting_key, assignment_id, reason)` is a unique
/// identity — `assignment_id` encodes the rule assignment — so a slim hash
/// suffices. Otherwise the per-user identity may live in
/// `fallthrough_assignments` (fallthrough-only / no-unit shapes), so every
/// field except `apply_time` is hashed; dropping any of them risks silently
/// swallowing another user's apply event.
pub fn compute_dedup_hash(assigned: &AssignedFlag) -> u64 {
    let mut h = DedupHasher::new();
    if !assigned.targeting_key.is_empty()
        && !assigned.assignment_id.is_empty()
        && assigned.fallthrough_assignments.is_empty()
    {
        h.mix(TIER_SLIM);
        h.write(assigned.flag.as_bytes());
        h.separator();
        h.write(assigned.targeting_key.as_bytes());
        h.separator();
        h.write(assigned.assignment_id.as_bytes());
        h.separator();
        h.write(&assigned.reason.to_le_bytes());
        return h.finish();
    }
    h.mix(TIER_FULL);
    h.write(assigned.flag.as_bytes());
    h.separator();
    h.write(assigned.targeting_key.as_bytes());
    h.separator();
    h.write(assigned.targeting_key_selector.as_bytes());
    h.separator();
    h.write(assigned.assignment_id.as_bytes());
    h.separator();
    h.write(assigned.variant.as_bytes());
    h.separator();
    h.write(assigned.segment.as_bytes());
    h.separator();
    h.write(assigned.rule.as_bytes());
    h.separator();
    h.write(&assigned.reason.to_le_bytes());
    for ft in &assigned.fallthrough_assignments {
        h.separator();
        h.write(ft.rule.as_bytes());
        h.separator();
        h.write(ft.assignment_id.as_bytes());
        h.separator();
        h.write(ft.targeting_key.as_bytes());
        h.separator();
        h.write(ft.targeting_key_selector.as_bytes());
    }
    h.finish()
}

#[derive(Default)]
pub struct AppliedFlagRef<'a> {
    pub flag: &'a str,
    pub targeting_key: &'a str,
    pub targeting_key_selector: &'a str,
    pub assignment_id: &'a str,
    pub rule: &'a str,
    pub variant: &'a str,
    pub segment: &'a str,
    pub reason: i32,
    pub fallthrough_assignments: &'a [FallthroughAssignment],
}

/// Must stay field-for-field consistent with [`compute_dedup_hash`],
/// including the slim/full tier split.
pub fn compute_applied_flag_dedup_hash(applied: &AppliedFlagRef<'_>) -> u64 {
    let mut h = DedupHasher::new();
    if !applied.targeting_key.is_empty()
        && !applied.assignment_id.is_empty()
        && applied.fallthrough_assignments.is_empty()
    {
        h.mix(TIER_SLIM);
        h.write(applied.flag.as_bytes());
        h.separator();
        h.write(applied.targeting_key.as_bytes());
        h.separator();
        h.write(applied.assignment_id.as_bytes());
        h.separator();
        h.write(&applied.reason.to_le_bytes());
        return h.finish();
    }
    h.mix(TIER_FULL);
    h.write(applied.flag.as_bytes());
    h.separator();
    h.write(applied.targeting_key.as_bytes());
    h.separator();
    h.write(applied.targeting_key_selector.as_bytes());
    h.separator();
    h.write(applied.assignment_id.as_bytes());
    h.separator();
    h.write(applied.variant.as_bytes());
    h.separator();
    h.write(applied.segment.as_bytes());
    h.separator();
    h.write(applied.rule.as_bytes());
    h.separator();
    h.write(&applied.reason.to_le_bytes());
    for ft in applied.fallthrough_assignments {
        h.separator();
        h.write(ft.rule.as_bytes());
        h.separator();
        h.write(ft.assignment_id.as_bytes());
        h.separator();
        h.write(ft.targeting_key.as_bytes());
        h.separator();
        h.write(ft.targeting_key_selector.as_bytes());
    }
    h.finish()
}

/// Minimum seconds between sweeps. Hosts drain large assign backlogs by
/// calling the flush functions in a tight loop; without this guard every
/// iteration would repeat the O(n) scan.
const SWEEP_MIN_INTERVAL_SECONDS: i64 = 10;

pub struct ApplyDedup {
    seen: HashMap<u64, i64, IdentityBuildHasher>,
    ttl_seconds: i64,
    max_entries: usize,
    last_sweep_seconds: i64,
}

impl ApplyDedup {
    pub fn new(ttl_seconds: i64, max_entries: usize) -> Self {
        Self {
            // Grows lazily: small deployments keep a small cache-resident
            // table (fast probes) instead of scattering lookups across a
            // pre-allocated multi-MB one. Resize work is bounded — once the
            // map reaches max_entries it never grows again.
            seen: HashMap::with_hasher(IdentityBuildHasher::default()),
            ttl_seconds,
            max_entries,
            last_sweep_seconds: 0,
        }
    }

    /// Returns `true` for each flag that should be logged (not a duplicate).
    /// The caller uses the returned mask to decide which flags to forward.
    ///
    /// Never scans the map — expiry is handled solely by [`Self::sweep`],
    /// which the host calls off the resolve path (from the log flush cycle).
    pub fn filter_duplicates(
        &mut self,
        flags: &[FlagToApply<'_>],
        now_seconds: i64,
    ) -> DedupResult {
        // Deferred applies carry skew-adjusted times that can lie far in the
        // past (client applied long before sending); inserting with such a
        // timestamp would create a pre-expired entry the next sweep drops
        // immediately. Clamp to the last known host time instead.
        let now_seconds = now_seconds.max(self.last_sweep_seconds);
        let mut keep = DedupResult::new(flags.len());
        for (i, fta) in flags.iter().enumerate() {
            let hash = compute_dedup_hash(fta.assigned_flag);
            // Presence means duplicate — stale entries are removed by sweep,
            // after which the flag gets re-logged on its next resolve.
            if self.seen.contains_key(&hash) {
                continue;
            }
            if self.seen.len() < self.max_entries {
                self.seen.insert(hash, now_seconds);
            }
            keep.mark(i);
        }
        keep
    }

    /// Removes entries older than the TTL. O(n) — call from the periodic
    /// log flush cycle, never from the resolve path. Self-throttling: runs
    /// at most once per [`SWEEP_MIN_INTERVAL_SECONDS`], so hosts that flush
    /// in a tight loop (draining a large assign backlog) don't repeat the
    /// scan on every call.
    pub fn sweep(&mut self, now_seconds: i64) {
        if now_seconds.saturating_sub(self.last_sweep_seconds) < SWEEP_MIN_INTERVAL_SECONDS {
            return;
        }
        self.last_sweep_seconds = now_seconds;
        let ttl = self.ttl_seconds;
        self.seen
            .retain(|_, ts| now_seconds.saturating_sub(*ts) < ttl);
    }
}

/// Bitmask of which flags in a slice survived dedup.
/// Avoids cloning `FlagToApply` — the caller reads from the original slice.
/// Inline `u64` for batches up to 64 flags (no allocation); spills to a
/// `Vec<u64>` beyond that — the resolver allows up to 1000 flags per request.
pub struct DedupResult {
    small: u64,
    large: Vec<u64>,
    count: usize,
}

impl DedupResult {
    fn new(len: usize) -> Self {
        let large = if len > 64 {
            vec![0u64; (len >> 6).saturating_add(1)]
        } else {
            Vec::new()
        };
        Self {
            small: 0,
            large,
            count: 0,
        }
    }

    fn mark(&mut self, idx: usize) {
        if self.large.is_empty() {
            if idx < 64 {
                self.small |= 1u64 << (idx & 63);
            }
        } else if let Some(word) = self.large.get_mut(idx >> 6) {
            *word |= 1u64 << (idx & 63);
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
        if self.large.is_empty() {
            idx < 64 && self.small & (1u64 << (idx & 63)) != 0
        } else {
            self.large
                .get(idx >> 6)
                .is_some_and(|word| word & (1u64 << (idx & 63)) != 0)
        }
    }

    /// Collect the kept flags — FlagToApply is Copy (borrowed data).
    pub fn collect<'a>(&self, flags: &[FlagToApply<'a>]) -> Vec<FlagToApply<'a>> {
        if self.count == flags.len() {
            return flags.to_vec();
        }
        let mut out = Vec::with_capacity(self.count);
        for (i, f) in flags.iter().enumerate() {
            if self.is_kept(i) {
                out.push(*f);
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

    /// Test-only: leaks the AssignedFlag to get a 'static borrow — a few KB
    /// per test process, keeps test call sites simple.
    fn wrap(assigned: AssignedFlag) -> FlagToApply<'static> {
        FlagToApply {
            assigned_flag: Box::leak(Box::new(assigned)),
            skew_adjusted_applied_time: Default::default(),
        }
    }

    fn make_flag_to_apply(flag: &str, targeting_key: &str, variant: &str) -> FlagToApply<'static> {
        wrap(make_assigned(flag, targeting_key, variant))
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
    fn dedup_allows_after_ttl_expires_and_sweep() {
        let mut dedup = ApplyDedup::new(120, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        let first = dedup.filter_duplicates(&flags, 1000);
        assert_eq!(first.kept_count(), 1);

        // Expired but not yet swept — still deduped.
        let second = dedup.filter_duplicates(&flags, 1121);
        assert!(second.is_empty());

        // Sweep removes the expired entry; the flag is logged again.
        dedup.sweep(1122);
        let third = dedup.filter_duplicates(&flags, 1122);
        assert_eq!(third.kept_count(), 1);
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
        let mut a1 = make_assigned("flags/a", "user1", "on");
        a1.assignment_id = "assign-v1".to_string();
        let f1 = wrap(a1);
        let mut a2 = make_assigned("flags/a", "user1", "off");
        a2.assignment_id = "assign-v2".to_string();
        let f2 = wrap(a2);

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
    fn sweep_removes_expired_entries() {
        let mut dedup = ApplyDedup::new(10, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        dedup.filter_duplicates(&flags, 100);
        assert_eq!(dedup.seen.len(), 1);

        // Sweep before expiry keeps the entry.
        dedup.sweep(105);
        assert_eq!(dedup.seen.len(), 1);

        // Sweep after expiry removes it (>= SWEEP_MIN_INTERVAL after last).
        dedup.sweep(115);
        assert_eq!(dedup.seen.len(), 0);
    }

    #[test]
    fn sweep_is_throttled() {
        let mut dedup = ApplyDedup::new(10, 1000);
        let flags = vec![make_flag_to_apply("flags/a", "user1", "on")];

        dedup.filter_duplicates(&flags, 100);
        dedup.sweep(105); // runs, sets last_sweep

        // Entry expired at 110, but this sweep is within the min interval
        // of the previous one — throttled, entry stays.
        dedup.sweep(112);
        assert_eq!(dedup.seen.len(), 1);

        // Past the min interval — sweep runs and removes the expired entry.
        dedup.sweep(115);
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

        let mut a1 = make_assigned("flags/a", "user1", "");
        a1.reason = 1; // NoSegmentMatch
        let f1 = wrap(a1);
        let mut a2 = make_assigned("flags/a", "user1", "");
        a2.reason = 3; // FlagArchived
        let f2 = wrap(a2);

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

        let mut a1 = make_assigned("flags/a", "user1", "on");
        a1.assignment_id = "assign-1".to_string();
        let f1 = wrap(a1);
        let mut a2 = make_assigned("flags/a", "user1", "on");
        a2.assignment_id = "assign-2".to_string();
        let f2 = wrap(a2);

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
        let ft = FallthroughAssignment {
            rule: "rule-ft".to_string(),
            assignment_id: "ft-1".to_string(),
            targeting_key: "ft-user".to_string(),
            targeting_key_selector: "visitor_id".to_string(),
        };
        let assigned = AssignedFlag {
            flag: "flags/test".to_string(),
            targeting_key: "user-42".to_string(),
            targeting_key_selector: "targeting_key".to_string(),
            assignment_id: "a-1".to_string(),
            variant: "control".to_string(),
            segment: "seg-1".to_string(),
            rule: "rule-1".to_string(),
            reason: 0,
            fallthrough_assignments: vec![ft.clone()],
        };
        let hash_assigned = compute_dedup_hash(&assigned);

        let applied_ref = AppliedFlagRef {
            flag: "flags/test",
            targeting_key: "user-42",
            targeting_key_selector: "targeting_key",
            assignment_id: "a-1",
            variant: "control",
            segment: "seg-1",
            rule: "rule-1",
            reason: 0,
            fallthrough_assignments: &[ft],
        };
        let hash_applied = compute_applied_flag_dedup_hash(&applied_ref);

        assert_eq!(
            hash_assigned, hash_applied,
            "same fields must produce same hash regardless of which function is used"
        );
    }

    #[test]
    fn fallthrough_only_applies_distinguish_users() {
        // Fallthrough-only assignments leave the parent targeting_key and
        // assignment_id empty — the user identity lives in the fallthrough
        // entries. Two different users must not dedup each other.
        let mut dedup = ApplyDedup::new(120, 1000);

        let mut for_user = |user: &str| {
            let mut assigned = make_assigned("flags/a", "", "");
            assigned.reason = 1; // NoSegmentMatch
            assigned.fallthrough_assignments = vec![FallthroughAssignment {
                rule: "rule-1".to_string(),
                assignment_id: "assign-1".to_string(),
                targeting_key: user.to_string(),
                targeting_key_selector: "targeting_key".to_string(),
            }];
            wrap(assigned)
        };

        let first = dedup.filter_duplicates(&[for_user("user-a")], 1000);
        assert_eq!(first.kept_count(), 1);

        let second = dedup.filter_duplicates(&[for_user("user-b")], 1000);
        assert_eq!(
            second.kept_count(),
            1,
            "fallthrough apply for a different user must not be deduped"
        );

        // Same user again is a duplicate.
        let third = dedup.filter_duplicates(&[for_user("user-a")], 1001);
        assert!(third.is_empty());
    }

    #[test]
    fn variant_changes_are_not_deduped_with_empty_assignment_id() {
        // No-unit full rollouts share an empty assignment_id; a variant
        // change must still produce a new event.
        let mut dedup = ApplyDedup::new(120, 1000);
        let on = make_flag_to_apply("flags/a", "", "on");
        let off = make_flag_to_apply("flags/a", "", "off");

        assert_eq!(dedup.filter_duplicates(&[on], 1000).kept_count(), 1);
        assert_eq!(
            dedup.filter_duplicates(&[off], 1000).kept_count(),
            1,
            "different variant must not be deduped"
        );
    }

    #[test]
    fn mixed_batch_beyond_64_flags() {
        // The keep-mask spills past 64 bits; a duplicate at a tail index
        // must be omitted from collect().
        let mut dedup = ApplyDedup::new(120, 1000);

        let flags: Vec<FlagToApply> = (0..70)
            .map(|i| make_flag_to_apply(&format!("flags/f{}", i), "user1", "on"))
            .collect();

        // Pre-seed only index 65 so the batch is mixed.
        dedup.filter_duplicates(&flags[65..66], 1000);

        let result = dedup.filter_duplicates(&flags, 1001);
        assert_eq!(result.kept_count(), 69);
        assert!(!result.is_kept(65), "index 65 is a duplicate");
        assert!(result.is_kept(64));
        assert!(result.is_kept(69));

        let collected = result.collect(&flags);
        assert_eq!(collected.len(), 69);
        assert!(
            !collected
                .iter()
                .any(|f| f.assigned_flag.flag == "flags/f65"),
            "duplicate tail flag must be omitted"
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

        // Sweep after TTL frees capacity for new entries.
        dedup.sweep(111);
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
    #[ignore = "benchmark — run with: cargo test --release -- --ignored"]
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

    #[test]
    fn no_collisions_on_structured_keys() {
        // Regression: word-at-a-time FNV collided ~10% on keys with common
        // prefixes and small numeric suffixes ("user-1", "user-2", ...). Every
        // collision silently drops another user's apply event.
        use std::collections::HashSet;

        let mut hashes = HashSet::new();
        let mut expected = 0usize;
        for u in 0..20_000 {
            for f in 0..10 {
                let assigned =
                    make_assigned(&format!("flags/f{}", f), &format!("user-{}", u), "on");
                hashes.insert(compute_dedup_hash(&assigned));
                expected += 1;
            }
        }
        assert_eq!(
            hashes.len(),
            expected,
            "hash collisions detected: {} unique hashes for {} distinct keys",
            hashes.len(),
            expected
        );
    }

    #[test]
    #[ignore = "benchmark — run with: cargo test --release -- --ignored"]
    fn perf_deployment_sizes() {
        use crate::assign_logger::AssignLogger;
        use std::time::Instant;

        let client = crate::Client {
            account: crate::Account::new("test-account"),
            client_name: "test-client".to_string(),
            client_credential_name: "cred".to_string(),
            environments: vec![],
        };
        let sdk: Option<crate::proto::confidence::flags::resolver::v1::Sdk> = None;
        const FLAGS_PER_USER: usize = 10;
        const PROD_CAP: usize = 100_000;

        // Baseline: no dedup, log every resolve.
        let baseline_ns = {
            let logger = AssignLogger::new();
            let flags: Vec<FlagToApply> = (0..FLAGS_PER_USER)
                .map(|j| make_flag_to_apply(&format!("flags/f{}", j), "user", "on"))
                .collect();
            let iters: u128 = 20_000;
            let t0 = Instant::now();
            for i in 0..iters {
                logger.log_assigns(&format!("r{}", i), &flags, &client, &sdk);
            }
            let ns = t0.elapsed().as_nanos() / iters;
            let _ = logger.checkpoint();
            ns
        };

        eprintln!();
        eprintln!("╔═══════════════════════════════════════════════════════════════════╗");
        eprintln!("║        Deployment Size Report — 10 flags/resolve, cap 100K        ║");
        eprintln!(
            "║        Baseline (no dedup): {:>5} ns/resolve                       ║",
            baseline_ns
        );
        eprintln!("╠═══════════════════════════════════════════════════════════════════╣");
        eprintln!("║ Deployment │  Users │ Entries │ Fill ns │  Hit ns │ New-user ns   ║");
        eprintln!("╠═══════════════════════════════════════════════════════════════════╣");

        for (label, users) in [
            ("small", 100usize),
            ("medium", 2_000),
            ("large", 10_000),
            ("at-cap", 50_000),
        ] {
            let mut dedup = ApplyDedup::new(i64::MAX, PROD_CAP);

            let user_flags: Vec<Vec<FlagToApply>> = (0..users)
                .map(|u| {
                    (0..FLAGS_PER_USER)
                        .map(|j| {
                            make_flag_to_apply(
                                &format!("flags/f{}", j),
                                &format!("user-{}", u),
                                "on",
                            )
                        })
                        .collect()
                })
                .collect();

            // Fill phase: every user's first resolve (includes lazy growth).
            let t0 = Instant::now();
            for flags in &user_flags {
                let _ = dedup.filter_duplicates(flags, 1000);
            }
            let fill_ns = t0.elapsed().as_nanos() / users as u128;

            // Steady state: same users resolve again — all dedup hits.
            // Multiple passes for a stable number.
            let passes: usize = (100_000 / users).max(1);
            let t0 = Instant::now();
            for _ in 0..passes {
                for flags in &user_flags {
                    let _ = dedup.filter_duplicates(flags, 1001);
                }
            }
            let hit_ns = t0.elapsed().as_nanos() / (users * passes) as u128;

            // New-user cost at this table size (insert if under cap,
            // probe+skip when at cap).
            let fresh: Vec<Vec<FlagToApply>> = (0..5_000)
                .map(|u| {
                    (0..FLAGS_PER_USER)
                        .map(|j| {
                            make_flag_to_apply(
                                &format!("flags/f{}", j),
                                &format!("fresh-{}-{}", label, u),
                                "on",
                            )
                        })
                        .collect()
                })
                .collect();
            let t0 = Instant::now();
            for flags in &fresh {
                let _ = dedup.filter_duplicates(flags, 1002);
            }
            let new_ns = t0.elapsed().as_nanos() / fresh.len() as u128;

            eprintln!(
                "║ {:>10} │ {:>6} │ {:>7} │ {:>7} │ {:>7} │ {:>10}    ║",
                label,
                users,
                dedup.seen.len(),
                fill_ns,
                hit_ns,
                new_ns,
            );
        }
        eprintln!("╚═══════════════════════════════════════════════════════════════════╝");
    }

    #[test]
    #[ignore = "benchmark — run with: cargo test --release -- --ignored"]
    fn stress_1m_find_degradation_point() {
        use std::time::Instant;

        let total: usize = 1_000_000;
        let batch: usize = 10_000;
        // no TTL expiry — timestamps stay within window so cache grows monotonically
        let mut dedup = ApplyDedup::new(i64::MAX, total);

        // pre-generate all flags to keep allocation out of the timed loop
        let all_flags: Vec<FlagToApply> = (0..total)
            .map(|i| make_flag_to_apply("flags/feature", &format!("u{}", i), "on"))
            .collect();

        eprintln!();
        eprintln!("╔═══════════════════════════════════════════════════════════╗");
        eprintln!("║   Stress Test — 1M unique entries, find degradation      ║");
        eprintln!("╠═══════════════════════════════════════════════════════════╣");
        eprintln!("║  Cache size │ ns/lookup (insert) │  ns/lookup (hit)  │ MB║");
        eprintln!("╠═══════════════════════════════════════════════════════════╣");

        let mut cursor: usize = 0;
        while cursor < total {
            let end = (cursor.saturating_add(batch)).min(total);
            let chunk = &all_flags[cursor..end];

            // measure INSERT cost (unique keys, all cache misses)
            let t0 = Instant::now();
            for f in chunk {
                let _ = dedup.filter_duplicates(std::slice::from_ref(f), 1000);
            }
            let insert_elapsed = t0.elapsed();
            let insert_ns = insert_elapsed.as_nanos() / (end.saturating_sub(cursor)) as u128;

            // measure HIT cost (same keys again, all cache hits)
            let t0 = Instant::now();
            for f in chunk {
                let _ = dedup.filter_duplicates(std::slice::from_ref(f), 1001);
            }
            let hit_elapsed = t0.elapsed();
            let hit_ns = hit_elapsed.as_nanos() / (end.saturating_sub(cursor)) as u128;

            let entries = dedup.seen.len();
            let mem_mb = (entries as f64 * 19.4) / (1024.0 * 1024.0);

            eprintln!(
                "║  {:>9} │          {:>6} ns │        {:>6} ns  │{:>3.0}║",
                entries, insert_ns, hit_ns, mem_mb
            );

            cursor = end;
        }

        eprintln!("╚═══════════════════════════════════════════════════════════╝");

        // sanity: cache holds all entries
        assert_eq!(dedup.seen.len(), total);
    }
}
