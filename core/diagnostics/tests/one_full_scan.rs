//! A full scan is done once and used everywhere.
//!
//! The Full scan screen and the assistant's inspection each used to run their
//! own, a minute or two apart, on the same parked vehicle. The owner's
//! question after watching both: if one has just scanned everything, why does
//! the other scan it again?

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_types::EventKind;
use std::sync::Arc;

const PERSON: &str = "user:api";
const ASSISTANT: &str = "agent:agent";

fn connected() -> DiagnosticService {
    let transport = SimulatedTransport::new(ScenarioId::DpfRegen);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let mut service = DiagnosticService::start(
        adapter,
        SessionStore::open_in_memory().unwrap(),
        Arc::new(DecoderSet::generic_obd().unwrap()),
        SafetyGate::phase1(),
        None,
    )
    .unwrap();
    assert!(service.connect(PERSON).success);
    assert!(service.identify_vehicle(PERSON).success);
    assert!(service.scan_modules(PERSON).success);
    service
}

/// How many requests the session has sent to the adapter so far.
fn asked(service: &DiagnosticService) -> usize {
    service
        .store()
        .events_since(service.session_id(), 0, 100_000)
        .unwrap()
        .iter()
        .filter(|e| matches!(e.kind, EventKind::AdapterRequest { .. }))
        .count()
}

fn codes(warnings: &[aim_types::Warning]) -> Vec<&str> {
    warnings.iter().map(|w| w.code.as_str()).collect()
}

#[test]
fn the_assistant_is_handed_the_scan_a_person_just_ran() {
    let mut service = connected();
    assert!(service.last_full_scan().is_none());

    let first = service.scan_all_modules(PERSON);
    assert!(first.success, "{:?}", first.error);
    let after_first = asked(&service);

    let reused = service.scan_all_modules(ASSISTANT);
    assert!(reused.success);
    assert_eq!(asked(&service), after_first, "the vehicle was not asked again");
    assert_eq!(reused.data, first.data, "the same scan, whole");
    assert!(codes(&reused.warnings).contains(&"full_scan_reused"), "and it says so");
    // Everything the first scan warned about is still said.
    for code in codes(&first.warnings) {
        assert!(codes(&reused.warnings).contains(&code), "{code} was dropped");
    }
    // The flight recorder still has the call: asked for, answered, by whom.
    let events = service.store().events_since(service.session_id(), 0, 100_000).unwrap();
    let calls = events
        .iter()
        .filter(|e| {
            matches!(&e.kind, EventKind::ToolInvoked { tool, .. } if tool == "scan_all_modules")
        })
        .count();
    assert_eq!(calls, 2);
}

#[test]
fn the_scan_the_assistant_ran_is_there_for_the_screen() {
    let mut service = connected();

    let scan = service.scan_all_modules(ASSISTANT);
    assert!(scan.success, "{:?}", scan.error);

    let kept = service.last_full_scan().expect("the scan is kept");
    assert_eq!(kept.result.data, scan.data);
    assert!(kept.by.starts_with("agent"));
    // And a second inspection in the same sitting reads it too.
    let before = asked(&service);
    assert!(service.scan_all_modules(ASSISTANT).success);
    assert_eq!(asked(&service), before);
}

/// Pressing the button is asking for a new scan.
#[test]
fn a_person_who_asks_for_a_scan_gets_a_new_one() {
    let mut service = connected();
    assert!(service.scan_all_modules(ASSISTANT).success);
    let before = asked(&service);

    let fresh = service.scan_all_modules(PERSON);
    assert!(fresh.success);
    assert!(asked(&service) > before, "the vehicle was asked again");
    assert!(!codes(&fresh.warnings).contains(&"full_scan_reused"));
    assert!(service.last_full_scan().unwrap().by.starts_with("user"));
}

/// Clearing codes is the one thing here that changes what a scan would find.
#[test]
fn a_scan_from_before_codes_were_cleared_is_not_handed_on() {
    let mut service = connected();
    assert!(service.scan_all_modules(PERSON).success);

    // Refused or not, an attempt to clear ends the scan's standing.
    let _ = service.clear_dtcs(None, PERSON, Some("CLEAR"));
    assert!(service.last_full_scan().is_none());

    let before = asked(&service);
    let again = service.scan_all_modules(ASSISTANT);
    assert!(again.success, "{:?}", again.error);
    assert!(asked(&service) > before, "a new scan was run");
    assert!(!codes(&again.warnings).contains(&"full_scan_reused"));
}

/// A scan that failed is not a scan to hand anybody.
#[test]
fn a_failed_scan_is_not_kept() {
    let transport = SimulatedTransport::new(ScenarioId::BusSilent);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let mut service = DiagnosticService::start(
        adapter,
        SessionStore::open_in_memory().unwrap(),
        Arc::new(DecoderSet::generic_obd().unwrap()),
        SafetyGate::phase1(),
        None,
    )
    .unwrap();
    let _ = service.connect(PERSON);

    let scan = service.scan_all_modules(PERSON);
    assert!(!scan.success);
    assert!(service.last_full_scan().is_none());
}
