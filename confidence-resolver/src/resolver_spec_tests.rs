use std::collections::HashMap;
use std::sync::LazyLock;

use bitvec::prelude as bv;
use bytes::Bytes;
use serde::Deserialize;

use crate::proto::confidence::flags::resolver::v1 as flags_resolver;
use crate::proto::confidence::flags::resolver::v1::{
    resolve_process_request, resolve_process_response, MaterializationRecord,
    ResolveProcessRequest, ResolveReason,
};
use crate::proto::google::Struct;
use crate::*;

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
// Serde types for spec parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SpecState {
    account: SpecAccount,
    flags: HashMap<String, serde_json::Value>,
    segments: HashMap<String, serde_json::Value>,
    bitsets: HashMap<String, serde_json::Value>,
    clients: HashMap<String, SpecClientEntry>,
}

#[derive(Deserialize)]
struct SpecAccount {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecClientEntry {
    client: SpecClient,
    client_credential: SpecCredential,
}

#[derive(Deserialize)]
struct SpecClient {
    name: String,
}

#[derive(Deserialize)]
struct SpecCredential {
    name: String,
    #[serde(default)]
    environments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecTestCase {
    name: String,
    resolve_request: SpecResolveRequest,
    materializations: Option<HashMap<String, HashMap<String, SpecMaterialization>>>,
    expected_result: SpecExpectedResult,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecResolveRequest {
    flags: Vec<String>,
    evaluation_context: Option<serde_json::Value>,
    client_secret: String,
    #[serde(default)]
    apply: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecMaterialization {
    is_unit_in_materialization: bool,
    rule_to_variant: HashMap<String, String>,
}

#[derive(Deserialize)]
struct SpecExpectedResult {
    general: SpecExpected,
    rust: Option<SpecExpected>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecExpected {
    resolved_flags: Vec<SpecResolvedFlag>,
    expect_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecResolvedFlag {
    flag: String,
    reason: String,
    variant: String,
    value: Option<serde_json::Value>,
    should_apply: Option<bool>,
}

// ---------------------------------------------------------------------------
// Building ResolverState from spec
// ---------------------------------------------------------------------------

fn build_state_from_spec(spec: &SpecState) -> ResolverState {
    let mut flags = HashMap::new();
    for (name, val) in &spec.flags {
        let flag: Flag = serde_json::from_value(val.clone()).unwrap();
        flags.insert(name.clone(), flag);
    }

    let mut segments = HashMap::new();
    for (name, val) in &spec.segments {
        let segment: Segment = serde_json::from_value(val.clone()).unwrap();
        segments.insert(name.clone(), segment);
    }

    let mut bitsets = HashMap::new();
    for (name, val) in &spec.bitsets {
        let bits = match val {
            serde_json::Value::String(s) if s == "ALL" => bv::BitVec::repeat(true, 1_000_000),
            serde_json::Value::Array(indices) => {
                let mut bv = bv::BitVec::repeat(false, 1_000_000);
                for idx in indices {
                    bv.set(idx.as_u64().unwrap() as usize, true);
                }
                bv
            }
            _ => panic!("unexpected bitset value for {}: {:?}", name, val),
        };
        bitsets.insert(name.clone(), bits);
    }

    let mut secrets = HashMap::new();
    for (secret, entry) in &spec.clients {
        secrets.insert(
            secret.clone(),
            Client {
                account: Account::new(&spec.account.name),
                client_name: entry.client.name.clone(),
                client_credential_name: entry.client_credential.name.clone(),
                environments: entry.client_credential.environments.clone(),
            },
        );
    }

    ResolverState {
        secrets,
        flags,
        segments,
        bitsets,
    }
}

// ---------------------------------------------------------------------------
// Materializations conversion
// ---------------------------------------------------------------------------

fn convert_materializations(
    mat_spec: &HashMap<String, HashMap<String, SpecMaterialization>>,
) -> Vec<MaterializationRecord> {
    let mut records = Vec::new();
    for (unit, mats) in mat_spec {
        for (mat_name, spec) in mats {
            if spec.is_unit_in_materialization {
                if spec.rule_to_variant.is_empty() {
                    records.push(MaterializationRecord {
                        unit: unit.clone(),
                        materialization: mat_name.clone(),
                        rule: String::new(),
                        variant: String::new(),
                    });
                } else {
                    for (rule, variant) in &spec.rule_to_variant {
                        records.push(MaterializationRecord {
                            unit: unit.clone(),
                            materialization: mat_name.clone(),
                            rule: rule.clone(),
                            variant: variant.clone(),
                        });
                    }
                }
            }
            // !is_unit_in_materialization → absence = not in materialization
        }
    }
    records
}

// ---------------------------------------------------------------------------
// Reason mapping
// ---------------------------------------------------------------------------

fn parse_reason(reason_str: &str) -> i32 {
    match reason_str {
        "RESOLVE_REASON_MATCH" => ResolveReason::Match as i32,
        "RESOLVE_REASON_NO_SEGMENT_MATCH" => ResolveReason::NoSegmentMatch as i32,
        "RESOLVE_REASON_NO_TREATMENT_MATCH" => ResolveReason::NoTreatmentMatch as i32,
        "RESOLVE_REASON_FLAG_ARCHIVED" => ResolveReason::FlagArchived as i32,
        "RESOLVE_REASON_TARGETING_KEY_ERROR" => ResolveReason::TargetingKeyError as i32,
        "RESOLVE_REASON_ERROR" => ResolveReason::Error as i32,
        "RESOLVE_REASON_UNSPECIFIED" => ResolveReason::Unspecified as i32,
        "RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE" => {
            ResolveReason::UnrecognizedTargetingRule as i32
        }
        "RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED" => {
            ResolveReason::MaterializationNotSupported as i32
        }
        _ => panic!("unknown resolve reason: {}", reason_str),
    }
}

// ---------------------------------------------------------------------------
// Spec data loading (once per process)
// ---------------------------------------------------------------------------

static SPEC_DATA: LazyLock<(ResolverState, Vec<SpecTestCase>)> = LazyLock::new(|| {
    let state_json = include_str!("../test-payloads/resolver-spec/state.json");
    let tests_json = include_str!("../test-payloads/resolver-spec/tests.json");

    let spec_state: SpecState = serde_json::from_str(state_json).unwrap();
    let tests: Vec<SpecTestCase> = serde_json::from_str(tests_json).unwrap();

    let resolver_state = build_state_from_spec(&spec_state);
    (resolver_state, tests)
});

fn get_spec_data() -> &'static (ResolverState, Vec<SpecTestCase>) {
    &SPEC_DATA
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

fn run_spec_test(state: &ResolverState, test_case: &SpecTestCase) {
    let expected = test_case
        .expected_result
        .rust
        .as_ref()
        .unwrap_or(&test_case.expected_result.general);

    let context_json = test_case
        .resolve_request
        .evaluation_context
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap())
        .unwrap_or_else(|| "{}".to_string());

    let resolver_result = state.get_resolver_with_json_context::<L>(
        &test_case.resolve_request.client_secret,
        &context_json,
        &ENCRYPTION_KEY,
    );

    let resolver = match resolver_result {
        Ok(r) => r,
        Err(e) => {
            if expected.expect_error.is_some() {
                return;
            }
            panic!(
                "[{}] Unexpected error creating resolver: {}",
                test_case.name, e
            );
        }
    };

    let req = flags_resolver::ResolveFlagsRequest {
        evaluation_context: Some(serde_json::from_str(&context_json).unwrap()),
        client_secret: test_case.resolve_request.client_secret.clone(),
        flags: test_case.resolve_request.flags.clone(),
        apply: test_case.resolve_request.apply,
        sdk: None,
    };

    let process_req = if let Some(mat_spec) = &test_case.materializations {
        let records = convert_materializations(mat_spec);
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::StaticMaterializations(
                resolve_process_request::StaticMaterializations {
                    resolve_request: Some(req),
                    materializations: records,
                },
            )),
        }
    } else {
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::DeferredMaterializations(
                req,
            )),
        }
    };

