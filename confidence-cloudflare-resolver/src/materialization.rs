use confidence_resolver::proto::confidence::flags::resolver::v1::MaterializationRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use worker::kv::KvStore;

type Key = (String, String);

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct MaterializationData {
    #[serde(default)]
    pub included: bool,
    #[serde(default)]
    pub rules: HashMap<String, String>,
}

fn kv_key(unit: &str, materialization: &str) -> String {
    format!("mat:{unit}:{materialization}")
}

fn group_by_key(records: &[MaterializationRecord]) -> HashMap<Key, Vec<&MaterializationRecord>> {
    let mut groups: HashMap<Key, Vec<&MaterializationRecord>> = HashMap::new();
    for record in records {
        groups
            .entry((record.unit.clone(), record.materialization.clone()))
            .or_default()
            .push(record);
    }
    groups
}

/// Given requested records and fetched KV data, build the result records.
///
/// Only records with actual assignments are returned — absence signals
/// "no sticky assignment", matching the convention in all other providers.
fn build_read_results(
    records: &[MaterializationRecord],
    fetched: &HashMap<Key, MaterializationData>,
) -> Vec<MaterializationRecord> {
    let groups = group_by_key(records);
    let mut results = Vec::new();

    for ((unit, materialization), group_records) in &groups {
        let Some(data) = fetched.get(&(unit.clone(), materialization.clone())) else {
            continue;
        };

        for record in group_records {
            if record.rule.is_empty() {
                if data.included {
                    results.push(MaterializationRecord {
                        unit: record.unit.clone(),
                        materialization: record.materialization.clone(),
                        rule: String::new(),
                        variant: String::new(),
                    });
                }
            } else if let Some(variant) = data.rules.get(&record.rule) {
                results.push(MaterializationRecord {
                    unit: record.unit.clone(),
                    materialization: record.materialization.clone(),
                    rule: record.rule.clone(),
                    variant: variant.clone(),
                });
            }
        }
    }

    results
}

/// Merge new write records into existing materialization data.
fn merge_writes(
    records: &[MaterializationRecord],
    existing: &HashMap<Key, MaterializationData>,
) -> HashMap<Key, MaterializationData> {
    let groups = group_by_key(records);
    let mut result = HashMap::new();

    for ((unit, materialization), group_records) in &groups {
        let key = (unit.clone(), materialization.clone());
        let mut data = existing.get(&key).cloned().unwrap_or_default();

        for record in group_records {
            if record.rule.is_empty() {
                data.included = true;
            } else {
                data.rules
                    .insert(record.rule.clone(), record.variant.clone());
                data.included = true;
            }
        }

        result.insert(key, data);
    }

    result
}

/// Reads materialization data from KV for the given records.
pub async fn read_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
) -> Vec<MaterializationRecord> {
    let groups = group_by_key(records);
    let mut fetched: HashMap<Key, MaterializationData> = HashMap::new();

    for (unit, materialization) in groups.keys() {
        let key = kv_key(unit, materialization);
        if let Ok(Some(text)) = kv.get(&key).text().await {
            if let Ok(data) = serde_json::from_str(&text) {
                fetched.insert((unit.clone(), materialization.clone()), data);
            }
        }
    }

    build_read_results(records, &fetched)
}

