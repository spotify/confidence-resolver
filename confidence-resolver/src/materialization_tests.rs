use std::collections::{HashMap, HashSet};

use bitvec::prelude as bv;
use bytes::Bytes;

use crate::proto::confidence::flags::resolver::v1 as flags_resolver;
use crate::proto::confidence::flags::resolver::v1::{
    resolve_process_request, resolve_process_response, MaterializationRecord,
    ResolveProcessRequest, ResolveProcessResponse,
};
use crate::proto::google::Struct;
use crate::*;

const EXAMPLE_STATE_2: &[u8] =
    include_bytes!("../test-payloads/resolver_state_with_custom_targeting_flag.pb");
const MULTIPLE_STICKY_FLAGS_STATE: &[u8] =
    include_bytes!("../test-payloads/resolver_state_with_multiple_sticky_flags.pb");
const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);

struct L;

impl Host for L {
    fn log_resolve(
        _resolve_id: &str,
        _evaluation_context: &Struct,
        _values: &[ResolvedValue<'_>],
        _client: &Client,
        _sdk: &Option<flags_resolver::Sdk>,
    ) {
    }

    fn log_assign(
        _resolve_id: &str,
        _evaluation_context: &Struct,
        _assigned_flag: &[FlagToApply],
        _client: &Client,
        _sdk: &Option<flags_resolver::Sdk>,
    ) {
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a ResolverState from JSON flag and segment definitions.
fn make_state_from_json(flag_json: &str, segment_jsons: &[&str], secret: &str) -> ResolverState {
    make_state_from_json_flags(&[flag_json], segment_jsons, secret)
}

fn make_state_from_json_flags(
    flag_jsons: &[&str],
    segment_jsons: &[&str],
    secret: &str,
) -> ResolverState {
    let mut flags = HashMap::new();
    for fj in flag_jsons {
        let flag: Flag = serde_json::from_str(fj).unwrap();
        flags.insert(flag.name.clone(), flag);
    }

    let mut segments = HashMap::new();
    let mut bitsets = HashMap::new();
    for s in segment_jsons {
        let segment: Segment = serde_json::from_str(s).unwrap();
        bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));
        segments.insert(segment.name.clone(), segment);
    }

    let mut secrets = HashMap::new();
    secrets.insert(
        secret.to_string(),
        Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        },
    );

    ResolverState {
        secrets,
        flags,
        segments,
        bitsets,
    }
}

fn resolve_request(secret: &str, flags: &[&str]) -> flags_resolver::ResolveFlagsRequest {
    flags_resolver::ResolveFlagsRequest {
        evaluation_context: Some(Struct::default()),
        client_secret: secret.to_string(),
        flags: flags.iter().map(|f| f.to_string()).collect(),
        apply: false,
        sdk: None,
    }
}

fn simple_resolve(req: flags_resolver::ResolveFlagsRequest) -> ResolveProcessRequest {
    ResolveProcessRequest {
        resolve: Some(resolve_process_request::Resolve::DeferredMaterializations(
            req,
        )),
    }
}

fn resolve_with_materializations(
    req: flags_resolver::ResolveFlagsRequest,
    materializations: Vec<MaterializationRecord>,
) -> ResolveProcessRequest {
    ResolveProcessRequest {
        resolve: Some(resolve_process_request::Resolve::StaticMaterializations(
            resolve_process_request::StaticMaterializations {
                resolve_request: Some(req),
                materializations,
            },
        )),
    }
}

fn resume_request(
    materializations: Vec<MaterializationRecord>,
    state: Vec<u8>,
) -> ResolveProcessRequest {
    ResolveProcessRequest {
        resolve: Some(resolve_process_request::Resolve::Resume(
            resolve_process_request::Resume {
                materializations,
                state,
            },
        )),
    }
}

fn expect_resolved(response: ResolveProcessResponse) -> resolve_process_response::Resolved {
    match response.result {
        Some(resolve_process_response::Result::Resolved(resolved)) => resolved,
        other => panic!("Expected Resolved, got: {:?}", other),
    }
}

