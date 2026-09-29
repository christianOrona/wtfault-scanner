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

/// A full scan straight after a module scan still reaches the primary bus.
///
/// Measured on a 2019 F-250 (2026-09-28): the module scan went back to the
/// primary bus with `ATSP0`, which makes the adapter search again, and every
/// `3E00` probe of the full scan that followed was cut off mid-search. It found
/// nothing on a truck that had answered three minutes earlier.
#[test]
fn a_full_scan_after_a_module_scan_still_finds_the_primary_bus() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);

    assert!(service.scan_modules(USER).success);
    let scan = service.scan_all_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let data = scan.data.expect("a full scan reports its buses");
    let primary = data["buses"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b["bus"] == "ECU"))
        .map(|b| b["modules"].as_u64().unwrap_or(0))
        .unwrap_or(0);
    assert!(primary > 0, "primary-bus modules in the full scan: {}", data["buses"]);
}

/// A Ford module that refuses the standard name is named by its part number.
///
/// Measured on a 2019 F-250 (2026-09-28): 34 of 36 modules stayed "Module at
/// ..." because every one refused F197, while 32 answered F113 with a Ford part
/// number whose base says what the module is.
#[test]
fn a_ford_module_is_named_by_its_part_number() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);
    assert!(service.identify_vehicle(USER).success, "identify");

    assert!(service.scan_all_modules(USER).success);
    let door = service
        .store()
        .modules(service.session_id())
        .unwrap()
        .into_iter()
        .find(|m| m.module_key == "BUS2_72E")
        .expect("the door module on the second bus");
    assert_eq!(door.name, "Driver door module (DDM)");

    // Opening the module afterwards keeps what the scan learned. It used to
    // replace the identity with service 09's answer, which a second-bus
    // module never gives.
    assert!(service.get_module_identity("BUS2_72E", USER).success);
    let door = service
        .store()
        .modules(service.session_id())
        .unwrap()
        .into_iter()
        .find(|m| m.module_key == "BUS2_72E")
        .unwrap();
    assert_eq!(door.name, "Driver door module (DDM)");
    assert!(door.identity.uds_identification_read);

    // A base this build does not know keeps the address, and the read is
    // still recorded as done.
    let seat = service
        .store()
        .modules(service.session_id())
        .unwrap()
        .into_iter()
        .find(|m| m.module_key == "BUS2_74E")
        .expect("the seat module on the second bus");
    assert_eq!(seat.name, "Module at 74E");
    assert!(seat.identity.uds_identification_read, "{:?}", seat.identity);
}

/// A capability probe of a second-bus module is asked on that bus.
///
/// Measured on a 2019 F-250 (2026-09-28): every second-bus module probed from
/// the primary bus reported no identifiers, after about 110 seconds each.
#[test]
fn a_capability_probe_reaches_a_module_on_the_second_bus() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);
    assert!(service.scan_all_modules(USER).success);

    let probe = service.probe_module_capabilities("BUS2_72E", USER);
    assert!(probe.success, "{:?}", probe.error);
    let data = probe.data.expect("a probe reports what it found");
    let found: Vec<&str> =
        data["identifiers"].as_array().unwrap().iter().filter_map(|i| i["did"].as_str()).collect();
    assert!(found.contains(&"F188"), "identifiers found: {found:?}");

    // And the adapter is back where it was.
    assert!(service.read_dtcs(Some("ECU_7E8"), USER).success);
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

/// A module found on the second bus can be read and the adapter comes back to
/// the primary bus. A failure here means the adapter was left on the other bus,
/// which the next read would fail for a reason nobody would connect to this one.
#[test]
fn a_module_found_on_the_second_bus_can_be_read_and_the_adapter_comes_back() {
    let transport =
        SimulatedTransport::with_personality(ScenarioId::Healthy, AdapterPersonality::obdlink_mx());
    let mut service = service_with(transport);

    let scan = service.scan_all_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let bus2 = keys(&service)
        .into_iter()
        .find(|k| k.starts_with("BUS2_"))
        .expect("a module on the second bus");
    let read = service.read_dtcs(Some(&bus2), USER);
    assert!(read.success, "reading {bus2} on the second bus: {:?}", read.error);

    // And it leaves the adapter where it found it, or the next read fails for
    // a reason nobody would connect to having run a scan.
    let back = service.read_dtcs(Some("ECU_7E8"), USER);
    assert!(back.success, "a primary-bus read after a second-bus read: {:?}", back.error);
}
