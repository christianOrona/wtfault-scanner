//! What the 2019 F-250's recorded session says when replayed through the
//! current core, beyond the scorecard counts the baseline test compares.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ReplayMode, ReplayTransport, Transcript};
use std::sync::Arc;

const USER: &str = "user:test";

fn full_scan() -> aim_types::ToolResult {
    let text = include_str!("replays/2019-f250-full-scan.transcript");
    let transport =
        ReplayTransport::new(Transcript::parse(text).unwrap(), ReplayMode::Lookup, "f250");
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
    assert!(service.connect(USER).success);
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);
    let scan = service.scan_all_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    scan
}

/// The truck's body modules are on its second bus, 29 of them by hand on
/// 2026-09-11 and by the app on 2026-09-28. A replay that answered by address
/// alone put them on the primary bus and could not reach the second at all.
#[test]
fn the_body_modules_are_found_on_the_second_bus() {
    let scan = full_scan();
    let buses = scan.data.as_ref().unwrap()["buses"].as_array().unwrap().clone();
    let second = buses.iter().find(|b| b["bus"] == "BUS2").expect("the second bus was swept");
    assert_eq!(second["reached"], true, "{second}");
    assert_eq!(second["modules"], 29, "{second}");
    assert!(!scan.warnings.iter().any(|w| w.code == "bus_unreachable"), "{:?}", scan.warnings);
}

/// That session never asked a module for its faults, so none answered. Zero
/// faults is then unknown, and the scan must say so rather than all clear.
#[test]
fn a_scan_that_read_no_fault_list_says_so() {
    let scan = full_scan();
    let warned: Vec<&str> = scan.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(warned.contains(&"no_fault_lists_read"), "{warned:?}");
    assert!(!warned.contains(&"modules_without_fault_memory"), "{warned:?}");
}
