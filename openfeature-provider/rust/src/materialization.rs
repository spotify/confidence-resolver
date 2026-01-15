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