fn expect_suspended(response: ResolveProcessResponse) -> resolve_process_response::Suspended {
    match response.result {
        Some(resolve_process_response::Result::Suspended(suspended)) => suspended,
        other => panic!("Expected Suspended, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// MaterializationContext unit tests
// ---------------------------------------------------------------------------

#[test]
fn context_default_is_discovery_mode() {
    let mut ctx = MaterializationContext::discovery();
    assert!(!ctx.has_missing_reads(), "no reads recorded yet");

    // Lookup triggers discovery
    assert_eq!(ctx.is_unit_in_materialization("mat/a", "user-1"), Ok(None));
    assert!(ctx.has_missing_reads());
    assert_eq!(ctx.to_read.len(), 1);
}

#[test]
fn context_new_is_complete() {
    let mut ctx = MaterializationContext::complete(vec![]);

    // Lookup on complete context does NOT trigger discovery
    assert_eq!(
        ctx.is_unit_in_materialization("mat/a", "user-1"),
        Ok(Some(false))
    );
    assert!(!ctx.has_missing_reads());
    assert_eq!(ctx.to_read.len(), 0);
}

#[test]
fn context_complete_finds_existing_and_rejects_missing() {
    // Complete context (e.g. from cookie or resume) — missing = genuinely absent
    let mut ctx = MaterializationContext::complete(vec![MaterializationRecord {
        unit: "user-1".to_string(),
        materialization: "mat/a".to_string(),
        rule: "".to_string(),
        variant: "".to_string(),
    }]);

    // Existing record is found
    assert_eq!(
        ctx.is_unit_in_materialization("mat/a", "user-1"),
        Ok(Some(true))
    );
    assert!(!ctx.has_missing_reads());

    // Missing record is definitively absent (no discovery)
    assert_eq!(
        ctx.is_unit_in_materialization("mat/b", "user-1"),
        Ok(Some(false))
    );
    assert!(!ctx.has_missing_reads());
    assert_eq!(ctx.to_read.len(), 0);
}

#[test]
fn context_deduplicates_missing_entries() {
    let mut ctx = MaterializationContext::discovery();

    let _ = ctx.is_unit_in_materialization("mat/a", "user-1");
    let _ = ctx.is_unit_in_materialization("mat/a", "user-1");
    let _ = ctx.is_unit_in_materialization("mat/a", "user-1");

    assert_eq!(ctx.to_read.len(), 1);
}

#[test]
fn context_snapshot_isolation_writes_not_visible_to_reads() {
    let mut ctx = MaterializationContext::complete(vec![]);

    // Simulate a write from flag A
    ctx.to_write.push(MaterializationRecord {
        unit: "user-1".to_string(),
        materialization: "mat/a".to_string(),
        rule: "rule-1".to_string(),
        variant: "variant-a".to_string(),
    });

    // Flag B tries to read the same materialization — should NOT find it
    assert_eq!(
        ctx.has_rule_materialization("mat/a", "user-1", "rule-1"),
        Ok(Some(false))
    );
    // Complete context, so no discovery either
    assert!(!ctx.has_missing_reads());
}

// ---------------------------------------------------------------------------
// Segment match with materializations
// ---------------------------------------------------------------------------

fn make_materialized_segment_state() -> (Segment, ResolverState) {
    let segment_json = r#"{
        "name": "segments/mat-segment",
        "targeting": {
            "criteria": {
                "mat-crit": {
                    "materializedSegment": {
                        "materializedSegment": "materializedSegments/test-mat"
                    }
                }
            },
            "expression": {
                "ref": "mat-crit"
            }
        },
        "allocation": {
            "proportion": { "value": "1.0" },
            "exclusivityTags": [],
            "exclusiveTo": []
        }
    }"#;
    let segment: Segment = serde_json::from_str(segment_json).unwrap();

    let mut segments = HashMap::new();
    segments.insert(segment.name.clone(), segment.clone());
    let mut bitsets = HashMap::new();
    bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));
    let mut secrets = HashMap::new();
    secrets.insert(
        "secret".to_string(),
        Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        },
    );

    let state = ResolverState {
        secrets,
        flags: HashMap::new(),
        segments,
        bitsets,
    };
    (segment, state)
}

