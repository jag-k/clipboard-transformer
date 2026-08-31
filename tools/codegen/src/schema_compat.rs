use serde_json::{json, Value};

/// Keeps the checked-in draft-07 contracts byte-stable across Schemars 0.8 and 1.x.
pub(crate) fn normalize_draft07_schema(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_draft07_schema(value);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                normalize_draft07_schema(value);
            }

            if let Some(Value::String(constant)) = object.remove("const") {
                object.insert("enum".to_string(), json!([constant]));
            }
            if let Some(Value::String(description)) = object.get_mut("description") {
                *description = description
                    .split("\n\n")
                    .map(|paragraph| paragraph.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            if let Some(Value::Array(required)) = object.get_mut("required") {
                required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            if object.get("minimum").and_then(Value::as_u64) == Some(0) {
                object.insert("minimum".to_string(), json!(0.0));
            }
        }
        _ => {}
    }
}
