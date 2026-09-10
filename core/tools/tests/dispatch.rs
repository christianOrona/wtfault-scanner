//! Tool dispatch against the virtual vehicle.
//!
//! This is the path the future agent runtime will take. It is tested now, with
//! no model attached, so that adding one is a change of caller and nothing
//! else.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_tools::{execute, ToolCall, ToolRegistry};
use aim_types::{ErrorCode, EventKind};
use serde_json::json;
use std::sync::Arc;

const AGENT: &str = "agent:planner";

fn connected(scenario: ScenarioId) -> (DiagnosticService, aim_simulator::SharedEmulator) {
    let transport = SimulatedTransport::new(scenario);
    let emulator = transport.emulator();
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    assert!(service.connect("user:test").success);
    (service, emulator)
}

fn call(
    service: &mut DiagnosticService,
    tool: &str,
    arguments: serde_json::Value,
) -> aim_types::ToolResult {
    let registry = ToolRegistry::phase1();
    execute(service, &registry, &ToolCall::new(tool, AGENT).with_arguments(arguments))
}

#[test]
fn a_read_only_diagnostic_sequence_runs_entirely_through_the_registry() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);

    let vin = call(&mut service, "identify_vehicle", json!({}));
    assert!(vin.success, "{:?}", vin.error);
    assert_eq!(vin.data.unwrap()["vin"], aim_simulator::SIMULATED_VIN);

    let scan = call(&mut service, "scan_modules", json!({}));
    assert!(scan.success);

    let pids = call(&mut service, "read_supported_pids", json!({ "module": "ECU_7E8" }));
    assert!(pids.success);

    let dtcs = call(&mut service, "read_dtcs", json!({ "module": "ECU_7E8" }));
    assert!(dtcs.success);
    assert_eq!(dtcs.data.unwrap()["confirmed_count"], 2);

    let live = call(
        &mut service,
        "read_live_data",
        json!({ "module": "ECU_7E8", "signals": ["engine_rpm", "coolant_temp"] }),
    );
    assert!(live.success);
    assert_eq!(live.values.len(), 2);

    // The envelope names the capability it ran under and the session it
    // belongs to, so a result is attributable after the fact.
    assert_eq!(live.capability_used, "obd2.read_live_data");
    assert_eq!(live.vehicle_session_id, *service.session_id());
}

#[test]
fn an_unknown_tool_is_refused_and_the_caller_is_told_what_exists() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let registry = ToolRegistry::phase1();
    let result = execute(&mut service, &registry, &ToolCall::new("reflash_ecu", AGENT));
    assert!(!result.success);
    let error = result.error.unwrap();
    assert_eq!(error.code, ErrorCode::OperationNotAllowed);
    let available = error.details.unwrap()["available"].clone();
    assert!(available.as_array().unwrap().iter().any(|t| t == "read_dtcs"));
}

#[test]
fn malformed_arguments_are_rejected_before_the_vehicle_is_touched() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    call(&mut service, "scan_modules", json!({}));
    let before = service.health().requests;

    for (tool, args) in [
        ("read_pid", json!({ "module": "ECU_7E8" })), // missing signal
        ("read_pid", json!({ "modul": "ECU_7E8", "signal": "engine_rpm" })), // typo
        ("read_live_data", json!({ "module": "ECU_7E8", "signals": [] })), // empty
        ("read_live_data", json!({ "module": "ECU_7E8", "signals": "rpm" })), // wrong type
        ("read_freeze_frame", json!({ "module": "ECU_7E8", "frame": 999 })), // out of range
    ] {
        let r = call(&mut service, tool, args.clone());
        assert!(!r.success, "{tool} {args} should have been rejected");
        assert_eq!(r.error.unwrap().code, ErrorCode::BadRequest);
    }

    assert_eq!(
        service.health().requests,
        before,
        "a malformed call must not put a single request on the bus"
    );
}

#[test]
fn a_call_with_no_initiator_is_refused() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let registry = ToolRegistry::phase1();
    let result = execute(&mut service, &registry, &ToolCall::new("scan_modules", "  "));
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::BadRequest);
}

#[test]
fn the_write_tool_is_refused_through_the_registry_too() {
    let (mut service, emulator) = connected(ScenarioId::DpfRegen);
    call(&mut service, "scan_modules", json!({}));
    let registry = ToolRegistry::phase1();

    // Even a well-formed, confirmed call from a named initiator.
    let result = execute(
        &mut service,
        &registry,
        &ToolCall::new("clear_dtcs", AGENT)
            .with_arguments(json!({ "module": "ECU_7E8" }))
            .confirmed_by("the-owner"),
    );
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::PermissionLevelDisabled);
    assert!(!emulator.lock().unwrap().vehicle.dtcs_cleared);
}

