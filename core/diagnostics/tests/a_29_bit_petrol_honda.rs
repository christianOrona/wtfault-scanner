//! The core against a vehicle that is not a Ford in every way the Ford was
//! convenient (#39): another make, 29-bit addressing, and a petrol engine.
//!
//! The simulated Honda is shaped after the owner's 2023 Odyssey: its engine at
//! `18DAF110` and transmission at `18DAF11E`, the addresses measured on the
//! real one. The roadmap's rule is that a silent empty result on a non-Ford
//! vehicle is a bug, so each test here asks for something real back.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{AdapterPersonality, ScenarioId, SimulatedTransport, VirtualVehicle};
use std::sync::Arc;

const USER: &str = "user:test";

fn honda() -> DiagnosticService {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::honda_odyssey(ScenarioId::Healthy),
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

fn module_keys(service: &DiagnosticService) -> Vec<String> {
    service
        .store()
        .modules(service.session_id())
        .unwrap()
        .into_iter()
        .map(|m| m.module_key)
        .collect()
}

#[test]
fn it_is_reached_on_29_bit_can_and_named_a_honda() {
    let mut service = honda();
    let identify = service.identify_vehicle(USER);
    assert!(identify.success, "{:?}", identify.error);
    let data = identify.data.unwrap();
    assert_eq!(data["vin"], aim_simulator::SIMULATED_HONDA_VIN);
    assert_eq!(data["reported_by"], "18DAF110");
    assert_eq!(service.identity().settled("make"), Some("Honda (US)"));

    assert!(service.scan_modules(USER).success);
    assert_eq!(module_keys(&service), ["ECU_18DAF110", "ECU_18DAF11E"]);
}

#[test]
fn the_full_scan_sweeps_29_bit_addresses_and_finds_both_modules() {
    let mut service = honda();
    let scan = service.scan_all_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let data = scan.data.unwrap();
    assert_eq!(data["module_count"], 2, "{data}");
    let addresses: Vec<&str> = data["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["address"].as_str().unwrap())
        .collect();
    assert_eq!(addresses, ["18DAF110", "18DAF11E"]);
    // 29-bit addressing has no legislated block to rule a module out of, so
    // neither is taken to keep no emissions codes.
    for module in data["modules"].as_array().unwrap() {
        assert_eq!(module["emissions_codes_read"], false, "{module}");
    }
    assert!(
        !scan.warnings.iter().any(|w| w.code == "no_fault_lists_read"),
        "both modules hand over fault lists: {:?}",
        scan.warnings
    );
}

/// A petrol engine has fuel trims, and they decode.
#[test]
fn its_fuel_trims_read_and_decode() {
    let mut service = honda();
    assert!(service.scan_modules(USER).success);
    let live = service.read_live_data(
        "ECU_18DAF110",
        &["short_fuel_trim_b1".into(), "long_fuel_trim_b1".into(), "fuel_type".into()],
        USER,
    );
    assert!(live.success, "{:?}", live.error);
    let ids: Vec<&str> = live.values.iter().map(|v| v.signal_id.as_str()).collect();
    assert!(ids.contains(&"short_fuel_trim_b1") && ids.contains(&"long_fuel_trim_b1"), "{ids:?}");
    let fuel = live.values.iter().find(|v| v.signal_id == "fuel_type").unwrap();
    assert_eq!(fuel.value, aim_types::Value::Text(String::from("Gasoline")));
}

/// Procedures read this engine, at its own address. They looked for `7E8`
/// alone, and on this vehicle waited forever on a module that was not there.
#[test]
fn a_procedure_reads_the_engine_at_its_29_bit_address() {
    let mut service = honda();
    assert!(service.scan_modules(USER).success);
    service.read_supported_pids("ECU_18DAF110", USER);

    let check = service.check_procedure("warm_idle", USER);
    let data = check.data.unwrap();
    assert_ne!(data["state"], "does_not_apply", "a petrol engine is what warm idle is for");
    for condition in data["conditions"].as_array().unwrap() {
        assert!(condition["unmeasurable"].is_null(), "{condition}");
        assert!(condition["value"].is_number(), "{condition}");
    }

    let steady = service.check_procedure("steady_rpm_2500", USER);
    let not_on = steady.data.unwrap()["not_on_this_engine"].clone();
    assert_eq!(not_on, serde_json::json!([]), "nothing is out of reach on a petrol engine");
}

/// Ford's configuration range is not asked of it, and the features list
/// offers it none of the Ford truck's settings.
#[test]
fn nothing_ford_is_offered_to_it() {
    let mut service = honda();
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);
    let features = service.list_features(USER);
    assert_eq!(features.data.unwrap()["features"], serde_json::json!([]));
    let probe = service.probe_module_capabilities("ECU_18DAF110", USER);
    assert!(probe.success, "{:?}", probe.error);
    assert!(
        probe.warnings.iter().any(|w| w.code == "ford_range_not_swept"),
        "{:?}",
        probe.warnings
    );
}

/// This engine answers a freeze frame request with a causing code of `0000`
/// when none is stored, which J1979 defines as "no frame". Read on, it would
/// have been reported as a frame for P0000.
#[test]
fn a_causing_code_of_zero_means_no_freeze_frame() {
    let mut service = honda();
    assert!(service.scan_modules(USER).success);
    let frame = service.read_freeze_frame("ECU_18DAF110", 0, USER);
    assert!(frame.success, "{:?}", frame.error);
    let data = frame.data.as_ref().unwrap();
    assert_eq!(data["stored"], false);
    assert!(data["dtc"].is_null(), "{data}");
    assert!(frame.values.is_empty());
    assert!(frame.warnings.iter().any(|w| w.code == "no_freeze_frame_stored"));
}

/// The feature catalogue has nothing for a Honda. An empty list is said to be
/// the catalogue's gap rather than left to read as the vehicle having nothing.
#[test]
fn an_empty_feature_list_says_it_is_the_catalogue_that_is_empty() {
    let mut service = honda();
    assert!(service.identify_vehicle(USER).success);
    let features = service.list_features(USER);
    assert!(features.success, "{:?}", features.error);
    assert_eq!(features.data.as_ref().unwrap()["features"].as_array().unwrap().len(), 0);
    let note = features
        .warnings
        .iter()
        .find(|w| w.code == "no_features_for_this_vehicle")
        .expect("the empty list is explained");
    assert!(note.message.contains("Honda"), "{}", note.message);
}

/// The same Honda through an adapter that can reach pins 3 and 11, which on
/// this vehicle carry nothing.
fn honda_on_a_two_bus_adapter() -> DiagnosticService {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::honda_odyssey(ScenarioId::Healthy),
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
    service
}

/// How many replies in the session were the adapter saying the bus did not
/// carry the request.
fn bus_errors(service: &DiagnosticService) -> usize {
    service
        .store()
        .events_since(service.session_id(), 0, 100_000)
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                aim_types::EventKind::AdapterResponse { classification, .. }
                    if classification == "bus_error"
            )
        })
        .count()
}

