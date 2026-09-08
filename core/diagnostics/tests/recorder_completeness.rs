//! Every tool the agent can call must appear in the flight recorder.
//!
//! The recorder is meant to be the complete account of a session. One method,
//! `adapter_health`, reached the vehicle without logging an invocation, which
//! made it invisible to anything reading the log to see what the agent had
//! done — including the UI's own live progress display, which showed "Starting
//! up…" while the agent was already working.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_types::EventKind;
use std::sync::Arc;

const AGENT: &str = "agent:agent";

fn connected() -> DiagnosticService {
    let transport = SimulatedTransport::new(ScenarioId::DpfRegen);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    let r = service.connect(AGENT);
    assert!(r.success, "connect failed: {:?}", r.error);
    service
}

#[test]
fn every_read_tool_records_an_invocation() {
    let mut svc = connected();

    let _ = svc.adapter_health(AGENT);
    let _ = svc.identify_vehicle(AGENT);
    let _ = svc.scan_modules(AGENT);
    let _ = svc.read_dtcs(None, AGENT);

    let events = svc.store().events_since(svc.session_id(), 0, 10_000).unwrap();
    let invoked: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolInvoked { tool, .. } => Some(tool.clone()),
            _ => None,
        })
        .collect();

    for tool in ["adapter_health", "identify_vehicle", "scan_modules", "read_dtcs"] {
        assert!(
            invoked.iter().any(|t| t == tool),
            "{tool} reached the vehicle without being recorded as an invocation; \
             the flight recorder is meant to be complete. Recorded: {invoked:?}"
        );
    }
}

#[test]
fn an_invocation_names_who_asked_for_it() {
    // Handoff section 10 makes the initiator mandatory: a reading the agent
    // chose to take has to be distinguishable from one a person clicked.
    let mut svc = connected();
    let _ = svc.adapter_health(AGENT);

    let events = svc.store().events_since(svc.session_id(), 0, 10_000).unwrap();
    let found = events.iter().any(|e| match &e.kind {
        EventKind::ToolInvoked { tool, initiator, .. } => {
            tool == "adapter_health" && initiator == AGENT
        }
        _ => false,
    });
    assert!(found, "the invocation did not record who asked for it");
}
