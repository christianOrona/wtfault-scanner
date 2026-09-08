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
    execute(
        service,
        &registry,
        &ToolCall::new(tool, AGENT).with_arguments(arguments),
    )
}

#[test]
fn a_read_only_diagnostic_sequence_runs_entirely_through_the_registry() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);

    let vin = call(&mut service, "identify_vehicle", json!({}));
    assert!(vin.success, "{:?}", vin.error);
    assert_eq!(vin.data.unwrap()["vin"], aim_simulator::SIMULATED_VIN);

    let scan = call(&mut service, "scan_modules", json!({}));
    assert!(scan.success);

    let pids = call(
        &mut service,
        "read_supported_pids",
        json!({ "module": "ECU_7E8" }),
    );
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
    let result = execute(
        &mut service,
        &registry,
        &ToolCall::new("reflash_ecu", AGENT),
    );
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
        ("read_pid", json!({ "module": "ECU_7E8" })),          // missing signal
        ("read_pid", json!({ "modul": "ECU_7E8", "signal": "engine_rpm" })), // typo
        ("read_live_data", json!({ "module": "ECU_7E8", "signals": [] })),   // empty
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
    assert_eq!(
        result.error.unwrap().code,
        ErrorCode::PermissionLevelDisabled
    );
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

    let events = service
        .store()
        .events_since(service.session_id(), 0, 10_000)
        .unwrap();
    let invoked: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            aim_types::EventKind::ToolInvoked {
                tool, initiator, ..
            } if initiator == AGENT => Some(tool.as_str()),
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
    let events = service
        .store()
        .events_since(service.session_id(), 0, 500)
        .unwrap();
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            EventKind::SafetyDecision { operation, allowed, initiator, .. }
                if operation == "obd2.clear_dtcs" && !*allowed && initiator == "agent:planner"
        )),
        "the refusal must be recorded"
    );
}
