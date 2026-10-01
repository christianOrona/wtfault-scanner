//! The core against a vehicle from before CAN: a 2004 Toyota on ISO 9141-2,
//! the K-line on pin 7.
//!
//! What it offers is the legislated OBD-II services and nothing else: no UDS,
//! no CAN addresses, frames the adapter prints with a header and a checksum.
//! The roadmap's rule is that a silent or misleading empty result on a vehicle
//! that is not the Ford is a bug, so each test asks for something real, or for
//! the honest reason it cannot be had.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{AdapterPersonality, ScenarioId, SimulatedTransport, VirtualVehicle};
use aim_types::ErrorCode;
use std::sync::Arc;

const USER: &str = "user:test";

fn toyota(scenario: ScenarioId) -> DiagnosticService {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::toyota_2004(scenario),
        AdapterPersonality::genuine_v1_5(),
    );
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
    service
}

/// The VIN arrives in five numbered frames, each ending in a checksum. Read
/// whole, it is the Toyota's; the calibration ID comes the same way.
#[test]
fn it_is_reached_on_the_k_line_and_named_from_its_vin() {
    let mut service = toyota(ScenarioId::Healthy);
    let identify = service.identify_vehicle(USER);
    assert!(identify.success, "{:?}", identify.error);
    let data = identify.data.unwrap();
    assert_eq!(data["vin"], aim_simulator::SIMULATED_TOYOTA_VIN);
    assert_eq!(data["reported_by"], "10");
    assert_eq!(data["calibration_ids"][0], "34715100");
    assert_eq!(service.identity().settled("make"), Some("Toyota (Japan)"));
}

/// The bus is described as what it is, not as CAN on pins 6 and 14.
#[test]
fn the_scan_names_the_k_line_and_gives_it_no_can_rate() {
    let mut service = toyota(ScenarioId::Healthy);
    let scan = service.scan_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let data = scan.data.unwrap();
    let bus = &data["buses"][0];
    assert_eq!(bus["label"], "K-line (pin 7)");
    assert!(bus["kbits"].is_null(), "{bus}");
    let keys: Vec<&str> = data["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["module_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["ECU_10", "ECU_18"]);
}

/// Supported PIDs and live readings over the K-line. (A checksum read as
/// data lands after the bytes a reading uses, so readings survived that bug;
/// the VIN and the fault lists, tested above and below, did not.)
#[test]
fn supported_pids_and_live_readings_decode() {
    let mut service = toyota(ScenarioId::Healthy);
    assert!(service.scan_modules(USER).success);
    let pids = service.read_supported_pids("ECU_10", USER);
    assert!(pids.success, "{:?}", pids.error);
    let listed: Vec<u64> = pids.data.unwrap()["pids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pid"].as_u64().unwrap())
        .collect();
    assert!(listed.contains(&0x0C) && listed.contains(&0x05));
    assert!(!listed.contains(&0x51), "this engine does not report a fuel type");

    let live = service.read_live_data(
        "ECU_10",
        &["engine_rpm".into(), "coolant_temp".into(), "short_fuel_trim_b1".into()],
        USER,
    );
    assert!(live.success, "{:?}", live.error);
    let rpm = live.values.iter().find(|v| v.signal_id == "engine_rpm").unwrap();
    let rpm = rpm.value.as_f64().unwrap();
    assert!((500.0..2500.0).contains(&rpm), "engine speed {rpm}");
}

/// Codes come three to a frame with no count byte, from both modules.
#[test]
fn fault_codes_are_read_from_every_module() {
    let mut service = toyota(ScenarioId::DpfRegen);
    assert!(service.scan_modules(USER).success);
    let dtcs = service.read_dtcs(None, USER);
    assert!(dtcs.success, "{:?}", dtcs.error);
    let data = dtcs.data.unwrap();
    let codes: Vec<&str> = data["dtcs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["module"] == "ECU_10")
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"P2463"), "{codes:?}");
    assert!(!codes.contains(&"P0000"), "padding read as a code: {codes:?}");

    let frame = service.read_freeze_frame("ECU_10", 0, USER);
    assert!(frame.success, "{:?}", frame.error);
    assert_eq!(frame.data.unwrap()["dtc"], "P2463");
}

