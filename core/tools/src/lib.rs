//! `aim-tools` — the typed tool registry the future agent will call through.
//!
//! Handoff §7 lists the tools an AI mechanic gets and §3 insists the agent
//! talks to an application-level registry rather than to serial ports. This
//! crate is that registry, built now, with no model attached.
//!
//! # What is here and what deliberately is not
//!
//! Here: a vendor-neutral schema for each READ tool, argument validation that
//! fails closed, and a dispatcher that runs a call through
//! [`aim_diagnostics::DiagnosticService`] and returns the §7 [`ToolResult`]
//! envelope.
//!
//! Not here: any model client, prompt, planner or loop. Nothing in this crate
//! knows whether the caller is Claude, a local Ollama model, the UI, or a
//! test. That is the point — when the agent runtime arrives it plugs in above
//! [`execute`], and the safety and recording guarantees below it do not move.
//!
//! # Two gates, not one
//!
//! A call is checked twice. This crate checks the *shape* of the arguments;
//! the [`aim_safety`] gate inside the service checks the *permission*. Neither
//! substitutes for the other: a perfectly-shaped `clear_dtcs` call is still
//! refused, and a malformed `read_pid` is rejected before the gate sees it.

#![warn(missing_docs)]

pub mod validate;

use aim_diagnostics::{capabilities, DiagnosticService};
use aim_safety::MAX_ENABLED_LEVEL;
use aim_types::{AimError, ErrorCode, PermissionLevel, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A tool as a model (or the UI) sees it.
///
/// The `parameters` field is ordinary JSON Schema, which is what every model
/// vendor's tool-calling API consumes. Keeping it vendor-neutral is what makes
/// the Anthropic/Ollama choice in handoff §20 a configuration decision rather
/// than a rewrite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name as the agent calls it, e.g. `read_dtcs`.
    pub name: String,
    /// What it does, written for a model to read.
    pub description: String,
    /// The [`aim_safety`] capability it executes under.
    pub capability: String,
    /// Permission level required.
    pub permission_level: PermissionLevel,
    /// JSON Schema for the arguments.
    pub parameters: Value,
    /// Whether this build will execute it. A disabled tool is still listed, so
    /// an agent can be told "this exists and is refused" rather than being left
    /// to infer it from a failure.
    pub enabled: bool,
    /// What a successful call returns, described for a model.
    pub returns: String,
}

impl ToolSchema {
    fn new(
        name: &str,
        capability: &str,
        level: PermissionLevel,
        description: &str,
        returns: &str,
        parameters: Value,
    ) -> Self {
        ToolSchema {
            name: name.to_string(),
            description: description.to_string(),
            capability: capability.to_string(),
            permission_level: level,
            parameters,
            enabled: level <= MAX_ENABLED_LEVEL,
            returns: returns.to_string(),
        }
    }
}

/// An object schema with no properties.
fn no_args() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn module_arg(required: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "module": {
                "type": "string",
                "description": "Module key from scan_modules, e.g. \"ECU_7E8\"."
            }
        },
        "additionalProperties": false
    });
    if required {
        schema["required"] = json!(["module"]);
    }
    schema
}

/// The tools this build exposes.
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolSchema>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        ToolRegistry::phase1()
    }
}

