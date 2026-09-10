//! Bounded, low-cardinality sampling of explicitly non-PII fields.
use crate::api;
use confidence_resolver::proto::{
    confidence::flags::admin::v1 as admin,
    google::{value::Kind, Struct, Value},
};
use prost::Message;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

pub type Samples = BTreeMap<String, BTreeMap<String, SeenValues>>;
const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, PartialEq, Message)]
pub struct FieldOverride {
    #[prost(string, tag = "8")]
    pub field: String,
    #[prost(string, repeated, tag = "9")]
    pub clients: Vec<String>,
    #[prost(bool, tag = "15")]
    pub non_pii: bool,
}
#[derive(Clone, PartialEq, Message)]
pub struct StateFields {
    #[prost(message, repeated, tag = "9")]
    pub fields: Vec<FieldOverride>,
}
struct FieldSample {
    since: Instant,
    units: HashSet<String>,
    values: BTreeMap<Vec<u8>, (Value, usize)>,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    fields: HashMap<(String, String), FieldSample>,
    bytes: usize,
}
pub struct Sampler {
    fields: Vec<FieldOverride>,
    cache: Mutex<Cache>,
}
impl Sampler {
    pub fn new(fields: Vec<FieldOverride>) -> Self {
        Self {
            fields: fields
                .into_iter()
                .filter(|field| field.non_pii)
                .take(100)
                .collect(),
            cache: Mutex::new(Cache::default()),
        }
    }
    pub fn observe(&self, credential: &str, client: &str, unit: &str, context: &Struct) {
        if self.fields.is_empty() || unit.is_empty() || unit.len() > 100 {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        for field in &self.fields {
            if !field.clients.is_empty() && !field.clients.iter().any(|c| c == client) {
                continue;
            }
            let Some(value) = extract(context, &field.field) else {
                continue;
            };
            let primitives = match &value.kind {
                Some(Kind::ListValue(list)) => list.values.iter().collect::<Vec<_>>(),
                _ => vec![value],
            };
            let values: BTreeMap<_, _> = primitives
                .into_iter()
                .filter(|v| {
                    matches!(
                        v.kind,
                        Some(Kind::StringValue(_) | Kind::NumberValue(_) | Kind::BoolValue(_))
                    )
                })
                .filter(|v| v.encoded_len() <= 1024)
                .take(100)
                .map(|v| (v.encode_to_vec(), v.clone()))
                .collect();
            if values.is_empty() {
                continue;
            }
            let key = (credential.to_string(), field.field.clone());
            if cache
                .fields
                .get(&key)
                .is_some_and(|entry| entry.since.elapsed() >= Duration::from_secs(3600))
            {
                let old = cache.fields.remove(&key).expect("entry exists");
                cache.bytes -= old.bytes;
            }
            if cache
                .fields
                .get(&key)
                .is_some_and(|entry| entry.units.len() >= 10000 || entry.units.contains(unit))
            {
                continue;
            }
            let cost = unit.len()
                + credential.len()
                + field.field.len()
                + 128
                + values.keys().map(|k| k.len() * 2 + 128).sum::<usize>();
            if cost > MAX_BYTES.saturating_sub(cache.bytes) {
                continue;
            }
            let entry = cache.fields.entry(key).or_insert_with(|| FieldSample {
                since: Instant::now(),
                units: HashSet::new(),
                values: BTreeMap::new(),
                bytes: 0,
            });
            entry.units.insert(unit.to_string());
            for (encoded, value) in values {
                entry.values.entry(encoded).or_insert((value, 0)).1 += 1;
            }
            entry.bytes += cost;
            cache.bytes += cost;
        }
    }
    pub fn snapshot(&self, credential: &str) -> Samples {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let mut fields = BTreeMap::new();
        for ((owner, field), entry) in &cache.fields {
            if owner != credential
                || entry.since.elapsed() >= Duration::from_secs(3600)
                || entry.units.len() < 1000
                || entry.units.len() < entry.values.len() * 10
            {
                continue;
            }
            let values: Vec<_> = entry
                .values
                .values()
                .filter(|(_, count)| *count > 10)
                .take(100)
                .map(|(v, _)| v.clone())
                .collect();
            if !values.is_empty() {
                fields.insert(field.clone(), SeenValues { values });
            }
        }
        if fields.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([(credential.to_string(), fields)])
        }
    }
}
fn extract<'a>(context: &'a Struct, field: &str) -> Option<&'a Value> {
    let mut parts = field.split('.');
    let mut value = context.fields.get(parts.next()?)?;
    for part in parts {
        match value.kind.as_ref()? {
            Kind::StructValue(s) => value = s.fields.get(part)?,
            _ => return None,
        }
    }
    Some(value)
}

