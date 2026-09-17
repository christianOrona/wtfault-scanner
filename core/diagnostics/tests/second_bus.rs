//! The second CAN bus, against a simulator that has one (#13).
//!
//! Body and comfort modules commonly live on pins 3 and 11. Only adapters with
//! STN firmware can select that bus, so the simulator plays one: `STI`, `STP53`,
//! `STPBR` and `ATMA` behave like an OBDLink, and the virtual truck carries
//! modules that answer only there.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{AdapterPersonality, ScenarioId, SimulatedTransport};
use std::sync::Arc;

const USER: &str = "user:test";

fn service_with(transport: SimulatedTransport) -> DiagnosticService {
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    assert!(service.connect(USER).success, "connect");
    service
}

fn keys(service: &DiagnosticService) -> Vec<String> {
    service
        .store()
        .modules(service.session_id())
        .unwrap()
        .into_iter()
        .map(|m| m.module_key)
        .collect()
}

#[test]
fn an_stn_adapter_reaches_modules_on_the_second_bus() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);
    assert!(service.capabilities().multiple_can_buses, "STN firmware can select the second bus");

    let scan = service.scan_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let keys = keys(&service);
    assert!(keys.iter().any(|k| k.starts_with("BUS2_")), "second-bus modules: {keys:?}");
    assert!(keys.iter().any(|k| !k.starts_with("BUS2_")), "primary modules still found: {keys:?}");
}

#[test]
fn a_clone_adapter_reports_only_the_primary_bus() {
    let mut service = service_with(SimulatedTransport::new(ScenarioId::Healthy));
    assert!(!service.capabilities().multiple_can_buses);
    assert!(service.scan_modules(USER).success);
    assert!(!keys(&service).iter().any(|k| k.starts_with("BUS2_")));
}

/// The full scan is the one that is supposed to reach everything, and body and
/// comfort modules are mostly not on the primary bus.
#[test]
fn a_full_scan_sweeps_both_buses() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);

    let scan = service.scan_all_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let keys = keys(&service);
    assert!(keys.iter().any(|k| k.starts_with("BUS2_")), "second-bus modules: {keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("ECU_")), "primary modules: {keys:?}");

    // And it leaves the adapter where it found it, or the next read fails for
    // a reason nobody would connect to having run a scan.
    let after = service.read_dtcs(Some("ECU_7E8"), USER);
    assert!(after.success, "a primary-bus read after the scan: {:?}", after.error);
}
