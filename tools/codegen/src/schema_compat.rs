use serde_json::{json, Value};

/// Keeps the checked-in draft-07 contracts byte-stable across Schemars 0.8 and 1.x.
///
/// Schemars 1.x emits a few representations that are valid draft-07 and mean the
/// same thing as the 0.8 output, but would rewrite large parts of the generated
/// plugin contracts. Canonicalizing those differences here makes an actual
/// contract change visible in the generated-file diff instead of mixing it with
/// generator-version noise.
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

            // A one-value enum and a string const accept exactly the same value.
            if let Some(Value::String(constant)) = object.remove("const") {
                object.insert("enum".to_string(), json!([constant]));
            }
            // Rustdoc wrapping is not part of the schema contract. Preserve
            // paragraph breaks while removing formatter-dependent line wraps.
            if let Some(Value::String(description)) = object.get_mut("description") {
                *description = description
                    .split("\n\n")
                    .map(|paragraph| paragraph.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            // JSON Schema treats `required` as a set; sorting only stabilizes
            // serialization order.
            if let Some(Value::Array(required)) = object.get_mut("required") {
                required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            // JSON numbers 0 and 0.0 compare equally. Retain the spelling used
            // in the already-published generated contracts.
            if object.get("minimum").and_then(Value::as_u64) == Some(0) {
                object.insert("minimum".to_string(), json!(0.0));
            }
        }
        _ => {}
    }
}
