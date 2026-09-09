use std::collections::BTreeMap;

use crate::proto::google::{value::Kind, Struct, Value};

#[cfg(feature = "json")]
type FieldMap = std::collections::HashMap<String, Value>;
#[cfg(not(feature = "json"))]
type FieldMap = BTreeMap<String, Value>;

const MAX_DEPTH: usize = 10;

/// Merge a Client's managed evaluation context (flat dot-paths -> scalar Values)
/// into the SDK-provided request context (nested Struct).
/// Managed context provides defaults; request context wins on exact-path
/// collisions. When both sides have a Struct for the same key, merge
/// recursively to preserve siblings from both.
pub fn merge(managed: &BTreeMap<String, Value>, request_context: Struct) -> Struct {
    if managed.is_empty() {
        return request_context;
    }
    let managed_struct = unflatten(managed);
    merge_structs(&managed_struct, &request_context)
}

fn unflatten(flat: &BTreeMap<String, Value>) -> Struct {
    let mut root: FieldMap = FieldMap::new();
    for (path, val) in flat {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.len() > MAX_DEPTH {
            continue;
        }
        set_nested(&mut root, &parts, 0, val.clone());
    }
    Struct { fields: root }
}

fn set_nested(current: &mut FieldMap, parts: &[&str], idx: usize, val: Value) {
    let Some(key) = parts.get(idx) else { return };
    let key = (*key).to_string();

    if idx.checked_add(1) == Some(parts.len()) {
        current.insert(key, val);
        return;
    }

    let mut nested = match current.get(&key) {
        Some(Value {
            kind: Some(Kind::StructValue(s)),
        }) => s.fields.clone(),
        _ => FieldMap::new(),
    };

    if let Some(next) = idx.checked_add(1) {
        set_nested(&mut nested, parts, next, val);
    }

    current.insert(
        key,
        Value {
            kind: Some(Kind::StructValue(Struct { fields: nested })),
        },
    );
}

fn merge_structs(managed: &Struct, request: &Struct) -> Struct {
    let mut result = managed.fields.clone();

    for (key, request_val) in &request.fields {
        let managed_val = result.get(key);

        match (&request_val.kind, managed_val.and_then(|v| v.kind.as_ref())) {
            (Some(Kind::StructValue(rs)), Some(Kind::StructValue(ms))) => {
                let merged = merge_structs(ms, rs);
                result.insert(
                    key.clone(),
                    Value {
                        kind: Some(Kind::StructValue(merged)),
                    },
                );
            }
            _ => {
                result.insert(key.clone(), request_val.clone());
            }
        }
    }

    Struct { fields: result }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn str_val(s: &str) -> Value {
        Value {
            kind: Some(Kind::StringValue(s.to_string())),
        }
    }

    fn num_val(n: f64) -> Value {
        Value {
            kind: Some(Kind::NumberValue(n)),
        }
    }

    fn struct_val(fields: FieldMap) -> Value {
        Value {
            kind: Some(Kind::StructValue(Struct { fields })),
        }
    }

    #[test]
    fn empty_managed_returns_request_unchanged() {
        let request = Struct {
            fields: FieldMap::from([("user".to_string(), str_val("abc"))]),
        };
        let result = merge(&BTreeMap::new(), request.clone());
        assert_eq!(result, request);
    }

    #[test]
    fn managed_fields_added_to_empty_request() {
        let managed = BTreeMap::from([("platform".to_string(), str_val("ios"))]);
        let result = merge(&managed, Struct::default());
        assert_eq!(
            result.fields.get("platform").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("ios".to_string()))
        );
    }

    #[test]
    fn request_wins_on_collision() {
        let managed = BTreeMap::from([("platform".to_string(), str_val("android"))]);
        let request = Struct {
            fields: FieldMap::from([("platform".to_string(), str_val("ios"))]),
        };
        let result = merge(&managed, request);
        assert_eq!(
            result.fields.get("platform").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("ios".to_string()))
        );
    }

    #[test]
    fn nested_managed_preserves_request_siblings() {
        let managed = BTreeMap::from([("app.name".to_string(), str_val("checkout"))]);
        let request = Struct {
            fields: FieldMap::from([(
                "app".to_string(),
                struct_val(FieldMap::from([("version".to_string(), str_val("1.2"))])),
            )]),
        };
        let result = merge(&managed, request);
        let app = match result.fields.get("app").and_then(|v| v.kind.as_ref()) {
            Some(Kind::StructValue(s)) => s,
            _ => panic!("expected struct"),
        };
        assert_eq!(
            app.fields.get("version").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("1.2".to_string()))
        );
        assert_eq!(
            app.fields.get("name").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("checkout".to_string()))
        );
    }

    #[test]
    fn request_scalar_replaces_managed_subtree() {
        let managed = BTreeMap::from([
            ("app.name".to_string(), str_val("checkout")),
            ("app.version".to_string(), str_val("1.2")),
        ]);
        let request = Struct {
            fields: FieldMap::from([("app".to_string(), str_val("monolith"))]),
        };
        let result = merge(&managed, request);
        assert_eq!(
            result.fields.get("app").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("monolith".to_string()))
        );
    }

    #[test]
    fn request_overrides_managed_default() {
        let managed = BTreeMap::from([
            ("platform".to_string(), str_val("ios")),
            ("app.name".to_string(), str_val("default-app")),
        ]);
        let request = Struct {
            fields: FieldMap::from([("platform".to_string(), str_val("android"))]),
        };
        let result = merge(&managed, request);
        assert_eq!(
            result.fields.get("platform").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("android".to_string())),
            "request should override managed default"
        );
        let app = match result.fields.get("app").and_then(|v| v.kind.as_ref()) {
            Some(Kind::StructValue(s)) => s,
            _ => panic!("expected managed app struct to still be present"),
        };
        assert_eq!(
            app.fields.get("name").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("default-app".to_string())),
            "managed default should fill in when request doesn't provide it"
        );
    }

    #[test]
    fn all_scalar_types() {
        let managed = BTreeMap::from([
            ("name".to_string(), str_val("checkout")),
            ("version".to_string(), num_val(2.1)),
            (
                "debug".to_string(),
                Value {
                    kind: Some(Kind::BoolValue(true)),
                },
            ),
        ]);
        let result = merge(&managed, Struct::default());
        assert_eq!(
            result.fields.get("name").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("checkout".to_string()))
        );
        assert_eq!(
            result.fields.get("version").and_then(|v| v.kind.as_ref()),
            Some(&Kind::NumberValue(2.1))
        );
        assert_eq!(
            result.fields.get("debug").and_then(|v| v.kind.as_ref()),
            Some(&Kind::BoolValue(true))
        );
    }

    #[test]
    fn request_fields_not_in_managed_preserved() {
        let managed = BTreeMap::from([("platform".to_string(), str_val("ios"))]);
        let request = Struct {
            fields: FieldMap::from([
                ("user_id".to_string(), str_val("abc")),
                ("locale".to_string(), str_val("en")),
            ]),
        };
        let result = merge(&managed, request);
        assert_eq!(result.fields.len(), 3);
        assert_eq!(
            result.fields.get("user_id").and_then(|v| v.kind.as_ref()),
            Some(&Kind::StringValue("abc".to_string()))
        );
    }
}
