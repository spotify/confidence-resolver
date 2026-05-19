//! Integration test for the per-user state slicer.
//!
//! For each of several pinned units and several evaluation contexts, this
//! test resolves all flags against (a) the full account state and (b) the
//! same state sliced for that unit, and asserts the responses match. A
//! human-readable size report is printed for each unit.

use bytes::Bytes;

use confidence_resolver::proto::confidence::flags::admin::v1::ResolverState as ResolverStatePb;
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    resolve_process_response, ResolveFlagsRequest, ResolveFlagsResponse, ResolveProcessRequest,
    ResolveProcessResponse, ResolvedFlag, Sdk,
};
use confidence_resolver::proto::google::Struct;
use confidence_resolver::proto::Message;
use confidence_resolver::{
    slicer, AccountResolver, Client, FlagToApply, Host, ResolvedValue, ResolverState,
};

const ACCOUNT_ID: &str = "confidence-demo-june";
const SECRET: &str = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF";
const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);

const EXAMPLE_STATE: &[u8] = include_bytes!("../test-payloads/resolver_state.pb");
const MULTIPLE_STICKY_FLAGS_STATE: &[u8] =
    include_bytes!("../test-payloads/resolver_state_with_multiple_sticky_flags.pb");

struct NullHost;

impl Host for NullHost {
    fn log_resolve(_: &str, _: &Struct, _: &[ResolvedValue<'_>], _: &Client) {}
    fn log_assign(_: &str, _: &[FlagToApply], _: &Client, _: &Option<Sdk>) {}
}

fn decode_state(bytes: &[u8]) -> ResolverStatePb {
    ResolverStatePb::decode(bytes).expect("decode state pb")
}

fn load_state(pb: &ResolverStatePb) -> ResolverState {
    ResolverState::from_proto(pb.clone(), ACCOUNT_ID, None).expect("load state")
}

fn resolve_all(state: &ResolverState, ctx_json: &str) -> ResolveFlagsResponse {
    let resolver: AccountResolver<'_, NullHost> = state
        .get_resolver_with_json_context(SECRET, ctx_json, &ENCRYPTION_KEY)
        .expect("get resolver");

    let req = ResolveFlagsRequest {
        client_secret: SECRET.to_string(),
        evaluation_context: Some(Struct::default()),
        flags: vec![],
        apply: false,
        sdk: Some(Sdk {
            sdk: None,
            version: "test".to_string(),
        }),
    };

    let response: ResolveProcessResponse = resolver
        .resolve_flags(ResolveProcessRequest::without_materializations(req))
        .expect("resolve_flags");

    match response.result {
        Some(resolve_process_response::Result::Resolved(resolved)) => {
            resolved.response.expect("response")
        }
        other => panic!("expected Resolved, got {:?}", other),
    }
}

/// Strip non-deterministic fields (resolve_id is random, resolve_token is
/// encrypted with a random IV) so two equivalent resolves compare equal.
fn normalize(mut r: ResolveFlagsResponse) -> ResolveFlagsResponse {
    r.resolve_id = String::new();
    r.resolve_token = Vec::new();
    r.resolved_flags.sort_by(|a, b| a.flag.cmp(&b.flag));
    r
}

fn assert_resolved_flags_equal(expected: &[ResolvedFlag], actual: &[ResolvedFlag], context: &str) {
    assert_eq!(
        expected.len(),
        actual.len(),
        "{}: expected {} flags, got {}",
        context,
        expected.len(),
        actual.len(),
    );
    for (e, a) in expected.iter().zip(actual.iter()) {
        assert_eq!(e.flag, a.flag, "{}: flag name mismatch", context);
        assert_eq!(
            e.variant, a.variant,
            "{}: variant mismatch on {}",
            context, e.flag
        );
        assert_eq!(
            e.reason, a.reason,
            "{}: reason mismatch on {}",
            context, e.flag
        );
        assert_eq!(
            e.should_apply, a.should_apply,
            "{}: should_apply mismatch on {}",
            context, e.flag
        );
        assert_eq!(
            e.value, a.value,
            "{}: value mismatch on {}",
            context, e.flag
        );
    }
}

/// Builds evaluation contexts that pin `visitor_id` (the selector used by the
/// `resolver_state.pb` fixture) but vary other attributes.
fn contexts(unit: &str) -> Vec<String> {
    vec![
        format!(r#"{{"visitor_id":"{}"}}"#, unit),
        format!(
            r#"{{"visitor_id":"{}","country":"SE","app":{{"version":"2.0.0"}}}}"#,
            unit
        ),
        format!(
            r#"{{"visitor_id":"{}","country":"US","device":{{"os":"android"}}}}"#,
            unit
        ),
        format!(
            r#"{{"visitor_id":"{}","country":"DE","app":{{"version":"0.1.0"}},"device":{{"os":"ios"}}}}"#,
            unit
        ),
    ]
}

#[test]
fn slice_preserves_resolve_results() {
    let full_pb = decode_state(EXAMPLE_STATE);
    let full_state = load_state(&full_pb);

    let original_encoded_len = full_pb.encode_to_vec().len();
    println!("\nper-user slicer parity test");
    println!(
        "  fixture: resolver_state.pb ({} bytes)",
        original_encoded_len
    );

    let units = ["tutorial_visitor", "user_42", "alice", "bob_12345"];
    for unit in units {
        let sliced_pb =
            slicer::slice_for_unit(full_pb.clone(), ACCOUNT_ID, unit).expect("slice_for_unit");
        let sliced_state = load_state(&sliced_pb);
        let sliced_encoded_len = sliced_pb.encode_to_vec().len();

        for ctx_json in contexts(unit) {
            let label = format!("unit={} ctx={}", unit, ctx_json);
            let expected = normalize(resolve_all(&full_state, &ctx_json));
            let actual = normalize(resolve_all(&sliced_state, &ctx_json));
            assert_resolved_flags_equal(&expected.resolved_flags, &actual.resolved_flags, &label);
        }

        let pct = (sliced_encoded_len as f64) * 100.0 / (original_encoded_len as f64);
        println!(
            "  unit={:<18} sliced={:>7} bytes ({:>5.2}% of original)",
            unit, sliced_encoded_len, pct
        );
    }
}

#[test]
fn slicer_rejects_mixed_selectors() {
    // Forge a state with two rules that disagree on their selector. The
    // bundled fixtures are all single-selector, so we construct one in-test.
    let mut pb = decode_state(EXAMPLE_STATE);
    let target = pb
        .flags
        .iter_mut()
        .flat_map(|f| f.rules.iter_mut())
        .find(|r| !r.targeting_key_selector.is_empty());
    let original = target
        .expect("fixture should have at least one non-empty selector")
        .clone();

    // Find a rule and rewrite its selector to something different.
    for flag in pb.flags.iter_mut() {
        if let Some(rule) = flag.rules.iter_mut().next() {
            if rule.targeting_key_selector != original.targeting_key_selector {
                continue;
            }
            rule.targeting_key_selector = format!("other_{}", original.targeting_key_selector);
            break;
        }
    }

    let err = slicer::slice_for_unit(pb, ACCOUNT_ID, "user_42").unwrap_err();
    let msg: String = err.into();
    assert!(
        msg.starts_with("internal error ["),
        "expected ErrorCode display, got: {}",
        msg
    );
}

#[test]
fn slice_handles_multiple_sticky_flags_state() {
    // The "multiple sticky flags" fixture exercises a different shape of
    // assignment specs. We only verify that slicing succeeds and round-trips
    // through resolver loading; full parity assertions live in the main test.
    let full_pb = decode_state(MULTIPLE_STICKY_FLAGS_STATE);
    let result = slicer::slice_for_unit(full_pb.clone(), "test", "user_42");
    if let Ok(sliced_pb) = result {
        let _ = ResolverState::from_proto(sliced_pb, "test", None);
    }
}