impl ToolRegistry {
    /// The READ tools of handoff §7, plus the write tool that is registered
    /// specifically so its refusal is explicit.
    pub fn phase1() -> ToolRegistry {
        let mut tools = BTreeMap::new();
        for t in [
            ToolSchema::new(
                "identify_vehicle",
                capabilities::IDENTIFY_VEHICLE,
                PermissionLevel::L0,
                "Read the vehicle identification number and calibration identifiers from the \
                 vehicle. Decodes only the fields the VIN standard actually encodes; model, trim \
                 and engine are not inferred.",
                "The VIN, its structural decoding, and any calibration identifiers reported.",
                no_args(),
            ),
            ToolSchema::new(
                "scan_modules",
                capabilities::SCAN_MODULES,
                PermissionLevel::L0,
                "Discover which control modules answer on the diagnostic bus. Modules are named \
                 by what they report about themselves, or by their address when they report \
                 nothing.",
                "A list of modules with their keys, addresses and protocol.",
                no_args(),
            ),
            ToolSchema::new(
                "get_module_identity",
                capabilities::MODULE_IDENTITY,
                PermissionLevel::L0,
                "Read one module's identity: ECU name, calibration identifiers and calibration \
                 verification numbers.",
                "The module record with its identity fields filled in.",
                module_arg(true),
            ),
            ToolSchema::new(
                "read_supported_pids",
                capabilities::READ_SUPPORTED_PIDS,
                PermissionLevel::L0,
                "Enumerate the live-data parameters a module supports, and say which of them \
                 this build can decode.",
                "Supported PID numbers with signal ids, units and decoder availability.",
                module_arg(true),
            ),
            ToolSchema::new(
                "read_monitor_tests",
                capabilities::READ_MONITOR_TESTS,
                PermissionLevel::L0,
                "Read the vehicle's own emissions self-test results (OBD service 06): for each \
                 monitor, the value it measured next to the limit it is judged by. Use this to \
                 find components that are still passing but close to their limit — a converter \
                 or sensor that is degrading shows up here months before it sets a trouble code, \
                 which makes this the single most useful check on a vehicle you are thinking of \
                 buying. Every result's pass or fail verdict is exact; the scaled numbers use an \
                 unverified unit table and are marked as such.",
                "Per-monitor results with pass/fail, how close each is to its limit, and counts \
                 of failing and marginal monitors. Vehicles that do not implement service 06 \
                 report supported=false rather than an error.",
                module_arg(true),
            ),
            ToolSchema::new(
                "list_vehicle_features",
                capabilities::LIST_FEATURES,
                PermissionLevel::L0,
                "List the settings this vehicle could have configured — things like automatic \
                 folding mirrors or a startup chime, which are switched on and off in a module \
                 rather than repaired. Use this when the user asks to change how the vehicle \
                 behaves rather than to diagnose a fault. Each entry says how far this build \
                 can actually go with it, which is often 'explain it and nothing more'.",
                "Feature ids with plain-language descriptions, risk class, owning modules, and \
                 whether this build can read or change each one.",
                no_args(),
            ),
            ToolSchema::new(
                "preview_configuration_change",
                capabilities::PREVIEW_CHANGE,
                PermissionLevel::L0,
                "Work out what changing one setting would involve, WITHOUT changing anything. \
                 Returns every check that has to pass and whether it does, so you can tell the \
                 user precisely what stands in the way — a wrong adapter, a flat battery, or a \
                 setting this build refuses to touch. You may only name a feature id from \
                 `list_vehicle_features`. You cannot name a module, an address, or a byte: \
                 where a setting lives comes from verified data, never from you.",
                "Every check with its answer, and whether the change could go ahead.",
                json!({
                    "type": "object",
                    "properties": {
                        "feature_id": {
                            "type": "string",
                            "description": "A feature id from list_vehicle_features.",
                        },
                        "desired": {
                            "type": "string",
                            "enum": ["on", "off"],
                            "description": "What the setting should be changed to.",
                        },
                    },
                    "required": ["feature_id", "desired"],
                    // Load-bearing, not boilerplate: this is what stops a model
                    // attaching a `module` or `address` field to the call and
                    // hoping something downstream reads it.
                    "additionalProperties": false,
                }),
            ),
            ToolSchema::new(
                "read_dtcs",
                capabilities::READ_DTCS,
                PermissionLevel::L0,
                "Read stored, pending and permanent diagnostic trouble codes. Descriptions come \
                 from a catalog; a code that is not in it is returned with its structural \
                 decoding and no description rather than an invented one.",
                "Decoded trouble codes with status, module, description and verification status.",
                module_arg(false),
            ),
            ToolSchema::new(
                "read_freeze_frame",
                capabilities::READ_FREEZE_FRAME,
                PermissionLevel::L0,
                "Read the stored snapshot of conditions captured when a fault was recorded. This \
                 is historical data, not a live reading.",
                "The causing trouble code and the frozen parameter values.",
                json!({
                    "type": "object",
                    "properties": {
                        "module": {
                            "type": "string",
                            "description": "Module key from scan_modules, e.g. \"ECU_7E8\"."
                        },
                        "frame": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 255,
                            "description": "Freeze frame number. Frame 0 is the standard one."
                        }
                    },
                    "required": ["module"],
                    "additionalProperties": false
                }),
            ),
            ToolSchema::new(
                "read_pid",
                capabilities::READ_PID,
                PermissionLevel::L0,
                "Read one live parameter from a module, by signal id (\"engine_rpm\") or by PID \
                 number (\"0x0C\").",
                "One or more decoded values, each carrying the raw bytes it came from.",
                json!({
                    "type": "object",
                    "properties": {
                        "module": {
                            "type": "string",
                            "description": "Module key from scan_modules, e.g. \"ECU_7E8\"."
                        },
                        "signal": {
                            "type": "string",
                            "description": "Signal id such as \"coolant_temp\", or a PID like \"0x05\"."
                        }
                    },
                    "required": ["module", "signal"],
                    "additionalProperties": false
                }),
            ),
            ToolSchema::new(
                "read_live_data",
                capabilities::READ_LIVE_DATA,
                PermissionLevel::L0,
                "Read several live parameters from a module as one sample. Signals that cannot \
                 be read are reported as warnings rather than dropped silently.",
                "Decoded values for the signals that could be read, plus warnings for those that \
                 could not.",
                json!({
                    "type": "object",
                    "properties": {
                        "module": {
                            "type": "string",
                            "description": "Module key from scan_modules, e.g. \"ECU_7E8\"."
                        },
                        "signals": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "description": "Signal ids or PID numbers to sample."
                        }
                    },
                    "required": ["module", "signals"],
                    "additionalProperties": false
                }),
            ),
            ToolSchema::new(
                "adapter_health",
                capabilities::HEALTH,
                PermissionLevel::L0,
                "Read the adapter link's health: state, request and failure counts, latency, \
                 measured capabilities and their caveats.",
                "Adapter health counters and the observed capability snapshot.",
                no_args(),
            ),
            // Registered, described, and refused *to the model*.
            //
            // A person can clear codes in the app — it is the one write every
            // code reader performs, and refusing it outright made this tool
            // less capable than a twenty-pound scanner. The model cannot, and
            // that asymmetry is deliberate rather than an oversight: clearing
            // erases the readiness monitors, which are the evidence a used-car
            // buyer most needs and which take 50 to 100 miles of driving to
            // rebuild. That is a decision for the person holding the keys, made
            // in front of a dialog that explains the cost, not something an
            // agent talks itself into partway through an inspection.
            //
            // Kept in the registry at L2 so a model that asks gets a specific
            // "this is disabled for you" answer rather than an "unknown tool"
            // that invites a workaround.
            ToolSchema::new(
                "clear_dtcs",
                capabilities::CLEAR_DTCS,
                PermissionLevel::L2,
                "Clear stored trouble codes, freeze frames and readiness monitors. NOT available \
                 to you. If the user wants codes cleared, tell them the Codes screen has a Clear \
                 button, and tell them what it costs: the readiness monitors reset, which fails \
                 an emissions test until the vehicle has been driven 50 to 100 miles, and the \
                 freeze frames that explain why a code was stored are destroyed. Clearing a code \
                 does not repair anything; if the fault is still present the code returns.",
                "Nothing. This tool is refused before any request reaches the vehicle.",
                module_arg(false),
            ),
        ] {
            tools.insert(t.name.clone(), t);
        }
        ToolRegistry { tools }
    }

    /// Look up a tool.
    pub fn get(&self, name: &str) -> Option<&ToolSchema> {
        self.tools.get(name)
    }

    /// Every registered tool, name order.
    pub fn all(&self) -> Vec<&ToolSchema> {
        self.tools.values().collect()
    }

    /// Only the tools this build will execute — the list to hand a model.
    pub fn enabled(&self) -> Vec<&ToolSchema> {
        self.tools.values().filter(|t| t.enabled).collect()
    }
}

