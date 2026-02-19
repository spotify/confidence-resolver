use std::time::Duration;

use async_trait::async_trait;
use prost::Message;

use confidence_resolver::proto::confidence::flags::resolver::v1::MaterializationRecord;
use reqwest_middleware::ClientWithMiddleware;

use crate::error::{Error, Result};
use crate::remote_proto;

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

// ---------------------------------------------------------------------------
// Conversions between MaterializationRecord (wasm API) and store types
// ---------------------------------------------------------------------------

/// Convert MaterializationRecords (from a Suspended response) to ReadOps for the store.
pub fn materialization_records_to_read_ops(records: &[MaterializationRecord]) -> Vec<ReadOpType> {
    records
        .iter()
        .map(|r| {
            if !r.rule.is_empty() {
                ReadOpType::Variant {
                    unit: r.unit.clone(),
                    materialization: r.materialization.clone(),
                    rule: r.rule.clone(),
                }
            } else {
                ReadOpType::Inclusion {
                    unit: r.unit.clone(),
                    materialization: r.materialization.clone(),
                }
            }
        })
        .collect()
}

/// Convert store ReadResults back to MaterializationRecords for a Resume request.
/// "Not included" inclusion results are omitted (absence = not included).
pub fn read_results_to_materialization_records(
    results: Vec<ReadResultType>,
) -> Vec<MaterializationRecord> {
    results
        .into_iter()
        .filter_map(|r| match r {
            ReadResultType::Variant {
                unit,
                materialization,
                rule,
                variant,
            } => variant.map(|v| MaterializationRecord {
                unit,
                materialization,
                rule,
                variant: v,
            }),
            ReadResultType::Inclusion {
                unit,
                materialization,
                included,
            } => {
                if included {
                    Some(MaterializationRecord {
                        unit,
                        materialization,
                        ..Default::default()
                    })
                } else {
                    None // absence = not included
                }
            }
        })
        .collect()
}

