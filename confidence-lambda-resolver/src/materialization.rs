use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DynamoClient;
use confidence_resolver::proto::confidence::flags::resolver::v1::MaterializationRecord;

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

#[derive(Debug, Clone)]
pub struct WriteOp {
    pub unit: String,
    pub materialization: String,
    pub rule: String,
    pub variant: String,
}

#[async_trait]
pub trait MaterializationStore: Send + Sync {
    async fn read_materializations(
        &self,
        read_ops: Vec<ReadOpType>,
    ) -> Result<Vec<ReadResultType>, String>;
    async fn write_materializations(&self, write_ops: Vec<WriteOp>) -> Result<(), String>;
}

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
                    None
                }
            }
        })
        .collect()
}

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

fn sort_key(materialization: &str, rule: &str) -> String {
    if rule.is_empty() {
        materialization.to_string()
    } else {
        format!("{}#{}", materialization, rule)
    }
}

pub struct DynamoDbMaterializationStore {
    client: DynamoClient,
    table_name: String,
}

impl DynamoDbMaterializationStore {
    pub fn new(client: DynamoClient, table_name: String) -> Self {
        Self { client, table_name }
    }
}

#[async_trait]
impl MaterializationStore for DynamoDbMaterializationStore {
    async fn read_materializations(
        &self,
        read_ops: Vec<ReadOpType>,
    ) -> Result<Vec<ReadResultType>, String> {
        let mut results = Vec::with_capacity(read_ops.len());

        for op in &read_ops {
            let (unit, sk) = match op {
                ReadOpType::Variant {
                    unit,
                    materialization,
                    rule,
                } => (unit.clone(), sort_key(materialization, rule)),
                ReadOpType::Inclusion {
                    unit,
                    materialization,
                } => (unit.clone(), sort_key(materialization, "")),
            };

            let output = self
                .client
                .get_item()
                .table_name(&self.table_name)
                .key("pk", AttributeValue::S(unit.clone()))
                .key("sk", AttributeValue::S(sk))
                .consistent_read(true)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            match op {
                ReadOpType::Variant {
                    unit,
                    materialization,
                    rule,
                } => {
                    let variant = output
                        .item()
                        .and_then(|item| item.get("variant"))
                        .and_then(|v| v.as_s().ok())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    results.push(ReadResultType::Variant {
                        unit: unit.clone(),
                        materialization: materialization.clone(),
                        rule: rule.clone(),
                        variant,
                    });
                }
                ReadOpType::Inclusion {
                    unit,
                    materialization,
                } => {
                    let included = output
                        .item()
                        .and_then(|item| item.get("included"))
                        .and_then(|v| v.as_bool().ok())
                        .copied()
                        .unwrap_or(false);
                    results.push(ReadResultType::Inclusion {
                        unit: unit.clone(),
                        materialization: materialization.clone(),
                        included,
                    });
                }
            }
        }

        Ok(results)
    }

    async fn write_materializations(&self, write_ops: Vec<WriteOp>) -> Result<(), String> {
        for op in write_ops {
            let sk = sort_key(&op.materialization, &op.rule);
            self.client
                .put_item()
                .table_name(&self.table_name)
                .item("pk", AttributeValue::S(op.unit))
                .item("sk", AttributeValue::S(sk))
                .item("variant", AttributeValue::S(op.variant))
                .item("materialization", AttributeValue::S(op.materialization))
                .item("rule", AttributeValue::S(op.rule))
                .send()
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