/// The full scan's sweep is CAN. On the K-line it reads the legislated fault
/// lists of the modules that answer, says that is the reach of this bus, and
/// sends nothing to CAN addresses.
#[test]
fn the_full_scan_reads_what_the_k_line_can_reach_and_says_so() {
    let mut service = toyota(ScenarioId::DpfRegen);
    let full = service.scan_all_modules(USER);
    assert!(full.success, "{:?}", full.error);
    let data = full.data.unwrap();
    assert_eq!(data["addresses_probed"], 0);
    assert_eq!(data["module_count"], 2);
    assert!(data["fault_count"].as_u64().unwrap() > 0);
    assert_eq!(data["buses"][0]["label"], "K-line (pin 7)");
    let engine = data["modules"].as_array().unwrap().iter().find(|m| m["address"] == "10").unwrap();
    // A code both confirmed and permanent is one fault, listed once.
    let faults = engine["faults"].as_array().unwrap();
    let p2463: Vec<&serde_json::Value> = faults.iter().filter(|f| f["code"] == "P2463").collect();
    assert_eq!(p2463.len(), 1, "{faults:?}");
    let summary = p2463[0]["status_summary"].as_str().unwrap();
    assert!(summary.starts_with("confirmed, permanent"), "{summary}");
    let fault = &engine["faults"][0];
    // Not reported on this protocol, which is not the same as "no".
    assert!(fault["failing_now"].is_null(), "{fault}");
    assert!(fault["warning_lamp"].is_null(), "{fault}");
    assert!(fault["status_summary"].as_str().unwrap().contains("not reported on this protocol"));
    assert!(full.warnings.iter().any(|w| w.code == "full_scan_reaches_emissions_modules_only"));
}

/// What needs UDS is refused with the reason, rather than asked down a wire
/// that cannot carry it and reported as the module's silence.
#[test]
fn what_needs_uds_says_the_k_line_does_not_carry_it() {
    let mut service = toyota(ScenarioId::Healthy);
    assert!(service.scan_modules(USER).success);
    for result in [
        service.probe_module_capabilities("ECU_10", USER),
        service.probe_write_gate("ECU_10", USER, Some(USER)),
        service.capture_configuration("ECU_10", &[0xF190], None, USER),
    ] {
        assert!(!result.success);
        let error = result.error.unwrap();
        assert_eq!(error.code, ErrorCode::CapabilityMissing);
        assert!(error.message.contains("K-line (pin 7)"), "{}", error.message);
        assert!(error.message.contains("legislated OBD-II services"), "{}", error.message);
    }
}

/// Service 06 before CAN is another message layout. It is not parsed as the
/// CAN one, which would invent test names and verdicts; it says so instead.
#[test]
fn self_test_results_are_not_read_in_the_wrong_layout() {
    let mut service = toyota(ScenarioId::Healthy);
    assert!(service.scan_modules(USER).success);
    let monitors = service.read_monitor_tests("ECU_10", USER);
    assert!(monitors.success, "{:?}", monitors.error);
    let data = monitors.data.unwrap();
    assert_eq!(data["supported"], false);
    assert_eq!(data["layout_not_decoded"], true);
    assert!(monitors.warnings.iter().any(|w| w.code == "monitor_tests_pre_can_layout"));
}

/// Some vehicles of this age have a CAN body bus beside a pre-CAN engine bus.
/// With an adapter that reaches a second bus, the K-line is read with the
/// legislated services and the second bus is still swept: here it is silent,
/// and the scan says so rather than failing.
#[test]
fn a_second_bus_beside_the_k_line_is_still_swept() {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::toyota_2004(ScenarioId::DpfRegen),
        AdapterPersonality::obdlink_mx(),
    );
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
    assert!(service.capabilities().multiple_can_buses);

    let full = service.scan_all_modules(USER);
    assert!(full.success, "{:?}", full.error);
    let data = full.data.unwrap();
    assert_eq!(data["module_count"], 2);
    assert_eq!(data["buses"][0]["label"], "K-line (pin 7)");
    assert!(full.warnings.iter().any(|w| w.code == "full_scan_reaches_emissions_modules_only"));
    assert!(full.warnings.iter().any(|w| w.code == "second_bus_silent"), "{:?}", full.warnings);
}