/// A request to run a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name.
    pub tool: String,
    /// Arguments, validated against the tool's schema before anything runs.
    #[serde(default)]
    pub arguments: Value,
    /// Who is asking. Recorded with every operation; handoff §10 makes this
    /// mandatory rather than optional.
    pub initiator: String,
    /// Username confirming an operation that requires confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<String>,
}

impl ToolCall {
    /// A call with no arguments.
    pub fn new(tool: impl Into<String>, initiator: impl Into<String>) -> Self {
        ToolCall {
            tool: tool.into(),
            arguments: json!({}),
            initiator: initiator.into(),
            confirmation: None,
        }
    }

    /// Attach arguments.
    pub fn with_arguments(mut self, arguments: Value) -> Self {
        self.arguments = arguments;
        self
    }

    /// Attach a human confirmation.
    pub fn confirmed_by(mut self, user: impl Into<String>) -> Self {
        self.confirmation = Some(user.into());
        self
    }
}

/// Run a tool call against the diagnostic core.
///
/// Always returns a [`ToolResult`]; a failure is an envelope with
/// `success: false` and a structured error, because that is what the §7
/// contract says a tool returns. The caller never has to distinguish "the tool
/// failed" from "the call failed".
pub fn execute(
    service: &mut DiagnosticService,
    registry: &ToolRegistry,
    call: &ToolCall,
) -> ToolResult {
    let session = service.session_id().clone();

    // Fail closed on an unknown name, before anything else happens. An agent
    // that hallucinates a tool must not find out by having something adjacent
    // executed on its behalf.
    let Some(schema) = registry.get(&call.tool) else {
        let known: Vec<&str> = registry.all().iter().map(|t| t.name.as_str()).collect();
        return ToolResult::failure(
            call.tool.clone(),
            session,
            "unknown",
            0,
            AimError::new(
                ErrorCode::OperationNotAllowed,
                format!("{:?} is not a registered tool", call.tool),
            )
            .with_details(json!({ "requested": call.tool, "available": known })),
        );
    };

    if call.initiator.trim().is_empty() {
        return ToolResult::failure(
            call.tool.clone(),
            session,
            schema.capability.clone(),
            0,
            AimError::bad_request("every tool call must name an initiator"),
        );
    }

    // A disabled tool is refused here, before the safety gate is consulted.
    //
    // This used to be implicit: every disabled tool also mapped to a capability
    // the gate refused, so nothing checked the tool's own level. That coupling
    // broke the moment `clear_dtcs` became something a *person* may do — the
    // capability moved to L1, and with only the gate enforcing, the tool
    // dispatch would have let an agent do it too.
    //
    // The two levels answer different questions and are allowed to differ. The
    // capability says what the product permits; the tool schema says what a
    // model may ask for. Where they disagree, the stricter one wins.
    if !schema.enabled {
        // Recorded before returning: a refusal that leaves no trace is exactly
        // the gap the flight recorder exists to close.
        service.record_refusal(
            &call.tool,
            &schema.capability,
            &call.initiator,
            "permission_level_disabled",
            call.arguments.clone(),
        );
        return ToolResult::failure(
            call.tool.clone(),
            session,
            schema.capability.clone(),
            0,
            AimError::new(
                ErrorCode::PermissionLevelDisabled,
                format!(
                    "{} is not available through the tool interface in this build",
                    call.tool
                ),
            )
            .with_details(serde_json::json!({
                "tool": call.tool,
                "tool_level": schema.permission_level,
                "max_enabled_level": MAX_ENABLED_LEVEL,
            })),
        );
    }

    if let Err(e) = validate::validate(&call.tool, &schema.parameters, &call.arguments) {
        return ToolResult::failure(
            call.tool.clone(),
            session,
            schema.capability.clone(),
            0,
            e,
        );
    }

    let module = call.arguments.get("module").and_then(Value::as_str);
    let initiator = call.initiator.as_str();

    match call.tool.as_str() {
        "identify_vehicle" => service.identify_vehicle(initiator),
        "scan_modules" => service.scan_modules(initiator),
        "adapter_health" => service.adapter_health(initiator),
        "get_module_identity" => {
            service.get_module_identity(module.unwrap_or_default(), initiator)
        }
        "read_supported_pids" => {
            service.read_supported_pids(module.unwrap_or_default(), initiator)
        }
        "read_monitor_tests" => {
            service.read_monitor_tests(module.unwrap_or_default(), initiator)
        }
        "list_vehicle_features" => service.list_features(initiator),
        "preview_configuration_change" => {
            // The model supplies a feature id and a value. Nothing else it
            // could say would be accepted here, which is the point: there is no
            // parameter on this call that carries a module, an address or a
            // byte offset.
            let feature = call
                .arguments
                .get("feature_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let desired = match call.arguments.get("desired").and_then(Value::as_str) {
                Some("off") => aim_diagnostics::DesiredValue::Off,
                _ => aim_diagnostics::DesiredValue::On,
            };
            service.preview_configuration_change(feature, desired, initiator)
        }
        "read_dtcs" => service.read_dtcs(module, initiator),
        "read_freeze_frame" => {
            let frame = call
                .arguments
                .get("frame")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u8;
            service.read_freeze_frame(module.unwrap_or_default(), frame, initiator)
        }
        "read_pid" => {
            let signal = call
                .arguments
                .get("signal")
                .and_then(Value::as_str)
                .unwrap_or_default();
            service.read_pid(module.unwrap_or_default(), signal, initiator)
        }
        "read_live_data" => {
            let signals: Vec<String> = call
                .arguments
                .get("signals")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            service.read_live_data(module.unwrap_or_default(), &signals, initiator)
        }
        "clear_dtcs" => service.clear_dtcs(module, initiator, call.confirmation.as_deref()),
        // A tool that is registered but has no dispatch arm is a bug in this
        // crate, and it fails closed like everything else.
        other => ToolResult::failure(
            other.to_string(),
            session,
            schema.capability.clone(),
            0,
            AimError::new(
                ErrorCode::NotImplemented,
                format!("tool {other:?} is registered but not wired to the diagnostic core"),
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_safety::CapabilityRegistry;

    #[test]
    fn every_read_tool_from_the_handoff_is_registered() {
        let r = ToolRegistry::phase1();
        for name in [
            "identify_vehicle",
            "scan_modules",
            "get_module_identity",
            "read_dtcs",
            "read_freeze_frame",
            "read_pid",
            "read_live_data",
            "read_supported_pids",
        ] {
            assert!(r.get(name).is_some(), "{name} is missing from the registry");
            assert!(r.get(name).unwrap().enabled, "{name} should be enabled");
        }
    }

    #[test]
    fn no_tool_is_less_restricted_than_the_capability_behind_it() {
        // These two levels answer different questions and are allowed to
        // differ: the capability says what the *product* permits, the tool
        // schema says what a *model* may ask for. `clear_dtcs` is the case that
        // forced the distinction — a person may clear codes, an agent may not.
        //
        // What must never happen is the reverse. A tool more permissive than
        // its capability would be a model asking for something the product has
        // decided needs more care, and relying on a downstream check to notice.
        let tools = ToolRegistry::phase1();
        let caps = CapabilityRegistry::phase1();
        for t in tools.all() {
            let cap = caps.get(&t.capability).unwrap_or_else(|| {
                panic!("tool {} names unregistered capability {}", t.name, t.capability)
            });
            assert!(
                t.permission_level >= cap.level,
                "tool {} is less restricted ({:?}) than its capability ({:?})",
                t.name,
                t.permission_level,
                cap.level
            );
        }
    }

    #[test]
    fn clearing_codes_is_open_to_a_person_and_closed_to_the_agent() {
        let tools = ToolRegistry::phase1();
        let caps = CapabilityRegistry::phase1();
        let tool = tools.get("clear_dtcs").unwrap();
        let cap = caps.get("obd2.clear_dtcs").unwrap();

        assert!(cap.level <= MAX_ENABLED_LEVEL, "a person can clear codes");
        assert!(!tool.enabled, "the agent cannot");
        assert!(
            !tools.enabled().iter().any(|t| t.name == "clear_dtcs"),
            "and it is absent from the list a model is handed"
        );
    }

    #[test]
    fn write_tools_are_listed_but_disabled() {
        let r = ToolRegistry::phase1();
        let clear = r.get("clear_dtcs").unwrap();
        assert!(!clear.enabled);
        assert_eq!(clear.permission_level, PermissionLevel::L2);
        assert!(!r.enabled().iter().any(|t| t.name == "clear_dtcs"));
        // But it is still discoverable, so a refusal can be specific.
        assert!(r.all().iter().any(|t| t.name == "clear_dtcs"));
    }

    #[test]
    fn every_schema_is_a_closed_json_schema_object() {
        for t in ToolRegistry::phase1().all() {
            assert_eq!(t.parameters["type"], "object", "{}", t.name);
            assert!(t.parameters.get("properties").is_some(), "{}", t.name);
            assert_eq!(
                t.parameters["additionalProperties"], false,
                "{} must reject unknown arguments",
                t.name
            );
            assert!(!t.description.is_empty());
            assert!(!t.returns.is_empty());
        }
    }

    #[test]
    fn schemas_serialize_to_the_shape_a_model_vendor_expects() {
        let t = ToolRegistry::phase1().get("read_pid").unwrap().clone();
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["name"], "read_pid");
        assert_eq!(v["permission_level"], "L0");
        assert_eq!(v["parameters"]["required"], json!(["module", "signal"]));
        assert_eq!(
            v["parameters"]["properties"]["signal"]["type"],
            "string"
        );
    }

    #[test]
    fn tool_calls_round_trip_through_json() {
        let call = ToolCall::new("read_live_data", "agent:planner")
            .with_arguments(json!({ "module": "ECU_7E8", "signals": ["engine_rpm"] }));
        let text = serde_json::to_string(&call).unwrap();
        assert_eq!(serde_json::from_str::<ToolCall>(&text).unwrap(), call);
        // Confirmation is omitted when absent rather than serialized as null.
        assert!(!text.contains("confirmation"));
    }
}