/// Convert MaterializationRecords (from a Resolved response) to WriteOps for the store.
pub fn materialization_records_to_write_ops(records: &[MaterializationRecord]) -> Vec<WriteOp> {
    records
        .iter()
        .map(|r| WriteOp {
            unit: r.unit.clone(),
            materialization: r.materialization.clone(),
            rule: r.rule.clone(),
            variant: r.variant.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Remote materialization store (uses generated proto types from internal_api.proto)
// ---------------------------------------------------------------------------

const MATERIALIZATION_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Remote materialization store that calls the Confidence API.
pub(crate) struct ConfidenceRemoteMaterializationStore {
    client: ClientWithMiddleware,
    client_secret: String,
}

impl ConfidenceRemoteMaterializationStore {
    pub(crate) fn new(client: ClientWithMiddleware, client_secret: String) -> Result<Self> {
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
        let request = read_ops_to_remote_proto(read_ops);
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
            .timeout(MATERIALIZATION_READ_TIMEOUT)
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

        let result = remote_proto::ReadOperationsResult::decode(bytes)
            .map_err(|e| Error::Proto(e.to_string()))?;

        Ok(read_results_from_remote_proto(result))
    }

    async fn write_materializations(&self, write_ops: Vec<WriteOp>) -> Result<()> {
        if write_ops.is_empty() {
            return Ok(());
        }

        let request = write_ops_to_remote_proto(write_ops);
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

// Conversions between store types and remote API proto types

fn read_ops_to_remote_proto(read_ops: Vec<ReadOpType>) -> remote_proto::ReadOperationsRequest {
    remote_proto::ReadOperationsRequest {
        ops: read_ops
            .into_iter()
            .map(|op| match op {
                ReadOpType::Variant {
                    unit,
                    materialization,
                    rule,
                } => remote_proto::ReadOp {
                    op: Some(remote_proto::read_op::Op::VariantReadOp(
                        remote_proto::VariantReadOp {
                            unit,
                            materialization,
                            rule,
                        },
                    )),
                },
                ReadOpType::Inclusion {
                    unit,
                    materialization,
                } => remote_proto::ReadOp {
                    op: Some(remote_proto::read_op::Op::InclusionReadOp(
                        remote_proto::InclusionReadOp {
                            unit,
                            materialization,
                        },
                    )),
                },
            })
            .collect(),
    }
}

fn read_results_from_remote_proto(
    result: remote_proto::ReadOperationsResult,
) -> Vec<ReadResultType> {
    result
        .results
        .into_iter()
        .filter_map(|r| match r.result {
            Some(remote_proto::read_result::Result::VariantResult(v)) => {
                Some(ReadResultType::Variant {
                    unit: v.unit,
                    materialization: v.materialization,
                    rule: v.rule,
                    variant: if v.variant.is_empty() {
                        None
                    } else {
                        Some(v.variant)
                    },
                })
            }
            Some(remote_proto::read_result::Result::InclusionResult(i)) => {
                Some(ReadResultType::Inclusion {
                    unit: i.unit,
                    materialization: i.materialization,
                    included: i.is_included,
                })
            }
            None => None,
        })
        .collect()
}

fn write_ops_to_remote_proto(write_ops: Vec<WriteOp>) -> remote_proto::WriteOperationsRequest {
    remote_proto::WriteOperationsRequest {
        store_variant_op: write_ops
            .into_iter()
            .map(|op| remote_proto::VariantData {
                unit: op.unit,
                materialization: op.materialization,
                rule: op.rule,
                variant: op.variant,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Client;
    use reqwest_middleware::ClientBuilder;

    use super::*;

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

    #[test]
    fn test_materialization_records_to_read_ops() {
        let records = vec![
            MaterializationRecord {
                unit: "user-1".to_string(),
                materialization: "mat-1".to_string(),
                rule: "rule-1".to_string(),
                variant: "".to_string(),
            },
            MaterializationRecord {
                unit: "user-2".to_string(),
                materialization: "mat-2".to_string(),
                rule: "".to_string(),
                variant: "".to_string(),
            },
        ];

        let ops = materialization_records_to_read_ops(&records);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], ReadOpType::Variant { rule, .. } if rule == "rule-1"));
        assert!(matches!(&ops[1], ReadOpType::Inclusion { .. }));
    }

    #[test]
    fn test_read_results_to_materialization_records() {
        let results = vec![
            ReadResultType::Variant {
                unit: "user-1".to_string(),
                materialization: "mat-1".to_string(),
                rule: "rule-1".to_string(),
                variant: Some("v-a".to_string()),
            },
            ReadResultType::Inclusion {
                unit: "user-2".to_string(),
                materialization: "mat-2".to_string(),
                included: true,
            },
            ReadResultType::Inclusion {
                unit: "user-3".to_string(),
                materialization: "mat-3".to_string(),
                included: false, // should be omitted
            },
        ];

        let records = read_results_to_materialization_records(results);
        assert_eq!(records.len(), 2); // "not included" is omitted
        assert_eq!(records[0].variant, "v-a");
        assert_eq!(records[1].unit, "user-2");
    }

    #[test]
    fn test_materialization_records_to_write_ops() {
        let records = vec![MaterializationRecord {
            unit: "user-1".to_string(),
            materialization: "mat-1".to_string(),
            rule: "rule-1".to_string(),
            variant: "variant-a".to_string(),
        }];

        let ops = materialization_records_to_write_ops(&records);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].unit, "user-1");
        assert_eq!(ops[0].variant, "variant-a");
    }

    #[test]
    fn test_read_ops_to_remote_proto() {
        let ops = vec![
            ReadOpType::Variant {
                unit: "user-1".to_string(),
                materialization: "mat-1".to_string(),
                rule: "rule-1".to_string(),
            },
            ReadOpType::Inclusion {
                unit: "user-2".to_string(),
                materialization: "mat-2".to_string(),
            },
        ];

        let proto = super::read_ops_to_remote_proto(ops);
        assert_eq!(proto.ops.len(), 2);
        assert!(matches!(
            &proto.ops[0].op,
            Some(remote_proto::read_op::Op::VariantReadOp(_))
        ));
        assert!(matches!(
            &proto.ops[1].op,
            Some(remote_proto::read_op::Op::InclusionReadOp(_))
        ));
    }

    #[test]
    fn test_confidence_remote_store_creation() {
        let store = ConfidenceRemoteMaterializationStore::new(
            ClientBuilder::new(Client::new()).build(),
            "test-secret".to_string(),
        );
        assert!(store.is_ok());
    }
}
