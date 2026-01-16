use async_trait::async_trait;
use prost::Message;

use confidence_resolver::proto::confidence::flags::resolver::v1::{
    read_op, read_result, InclusionData, InclusionReadOp, ReadOp, ReadOperationsRequest,
    ReadResult, VariantData, VariantReadOp,
};

use crate::error::{Error, Result};

/// Read operation for materialization store.
#[derive(Debug, Clone)]
pub enum ReadOpType {
    Variant {
        unit: String,
        materialization: String,
        rule: String,
    },
    Inclusion {
        unit: String,
        materialization: String,
    },
}

/// Read result from materialization store.
#[derive(Debug, Clone)]
pub enum ReadResultType {
    Variant {
        unit: String,
        materialization: String,
        rule: String,
        variant: Option<String>,
    },
    Inclusion {
        unit: String,
        materialization: String,
        included: bool,
    },
}

/// Write operation for materialization store.
#[derive(Debug, Clone)]
pub struct WriteOp {
    pub unit: String,
    pub materialization: String,
    pub rule: String,
    pub variant: String,
}

/// Interface for storing and retrieving materialization data.
///
/// Implementations can use any storage backend (e.g., Redis, DynamoDB, or the
/// Confidence remote materialization store).
#[async_trait]
pub trait MaterializationStore: Send + Sync {
    /// Reads materialization data for the given operations.
    async fn read_materializations(&self, read_ops: Vec<ReadOpType>)
        -> Result<Vec<ReadResultType>>;

    /// Persists variant assignments for sticky bucketing.
    /// This method is optional - implementations may return Ok(()) if write is not supported.
    async fn write_materializations(&self, write_ops: Vec<WriteOp>) -> Result<()>;
}

/// Remote materialization store that calls the Confidence API.
pub struct ConfidenceRemoteMaterializationStore {
    client: reqwest::Client,
    client_secret: String,
}

impl ConfidenceRemoteMaterializationStore {
    pub fn new(client_secret: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;

        Ok(Self {
            client,
            client_secret,
        })
    }
}

