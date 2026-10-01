//! What a vehicle of another make is spared: Ford's own ranges are not swept
//! on it, and it is told so rather than shown an empty result (#60).
//!
//! The simulator is a Ford. Replaying its recorded session with the VIN
//! swapped for a Honda's gives the same modules on a vehicle whose make says
//! otherwise, which is the case this exists to test.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::transcript::redact_vin;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ReplayMode, ReplayTransport, Transcript};
use std::sync::Arc;

const USER: &str = "user:test";
const RECORDED: &str = include_str!("replays/simulator-healthy.transcript");
const SIMULATOR_VIN: &str = "1FT7W2BT6KEC00001";

/// A Honda VIN with a correct check digit.
fn honda_vin() -> String {
    let mut vin: Vec<char> = "JHMRL3880JB000000".chars().collect();
    vin[8] = aim_decoders::vin::check_digit(&vin.iter().collect::<String>()).unwrap();
    vin.into_iter().collect()
}

fn visit(vin: &str) -> DiagnosticService {
    let text = redact_vin(RECORDED, SIMULATOR_VIN, vin);
    let transport =
        ReplayTransport::new(Transcript::parse(&text).unwrap(), ReplayMode::Lookup, vin);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    assert!(service.connect(USER).success);
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);
    service
}

fn ford_range(probe: &serde_json::Value) -> serde_json::Value {
    probe["ranges_probed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["from"] == "DE00")
        .cloned()
        .expect("the Ford range is listed either way")
}

#[test]
fn ford_configuration_is_not_swept_on_a_honda_and_it_says_why() {
    let mut service = visit(&honda_vin());
    let make = service.identity().settled("make").map(str::to_string);
    assert!(make.as_deref().is_some_and(|m| m.contains("Honda")), "{make:?}");

    let probe = service.probe_module_capabilities("ECU_7E8", USER);
    assert!(probe.success, "{:?}", probe.error);
    let data = probe.data.as_ref().unwrap();

    let range = ford_range(data);
    let why = range["skipped_because"].as_str().expect("skipped, with a reason");
    assert!(why.contains("Honda"), "{why}");
    assert!(why.contains("says nothing about whether this module has configuration"), "{why}");
    assert!(probe.warnings.iter().any(|w| w.code == "ford_range_not_swept"));

    // The standard ranges are still asked.
    let identification =
        data["ranges_probed"].as_array().unwrap().iter().find(|r| r["from"] == "F180").unwrap();
    assert!(identification.get("skipped_because").is_none(), "{identification}");
}

#[test]
fn ford_configuration_is_still_swept_on_a_ford() {
    let mut service = visit(SIMULATOR_VIN);
    let probe = service.probe_module_capabilities("ECU_7E8", USER);
    assert!(probe.success, "{:?}", probe.error);
    let range = ford_range(probe.data.as_ref().unwrap());
    assert!(range.get("skipped_because").is_none(), "{range}");
    assert!(!probe.warnings.iter().any(|w| w.code == "ford_range_not_swept"));
}
