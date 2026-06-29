use confidence_resolver::proto::confidence::flags::resolver::v1::MaterializationRecord;
use worker::kv::KvStore;

fn encode_component(s: &str) -> String {
    s.replace('%', "%25").replace(':', "%3A")
}

fn kv_key(record: &MaterializationRecord) -> String {
    let unit = encode_component(&record.unit);
    let mat = encode_component(&record.materialization);
    if record.rule.is_empty() {
        format!("mat:{unit}:{mat}:__included__")
    } else {
        let rule = encode_component(&record.rule);
        format!("mat:{unit}:{mat}:{rule}")
    }
}

pub async fn read_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
) -> Vec<MaterializationRecord> {
    let mut results = Vec::new();

    for record in records {
        let key = kv_key(record);
        if let Ok(Some(value)) = kv.get(&key).text().await {
            if record.rule.is_empty() {
                results.push(MaterializationRecord {
                    unit: record.unit.clone(),
                    materialization: record.materialization.clone(),
                    rule: String::new(),
                    variant: String::new(),
                });
            } else {
                results.push(MaterializationRecord {
                    unit: record.unit.clone(),
                    materialization: record.materialization.clone(),
                    rule: record.rule.clone(),
                    variant: value,
                });
            }
        }
    }

    results
}

pub async fn write_materializations(
    kv: &KvStore,
    records: &[MaterializationRecord],
    ttl_seconds: Option<u64>,
) {
    for record in records {
        let key = kv_key(record);
        let value = if record.rule.is_empty() {
            "1".to_string()
        } else {
            record.variant.clone()
        };

        let builder = kv.put(&key, value);
        let builder = match (builder, ttl_seconds) {
            (Ok(b), Some(ttl)) => Ok(b.expiration_ttl(ttl)),
            (builder, _) => builder,
        };
        if let Ok(b) = builder {
            let _ = b.execute().await;
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

    // --- kv_key ---

    #[wasm_bindgen_test]
    fn key_format_with_rule() {
        let r = record("user_123", "exp_abc", "rule_1", "treatment");
        assert_eq!(kv_key(&r), "mat:user_123:exp_abc:rule_1");
    }

    #[wasm_bindgen_test]
    fn key_format_inclusion() {
        let r = record("user_123", "exp_abc", "", "");
        assert_eq!(kv_key(&r), "mat:user_123:exp_abc:__included__");
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