/// Writes materialization assignments to KV.
///
/// Groups records by (unit, materialization) and performs a read-modify-write
/// for each group to merge new assignments into existing data.
pub async fn write_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
    ttl_seconds: Option<u64>,
) {
    let groups = group_by_key(records);
    let mut existing: HashMap<Key, MaterializationData> = HashMap::new();

    for (unit, materialization) in groups.keys() {
        let key = kv_key(unit, materialization);
        if let Ok(Some(text)) = kv.get(&key).text().await {
            if let Ok(data) = serde_json::from_str(&text) {
                existing.insert((unit.clone(), materialization.clone()), data);
            }
        }
    }

    let merged = merge_writes(records, &existing);

    for ((unit, materialization), data) in &merged {
        let key = kv_key(unit, materialization);
        if let Ok(json) = serde_json::to_string(data) {
            let builder = kv.put(&key, json);
            let builder = match (builder, ttl_seconds) {
                (Ok(b), Some(ttl)) => Ok(b.expiration_ttl(ttl)),
                (builder, _) => builder,
            };
            if let Ok(b) = builder {
                let _ = b.execute().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    fn record(unit: &str, mat: &str, rule: &str, variant: &str) -> MaterializationRecord {
        MaterializationRecord {
            unit: unit.to_string(),
            materialization: mat.to_string(),
            rule: rule.to_string(),
            variant: variant.to_string(),
        }
    }

    fn data(included: bool, rules: &[(&str, &str)]) -> MaterializationData {
        MaterializationData {
            included,
            rules: rules
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    // --- kv_key ---

    #[wasm_bindgen_test]
    fn key_format() {
        assert_eq!(kv_key("user_123", "exp_abc"), "mat:user_123:exp_abc");
    }

    #[wasm_bindgen_test]
    fn key_with_special_chars() {
        assert_eq!(
            kv_key("user/foo", "mat:bar"),
            "mat:user/foo:mat:bar"
        );
    }

    // --- MaterializationData serde ---

    #[wasm_bindgen_test]
    fn serde_roundtrip() {
        let d = data(true, &[("rule_1", "treatment"), ("rule_2", "control")]);
        let json = serde_json::to_string(&d).unwrap();
        let d2: MaterializationData = serde_json::from_str(&json).unwrap();
        assert_eq!(d, d2);
    }

    #[wasm_bindgen_test]
    fn serde_defaults_on_missing_fields() {
        let d: MaterializationData = serde_json::from_str("{}").unwrap();
        assert!(!d.included);
        assert!(d.rules.is_empty());
    }

    #[wasm_bindgen_test]
    fn serde_partial_fields() {
        let d: MaterializationData = serde_json::from_str(r#"{"included": true}"#).unwrap();
        assert!(d.included);
        assert!(d.rules.is_empty());
    }

    // --- build_read_results ---

    #[wasm_bindgen_test]
    fn read_variant_found() {
        let records = vec![record("u1", "m1", "rule_a", "")];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_a", "treatment")]),
        );

        let results = build_read_results(&records, &fetched);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant, "treatment");
        assert_eq!(results[0].rule, "rule_a");
    }

    #[wasm_bindgen_test]
    fn read_variant_not_found() {
        let records = vec![record("u1", "m1", "rule_a", "")];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_b", "control")]),
        );

        let results = build_read_results(&records, &fetched);
        assert!(results.is_empty());
    }

    #[wasm_bindgen_test]
    fn read_no_kv_data() {
        let records = vec![record("u1", "m1", "rule_a", "")];
        let fetched = HashMap::new();

        let results = build_read_results(&records, &fetched);
        assert!(results.is_empty());
    }

    #[wasm_bindgen_test]
    fn read_inclusion_included() {
        let records = vec![record("u1", "m1", "", "")];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[]),
        );

        let results = build_read_results(&records, &fetched);
        assert_eq!(results.len(), 1);
        assert!(results[0].rule.is_empty());
        assert!(results[0].variant.is_empty());
    }

    #[wasm_bindgen_test]
    fn read_inclusion_not_included() {
        let records = vec![record("u1", "m1", "", "")];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(false, &[]),
        );

        let results = build_read_results(&records, &fetched);
        assert!(results.is_empty());
    }

    #[wasm_bindgen_test]
    fn read_multiple_records_same_key() {
        let records = vec![
            record("u1", "m1", "rule_a", ""),
            record("u1", "m1", "rule_b", ""),
            record("u1", "m1", "", ""),
        ];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_a", "treatment")]),
        );

        let results = build_read_results(&records, &fetched);
        // rule_a found + inclusion found, rule_b not found
        assert_eq!(results.len(), 2);
        let variants: Vec<&str> = results.iter().map(|r| r.rule.as_str()).collect();
        assert!(variants.contains(&"rule_a"));
        assert!(variants.contains(&""));
    }

    #[wasm_bindgen_test]
    fn read_multiple_keys() {
        let records = vec![
            record("u1", "m1", "rule_a", ""),
            record("u2", "m2", "rule_b", ""),
        ];
        let mut fetched = HashMap::new();
        fetched.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_a", "treatment")]),
        );
        // u2/m2 not in KV

        let results = build_read_results(&records, &fetched);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].unit, "u1");
    }

    // --- merge_writes ---

    #[wasm_bindgen_test]
    fn write_new_entry() {
        let records = vec![record("u1", "m1", "rule_a", "treatment")];
        let existing = HashMap::new();

        let merged = merge_writes(&records, &existing);
        let key = ("u1".to_string(), "m1".to_string());
        assert_eq!(merged.len(), 1);
        let d = &merged[&key];
        assert!(d.included);
        assert_eq!(d.rules.get("rule_a").unwrap(), "treatment");
    }

    #[wasm_bindgen_test]
    fn write_merge_into_existing() {
        let records = vec![record("u1", "m1", "rule_b", "control")];
        let mut existing = HashMap::new();
        existing.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_a", "treatment")]),
        );

        let merged = merge_writes(&records, &existing);
        let key = ("u1".to_string(), "m1".to_string());
        let d = &merged[&key];
        assert!(d.included);
        assert_eq!(d.rules.get("rule_a").unwrap(), "treatment");
        assert_eq!(d.rules.get("rule_b").unwrap(), "control");
    }

    #[wasm_bindgen_test]
    fn write_overwrite_existing_variant() {
        let records = vec![record("u1", "m1", "rule_a", "control")];
        let mut existing = HashMap::new();
        existing.insert(
            ("u1".to_string(), "m1".to_string()),
            data(true, &[("rule_a", "treatment")]),
        );

        let merged = merge_writes(&records, &existing);
        let key = ("u1".to_string(), "m1".to_string());
        assert_eq!(merged[&key].rules.get("rule_a").unwrap(), "control");
    }

    #[wasm_bindgen_test]
    fn write_inclusion_only() {
        let records = vec![record("u1", "m1", "", "")];
        let existing = HashMap::new();

        let merged = merge_writes(&records, &existing);
        let key = ("u1".to_string(), "m1".to_string());
        assert!(merged[&key].included);
        assert!(merged[&key].rules.is_empty());
    }

    #[wasm_bindgen_test]
    fn write_multiple_records_same_key() {
        let records = vec![
            record("u1", "m1", "rule_a", "treatment"),
            record("u1", "m1", "rule_b", "control"),
        ];
        let existing = HashMap::new();

        let merged = merge_writes(&records, &existing);
        let key = ("u1".to_string(), "m1".to_string());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[&key].rules.len(), 2);
        assert_eq!(merged[&key].rules.get("rule_a").unwrap(), "treatment");
        assert_eq!(merged[&key].rules.get("rule_b").unwrap(), "control");
    }

    #[wasm_bindgen_test]
    fn write_multiple_keys() {
        let records = vec![
            record("u1", "m1", "rule_a", "treatment"),
            record("u2", "m2", "rule_b", "control"),
        ];
        let existing = HashMap::new();

        let merged = merge_writes(&records, &existing);
        assert_eq!(merged.len(), 2);
        assert!(merged.contains_key(&("u1".to_string(), "m1".to_string())));
        assert!(merged.contains_key(&("u2".to_string(), "m2".to_string())));
    }

    // --- group_by_key ---

    #[wasm_bindgen_test]
    fn grouping_deduplicates_kv_reads() {
        let records = vec![
            record("u1", "m1", "rule_a", ""),
            record("u1", "m1", "rule_b", ""),
            record("u1", "m1", "", ""),
        ];
        let groups = group_by_key(&records);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[&("u1".to_string(), "m1".to_string())].len(),
            3
        );
    }
}
