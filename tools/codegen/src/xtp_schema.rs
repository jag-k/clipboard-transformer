//! Repository-only XTP Schema generation for the runtime-neutral Plugin API.
//!
//! Protocol request types derive `JsonSchema`; this module converts that
//! schema subset into XTP's smaller IDL and supplies the export signatures.
//! The two tagged response enums use flattened schema-only adapters because
//! XTP v1-draft cannot represent tagged unions.

use std::collections::BTreeMap;

use schemars::{generate::SchemaSettings, JsonSchema};
use serde_json::{json, Map, Value};

use ct_plugin_api::REQUIRED_EXPORTS;
use ct_plugin_api::{CompileRuleRequest, InitializeRequest, InitializeResponse, TransformRequest};

use crate::schema_compat::normalize_draft07_schema;

/// XTP-compatible flattened view of `CompileRuleResponse`.
// Fields exist only so `derive(JsonSchema)` emits them; nothing reads them.
#[allow(dead_code)]
#[derive(JsonSchema)]
struct CompileRuleResponseSchema {
    /// Discriminant: "ok" or "error".
    result: CompileRuleResultSchema,
    /// Opaque compiled rule value, echoed back on transform.
    rule: Option<BTreeMap<String, serde_json::Value>>,
    /// Why the rule settings were rejected.
    message: Option<String>,
}

// Fields exist only so `derive(JsonSchema)` emits them; nothing reads them.
#[allow(dead_code)]
#[derive(JsonSchema)]
#[schemars(rename = "CompileRuleResult")]
enum CompileRuleResultSchema {
    #[schemars(rename = "ok")]
    Ok,
    #[schemars(rename = "error")]
    Error,
}

/// XTP-compatible flattened view of `TransformResponse`.
// Fields exist only so `derive(JsonSchema)` emits them; nothing reads them.
#[allow(dead_code)]
#[derive(JsonSchema)]
struct TransformResponseSchema {
    /// Discriminant: "no-match" or "replace".
    ///
    /// This stays a string because the XTP Rust generator rejects hyphenated
    /// enum values as source-language identifiers.
    action: String,
    /// Authored plain text. Stale textual representations are removed while
    /// unrelated native payloads remain unchanged.
    text: Option<String>,
    /// Optional notification body.
    message: Option<String>,
}

/// Generates the checked-in `plugins/plugin-api-v1.xtp.yaml` artifact.
pub fn plugin_api_xtp_schema() -> String {
    let mut schemas = BTreeMap::<String, Value>::new();
    collect_schema::<InitializeRequest>("InitializeRequest", &mut schemas);
    collect_schema::<InitializeResponse>("InitializeResponse", &mut schemas);
    collect_schema::<CompileRuleRequest>("CompileRuleRequest", &mut schemas);
    collect_schema::<CompileRuleResponseSchema>("CompileRuleResponse", &mut schemas);
    collect_schema::<TransformRequest>("TransformRequest", &mut schemas);
    collect_schema::<TransformResponseSchema>("TransformResponse", &mut schemas);

    let definitions = schemas.clone();
    let components: Map<String, Value> = schemas
        .iter()
        .filter_map(|(name, schema)| {
            convert_component(name, schema, &definitions).map(|value| (name.clone(), value))
        })
        .collect();

    let mut exports = Map::new();
    for export in REQUIRED_EXPORTS {
        let (input, output, description) = match *export {
            "initialize" => (
                "InitializeRequest",
                "InitializeResponse",
                "Initialize one plugin instance with resolved settings and effective capability grants.",
            ),
            "compile_rule" => (
                "CompileRuleRequest",
                "CompileRuleResponse",
                "Validate one configured rule and return an opaque compiled value or an error.",
            ),
            "transform" => (
                "TransformRequest",
                "TransformResponse",
                "Apply one compiled text-transform rule to the selected clipboard representation.",
            ),
            other => panic!("required plugin export {other:?} has no XTP schema mapping"),
        };
        exports.insert(
            (*export).to_string(),
            json!({
                "description": description,
                "input": {
                    "contentType": "application/json",
                    "$ref": format!("#/components/schemas/{input}"),
                },
                "output": {
                    "contentType": "application/json",
                    "$ref": format!("#/components/schemas/{output}"),
                },
            }),
        );
    }

    let document = json!({
        "version": "v1-draft",
        "exports": exports,
        "imports": {
            "http_host_allowed": {
                "description": "Return whether one literal hostname is allowed by the host-owned Extism HTTP policy. Glob input is rejected.",
                "input": {
                    "type": "string",
                    "contentType": "text/plain; charset=utf-8",
                },
                "output": {
                    "type": "boolean",
                    "contentType": "application/json",
                },
            },
        },
        "components": { "schemas": components },
    });
    let yaml = serde_yaml::to_string(&document).expect("XTP schema serializes as YAML");
    format!(
        "# @generated by `just gen-schemas`; do not edit by hand.\n\
         # Source of truth: Rust protocol types in crates/plugin-api/src/lib.rs.\n\
         # Generator: tools/codegen/src/xtp_schema.rs.\n\
         # This file is an INPUT to the XTP CLI, which generates guest bindings\n\
         # from it (see plugins/gitlab-link/src/pdk.rs). XTP does not produce it.\n{}",
        yaml.strip_prefix("---\n").unwrap_or(&yaml)
    )
}

fn collect_schema<T: JsonSchema>(name: &str, schemas: &mut BTreeMap<String, Value>) {
    let mut root = SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value();
    normalize_draft07_schema(&mut root);
    let object = root
        .as_object_mut()
        .expect("generated root schema is an object");
    if let Some(Value::Object(definitions)) = object.remove("definitions") {
        for (definition_name, definition) in definitions {
            schemas.entry(definition_name).or_insert(definition);
        }
    }
    object.remove("$schema");
    schemas.insert(name.to_string(), root);
}