// Protobuf projections add seen_values while preserving the existing log wire layout.
#[derive(Clone, PartialEq, Message)]
pub struct SeenValues {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<Value>,
}
#[derive(Clone, PartialEq, Message)]
pub struct WireSchema {
    #[prost(btree_map = "string, int32", tag = "1")]
    pub schema: BTreeMap<String, i32>,
    #[prost(btree_map = "string, message", tag = "2")]
    pub semantic_types: BTreeMap<String, admin::ContextFieldSemanticType>,
    #[prost(btree_map = "string, message", tag = "3")]
    pub seen_values: BTreeMap<String, SeenValues>,
}
#[derive(Clone, PartialEq, Message)]
pub struct WireClient {
    #[prost(string, tag = "1")]
    pub client: String,
    #[prost(string, tag = "2")]
    pub client_credential: String,
    #[prost(message, repeated, tag = "3")]
    pub schema: Vec<WireSchema>,
}
#[derive(Clone, PartialEq, Message)]
pub struct WireLogs {
    #[prost(message, repeated, tag = "1")]
    pub flag_assigned: Vec<api::events::FlagAssigned>,
    #[prost(message, optional, tag = "2")]
    pub telemetry_data: Option<api::TelemetryData>,
    #[prost(message, repeated, tag = "3")]
    pub client_resolve_info: Vec<WireClient>,
    #[prost(message, repeated, tag = "4")]
    pub flag_resolve_info: Vec<admin::FlagResolveInfo>,
}
impl WireLogs {
    pub fn new(logs: api::WriteFlagLogsRequest, samples: &Samples) -> Self {
        Self {
            flag_assigned: logs.flag_assigned,
            telemetry_data: logs.telemetry_data,
            flag_resolve_info: logs.flag_resolve_info,
            client_resolve_info: logs
                .client_resolve_info
                .into_iter()
                .map(|client| {
                    let fields = samples
                        .get(&client.client_credential)
                        .cloned()
                        .unwrap_or_default();
                    WireClient {
                        client: client.client,
                        client_credential: client.client_credential,
                        schema: client
                            .schema
                            .into_iter()
                            .map(|schema| WireSchema {
                                schema: schema.schema,
                                semantic_types: schema.semantic_types,
                                seen_values: fields.clone(),
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_only_approved_low_cardinality_values_after_enough_unique_units() {
        let sampler = Sampler::new(vec![
            FieldOverride {
                field: "country".into(),
                clients: vec!["clients/a".into()],
                non_pii: true,
            },
            FieldOverride {
                field: "email".into(),
                clients: vec![],
                non_pii: false,
            },
        ]);
        let context: Struct = serde_json::from_value(
            serde_json::json!({"country":"SE","email":"private@example.com"}),
        )
        .unwrap();
        for _ in 0..1000 {
            sampler.observe("cred", "clients/a", "same-user", &context);
        }
        assert!(sampler.snapshot("cred").is_empty());
        for i in 0..1000 {
            sampler.observe("cred", "clients/a", &format!("user-{i}"), &context);
        }
        let samples = sampler.snapshot("cred");
        assert_eq!(samples["cred"].len(), 1);
        assert_eq!(samples["cred"]["country"].values.len(), 1);
        assert!(sampler.snapshot("other-cred").is_empty());
    }
}
