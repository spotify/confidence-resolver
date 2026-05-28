use crate::proto::confidence::flags::resolver::v1 as flags_resolver;
use crate::proto::google::Struct;
use crate::*;

use crate::resolver_spec_tests::build_state_from_json;

const SECRET: &str = "diamond-test-secret";
const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);
const CONTEXT: &str = r#"{"targeting_key": "user1", "email": "test@example.com"}"#;

struct L;

impl Host for L {
    fn log_resolve(
        _resolve_id: &str,
        _evaluation_context: &Struct,
        _values: &[ResolvedValue<'_>],
        _client: &Client,
    ) {
    }

    fn log_assign(
        _resolve_id: &str,
        _assigned_flag: &[FlagToApply],
        _client: &Client,
        _sdk: &Option<flags_resolver::Sdk>,
    ) {
    }
}

/// Minimal state reproducing the diamond segment dependency bug:
///
///   diamond-parent ──OR──▶ diamond-child ──▶ diamond-leaf
///        │                                        ▲
///        └────────────OR──────────────────────────┘
///
/// The visited set marks diamond-leaf when evaluating via diamond-child,
/// then falsely rejects it as circular when evaluating the direct reference.
fn make_diamond_state() -> ResolverState {
    build_state_from_json(
        r#"{
        "account": { "name": "accounts/test-account" },
        "flags": {
            "flags/diamond-flag": {
                "name": "flags/diamond-flag",
                "state": "ACTIVE",
                "clients": ["clients/test-client"],
                "variants": [{ "name": "on", "value": { "enabled": true } }],
                "rules": [{
                    "name": "flags/diamond-flag/rules/rule1",
                    "segment": "segments/diamond-parent",
                    "enabled": true,
                    "targetingKeySelector": "targeting_key",
                    "assignmentSpec": {
                        "bucketCount": 1000,
                        "assignments": [{
                            "assignmentId": "a1",
                            "variant": { "variant": "on" },
                            "bucketRanges": [{ "lower": 0, "upper": 1000 }]
                        }]
                    }
                }]
            },
            "flags/ok-flag": {
                "name": "flags/ok-flag",
                "state": "ACTIVE",
                "clients": ["clients/test-client"],
                "variants": [{ "name": "on", "value": { "enabled": true } }],
                "rules": [{
                    "name": "flags/ok-flag/rules/rule1",
                    "segment": "segments/simple-match",
                    "enabled": true,
                    "targetingKeySelector": "targeting_key",
                    "assignmentSpec": {
                        "bucketCount": 1000,
                        "assignments": [{
                            "assignmentId": "a1",
                            "variant": { "variant": "on" },
                            "bucketRanges": [{ "lower": 0, "upper": 1000 }]
                        }]
                    }
                }]
            }
        },
        "segments": {
            "segments/diamond-parent": {
                "name": "segments/diamond-parent",
                "state": "OK",
                "targeting": {
                    "criteria": {
                        "c1": { "segment": { "segment": "segments/diamond-child" } },
                        "c2": { "segment": { "segment": "segments/diamond-leaf" } }
                    },
                    "expression": { "or": { "operands": [{ "ref": "c1" }, { "ref": "c2" }] } }
                }
            },
            "segments/diamond-child": {
                "name": "segments/diamond-child",
                "state": "OK",
                "targeting": {
                    "criteria": {
                        "c1": { "segment": { "segment": "segments/diamond-leaf" } }
                    },
                    "expression": { "ref": "c1" }
                }
            },
            "segments/simple-match": {
                "name": "segments/simple-match",
                "state": "OK",
                "targeting": {
                    "criteria": {
                        "c1": {
                            "attribute": {
                                "attributeName": "email",
                                "eqRule": { "value": { "stringValue": "test@example.com" } }
                            }
                        }
                    },
                    "expression": { "ref": "c1" }
                }
            },
            "segments/diamond-leaf": {
                "name": "segments/diamond-leaf",
                "state": "OK",
                "targeting": {
                    "criteria": {
                        "c1": {
                            "attribute": {
                                "attributeName": "email",
                                "eqRule": { "value": { "stringValue": "will-not-match@example.com" } }
                            }
                        }
                    },
                    "expression": { "ref": "c1" }
                }
            }
        },
        "bitsets": {
            "segments/simple-match": "ALL",
            "segments/diamond-parent": "ALL",
            "segments/diamond-child": "ALL",
            "segments/diamond-leaf": "ALL"
        },
        "clients": {
            "diamond-test-secret": {
                "client": { "name": "clients/test-client", "displayName": "Test Client" },
                "clientCredential": {
                    "name": "clients/test-client/credentials/cred1",
                    "displayName": "Cred",
                    "clientSecret": { "secret": "diamond-test-secret" }
                }
            }
        },
        "schema": {},
        "stateFileHash": "diamond-test"
    }"#,
    )
}

fn make_resolver(state: &ResolverState) -> AccountResolver<'_, L> {
    state
        .get_resolver_with_json_context(SECRET, CONTEXT, &ENCRYPTION_KEY)
        .unwrap()
}

#[test]
fn diamond_single_flag_resolves_ok() {
    let state = make_diamond_state();
    let resolver = make_resolver(&state);
    let req = flags_resolver::ResolveFlagsRequest {
        evaluation_context: Some(Struct::default()),
        client_secret: SECRET.to_string(),
        flags: vec!["flags/diamond-flag".to_string()],
        apply: false,
        sdk: None,
    };
    let resp = resolver.resolve_flags_no_materialization(&req).unwrap();
    assert_eq!(resp.resolved_flags.len(), 1);
    assert_eq!(
        resp.resolved_flags[0].reason,
        flags_resolver::ResolveReason::NoSegmentMatch as i32,
    );
}

#[test]
fn diamond_unaffected_flag_resolves_ok() {
    let state = make_diamond_state();
    let resolver = make_resolver(&state);
    let req = flags_resolver::ResolveFlagsRequest {
        evaluation_context: Some(Struct::default()),
        client_secret: SECRET.to_string(),
        flags: vec!["flags/ok-flag".to_string()],
        apply: false,
        sdk: None,
    };
    let resp = resolver.resolve_flags_no_materialization(&req).unwrap();
    assert_eq!(resp.resolved_flags.len(), 1);
    assert_eq!(
        resp.resolved_flags[0].reason,
        flags_resolver::ResolveReason::Match as i32,
    );
}

#[test]
fn diamond_batch_resolve_succeeds() {
    let state = make_diamond_state();
    let resolver = make_resolver(&state);
    let req = flags_resolver::ResolveFlagsRequest {
        evaluation_context: Some(Struct::default()),
        client_secret: SECRET.to_string(),
        flags: vec![],
        apply: false,
        sdk: None,
    };
    let resp = resolver.resolve_flags_no_materialization(&req).unwrap();
    assert_eq!(resp.resolved_flags.len(), 2);
}