/// Measured on the real Odyssey (2026-10-04): nothing is on its second-bus
/// pins, every request there came back `CAN ERROR`, and a module scan and two
/// full scans sent 1,602 of them, about a minute of each full scan. A bus that
/// does not carry the first few requests will not carry the rest.
#[test]
fn a_second_bus_that_carries_nothing_is_given_up_on_after_a_few_requests() {
    let mut service = honda_on_a_two_bus_adapter();

    let scan = service.scan_modules(USER);
    assert!(scan.success, "{:?}", scan.error);
    let after_module_scan = bus_errors(&service);
    // Three requests, each retried once by the adapter after a bus error.
    assert!(after_module_scan <= 6, "the module scan sent {after_module_scan} into a dead bus");

    let full = service.scan_all_modules(USER);
    assert!(full.success, "{:?}", full.error);
    let in_full_scan = bus_errors(&service) - after_module_scan;
    assert!(in_full_scan <= 6, "the full scan sent {in_full_scan} into a dead bus");

    // The bus that is there was still read, and the one that is not is named.
    let keys = module_keys(&service);
    assert!(keys.iter().any(|k| k == "ECU_18DAF110"), "{keys:?}");
    assert!(keys.iter().all(|k| !k.starts_with("BUS2_")), "{keys:?}");
    assert!(
        full.warnings.iter().any(|w| w.code == "second_bus_silent"),
        "{:?}",
        full.warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
    );

    // And the adapter is back where requests are answered.
    let read = service.read_dtcs(Some("ECU_18DAF110"), USER);
    assert!(read.success, "{:?}", read.error);
}
