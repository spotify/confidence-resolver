//! Test utilities for the Confidence OpenFeature provider.
//!
//! This module provides mock implementations and state builders for testing,
//! similar to Go's `testutil/helpers.go`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use prost::Message;

use confidence_resolver::proto::confidence::flags::admin::v1::{
    flag, Flag, ResolverState, Segment,
};
use confidence_resolver::proto::confidence::iam::v1::{
    client_credential, Client, ClientCredential,
};
use confidence_resolver::proto::google::{value, Struct, Value as ProtoValue};
use confidence_resolver::ResolverState as ResolverStateRuntime;

use crate::error::Result;
use crate::materialization::{MaterializationStore, ReadOpType, ReadResultType, WriteOp};

/// Test client secret used in mock states.
pub const TEST_CLIENT_SECRET: &str = "test-secret";

/// Test account ID used in mock states.
pub const TEST_ACCOUNT_ID: &str = "test-account";

/// Test client name.
const TEST_CLIENT_NAME: &str = "clients/test-client";

/// Test credential name.
const TEST_CREDENTIAL_NAME: &str = "clients/test-client/credentials/test-credential";

/// Create a minimal resolver state with just client credentials (no flags).
///
/// Similar to Go's `CreateMinimalResolverState`.
pub fn create_minimal_state() -> (ResolverStateRuntime, String) {
    let state_pb = ResolverState {
        flags: vec![],
        clients: vec![Client {
            name: TEST_CLIENT_NAME.to_string(),
            ..Default::default()
        }],
        client_credentials: vec![ClientCredential {
            name: TEST_CREDENTIAL_NAME.to_string(),
            credential: Some(client_credential::Credential::ClientSecret(
                client_credential::ClientSecret {
                    secret: TEST_CLIENT_SECRET.to_string(),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };

    let state_bytes = state_pb.encode_to_vec();
    let state = ResolverStateRuntime::from_proto(
        ResolverState::decode(Bytes::from(state_bytes)).unwrap(),
        TEST_ACCOUNT_ID,
    )
    .expect("Failed to create minimal state");

    (state, TEST_ACCOUNT_ID.to_string())
}

/// Create a resolver state with a simple flag (no materializations).
///
/// This is useful for testing basic flag resolution.
pub fn create_state_with_flag() -> (ResolverStateRuntime, String) {
    let segment_name = "segments/always-true";

    // Create flag with two variants
    let flag_name = "flags/test-flag";
    let variant_on = format!("{}/variants/on", flag_name);
    let variant_off = format!("{}/variants/off", flag_name);

    let mut on_value_fields = HashMap::new();
    on_value_fields.insert(
        "enabled".to_string(),
        ProtoValue {
            kind: Some(value::Kind::BoolValue(true)),
        },
    );
    on_value_fields.insert(
        "message".to_string(),
        ProtoValue {
            kind: Some(value::Kind::StringValue("Feature is on".to_string())),
        },
    );

    let mut off_value_fields = HashMap::new();
    off_value_fields.insert(
        "enabled".to_string(),
        ProtoValue {
            kind: Some(value::Kind::BoolValue(false)),
        },
    );
    off_value_fields.insert(
        "message".to_string(),
        ProtoValue {
            kind: Some(value::Kind::StringValue("Feature is off".to_string())),
        },
    );

    let state_pb = ResolverState {
        flags: vec![Flag {
            name: flag_name.to_string(),
            state: flag::State::Active as i32,
            clients: vec![TEST_CLIENT_NAME.to_string()],
            variants: vec![
                flag::Variant {
                    name: variant_on.clone(),
                    value: Some(Struct {
                        fields: on_value_fields,
                    }),
                    ..Default::default()
                },
                flag::Variant {
                    name: variant_off.clone(),
                    value: Some(Struct {
                        fields: off_value_fields,
                    }),
                    ..Default::default()
                },
            ],
            rules: vec![flag::Rule {
                name: format!("{}/rules/default", flag_name),
                segment: segment_name.to_string(),
                targeting_key_selector: "targeting_key".to_string(),
                enabled: true,
                assignment_spec: Some(flag::rule::AssignmentSpec {
                    bucket_count: 2,
                    assignments: vec![
                        flag::rule::Assignment {
                            assignment_id: "on".to_string(),
                            assignment: Some(flag::rule::assignment::Assignment::Variant(
                                flag::rule::assignment::VariantAssignment {
                                    variant: variant_on.clone(),
                                },
                            )),
                            bucket_ranges: vec![flag::rule::BucketRange { lower: 0, upper: 1 }],
                        },
                        flag::rule::Assignment {
                            assignment_id: "off".to_string(),
                            assignment: Some(flag::rule::assignment::Assignment::Variant(
                                flag::rule::assignment::VariantAssignment {
                                    variant: variant_off.clone(),
                                },
                            )),
                            bucket_ranges: vec![flag::rule::BucketRange { lower: 1, upper: 2 }],
                        },
                    ],
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        segments_no_bitsets: vec![Segment {
            name: segment_name.to_string(),
            ..Default::default()
        }],
        bitsets: vec![
            confidence_resolver::proto::confidence::flags::admin::v1::resolver_state::PackedBitset {
                segment: segment_name.to_string(),
                bitset: Some(
                    confidence_resolver::proto::confidence::flags::admin::v1::resolver_state::packed_bitset::Bitset::FullBitset(true),
                ),
            },
        ],
        clients: vec![Client {
            name: TEST_CLIENT_NAME.to_string(),
            ..Default::default()
        }],
        client_credentials: vec![ClientCredential {
            name: TEST_CREDENTIAL_NAME.to_string(),
            credential: Some(client_credential::Credential::ClientSecret(
                client_credential::ClientSecret {
                    secret: TEST_CLIENT_SECRET.to_string(),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };

    let state_bytes = state_pb.encode_to_vec();
    let state = ResolverStateRuntime::from_proto(
        ResolverState::decode(Bytes::from(state_bytes)).unwrap(),
        TEST_ACCOUNT_ID,
    )
    .expect("Failed to create state with flag");

    (state, TEST_ACCOUNT_ID.to_string())
}

/// Create a resolver state with a flag that requires materializations (sticky flag).
///
/// Similar to Go's `CreateStateWithStickyFlag`.
pub fn create_state_with_sticky_flag() -> (ResolverStateRuntime, String) {
    let segment_name = "segments/always-true";
    let flag_name = "flags/sticky-test-flag";
    let variant_on = format!("{}/variants/on", flag_name);
    let variant_off = format!("{}/variants/off", flag_name);

    let mut on_value_fields = HashMap::new();
    on_value_fields.insert(
        "enabled".to_string(),
        ProtoValue {
            kind: Some(value::Kind::BoolValue(true)),
        },
    );

    let mut off_value_fields = HashMap::new();
    off_value_fields.insert(
        "enabled".to_string(),
        ProtoValue {
            kind: Some(value::Kind::BoolValue(false)),
        },
    );

    let state_pb = ResolverState {
        flags: vec![Flag {
            name: flag_name.to_string(),
            state: flag::State::Active as i32,
            clients: vec![TEST_CLIENT_NAME.to_string()],
            variants: vec![
                flag::Variant {
                    name: variant_on.clone(),
                    value: Some(Struct {
                        fields: on_value_fields,
                    }),
                    ..Default::default()
                },
                flag::Variant {
                    name: variant_off.clone(),
                    value: Some(Struct {
                        fields: off_value_fields,
                    }),
                    ..Default::default()
                },
            ],
            rules: vec![flag::Rule {
                name: format!("{}/rules/sticky-rule", flag_name),
                segment: segment_name.to_string(),
                targeting_key_selector: "user_id".to_string(),
                enabled: true,
                assignment_spec: Some(flag::rule::AssignmentSpec {
                    bucket_count: 2,
                    assignments: vec![flag::rule::Assignment {
                        assignment_id: "on".to_string(),
                        assignment: Some(flag::rule::assignment::Assignment::Variant(
                            flag::rule::assignment::VariantAssignment {
                                variant: variant_on.clone(),
                            },
                        )),
                        bucket_ranges: vec![flag::rule::BucketRange { lower: 0, upper: 2 }],
                    }],
                }),
                // This rule requires a materialization
                materialization_spec: Some(flag::rule::MaterializationSpec {
                    read_materialization: "experiment_v1".to_string(),
                    write_materialization: "experiment_v1".to_string(),
                    mode: Some(flag::rule::materialization_spec::MaterializationReadMode {
                        materialization_must_match: false,
                        segment_targeting_can_be_ignored: false,
                    }),
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        segments_no_bitsets: vec![Segment {
            name: segment_name.to_string(),
            ..Default::default()
        }],
        bitsets: vec![
            confidence_resolver::proto::confidence::flags::admin::v1::resolver_state::PackedBitset {
                segment: segment_name.to_string(),
                bitset: Some(
                    confidence_resolver::proto::confidence::flags::admin::v1::resolver_state::packed_bitset::Bitset::FullBitset(true),
                ),
            },
        ],
        clients: vec![Client {
            name: TEST_CLIENT_NAME.to_string(),
            ..Default::default()
        }],
        client_credentials: vec![ClientCredential {
            name: TEST_CREDENTIAL_NAME.to_string(),
            credential: Some(client_credential::Credential::ClientSecret(
                client_credential::ClientSecret {
                    secret: TEST_CLIENT_SECRET.to_string(),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };

    let state_bytes = state_pb.encode_to_vec();
    let state = ResolverStateRuntime::from_proto(
        ResolverState::decode(Bytes::from(state_bytes)).unwrap(),
        TEST_ACCOUNT_ID,
    )
    .expect("Failed to create state with sticky flag");

    (state, TEST_ACCOUNT_ID.to_string())
}

/// A mock materialization store that always returns an error (unsupported).
///
/// Similar to Java's `UnsupportedMaterializationStore`.
pub struct UnsupportedMaterializationStore;

#[async_trait]
impl MaterializationStore for UnsupportedMaterializationStore {
    async fn read_materializations(&self, _ops: Vec<ReadOpType>) -> Result<Vec<ReadResultType>> {
        Err(crate::error::Error::Materialization(
            "materialization read not supported".to_string(),
        ))
    }

    async fn write_materializations(&self, _ops: Vec<WriteOp>) -> Result<()> {
        Err(crate::error::Error::Materialization(
            "materialization write not supported".to_string(),
        ))
    }
}

/// A mock materialization store with configurable behavior.
///
/// Similar to Go's mock materialization store.
pub struct MockMaterializationStore {
    /// Results to return for read operations.
    pub read_results: Arc<tokio::sync::Mutex<Vec<ReadResultType>>>,
    /// Whether write operations should succeed.
    pub write_succeeds: bool,
}

impl MockMaterializationStore {
    /// Create a new mock store that returns empty results.
    pub fn new() -> Self {
        Self {
            read_results: Arc::new(tokio::sync::Mutex::new(vec![])),
            write_succeeds: true,
        }
    }

    /// Create a mock store with specific read results.
    pub fn with_read_results(results: Vec<ReadResultType>) -> Self {
        Self {
            read_results: Arc::new(tokio::sync::Mutex::new(results)),
            write_succeeds: true,
        }
    }
}

impl Default for MockMaterializationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MaterializationStore for MockMaterializationStore {
    async fn read_materializations(&self, _ops: Vec<ReadOpType>) -> Result<Vec<ReadResultType>> {
        let results = self.read_results.lock().await;
        Ok(results.clone())
    }

    async fn write_materializations(&self, _ops: Vec<WriteOp>) -> Result<()> {
        if self.write_succeeds {
            Ok(())
        } else {
            Err(crate::error::Error::Materialization(
                "mock write failed".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_minimal_state() {
        let (state, account_id) = create_minimal_state();
        assert_eq!(account_id, TEST_ACCOUNT_ID);
        // State should be created successfully
        assert!(state.flags.is_empty());
    }

    #[test]
    fn test_create_state_with_flag() {
        let (state, account_id) = create_state_with_flag();
        assert_eq!(account_id, TEST_ACCOUNT_ID);
        // State should contain one flag
        assert_eq!(state.flags.len(), 1);
    }

    #[test]
    fn test_create_state_with_sticky_flag() {
        let (state, account_id) = create_state_with_sticky_flag();
        assert_eq!(account_id, TEST_ACCOUNT_ID);
        // State should contain one sticky flag
        assert_eq!(state.flags.len(), 1);
    }

    #[tokio::test]
    async fn test_unsupported_materialization_store() {
        let store = UnsupportedMaterializationStore;
        let result = store.read_materializations(vec![]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_materialization_store() {
        let store = MockMaterializationStore::new();
        let result = store.read_materializations(vec![]).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