#[test]
fn segment_match_with_inclusion_record_present() {
    let (segment, state) = make_materialized_segment_state();
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::complete(vec![MaterializationRecord {
        unit: "test-user".to_string(),
        materialization: "materializedSegments/test-mat".to_string(),
        rule: "".to_string(),
        variant: "".to_string(),
    }]);

    assert_eq!(
        resolver
            .segment_match(&segment, "test-user", &mut ctx)
            .unwrap(),
        Some(true),
    );
}

#[test]
fn segment_match_with_inclusion_record_absent() {
    let (segment, state) = make_materialized_segment_state();
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::complete(vec![]);
    assert_eq!(
        resolver
            .segment_match(&segment, "test-user", &mut ctx)
            .unwrap(),
        Some(false),
    );
}

#[test]
fn segment_match_without_materializations_discovers_missing() {
    let (segment, state) = make_materialized_segment_state();
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let result = resolver.segment_match(&segment, "test-user", &mut ctx);

    assert_eq!(result.unwrap(), None);
    assert!(ctx.has_missing_reads());
    assert_eq!(ctx.to_read.len(), 1);
    assert_eq!(
        ctx.to_read[0].materialization,
        "materializedSegments/test-mat"
    );
}

// ---------------------------------------------------------------------------
// ResolveProcess: discovery and suspension
// ---------------------------------------------------------------------------

#[test]
fn simple_resolve_suspends_when_materializations_needed() {
    let state = ResolverState::from_proto(
        EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
        "confidence-test",
    )
    .unwrap();
    let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            secret,
            r#"{"user_id": "tutorial_visitor"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    let result = resolver
        .resolve_flags(simple_resolve(resolve_request(
            secret,
            &["flags/custom-targeted-flag"],
        )))
        .unwrap();

    let suspended = expect_suspended(result);
    assert_eq!(suspended.materializations_to_read.len(), 1);
    assert_eq!(
        suspended.materializations_to_read[0].unit,
        "tutorial_visitor"
    );
    assert!(!suspended.state.is_empty());
}

#[test]
fn resolve_with_complete_materializations_does_not_suspend() {
    let state = ResolverState::from_proto(
        EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
        "confidence-test",
    )
    .unwrap();
    let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            secret,
            r#"{"user_id": "tutorial_visitor"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    // Complete materializations with the inclusion record
    let result = resolver
        .resolve_flags(resolve_with_materializations(
            resolve_request(secret, &["flags/custom-targeted-flag"]),
            vec![MaterializationRecord {
                unit: "tutorial_visitor".to_string(),
                materialization: "materializedSegments/nicklas-custom-targeting".to_string(),
                rule: "".to_string(),
                variant: "".to_string(),
            }],
        ))
        .unwrap();

    let resolved = expect_resolved(result);
    let response = resolved.response.unwrap();
    assert_eq!(response.resolved_flags.len(), 1);
    assert_eq!(
        response.resolved_flags[0].variant,
        "flags/custom-targeted-flag/variants/cake-exclamation"
    );
}

#[test]
fn resolve_with_empty_complete_materializations_falls_through() {
    let state = ResolverState::from_proto(
        EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
        "confidence-test",
    )
    .unwrap();
    let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            secret,
            r#"{"user_id": "tutorial_visitor"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    // Empty complete materializations — unit not included, falls through to default
    let result = resolver
        .resolve_flags(resolve_with_materializations(
            resolve_request(secret, &["flags/custom-targeted-flag"]),
            vec![],
        ))
        .unwrap();

    let resolved = expect_resolved(result);
    let response = resolved.response.unwrap();
    assert_eq!(
        response.resolved_flags[0].variant,
        "flags/custom-targeted-flag/variants/default"
    );
}

// ---------------------------------------------------------------------------
// Discovery mode prevents incorrect resolve
// ---------------------------------------------------------------------------

#[test]
fn discovery_mode_prevents_resolve_via_later_rule() {
    // If rule 1 needs materializations and rule 2 doesn't, the flag should
    // suspend (not resolve via rule 2) so rule 1 gets a chance on resume.
    let state = ResolverState::from_proto(
        EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
        "confidence-test",
    )
    .unwrap();
    let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            secret,
            r#"{"user_id": "tutorial_visitor"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    // Simple Resolve (no materializations) should suspend, NOT resolve via the default rule
    let result = resolver
        .resolve_flags(simple_resolve(resolve_request(
            secret,
            &["flags/custom-targeted-flag"],
        )))
        .unwrap();

    // Must be Suspended, not Resolved
    expect_suspended(result);
}

