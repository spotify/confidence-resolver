use confidence_resolver::proto::confidence::flags::resolver::v1::MaterializationRecord;
use futures_util::future::join_all;
use worker::kv::KvStore;

fn encode_component(s: &str) -> String {
    s.replace('%', "%25").replace(':', "%3A")
}

fn kv_key(record: &MaterializationRecord) -> String {
    let unit = encode_component(&record.unit);
    let mat = encode_component(&record.materialization);
    let rule = encode_component(&record.rule);
    format!("mat:{unit}:{mat}:{rule}")
}

fn kv_prefix(unit: &str, materialization: &str) -> String {
    let unit = encode_component(unit);
    let mat = encode_component(materialization);
    format!("mat:{unit}:{mat}:")
}

pub async fn read_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
) -> Vec<MaterializationRecord> {
    let futures: Vec<_> = records
        .iter()
        .map(|record| async move {
            if record.rule.is_empty() {
                let prefix = kv_prefix(&record.unit, &record.materialization);
                match kv.list().prefix(prefix).limit(1).execute().await {
                    Ok(list) if !list.keys.is_empty() => Some(MaterializationRecord {
                        unit: record.unit.clone(),
                        materialization: record.materialization.clone(),
                        rule: String::new(),
                        variant: String::new(),
                    }),
                    _ => None,
                }
            } else {
                let key = kv_key(record);
                match kv.get(&key).text().await {
                    Ok(Some(value)) => Some(MaterializationRecord {
                        unit: record.unit.clone(),
                        materialization: record.materialization.clone(),
                        rule: record.rule.clone(),
                        variant: value,
                    }),
                    _ => None,
                }
            }
        })
        .collect();

    join_all(futures).await.into_iter().flatten().collect()
}

pub async fn write_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
    ttl_seconds: Option<u64>,
) {
    let futures: Vec<_> = records
        .iter()
        .filter(|r| !r.rule.is_empty())
        .map(|record| async move {
            let key = kv_key(record);
            let builder = kv.put(&key, &record.variant);
            let builder = match (builder, ttl_seconds) {
                (Ok(b), Some(ttl)) => Ok(b.expiration_ttl(ttl)),
                (builder, _) => builder,
            };
            if let Ok(b) = builder {
                let _ = b.execute().await;
            }
        })
        .collect();

    join_all(futures).await;
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

    // --- kv_key ---

    #[wasm_bindgen_test]
    fn key_format_with_rule() {
        let r = record("user_123", "exp_abc", "rule_1", "treatment");
        assert_eq!(kv_key(&r), "mat:user_123:exp_abc:rule_1");
    }

    #[wasm_bindgen_test]
    fn key_escapes_colons_in_unit() {
        let r = record("user:123", "exp_abc", "rule_1", "");
        assert_eq!(kv_key(&r), "mat:user%3A123:exp_abc:rule_1");
    }

    #[wasm_bindgen_test]
    fn key_escapes_colons_in_materialization() {
        let r = record("user_123", "mat:seg/abc", "rule_1", "");
        assert_eq!(kv_key(&r), "mat:user_123:mat%3Aseg/abc:rule_1");
    }

    #[wasm_bindgen_test]
    fn key_escapes_percent_before_colon() {
        let r = record("user%3A", "exp", "rule", "");
        assert_eq!(kv_key(&r), "mat:user%253A:exp:rule");
    }

    // --- kv_prefix ---

    #[wasm_bindgen_test]
    fn prefix_format() {
        assert_eq!(kv_prefix("user_123", "exp_abc"), "mat:user_123:exp_abc:");
    }

    #[wasm_bindgen_test]
    fn prefix_escapes_colons() {
        assert_eq!(kv_prefix("user:1", "mat:x"), "mat:user%3A1:mat%3Ax:");
    }

    // --- encode_component ---

    #[wasm_bindgen_test]
    fn encode_no_special_chars() {
        assert_eq!(encode_component("hello_world"), "hello_world");
    }

    #[wasm_bindgen_test]
    fn encode_colon() {
        assert_eq!(encode_component("a:b"), "a%3Ab");
    }

    #[wasm_bindgen_test]
    fn encode_percent_then_colon() {
        assert_eq!(encode_component("a%:b"), "a%25%3Ab");
    }

    #[wasm_bindgen_test]
    fn encode_multiple_colons() {
        assert_eq!(encode_component("a:b:c"), "a%3Ab%3Ac");
    }
}