fn convert_component(
    name: &str,
    schema: &Value,
    definitions: &BTreeMap<String, Value>,
) -> Option<Value> {
    let schema = schema.as_object()?;
    if let Some(values) = string_enum_values(schema) {
        if values.iter().all(|value| is_xtp_identifier(value)) {
            return Some(json!({
                "description": description(schema).unwrap_or_else(|| format!("{name} values.")),
                "type": "string",
                "enum": values,
            }));
        }
        return None;
    }

    let object = schema.get("properties")?.as_object()?;
    let properties: Map<String, Value> = object
        .iter()
        .map(|(property, schema)| (property.clone(), convert_property(schema, definitions)))
        .collect();
    let mut component = Map::new();
    component.insert(
        "description".to_string(),
        Value::String(description(schema).unwrap_or_else(|| format!("{name} payload."))),
    );
    component.insert("properties".to_string(), Value::Object(properties));
    if let Some(required) = schema.get("required") {
        component.insert("required".to_string(), required.clone());
    }
    Some(Value::Object(component))
}

fn convert_property(schema: &Value, definitions: &BTreeMap<String, Value>) -> Value {
    let mut value = convert_property_shape(schema, definitions);
    if let (Some(schema), Value::Object(value)) = (schema.as_object(), &mut value) {
        if let Some(description) = description(schema) {
            value.insert("description".to_string(), Value::String(description));
        }
    }
    value
}

fn convert_property_shape(schema: &Value, definitions: &BTreeMap<String, Value>) -> Value {
    let Some(schema) = schema.as_object() else {
        return json!({ "type": "object" });
    };

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        if let [inner] = all_of.as_slice() {
            return convert_property_shape(inner, definitions);
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        let non_null: Vec<&Value> = any_of
            .iter()
            .filter(|candidate| !is_null_schema(candidate))
            .collect();
        if non_null.len() == 1 && non_null.len() != any_of.len() {
            let mut value = convert_property(non_null[0], definitions);
            if let Value::Object(value) = &mut value {
                value.insert("nullable".to_string(), Value::Bool(true));
            }
            return value;
        }
    }

    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let raw_name = reference.rsplit('/').next().unwrap_or(reference);
        let name = raw_name
            .strip_suffix("Schema")
            .filter(|candidate| definitions.contains_key(*candidate))
            .unwrap_or(raw_name);
        if definitions
            .get(name)
            .and_then(Value::as_object)
            .and_then(string_enum_values)
            .is_some_and(|values| values.iter().any(|value| !is_xtp_identifier(value)))
        {
            return json!({ "type": "string" });
        }
        return json!({ "$ref": format!("#/components/schemas/{name}") });
    }

    let mut nullable = false;
    let instance_type = schema.get("type").and_then(|types| match types {
        Value::String(value) => Some(value.as_str()),
        Value::Array(values) => {
            nullable = values.iter().any(|value| value.as_str() == Some("null"));
            values
                .iter()
                .find_map(|value| value.as_str().filter(|value| *value != "null"))
        }
        _ => None,
    });

    let mut value = match instance_type {
        Some("string") => json!({ "type": "string" }),
        Some("integer") => json!({ "type": "integer" }),
        Some("number") => json!({ "type": "number" }),
        Some("boolean") => json!({ "type": "boolean" }),
        Some("array") => {
            let items = schema
                .get("items")
                .map(|items| convert_array_item(items, definitions))
                .unwrap_or_else(|| json!({ "type": "object" }));
            json!({ "type": "array", "items": items })
        }
        Some("object") | None => json!({ "type": "object" }),
        Some("null") => json!({ "type": "object", "nullable": true }),
        Some(_) => json!({ "type": "object" }),
    };
    if nullable {
        if let Value::Object(value) = &mut value {
            value.insert("nullable".to_string(), Value::Bool(true));
        }
    }
    value
}

fn convert_array_item(schema: &Value, definitions: &BTreeMap<String, Value>) -> Value {
    let value = convert_property_shape(schema, definitions);
    let Value::Object(mut value) = value else {
        return json!({ "type": "object" });
    };
    value.remove("description");
    value.remove("nullable");
    Value::Object(value)
}

fn is_null_schema(schema: &Value) -> bool {
    let Some(schema) = schema.as_object() else {
        return false;
    };
    schema.get("const").is_some_and(Value::is_null)
        || schema.get("type").is_some_and(|types| match types {
            Value::String(value) => value == "null",
            Value::Array(values) => {
                values.len() == 1 && values.first().and_then(Value::as_str) == Some("null")
            }
            _ => false,
        })
}

fn string_enum_values(schema: &Map<String, Value>) -> Option<Vec<String>> {
    if let Some(value) = schema.get("const").and_then(Value::as_str) {
        return Some(vec![value.to_string()]);
    }
    schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_string))
                .collect()
        })
}

fn description(schema: &Map<String, Value>) -> Option<String> {
    schema
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn is_xtp_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && chars
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_plugin_api_schema_is_current() {
        // Read at run time rather than with include_str!: a relative path from
        // this file breaks whenever the package moves.
        let committed =
            std::fs::read_to_string(crate::workspace_root().join("plugins/plugin-api-v1.xtp.yaml"))
                .expect("read committed XTP schema");
        assert_eq!(committed, plugin_api_xtp_schema(), "run `just gen-schemas`");
    }
}