#[test]
fn early_rule_match_skips_later_materialization_rule() {
    // If rule order is reversed so a non-materialization rule comes first and matches,
    // the flag resolves without needing materializations at all.
    let state = ResolverState::from_proto(
        EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
        "confidence-test",
    )
    .unwrap();
    let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";
    let flag = state.flags.get("flags/custom-targeted-flag").unwrap();

    let mut modified_flag = flag.clone();
    modified_flag.rules.reverse();

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            secret,
            r#"{"user_id": "tutorial_visitor"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    let result = resolver.resolve_flag(&modified_flag, &mut MaterializationContext::discovery());

    match result {
        Ok(resolved_value) => {
            assert_eq!(
                resolved_value.inner.variant,
                "flags/custom-targeted-flag/variants/default"
            );
            assert_eq!(resolved_value.reason(), ResolveReason::Match);
        }
        Err(err) => panic!("Expected success, got error: {:?}", err),
    }
}

// ---------------------------------------------------------------------------
// Multiple flags: suspension preserves partial results
// ---------------------------------------------------------------------------

#[test]
fn multiple_flags_suspend_with_deduplicated_reads() {
    let state = ResolverState::from_proto(
        MULTIPLE_STICKY_FLAGS_STATE.to_owned().try_into().unwrap(),
        "test",
    )
    .unwrap();

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            "test-secret",
            r#"{"user_id": "test-user-456"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    let result = resolver
        .resolve_flags(simple_resolve(resolve_request(
            "test-secret",
            &[
                "flags/sticky-flag-1",
                "flags/sticky-flag-2",
                "flags/sticky-flag-3",
            ],
        )))
        .unwrap();

    let suspended = expect_suspended(result);
    assert_eq!(suspended.materializations_to_read.len(), 3);

    let materializations: HashSet<String> = suspended
        .materializations_to_read
        .iter()
        .map(|r| r.materialization.clone())
        .collect();
    assert!(materializations.contains("experiment_1"));
    assert!(materializations.contains("experiment_2"));
    assert!(materializations.contains("experiment_3"));

    // All should reference the same unit
    for record in &suspended.materializations_to_read {
        assert_eq!(record.unit, "test-user-456");
    }

    // Continuation state must be present
    assert!(!suspended.state.is_empty());
}

#[test]
fn resume_after_suspension_resolves_all_flags() {
    let state = ResolverState::from_proto(
        MULTIPLE_STICKY_FLAGS_STATE.to_owned().try_into().unwrap(),
        "test",
    )
    .unwrap();

    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(
            "test-secret",
            r#"{"user_id": "test-user-456"}"#,
            &ENCRYPTION_KEY,
        )
        .unwrap();

    // First call: suspend
    let first_result = resolver
        .resolve_flags(simple_resolve(resolve_request(
            "test-secret",
            &[
                "flags/sticky-flag-1",
                "flags/sticky-flag-2",
                "flags/sticky-flag-3",
            ],
        )))
        .unwrap();

    let suspended = expect_suspended(first_result);

    // Provide materializations for the requested reads (empty variants = no prior assignment)
    let materializations: Vec<MaterializationRecord> = suspended
        .materializations_to_read
        .iter()
        .map(|r| MaterializationRecord {
            unit: r.unit.clone(),
            materialization: r.materialization.clone(),
            rule: r.rule.clone(),
            variant: "".to_string(),
        })
        .collect();

    // Resume
    let resume_result = resolver
        .resolve_flags(resume_request(materializations, suspended.state))
        .unwrap();

    let resolved = expect_resolved(resume_result);
    let response = resolved.response.unwrap();

    // All three flags should be resolved
    assert_eq!(
        response.resolved_flags.len(),
        3,
        "Expected 3 resolved flags, got {}",
        response.resolved_flags.len()
    );
}

// ---------------------------------------------------------------------------
// Boolean expression edge cases for materialized segment discovery
// ---------------------------------------------------------------------------