#[test]
fn the_enabled_tool_list_is_what_a_model_would_be_handed() {
    let registry = ToolRegistry::phase1();
    let enabled: Vec<&str> = registry.enabled().iter().map(|t| t.name.as_str()).collect();
    assert!(enabled.contains(&"read_dtcs"));
    assert!(!enabled.contains(&"clear_dtcs"));

    // The whole list serializes to plain JSON Schema, with no vendor-specific
    // wrapper, which is what keeps the Anthropic/Ollama choice a config knob.
    let json = serde_json::to_value(registry.enabled()).unwrap();
    for tool in json.as_array().unwrap() {
        assert!(tool["name"].is_string());
        assert_eq!(tool["parameters"]["type"], "object");
        assert_eq!(tool["enabled"], true);
    }
}

#[test]
fn every_dispatched_tool_is_recorded_in_the_flight_recorder() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    call(&mut service, "scan_modules", json!({}));

    let events = service.store().events_since(service.session_id(), 0, 10_000).unwrap();
    let invoked: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            aim_types::EventKind::ToolInvoked { tool, initiator, .. } if initiator == AGENT => {
                Some(tool.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(invoked.contains(&"scan_modules"));
}

#[test]
fn an_agent_cannot_clear_codes_even_though_a_person_can() {
    // The asymmetry, pinned. Clearing codes is enabled as a *capability* — a
    // person may do it, in front of a dialog that says what it destroys — and
    // disabled as a *tool*, so the model that speaks through this interface may
    // not. Those two levels are allowed to differ and the stricter wins.
    //
    // This is not hypothetical caution. The capability moved from L2 to L1 to
    // give people the button every cheap code reader has, and the tool
    // interface had been relying on the gate to refuse it. It no longer does.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules("user:test");

    let registry = ToolRegistry::phase1();
    let result = execute(
        &mut service,
        &registry,
        &ToolCall::new("clear_dtcs", AGENT)
            .with_arguments(json!({ "module": "ECU_7E8" }))
            // Even with a confirmation the model made up for itself.
            .confirmed_by("the-owner"),
    );

    assert!(!result.success);
    assert_eq!(
        result.error.unwrap().code,
        ErrorCode::PermissionLevelDisabled,
        "the model must not reach service 04 whatever it confirms"
    );

    // And the refusal is in the flight recorder. A refusal that leaves no trace
    // is the one shape an audit log must never have.
    let events = service.store().events_since(service.session_id(), 0, 500).unwrap();
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            EventKind::SafetyDecision { operation, allowed, initiator, .. }
                if operation == "obd2.clear_dtcs" && !*allowed && initiator == "agent:planner"
        )),
        "the refusal must be recorded"
    );
}

// ------------------------------------------------- adversarial model output
//
// These assert on the property rather than on a list of names, so a tool added
// later is covered without anybody remembering this file exists.

/// No tool a model is offered can carry raw bus data.
///
/// This is the structural half of "the model never generates arbitrary CAN or
/// UDS": not a check that refuses such a request, but a set of schemas with
/// nowhere to express one.
///
/// The property is about *payloads*, not about every parameter that sounds
/// low-level. `read_freeze_frame` takes a frame number, which is a bounded
/// index into something the vehicle already stored - it selects a record and
/// cannot become bus content. What must not exist is a parameter a model can
/// fill with bytes.
#[test]
fn no_tool_offered_to_a_model_accepts_raw_bus_data() {
    let registry = aim_tools::ToolRegistry::phase1();
    for tool in registry.enabled() {
        let props = tool.parameters["properties"].as_object().expect("object schema");
        for (name, spec) in props {
            assert!(
                !["bytes", "pdu", "raw", "data", "payload", "can_id", "header", "did"]
                    .contains(&name.as_str()),
                "{} exposes a payload parameter {name:?}",
                tool.name
            );
            // An integer parameter must be bounded. An unbounded one is an
            // address in waiting.
            if spec["type"] == "integer" {
                assert!(
                    spec.get("minimum").is_some() && spec.get("maximum").is_some(),
                    "{}.{name} is an unbounded integer: {spec}",
                    tool.name
                );
            }
        }
    }
}

/// Every schema is closed, so an unexpected argument is rejected rather than
/// ignored. An ignored argument is how a model learns a field exists.
#[test]
fn every_offered_schema_refuses_unexpected_arguments() {
    for tool in aim_tools::ToolRegistry::phase1().enabled() {
        assert_eq!(
            tool.parameters["additionalProperties"], false,
            "{} accepts unknown arguments",
            tool.name
        );
    }
}

/// A tool that does not exist is refused as unknown, not attempted.
#[test]
fn a_tool_that_does_not_exist_is_unknown_rather_than_best_effort() {
    let registry = aim_tools::ToolRegistry::phase1();
    for invented in ["write_can_frame", "send_uds", "program_module", "clear_everything"] {
        assert!(registry.get(invented).is_none(), "{invented} should not exist");
    }
}

/// Destructive operations stay out of the offered set whatever the build's
/// write ceiling is, and stay *listed* so a refusal can be specific.
#[test]
fn destructive_tools_are_listed_but_never_offered() {
    let registry = aim_tools::ToolRegistry::phase1();
    let offered: Vec<&str> = registry.enabled().iter().map(|t| t.name.as_str()).collect();
    assert!(!offered.contains(&"clear_dtcs"), "{offered:?}");
    assert!(registry.get("clear_dtcs").is_some(), "it must still be describable");
}
