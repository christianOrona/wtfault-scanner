//! The bridge between the model and the diagnostic core.
//!
//! The model never reaches the vehicle directly. It names a tool; something
//! implementing [`ToolExecutor`] validates that name against the registry, runs
//! it through the safety gate, and hands back the same `ToolResult` envelope the
//! UI gets. A name the registry does not know cannot be executed, and arguments
//! that fail the schema never reach the bus.
//!
//! [`ToolExecutor`] is a trait rather than a concrete type so this crate does
//! not depend on `aim-diagnostics` — that pulls in the serial transport, which
//! would make the agent untestable without hardware. `apps/api` supplies the
//! real implementation; the tests here supply a fake.

use crate::provider::ToolSpec;
use serde_json::Value;

/// Runs tool calls on the model's behalf.
///
/// Async because the real implementation reaches a vehicle behind a mutex on a
/// blocking thread; a synchronous trait would either block the runtime or force
/// the lock to be held across an await.
#[async_trait::async_trait]
pub trait ToolExecutor: Send {
    /// The tools the model may call, already filtered to what is enabled.
    ///
    /// Disabled operations are deliberately **not** offered. Describing
    /// `clear_dtcs` to a model and then refusing every call wastes a turn and
    /// invites it to keep trying; the prompt states the boundary in prose
    /// instead.
    fn specs(&self) -> Vec<ToolSpec>;

    /// Execute one call, returning the `ToolResult` envelope as JSON.
    ///
    /// Never fails: a refusal, a silent vehicle or a malformed argument all come
    /// back as an envelope with `success: false`, because the model has to be
    /// able to read what went wrong and adapt.
    async fn run(&mut self, name: &str, arguments: &Value, initiator: &str) -> Value;
}

/// Convert a registry entry into a provider-neutral tool spec.
///
/// The `returns` text is appended to the description: providers have no field
/// for it, and a model that does not know what comes back tends to call the
/// same tool twice to find out.
pub fn spec_from_schema(
    name: &str,
    description: &str,
    returns: &str,
    parameters: Value,
) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: if returns.trim().is_empty() {
            description.to_string()
        } else {
            format!("{description}\n\nReturns: {returns}")
        },
        input_schema: parameters,
    }
}

/// Trim a `ToolResult` down to what is useful to a model.
///
/// Full envelopes are large — a supported-PID scan is dozens of entries, each
/// with provenance — and a model that spends its context on raw byte fields has
/// less left for reasoning. What is kept is everything that changes a
/// conclusion: the values, their units, whether they are verified, whether they
/// are out of range, the warnings, and the evidence reference so a claim can
/// still be traced. What is dropped is duplication.
pub fn summarise_result(result: &Value) -> Value {
    let mut out = serde_json::Map::new();

    for key in ["tool", "success", "module", "raw_evidence_ref"] {
        if let Some(v) = result.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }

    if let Some(values) = result.get("values").and_then(Value::as_array) {
        let compact: Vec<Value> = values
            .iter()
            .map(|v| {
                let mut m = serde_json::Map::new();
                for key in ["signal_id", "name", "unit", "out_of_range"] {
                    if let Some(x) = v.get(key) {
                        m.insert(key.to_string(), x.clone());
                    }
                }
                if let Some(val) = v.get("value").and_then(|x| x.get("value")) {
                    m.insert("value".into(), val.clone());
                }
                // Verification is the difference between a measurement and a
                // number, so it survives the trim even though provenance does not.
                if let Some(ver) = v.get("provenance").and_then(|p| p.get("verification")) {
                    m.insert("verification".into(), ver.clone());
                }
                if let Some(r) = v.get("provenance").and_then(|p| p.get("evidence_ref")) {
                    m.insert("evidence_ref".into(), r.clone());
                }
                Value::Object(m)
            })
            .collect();
        if !compact.is_empty() {
            out.insert("values".into(), Value::Array(compact));
        }
    }

    if let Some(data) = result.get("data").filter(|d| !d.is_null()) {
        out.insert("data".into(), data.clone());
    }

    if let Some(w) = result.get("warnings").and_then(Value::as_array) {
        if !w.is_empty() {
            out.insert("warnings".into(), Value::Array(w.clone()));
        }
    }

    if let Some(e) = result.get("error").filter(|e| !e.is_null()) {
        out.insert("error".into(), e.clone());

        // Say whose fault it is.
        //
        // Measured against a 7B model: it sent `{"modules": [...]}` where the
        // schema wants `{"module": "..."}`, the call was correctly rejected,
        // and the model then reported to the buyer that "the adapter dropped
        // requests frequently". A rejected argument and a silent vehicle are
        // completely different facts and must not look alike.
        let code = e.get("code").and_then(Value::as_str).unwrap_or("");
        if matches!(
            code,
            "bad_request" | "decoder_input_invalid" | "operation_not_allowed" | "decoder_not_found"
        ) {
            out.insert(
                "hint".into(),
                Value::String(
                    "This is a mistake in the arguments you sent, NOT a fault with the vehicle \
                     or the adapter. Read the tool's schema, correct the call and try again. Do \
                     not report this to the user as a vehicle problem."
                        .into(),
                ),
            );
        }
    }

    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_summary_keeps_what_changes_a_conclusion() {
        let full = json!({
            "tool": "read_live_data",
            "success": true,
            "module": "ECU_7E8",
            "raw_evidence_ref": 84,
            "execution_time_ms": 8,
            "capability_used": "obd2.read_live_data",
            "values": [{
                "signal_id": "dpf_temp_bank1_inlet",
                "name": "DPF inlet temperature",
                "value": { "type": "number", "value": 392.9 },
                "unit": "degC",
                "valid_range": { "min": -40.0, "max": 1200.0 },
                "out_of_range": false,
                "provenance": {
                    "source": "decoder", "raw_hex": "0f10e9",
                    "decoder_id": "x", "decoder_version": "1",
                    "verification": "unverified", "evidence_ref": 84
                }
            }],
            "warnings": [{"code":"unverified_decoder","severity":"caution","message":"..."}],
            "error": null
        });

        let s = summarise_result(&full);
        assert_eq!(s["values"][0]["value"], 392.9);
        assert_eq!(s["values"][0]["unit"], "degC");
        // The model must be able to see that this number is not validated.
        assert_eq!(s["values"][0]["verification"], "unverified");
        assert_eq!(s["values"][0]["evidence_ref"], 84);
        assert_eq!(s["warnings"][0]["code"], "unverified_decoder");
        // Bulk that cannot change a conclusion is dropped.
        assert!(s["values"][0].get("valid_range").is_none());
        assert!(s.get("capability_used").is_none());
        // A null error is omitted rather than shown as a field.
        assert!(s.get("error").is_none());
    }

    #[test]
    fn failures_keep_their_error_and_evidence() {
        let failed = json!({
            "tool": "read_dtcs", "success": false, "raw_evidence_ref": 441,
            "values": [], "warnings": [],
            "error": { "code": "vehicle_not_responding", "message": "UNABLE TO CONNECT" }
        });
        let s = summarise_result(&failed);
        assert_eq!(s["success"], false);
        assert_eq!(s["error"]["code"], "vehicle_not_responding");
        assert_eq!(s["raw_evidence_ref"], 441);
    }

    #[test]
    fn returns_text_is_folded_into_the_description() {
        let s = spec_from_schema("read_dtcs", "Read codes.", "A list of DTCs.", json!({}));
        assert!(s.description.contains("Read codes."));
        assert!(s.description.contains("Returns: A list of DTCs."));
    }
}