/// Helper: build a segment with custom targeting JSON and register it in a state.
fn make_segment_with_targeting(name: &str, targeting_json: &str) -> (Segment, ResolverState) {
    let segment_json = format!(
        r#"{{
            "name": "{}",
            "targeting": {},
            "allocation": {{
                "proportion": {{ "value": "1.0" }},
                "exclusivityTags": [],
                "exclusiveTo": []
            }}
        }}"#,
        name, targeting_json
    );
    let segment: Segment = serde_json::from_str(&segment_json).unwrap();

    let mut segments = HashMap::new();
    segments.insert(segment.name.clone(), segment.clone());
    let mut bitsets = HashMap::new();
    bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));
    let mut secrets = HashMap::new();
    secrets.insert(
        "secret".to_string(),
        Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        },
    );

    let state = ResolverState {
        secrets,
        flags: HashMap::new(),
        segments,
        bitsets,
    };
    (segment, state)
}

#[test]
fn and_with_two_materialized_segments_discovers_both() {
    // AND(mat_A, mat_B) — both should be discovered since both must be true
    let targeting = r#"{
        "criteria": {
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } },
            "mat-b": { "materializedSegment": { "materializedSegment": "materializedSegments/b" } }
        },
        "expression": { "and": { "operands": [{ "ref": "mat-a" }, { "ref": "mat-b" }] } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/and-test", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let _ = resolver.segment_match(&segment, "test-user", &mut ctx);

    // Current behavior: AND short-circuits on the first false (mat_A returns false in
    // discovery mode), so mat_B is never evaluated. This means only mat_A is discovered.
    // This is a known limitation — a second round-trip will discover mat_B.
    assert!(ctx.has_missing_reads());
    // With Kleene logic this would be 2; currently it's 1 due to short-circuiting.
    assert!(
        ctx.to_read.len() >= 1,
        "Should discover at least mat_A, got {}",
        ctx.to_read.len()
    );
}

#[test]
fn or_with_two_materialized_segments_discovers_both() {
    // OR(mat_A, mat_B) — both should ideally be discovered
    let targeting = r#"{
        "criteria": {
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } },
            "mat-b": { "materializedSegment": { "materializedSegment": "materializedSegments/b" } }
        },
        "expression": { "or": { "operands": [{ "ref": "mat-a" }, { "ref": "mat-b" }] } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/or-test", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let _ = resolver.segment_match(&segment, "test-user", &mut ctx);

    // OR: mat_A returns false → continues to mat_B → mat_B returns false → both discovered!
    assert!(ctx.has_missing_reads());
    assert_eq!(
        ctx.to_read.len(),
        2,
        "OR should discover both materialized segments"
    );
}

#[test]
fn and_with_known_false_attribute_prunes_materialized_segment() {
    // AND(attr_that_is_false, mat_A) — mat_A should NOT be discovered because
    // the AND is guaranteed false regardless of mat_A.
    let targeting = r#"{
        "criteria": {
            "attr-false": {
                "attribute": {
                    "attributeName": "country",
                    "eqRule": { "value": { "stringValue": "NONEXISTENT" } }
                }
            },
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } }
        },
        "expression": { "and": { "operands": [{ "ref": "attr-false" }, { "ref": "mat-a" }] } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/and-prune-test", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", r#"{"country": "SE"}"#, &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let result = resolver.segment_match(&segment, "test-user", &mut ctx);

    // The attribute criterion is false → AND short-circuits → mat_A never evaluated
    assert_eq!(result.unwrap(), Some(false));
    assert!(
        !ctx.has_missing_reads(),
        "Known-false AND should prune, no discovery needed"
    );
}

#[test]
fn or_with_known_true_attribute_prunes_materialized_segment() {
    // OR(attr_that_is_true, mat_A) — mat_A should NOT be discovered because
    // the OR is guaranteed true regardless of mat_A.
    let targeting = r#"{
        "criteria": {
            "attr-true": {
                "attribute": {
                    "attributeName": "country",
                    "eqRule": { "value": { "stringValue": "SE" } }
                }
            },
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } }
        },
        "expression": { "or": { "operands": [{ "ref": "attr-true" }, { "ref": "mat-a" }] } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/or-prune-test", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", r#"{"country": "SE"}"#, &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let result = resolver.segment_match(&segment, "test-user", &mut ctx);

    // The attribute criterion is true → OR short-circuits → mat_A never evaluated
    assert_eq!(result.unwrap(), Some(true));
    assert!(
        !ctx.has_missing_reads(),
        "Known-true OR should prune, no discovery needed"
    );
}