#[async_trait]
impl MaterializationStore for ConfidenceRemoteMaterializationStore {
    async fn read_materializations(
        &self,
        read_ops: Vec<ReadOpType>,
    ) -> Result<Vec<ReadResultType>> {
        let request = read_ops_to_proto(read_ops);
        let body = request.encode_to_vec();

        let response = self
            .client
            .post("https://resolver.confidence.dev/v1/materialization:readMaterializedOperations")
            .header("Content-Type", "application/x-protobuf")
            .header(
                "Authorization",
                format!("ClientSecret {}", self.client_secret),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Http(format!(
                "Failed to read materializations: {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        let result =
            ReadOperationsResult::decode(bytes).map_err(|e| Error::Proto(e.to_string()))?;

        Ok(read_results_from_proto(result))
    }

    async fn write_materializations(&self, write_ops: Vec<WriteOp>) -> Result<()> {
        if write_ops.is_empty() {
            return Ok(());
        }

        let request = write_ops_to_proto(write_ops);
        let body = request.encode_to_vec();

        let response = self
            .client
            .post("https://resolver.confidence.dev/v1/materialization:writeMaterializedOperations")
            .header("Content-Type", "application/x-protobuf")
            .header(
                "Authorization",
                format!("ClientSecret {}", self.client_secret),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Http(format!(
                "Failed to write materializations: {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        Ok(())
    }
}

/// Proto message for ReadOperationsResult (matches wasm_api.proto).
#[derive(Clone, PartialEq, Message)]
pub struct ReadOperationsResult {
    #[prost(message, repeated, tag = "1")]
    pub results: Vec<ReadResult>,
}

/// Proto message for WriteOperationsRequest (matches internal_api.proto).
#[derive(Clone, PartialEq, Message)]
pub struct WriteOperationsRequest {
    #[prost(message, repeated, tag = "1")]
    pub store_variant_op: Vec<VariantData>,
}

fn read_ops_to_proto(read_ops: Vec<ReadOpType>) -> ReadOperationsRequest {
    ReadOperationsRequest {
        ops: read_ops
            .into_iter()
            .map(|op| match op {
                ReadOpType::Variant {
                    unit,
                    materialization,
                    rule,
                } => ReadOp {
                    op: Some(read_op::Op::VariantReadOp(VariantReadOp {
                        unit,
                        materialization,
                        rule,
                    })),
                },
                ReadOpType::Inclusion {
                    unit,
                    materialization,
                } => ReadOp {
                    op: Some(read_op::Op::InclusionReadOp(InclusionReadOp {
                        unit,
                        materialization,
                    })),
                },
            })
            .collect(),
    }
}

fn read_results_from_proto(result: ReadOperationsResult) -> Vec<ReadResultType> {
    result
        .results
        .into_iter()
        .filter_map(|r| match r.result {
            Some(read_result::Result::VariantResult(v)) => Some(ReadResultType::Variant {
                unit: v.unit,
                materialization: v.materialization,
                rule: v.rule,
                variant: if v.variant.is_empty() {
                    None
                } else {
                    Some(v.variant)
                },
            }),
            Some(read_result::Result::InclusionResult(i)) => Some(ReadResultType::Inclusion {
                unit: i.unit,
                materialization: i.materialization,
                included: i.is_included,
            }),
            None => None,
        })
        .collect()
}

fn write_ops_to_proto(write_ops: Vec<WriteOp>) -> WriteOperationsRequest {
    WriteOperationsRequest {
        store_variant_op: write_ops
            .into_iter()
            .map(|op| VariantData {
                unit: op.unit,
                materialization: op.materialization,
                rule: op.rule,
                variant: op.variant,
            })
            .collect(),
    }
}

/// Convert proto ReadOperationsRequest to our ReadOpType vec.
pub fn read_ops_from_proto(request: &ReadOperationsRequest) -> Vec<ReadOpType> {
    request
        .ops
        .iter()
        .filter_map(|op| match &op.op {
            Some(read_op::Op::VariantReadOp(v)) => Some(ReadOpType::Variant {
                unit: v.unit.clone(),
                materialization: v.materialization.clone(),
                rule: v.rule.clone(),
            }),
            Some(read_op::Op::InclusionReadOp(i)) => Some(ReadOpType::Inclusion {
                unit: i.unit.clone(),
                materialization: i.materialization.clone(),
            }),
            None => None,
        })
        .collect()
}

/// Convert our ReadResultType vec to proto ReadResult vec (for passing to resolver).
pub fn read_results_to_proto(results: Vec<ReadResultType>) -> Vec<ReadResult> {
    results
        .into_iter()
        .map(|r| match r {
            ReadResultType::Variant {
                unit,
                materialization,
                rule,
                variant,
            } => ReadResult {
                result: Some(read_result::Result::VariantResult(VariantData {
                    unit,
                    materialization,
                    rule,
                    variant: variant.unwrap_or_default(),
                })),
            },
            ReadResultType::Inclusion {
                unit,
                materialization,
                included,
            } => ReadResult {
                result: Some(read_result::Result::InclusionResult(InclusionData {
                    unit,
                    materialization,
                    is_included: included,
                })),
            },
        })
        .collect()
}

/// Convert proto VariantData vec to WriteOp vec.
pub fn write_ops_from_proto(variant_data: &[VariantData]) -> Vec<WriteOp> {
    variant_data
        .iter()
        .map(|v| WriteOp {
            unit: v.unit.clone(),
            materialization: v.materialization.clone(),
            rule: v.rule.clone(),
            variant: v.variant.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== ReadOpType tests ====================

    #[test]
    fn test_read_op_type_variant() {
        let op = ReadOpType::Variant {
            unit: "user-123".to_string(),
            materialization: "experiment_v1".to_string(),
            rule: "my-rule".to_string(),
        };

        if let ReadOpType::Variant {
            unit,
            materialization,
            rule,
        } = op
        {
            assert_eq!(unit, "user-123");
            assert_eq!(materialization, "experiment_v1");
            assert_eq!(rule, "my-rule");
        } else {
            panic!("Expected Variant");
        }
    }

    #[test]
    fn test_read_op_type_inclusion() {
        let op = ReadOpType::Inclusion {
            unit: "user-123".to_string(),
            materialization: "segment_v1".to_string(),
        };

        if let ReadOpType::Inclusion {
            unit,
            materialization,
        } = op
        {
            assert_eq!(unit, "user-123");
            assert_eq!(materialization, "segment_v1");
        } else {
            panic!("Expected Inclusion");
        }
    }

    // ==================== ReadResultType tests ====================

    #[test]
    fn test_read_result_type_variant_with_value() {
        let result = ReadResultType::Variant {
            unit: "user-123".to_string(),
            materialization: "experiment_v1".to_string(),
            rule: "my-rule".to_string(),
            variant: Some("variant-a".to_string()),
        };

        if let ReadResultType::Variant {
            unit,
            materialization,
            rule,
            variant,
        } = result
        {
            assert_eq!(unit, "user-123");
            assert_eq!(materialization, "experiment_v1");
            assert_eq!(rule, "my-rule");
            assert_eq!(variant, Some("variant-a".to_string()));
        } else {
            panic!("Expected Variant");
        }
    }

    #[test]
    fn test_read_result_type_variant_without_value() {
        let result = ReadResultType::Variant {
            unit: "user-123".to_string(),
            materialization: "experiment_v1".to_string(),
            rule: "my-rule".to_string(),
            variant: None,
        };

        if let ReadResultType::Variant { variant, .. } = result {
            assert_eq!(variant, None);
        } else {
            panic!("Expected Variant");
        }
    }

    #[test]
    fn test_read_result_type_inclusion() {
        let result = ReadResultType::Inclusion {
            unit: "user-123".to_string(),
            materialization: "segment_v1".to_string(),
            included: true,
        };

        if let ReadResultType::Inclusion {
            unit,
            materialization,
            included,
        } = result
        {
            assert_eq!(unit, "user-123");
            assert_eq!(materialization, "segment_v1");
            assert!(included);
        } else {
            panic!("Expected Inclusion");
        }
    }

    // ==================== WriteOp tests ====================

    #[test]
    fn test_write_op() {
        let op = WriteOp {
            unit: "user-123".to_string(),
            materialization: "experiment_v1".to_string(),
            rule: "my-rule".to_string(),
            variant: "variant-a".to_string(),
        };

        assert_eq!(op.unit, "user-123");
        assert_eq!(op.materialization, "experiment_v1");
        assert_eq!(op.rule, "my-rule");
        assert_eq!(op.variant, "variant-a");
    }

    // ==================== Conversion function tests ====================

    #[test]
    fn test_read_ops_to_proto_variant() {
        let ops = vec![ReadOpType::Variant {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            rule: "rule-1".to_string(),
        }];

        let proto = read_ops_to_proto(ops);

        assert_eq!(proto.ops.len(), 1);
        if let Some(read_op::Op::VariantReadOp(v)) = &proto.ops[0].op {
            assert_eq!(v.unit, "user-1");
            assert_eq!(v.materialization, "mat-1");
            assert_eq!(v.rule, "rule-1");
        } else {
            panic!("Expected VariantReadOp");
        }
    }

    #[test]
    fn test_read_ops_to_proto_inclusion() {
        let ops = vec![ReadOpType::Inclusion {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
        }];

        let proto = read_ops_to_proto(ops);

        assert_eq!(proto.ops.len(), 1);
        if let Some(read_op::Op::InclusionReadOp(i)) = &proto.ops[0].op {
            assert_eq!(i.unit, "user-1");
            assert_eq!(i.materialization, "mat-1");
        } else {
            panic!("Expected InclusionReadOp");
        }
    }

    #[test]
    fn test_read_ops_from_proto() {
        let proto = ReadOperationsRequest {
            ops: vec![
                ReadOp {
                    op: Some(read_op::Op::VariantReadOp(VariantReadOp {
                        unit: "user-1".to_string(),
                        materialization: "mat-1".to_string(),
                        rule: "rule-1".to_string(),
                    })),
                },
                ReadOp {
                    op: Some(read_op::Op::InclusionReadOp(InclusionReadOp {
                        unit: "user-2".to_string(),
                        materialization: "mat-2".to_string(),
                    })),
                },
            ],
        };

        let ops = read_ops_from_proto(&proto);

        assert_eq!(ops.len(), 2);

        if let ReadOpType::Variant {
            unit,
            materialization,
            rule,
        } = &ops[0]
        {
            assert_eq!(unit, "user-1");
            assert_eq!(materialization, "mat-1");
            assert_eq!(rule, "rule-1");
        } else {
            panic!("Expected Variant");
        }

        if let ReadOpType::Inclusion {
            unit,
            materialization,
        } = &ops[1]
        {
            assert_eq!(unit, "user-2");
            assert_eq!(materialization, "mat-2");
        } else {
            panic!("Expected Inclusion");
        }
    }

    #[test]
    fn test_read_results_to_proto_variant() {
        let results = vec![ReadResultType::Variant {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            rule: "rule-1".to_string(),
            variant: Some("variant-a".to_string()),
        }];

        let proto = read_results_to_proto(results);

        assert_eq!(proto.len(), 1);
        if let Some(read_result::Result::VariantResult(v)) = &proto[0].result {
            assert_eq!(v.unit, "user-1");
            assert_eq!(v.materialization, "mat-1");
            assert_eq!(v.rule, "rule-1");
            assert_eq!(v.variant, "variant-a");
        } else {
            panic!("Expected VariantResult");
        }
    }

    #[test]
    fn test_read_results_to_proto_variant_empty() {
        let results = vec![ReadResultType::Variant {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            rule: "rule-1".to_string(),
            variant: None,
        }];

        let proto = read_results_to_proto(results);

        if let Some(read_result::Result::VariantResult(v)) = &proto[0].result {
            assert_eq!(v.variant, ""); // None becomes empty string
        } else {
            panic!("Expected VariantResult");
        }
    }

    #[test]
    fn test_read_results_to_proto_inclusion() {
        let results = vec![ReadResultType::Inclusion {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            included: true,
        }];

        let proto = read_results_to_proto(results);

        assert_eq!(proto.len(), 1);
        if let Some(read_result::Result::InclusionResult(i)) = &proto[0].result {
            assert_eq!(i.unit, "user-1");
            assert_eq!(i.materialization, "mat-1");
            assert!(i.is_included);
        } else {
            panic!("Expected InclusionResult");
        }
    }

    #[test]
    fn test_write_ops_from_proto() {
        let variant_data = vec![
            VariantData {
                unit: "user-1".to_string(),
                materialization: "mat-1".to_string(),
                rule: "rule-1".to_string(),
                variant: "variant-a".to_string(),
            },
            VariantData {
                unit: "user-2".to_string(),
                materialization: "mat-2".to_string(),
                rule: "rule-2".to_string(),
                variant: "variant-b".to_string(),
            },
        ];

        let ops = write_ops_from_proto(&variant_data);

        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].unit, "user-1");
        assert_eq!(ops[0].variant, "variant-a");
        assert_eq!(ops[1].unit, "user-2");
        assert_eq!(ops[1].variant, "variant-b");
    }

    #[test]
    fn test_write_ops_to_proto() {
        let ops = vec![WriteOp {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            rule: "rule-1".to_string(),
            variant: "variant-a".to_string(),
        }];

        let proto = write_ops_to_proto(ops);

        assert_eq!(proto.store_variant_op.len(), 1);
        assert_eq!(proto.store_variant_op[0].unit, "user-1");
        assert_eq!(proto.store_variant_op[0].materialization, "mat-1");
        assert_eq!(proto.store_variant_op[0].rule, "rule-1");
        assert_eq!(proto.store_variant_op[0].variant, "variant-a");
    }

    #[test]
    fn test_read_results_from_proto() {
        let proto_result = ReadOperationsResult {
            results: vec![
                ReadResult {
                    result: Some(read_result::Result::VariantResult(VariantData {
                        unit: "user-1".to_string(),
                        materialization: "mat-1".to_string(),
                        rule: "rule-1".to_string(),
                        variant: "variant-a".to_string(),
                    })),
                },
                ReadResult {
                    result: Some(read_result::Result::InclusionResult(InclusionData {
                        unit: "user-2".to_string(),
                        materialization: "mat-2".to_string(),
                        is_included: false,
                    })),
                },
            ],
        };

        let results = read_results_from_proto(proto_result);

        assert_eq!(results.len(), 2);

        if let ReadResultType::Variant { unit, variant, .. } = &results[0] {
            assert_eq!(unit, "user-1");
            assert_eq!(variant.as_deref(), Some("variant-a"));
        } else {
            panic!("Expected Variant");
        }

        if let ReadResultType::Inclusion { unit, included, .. } = &results[1] {
            assert_eq!(unit, "user-2");
            assert!(!included);
        } else {
            panic!("Expected Inclusion");
        }
    }

    #[test]
    fn test_read_results_from_proto_empty_variant() {
        let proto_result = ReadOperationsResult {
            results: vec![ReadResult {
                result: Some(read_result::Result::VariantResult(VariantData {
                    unit: "user-1".to_string(),
                    materialization: "mat-1".to_string(),
                    rule: "rule-1".to_string(),
                    variant: "".to_string(), // Empty string
                })),
            }],
        };

        let results = read_results_from_proto(proto_result);

        if let ReadResultType::Variant { variant, .. } = &results[0] {
            assert_eq!(*variant, None); // Empty string becomes None
        } else {
            panic!("Expected Variant");
        }
    }

    #[test]
    fn test_read_results_from_proto_skips_none() {
        let proto_result = ReadOperationsResult {
            results: vec![
                ReadResult { result: None },
                ReadResult {
                    result: Some(read_result::Result::VariantResult(VariantData {
                        unit: "user-1".to_string(),
                        materialization: "mat-1".to_string(),
                        rule: "rule-1".to_string(),
                        variant: "variant-a".to_string(),
                    })),
                },
            ],
        };

        let results = read_results_from_proto(proto_result);

        // Only one result (the None is skipped)
        assert_eq!(results.len(), 1);
    }

    // ==================== ConfidenceRemoteMaterializationStore tests ====================

    #[test]
    fn test_confidence_remote_store_creation() {
        let store = ConfidenceRemoteMaterializationStore::new("test-secret".to_string());
        assert!(store.is_ok());
    }
}
