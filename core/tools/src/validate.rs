//! Argument validation against a tool's JSON Schema.
//!
//! The schemas here are deliberately a small, closed subset of JSON Schema —
//! objects whose properties are strings, integers, booleans or arrays of
//! strings — because that is all a tool call needs and because a validator you
//! can read in one sitting is a validator you can trust at a safety boundary.
//!
//! The rule is fail closed. An unknown property, a missing required argument
//! or a wrong type is a rejection, never a best-effort coercion: a model that
//! sends `{"modul": "ECU_7E8"}` must get an error, not a broadcast request.

use aim_types::{AimError, AimResult, ErrorCode};
use serde_json::Value;

/// Check `arguments` against `schema`, or explain exactly what is wrong.
pub fn validate(tool: &str, schema: &Value, arguments: &Value) -> AimResult<()> {
    let properties = schema.get("properties").and_then(Value::as_object).ok_or_else(|| {
        AimError::internal(format!("tool {tool} has a schema with no properties object"))
    })?;

    let object = arguments.as_object().ok_or_else(|| {
        reject(tool, format!("arguments must be a JSON object, got {}", type_name(arguments)))
    })?;

    // Unknown properties are rejected rather than ignored: a typo that is
    // silently dropped becomes a request with a default the caller never asked
    // for.
    let additional_allowed =
        schema.get("additionalProperties").and_then(Value::as_bool).unwrap_or(true);
    if !additional_allowed {
        for key in object.keys() {
            if !properties.contains_key(key) {
                let known: Vec<&str> = properties.keys().map(|k| k.as_str()).collect();
                return Err(reject(
                    tool,
                    format!("unknown argument {key:?}; accepted arguments are {known:?}"),
                ));
            }
        }
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for r in required {
            let Some(name) = r.as_str() else { continue };
            if !object.contains_key(name) || object[name].is_null() {
                return Err(reject(tool, format!("missing required argument {name:?}")));
            }
        }
    }

    for (name, spec) in properties {
        let Some(value) = object.get(name) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        check_type(tool, name, spec, value)?;
    }

    Ok(())
}

fn check_type(tool: &str, name: &str, spec: &Value, value: &Value) -> AimResult<()> {
    let expected = spec.get("type").and_then(Value::as_str).unwrap_or("string");
    let ok = match expected {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        other => {
            return Err(AimError::internal(format!(
                "tool {tool} declares unsupported schema type {other:?} for {name:?}"
            )))
        }
    };
    if !ok {
        return Err(reject(
            tool,
            format!("argument {name:?} must be {expected}, got {}", type_name(value)),
        ));
    }

    if expected == "array" {
        let items_type = spec
            .get("items")
            .and_then(|i| i.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("string");
        for (i, item) in value.as_array().into_iter().flatten().enumerate() {
            check_type(
                tool,
                &format!("{name}[{i}]"),
                &serde_json::json!({"type": items_type}),
                item,
            )?;
        }
        if spec
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| (value.as_array().map(|a| a.len()).unwrap_or(0) as u64) < min)
        {
            return Err(reject(
                tool,
                format!("argument {name:?} needs at least {} item(s)", spec["minItems"]),
            ));
        }
    }

    if expected == "integer" {
        if let (Some(v), Some(min)) = (value.as_i64(), spec.get("minimum").and_then(Value::as_i64))
        {
            if v < min {
                return Err(reject(tool, format!("argument {name:?} must be >= {min}")));
            }
        }
        if let (Some(v), Some(max)) = (value.as_i64(), spec.get("maximum").and_then(Value::as_i64))
        {
            if v > max {
                return Err(reject(tool, format!("argument {name:?} must be <= {max}")));
            }
        }
    }

    Ok(())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn reject(tool: &str, why: String) -> AimError {
    AimError::new(ErrorCode::BadRequest, format!("{tool}: {why}"))
        .with_details(serde_json::json!({ "tool": tool, "reason": why }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "module": { "type": "string" },
                "signals": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "frame": { "type": "integer", "minimum": 0, "maximum": 255 }
            },
            "required": ["module"],
            "additionalProperties": false
        })
    }

    #[test]
    fn a_well_formed_call_passes() {
        validate(
            "read_live_data",
            &schema(),
            &json!({ "module": "ECU_7E8", "signals": ["engine_rpm"], "frame": 0 }),
        )
        .unwrap();
    }

    #[test]
    fn optional_arguments_may_be_omitted_or_null() {
        validate("t", &schema(), &json!({ "module": "ECU_7E8" })).unwrap();
        validate("t", &schema(), &json!({ "module": "ECU_7E8", "signals": null })).unwrap();
    }

    #[test]
    fn a_missing_required_argument_is_rejected() {
        let e = validate("t", &schema(), &json!({ "signals": ["x"] })).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        assert!(e.message.contains("module"), "{}", e.message);
    }

    #[test]
    fn a_misspelled_argument_is_rejected_rather_than_ignored() {
        let e = validate("t", &schema(), &json!({ "modul": "ECU_7E8" })).unwrap_err();
        assert!(e.message.contains("unknown argument"), "{}", e.message);
        // And the caller is told what it could have said.
        assert!(e.message.contains("module"));
    }

    #[test]
    fn wrong_types_are_rejected_not_coerced() {
        for bad in [
            json!({ "module": 12 }),
            json!({ "module": "x", "signals": "engine_rpm" }),
            json!({ "module": "x", "frame": "0" }),
            json!({ "module": "x", "signals": [1, 2] }),
        ] {
            let e = validate("t", &schema(), &bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadRequest, "{bad} should be rejected");
        }
    }

    #[test]
    fn numeric_and_length_bounds_are_enforced() {
        assert!(validate("t", &schema(), &json!({ "module": "x", "frame": -1 })).is_err());
        assert!(validate("t", &schema(), &json!({ "module": "x", "frame": 256 })).is_err());
        assert!(validate("t", &schema(), &json!({ "module": "x", "signals": [] })).is_err());
    }

    #[test]
    fn non_object_arguments_are_rejected() {
        let e = validate("t", &schema(), &json!(["module"])).unwrap_err();
        assert!(e.message.contains("must be a JSON object"));
    }
}