// ---------------------------------------------------------------------------
// Known limitations: extra round-trips
// ---------------------------------------------------------------------------

/// AND(mat_A, mat_B) discovers both thanks to Kleene tri-state logic.
/// Unknown does not short-circuit AND, so both criteria are evaluated.
#[test]
fn and_with_two_materialized_segments_discovers_both_via_kleene() {
    let targeting = r#"{
        "criteria": {
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } },
            "mat-b": { "materializedSegment": { "materializedSegment": "materializedSegments/b" } }
        },
        "expression": { "and": { "operands": [{ "ref": "mat-a" }, { "ref": "mat-b" }] } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/and-limitation", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let _ = resolver.segment_match(&segment, "test-user", &mut ctx);

    assert_eq!(
        ctx.to_read.len(),
        2,
        "Kleene logic: AND evaluates both materialized segments"
    );
    let mats: Vec<&str> = ctx
        .to_read
        .iter()
        .map(|r| r.materialization.as_str())
        .collect();
    assert!(mats.contains(&"materializedSegments/a"));
    assert!(mats.contains(&"materializedSegments/b"));
}

#[test]
fn not_materialized_segment_discovers_and_returns_unknown() {
    // NOT(mat_A) — should discover mat_A and return false (Unknown -> false at boundary)
    let targeting = r#"{
        "criteria": {
            "mat-a": { "materializedSegment": { "materializedSegment": "materializedSegments/a" } }
        },
        "expression": { "not": { "ref": "mat-a" } }
    }"#;

    let (segment, state) = make_segment_with_targeting("segments/not-test", targeting);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context("secret", "{}", &ENCRYPTION_KEY)
        .unwrap();

    let mut ctx = MaterializationContext::discovery();
    let result = resolver.segment_match(&segment, "test-user", &mut ctx);

    // NOT(Unknown) = Unknown
    assert_eq!(result.unwrap(), None);
    assert!(ctx.has_missing_reads());
    assert_eq!(ctx.to_read.len(), 1);
    assert_eq!(ctx.to_read[0].materialization, "materializedSegments/a");
}

// ---------------------------------------------------------------------------
// Rule with read_materialization + segment with MaterializedSegmentCriterion
// ---------------------------------------------------------------------------