    let result = resolver.resolve_flags(process_req);

    if let Some(expected_error) = &expected.expect_error {
        assert!(
            result.is_err(),
            "[{}] Expected error '{}' but got success",
            test_case.name,
            expected_error,
        );
        return;
    }

    let response = result.unwrap_or_else(|e| {
        panic!("[{}] resolve_flags failed: {}", test_case.name, e);
    });

    let resolved = match response.result {
        Some(resolve_process_response::Result::Resolved(r)) => r,
        Some(resolve_process_response::Result::Suspended(s)) => {
            panic!(
                "[{}] Expected Resolved but got Suspended with {} reads",
                test_case.name,
                s.materializations_to_read.len(),
            );
        }
        None => panic!("[{}] Empty response result", test_case.name),
    };

    let resolve_response = resolved
        .response
        .unwrap_or_else(|| panic!("[{}] Missing response in Resolved", test_case.name));
    let actual_flags = &resolve_response.resolved_flags;

    assert_eq!(
        actual_flags.len(),
        expected.resolved_flags.len(),
        "[{}] Number of resolved flags: expected {}, got {}",
        test_case.name,
        expected.resolved_flags.len(),
        actual_flags.len(),
    );

    for expected_flag in &expected.resolved_flags {
        let actual = actual_flags
            .iter()
            .find(|f| f.flag == expected_flag.flag)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] Expected flag '{}' not found in response. Got: {:?}",
                    test_case.name,
                    expected_flag.flag,
                    actual_flags.iter().map(|f| &f.flag).collect::<Vec<_>>(),
                )
            });

        assert_eq!(
            actual.reason,
            parse_reason(&expected_flag.reason),
            "[{}] Reason mismatch for flag '{}': expected {}, got {}",
            test_case.name,
            expected_flag.flag,
            expected_flag.reason,
            actual.reason,
        );

        assert_eq!(
            actual.variant, expected_flag.variant,
            "[{}] Variant mismatch for flag '{}'",
            test_case.name, expected_flag.flag,
        );

        // Value comparison
        match (&expected_flag.value, &actual.value) {
            (None, None) => {}
            // client_default returns Some(empty Struct)
            (None, Some(s)) if s.fields.is_empty() => {}
            (Some(expected_val), Some(actual_struct)) => {
                let expected_struct: Struct = serde_json::from_value(expected_val.clone())
                    .unwrap_or_else(|e| {
                        panic!(
                            "[{}] Failed to parse expected value for '{}': {}",
                            test_case.name, expected_flag.flag, e,
                        )
                    });
                assert_eq!(
                    *actual_struct, expected_struct,
                    "[{}] Value mismatch for flag '{}'",
                    test_case.name, expected_flag.flag,
                );
            }
            (expected, actual) => {
                panic!(
                    "[{}] Value mismatch for flag '{}': expected {:?}, got {:?}",
                    test_case.name, expected_flag.flag, expected, actual,
                );
            }
        }

        // shouldApply assertion
        if let Some(expected_should_apply) = expected_flag.should_apply {
            let actual_should_apply = actual.should_apply;
            assert_eq!(
                actual_should_apply, expected_should_apply,
                "[{}] shouldApply mismatch for flag '{}': expected {}, got {}",
                test_case.name, expected_flag.flag, expected_should_apply, actual_should_apply,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test generation macro
// ---------------------------------------------------------------------------

macro_rules! spec_test {
    ($name:ident) => {
        #[test]
        fn $name() {
            let (state, tests) = get_spec_data();
            let test_case = tests
                .iter()
                .find(|t| t.name == stringify!($name))
                .unwrap_or_else(|| {
                    panic!("Test case '{}' not found in tests.json", stringify!($name))
                });
            run_spec_test(state, test_case);
        }
    };
}

// Basic flag filtering
spec_test!(no_flags_for_client);
spec_test!(inactive_flag_filtered);
spec_test!(flag_name_filter);

// Rule enablement
spec_test!(rule_not_enabled);

spec_test!(rule_not_enabled_for_env);

// Segment lookup
spec_test!(segment_not_found);

// Targeting key handling
spec_test!(targeting_key_blank_full_rollout);
spec_test!(targeting_key_null_value);
spec_test!(targeting_key_string);
spec_test!(targeting_key_integer);
spec_test!(targeting_key_fractional);
spec_test!(targeting_key_non_string_number);
spec_test!(targeting_key_too_long);

// Targeting criteria
spec_test!(targeting_eq_match);
spec_test!(targeting_eq_no_match);
spec_test!(targeting_set_match);
spec_test!(targeting_range_match);
spec_test!(targeting_starts_with);
spec_test!(targeting_ends_with);
spec_test!(targeting_any_rule);
spec_test!(targeting_all_rule);
spec_test!(targeting_missing_attribute);
spec_test!(unrecognized_targeting_rule);

// Bitset
spec_test!(bitset_not_matched);
spec_test!(bitset_no_unit);

// Nested / circular segments
spec_test!(nested_segment_match);
spec_test!(circular_segment_dependency);

// Variant / assignment
spec_test!(variant_match);
spec_test!(fallthrough_then_match);
spec_test!(client_default_match);
spec_test!(bucket_gap_no_match);
spec_test!(full_rollout_no_targeting_key);
spec_test!(multi_variant_no_targeting_key);

// Any rule with inner rules
spec_test!(any_rule_set_inner);
spec_test!(any_rule_starts_with_inner);
spec_test!(any_rule_ends_with_inner);

// Range variants
spec_test!(range_exclusive_match);
spec_test!(range_end_inclusive_only);
spec_test!(range_end_exclusive_only);

spec_test!(env_matching);

// Materializations
spec_test!(mat_unit_in_mat_variant_found);
spec_test!(mat_unit_not_in_mat);
spec_test!(mat_must_match_skip);
spec_test!(mat_ignore_targeting);
spec_test!(mat_no_unit);
spec_test!(mat_write);
spec_test!(mat_unit_in_mat_targeting_fails);
spec_test!(mat_criterion_match);
spec_test!(mat_matched_no_variant_for_rule);
spec_test!(mat_write_no_unit);
spec_test!(mat_variant_mismatch);

// Batch
spec_test!(batch_mat_and_non_mat);

// Resolve all
spec_test!(resolve_all_for_client);

// Bucket range
spec_test!(multi_bucket_range_match);
spec_test!(nonzero_lower_match);

// Criterion / rule edge cases
spec_test!(criterion_not_set);
spec_test!(empty_set_rule);
spec_test!(empty_range_rule);
spec_test!(any_empty_set_inner);
spec_test!(any_unrecognized_inner_rule);

// And / Or / Not expressions
spec_test!(and_targeting_both_match);
spec_test!(and_targeting_one_fails);
spec_test!(or_targeting_one_match);
spec_test!(or_targeting_none_match);
spec_test!(not_expression_inverts_match);
spec_test!(not_expression_inverts_non_match);

// Rule fallthrough
spec_test!(disabled_rule_then_match);
spec_test!(env_mismatch_then_match);
spec_test!(missing_segment_then_match);
spec_test!(null_targeting_key_then_match);
spec_test!(no_assignment_then_match);
spec_test!(fallthrough_with_write_mat);
spec_test!(fallthrough_then_no_match_should_apply);

// Additional materialization tests
spec_test!(mat_targeting_check_no_variant);
spec_test!(mat_targeting_check_fails);

// Additional targeting rule tests
spec_test!(all_rule_one_element_fails);
spec_test!(any_rule_none_match);
spec_test!(set_no_match);
spec_test!(range_no_match);
spec_test!(starts_with_no_match);
spec_test!(ends_with_no_match);

// Apply tests
spec_test!(resolve_with_apply_true);
spec_test!(resolve_with_apply_true_empty_result);
spec_test!(resolve_with_apply_true_no_match);