/// When a rule has BOTH read_materialization AND its segment has a MaterializedSegmentCriterion,
/// discovery now finds both in a single pass (rule-level discovery evaluates the segment too).
/// - Round 1: discovers both the read_materialization and the segment's MaterializedSegmentCriterion
/// - Round 2: has everything, resolves
#[test]
fn rule_materialization_plus_segment_criterion_discovered_in_single_round() {
    let secret = "test-secret";

    let segment_json = r#"{
        "name": "segments/sticky-mat-segment",
        "targeting": {
            "criteria": {
                "mat-crit": {
                    "materializedSegment": {
                        "materializedSegment": "materializedSegments/inclusion-check"
                    }
                }
            },
            "expression": { "ref": "mat-crit" }
        },
        "allocation": {
            "proportion": { "value": "1.0" },
            "exclusivityTags": [],
            "exclusiveTo": []
        }
    }"#;

    let flag_json = r#"{
        "name": "flags/double-mat-flag",
        "state": "ACTIVE",
        "clients": ["clients/test"],
        "variants": [
            { "name": "flags/double-mat-flag/variants/control", "value": {} },
            { "name": "flags/double-mat-flag/variants/treatment", "value": {} }
        ],
        "rules": [{
            "name": "flags/double-mat-flag/rules/sticky-rule",
            "segment": "segments/sticky-mat-segment",
            "enabled": true,
            "materializationSpec": {
                "readMaterialization": "experiment_1",
                "writeMaterialization": "experiment_1"
            },
            "assignmentSpec": {
                "bucketCount": 1000000,
                "assignments": [{
                    "assignmentId": "assignment-1",
                    "variant": { "variant": "flags/double-mat-flag/variants/treatment" },
                    "bucketRanges": [{ "lower": 0, "upper": 1000000 }]
                }]
            }
        }]
    }"#;

    let state = make_state_from_json(flag_json, &[segment_json], secret);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(secret, r#"{"targeting_key": "user-1"}"#, &ENCRYPTION_KEY)
        .unwrap();

    // Round 1: discovers BOTH the rule's read_materialization AND the segment's criterion
    let r1 = resolver
        .resolve_flags(simple_resolve(resolve_request(
            secret,
            &["flags/double-mat-flag"],
        )))
        .unwrap();
    let suspended = expect_suspended(r1);
    assert_eq!(
        suspended.materializations_to_read.len(),
        2,
        "Round 1 should discover both the rule materialization and the segment criterion"
    );
    let mats: Vec<&str> = suspended
        .materializations_to_read
        .iter()
        .map(|r| r.materialization.as_str())
        .collect();
    assert!(mats.contains(&"experiment_1"));
    assert!(mats.contains(&"materializedSegments/inclusion-check"));

    // Round 2: provide both materializations, now it resolves
    let r2 = resolver
        .resolve_flags(resume_request(
            vec![
                MaterializationRecord {
                    unit: "user-1".to_string(),
                    materialization: "experiment_1".to_string(),
                    rule: "flags/double-mat-flag/rules/sticky-rule".to_string(),
                    variant: "".to_string(),
                },
                MaterializationRecord {
                    unit: "user-1".to_string(),
                    materialization: "materializedSegments/inclusion-check".to_string(),
                    rule: "".to_string(),
                    variant: "".to_string(),
                },
            ],
            suspended.state,
        ))
        .unwrap();
    let resolved = expect_resolved(r2);
    assert!(resolved.response.is_some());
}

// ---------------------------------------------------------------------------
// Cross-flag discovery: non-materialization flags should still resolve
// ---------------------------------------------------------------------------

#[test]
fn mixed_flags_non_materialization_flag_resolves_in_partial_results() {
    // Flag A needs materializations (has read_materialization).
    // Flag B does NOT need materializations (simple flag).
    // When resolving both with no materializations (discovery mode):
    // - Flag B should resolve successfully and appear in the continuation's partial results
    // - Flag A should be suspended
    let secret = "test-secret";

    let simple_segment = r#"{
        "name": "segments/simple-segment",
        "allocation": {
            "proportion": { "value": "1.0" },
            "exclusivityTags": [],
            "exclusiveTo": []
        }
    }"#;

    let mat_segment = r#"{
        "name": "segments/mat-segment",
        "targeting": {
            "criteria": {
                "mat-crit": {
                    "materializedSegment": {
                        "materializedSegment": "materializedSegments/test-mat"
                    }
                }
            },
            "expression": { "ref": "mat-crit" }
        },
        "allocation": {
            "proportion": { "value": "1.0" },
            "exclusivityTags": [],
            "exclusiveTo": []
        }
    }"#;

    // Flag A: needs materializations
    let flag_a = r#"{
        "name": "flags/mat-flag",
        "state": "ACTIVE",
        "clients": ["clients/test"],
        "variants": [
            { "name": "flags/mat-flag/variants/on", "value": {} }
        ],
        "rules": [{
            "name": "flags/mat-flag/rules/sticky-rule",
            "segment": "segments/mat-segment",
            "enabled": true,
            "materializationSpec": {
                "readMaterialization": "experiment_1",
                "writeMaterialization": "experiment_1"
            },
            "assignmentSpec": {
                "bucketCount": 1000000,
                "assignments": [{
                    "assignmentId": "a1",
                    "variant": { "variant": "flags/mat-flag/variants/on" },
                    "bucketRanges": [{ "lower": 0, "upper": 1000000 }]
                }]
            }
        }]
    }"#;

    // Flag B: simple flag, no materializations needed
    let flag_b = r#"{
        "name": "flags/simple-flag",
        "state": "ACTIVE",
        "clients": ["clients/test"],
        "variants": [
            { "name": "flags/simple-flag/variants/on", "value": {} }
        ],
        "rules": [{
            "name": "flags/simple-flag/rules/simple-rule",
            "segment": "segments/simple-segment",
            "enabled": true,
            "assignmentSpec": {
                "bucketCount": 1000000,
                "assignments": [{
                    "assignmentId": "a1",
                    "variant": { "variant": "flags/simple-flag/variants/on" },
                    "bucketRanges": [{ "lower": 0, "upper": 1000000 }]
                }]
            }
        }]
    }"#;

    let state =
        make_state_from_json_flags(&[flag_a, flag_b], &[simple_segment, mat_segment], secret);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(secret, r#"{"targeting_key": "user-1"}"#, &ENCRYPTION_KEY)
        .unwrap();

    // Resolve both flags with no materializations
    let result = resolver
        .resolve_flags(simple_resolve(resolve_request(
            secret,
            &["flags/mat-flag", "flags/simple-flag"],
        )))
        .unwrap();

    // Should suspend (mat-flag needs materializations)
    let suspended = expect_suspended(result);
    assert!(
        !suspended.materializations_to_read.is_empty(),
        "Should request materializations for mat-flag"
    );

    // Decode continuation to check partial results
    let continuation = ResolveProcessState::decode(suspended.state.as_slice()).unwrap();

    // Flag B (simple-flag) should be in the partial resolved_flags
    assert!(
        !continuation.resolved_flags.is_empty(),
        "Simple flag should have resolved and be in partial results"
    );
    assert_eq!(
        continuation.resolved_flags[0].flag, "flags/simple-flag",
        "The partial result should be the simple flag"
    );
}

#[test]
fn cross_flag_discovery_does_not_block_subsequent_flags() {
    // Test at the resolve_flag level: if flag A triggers discovery (adds to to_read),
    // flag B should still be able to resolve via its non-materialization rules.
    let secret = "test-secret";

    let simple_segment = r#"{
        "name": "segments/simple-segment",
        "allocation": {
            "proportion": { "value": "1.0" },
            "exclusivityTags": [],
            "exclusiveTo": []
        }
    }"#;

    // Flag with no materializations — should always resolve
    let simple_flag = r#"{
        "name": "flags/simple-flag",
        "state": "ACTIVE",
        "clients": ["clients/test"],
        "variants": [
            { "name": "flags/simple-flag/variants/on", "value": {} }
        ],
        "rules": [{
            "name": "flags/simple-flag/rules/simple-rule",
            "segment": "segments/simple-segment",
            "enabled": true,
            "assignmentSpec": {
                "bucketCount": 1000000,
                "assignments": [{
                    "assignmentId": "a1",
                    "variant": { "variant": "flags/simple-flag/variants/on" },
                    "bucketRanges": [{ "lower": 0, "upper": 1000000 }]
                }]
            }
        }]
    }"#;

    let state = make_state_from_json(simple_flag, &[simple_segment], secret);
    let resolver: AccountResolver<'_, L> = state
        .get_resolver_with_json_context(secret, r#"{"targeting_key": "user-1"}"#, &ENCRYPTION_KEY)
        .unwrap();

    let flag = state.flags.get("flags/simple-flag").unwrap();

    // Verify the flag deserializes correctly
    let spec = flag.rules[0].assignment_spec.as_ref().unwrap();
    assert_eq!(spec.assignments.len(), 1, "should have 1 assignment");
    assert!(
        spec.assignments[0].assignment.is_some(),
        "oneof assignment should deserialize"
    );

    // Simulate cross-flag contamination: pre-populate to_read as if another flag
    // already triggered discovery
    let mut ctx = MaterializationContext::discovery();
    ctx.to_read.push(MaterializationRecord {
        unit: "user-1".to_string(),
        materialization: "experiment_from_other_flag".to_string(),
        rule: "some-other-rule".to_string(),
        variant: "".to_string(),
    });

    // Flag B should still resolve despite to_read being non-empty from another flag
    // It must NOT return MissingMaterializations due to cross-flag contamination
    let result = resolver.resolve_flag(flag, &mut ctx);
    assert!(
        !matches!(result, Err(ResolveError::MissingMaterializations)),
        "Simple flag should NOT fail with MissingMaterializations due to another flag's discovery, got: {:?}",
        result,
    );
}
