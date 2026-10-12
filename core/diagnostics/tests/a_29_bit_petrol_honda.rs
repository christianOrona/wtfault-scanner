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

/// Measured on the real Odyssey (2026-10-04): its engine has no PID 05, 0F or
/// 10, and reports coolant temperature, intake air temperature and air flow
/// on 67, 68 and 66 alone. Asked for by their older names it had none of the
/// three, and *Warm idle* waited for ever on a coolant temperature that was
/// one PID away.
#[test]
fn a_reading_it_carries_on_a_newer_pid_is_read_from_there() {
    let mut service = honda();
    assert!(service.scan_modules(USER).success);

    // Nothing has asked this engine for its list of PIDs yet.
    let live = service.read_live_data(
        "ECU_18DAF110",
        &["coolant_temp".into(), "intake_air_temp".into(), "maf_rate".into(), "engine_rpm".into()],
        USER,
    );
    assert!(live.success, "{:?}", live.error);
    let ids: Vec<&str> = live.values.iter().map(|v| v.signal_id.as_str()).collect();
    for read in ["coolant_temp_sensor_1", "intake_air_temp_sensor_1", "maf_sensor_a", "engine_rpm"]
    {
        assert!(ids.contains(&read), "{read} was not read: {ids:?}");
    }
    assert!(!live.warnings.iter().any(|w| w.code == "signal_unavailable"), "{:?}", live.warnings);
    // And it says which was read for which, once each.
    assert_eq!(
        live.data.as_ref().unwrap()["read_instead"],
        serde_json::json!({
            "coolant_temp": "coolant_temp_sensor_1",
            "intake_air_temp": "intake_air_temp_sensor_1",
            "maf_rate": "maf_sensor_a",
        })
    );
    let notes: Vec<&str> = live
        .warnings
        .iter()
        .filter(|w| w.code == "signal_read_from_another_pid")
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(notes[0].contains("PID 05") && notes[0].contains("PID 67"), "{}", notes[0]);
    assert!(notes[0].contains("not yet been checked"), "{}", notes[0]);

    // The values are the vehicle's: a second coolant sensor it has, and no
    // second air flow sensor, which it has not.
    let value = |id: &str| live.values.iter().find(|v| v.signal_id == id).map(|v| v.value.as_f64());
    let coolant = value("coolant_temp_sensor_1").flatten().unwrap();
    assert!((40.0..=110.0).contains(&coolant), "{coolant}");
    assert_eq!(value("coolant_temp_sensor_2").flatten(), Some(coolant - 30.0));
    assert!(value("maf_sensor_b").is_none(), "{ids:?}");
}

/// *Warm idle* is what this engine's missing coolant temperature broke: a
/// petrol engine, at idle, and no way to see that it was warm.
#[test]
fn warm_idle_sees_this_engine_warm() {
    let mut service = honda();
    assert!(service.scan_modules(USER).success);

    let check = service.check_procedure("warm_idle", USER);
    assert!(check.success, "{:?}", check.error);
    let data = check.data.as_ref().unwrap();
    let coolant = data["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["condition"]["kind"] == "coolant_at_least")
        .expect("warm idle waits on coolant temperature");
    assert_eq!(coolant["signal"], "coolant_temp_sensor_1", "{coolant}");
    assert!(coolant["value"].is_number(), "{coolant}");
    assert!(coolant["unmeasurable"].is_null(), "{coolant}");
    assert!(
        check.warnings.iter().any(|w| w.code == "signal_read_from_another_pid"),
        "{:?}",
        check.warnings
    );

    // Run, it measures what the procedure exists for, with nothing missing:
    // the three it carries elsewhere count as read.
    let run = service.run_procedure("warm_idle", USER);
    assert!(run.success, "{:?}", run.error);
    let codes: Vec<&str> = run.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(!codes.contains(&"declared_measurements_missing"), "{:?}", run.warnings);
    if !codes.contains(&"conditions_not_met") {
        let ids: Vec<&str> = run.values.iter().map(|v| v.signal_id.as_str()).collect();
        for read in ["coolant_temp_sensor_1", "short_fuel_trim_b1", "maf_sensor_a"] {
            assert!(ids.contains(&read), "{read} was not measured: {ids:?}");
        }
    }
}

/// A vehicle that has the older PID is asked for it, as before.
#[test]
fn an_engine_with_the_older_pid_is_still_asked_for_that_one() {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::toyota_2004(ScenarioId::Healthy),
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
    assert!(service.scan_modules(USER).success);
    let engine = module_keys(&service).into_iter().next().unwrap();

    let live = service.read_live_data(&engine, &["coolant_temp".into()], USER);
    assert!(live.success, "{:?}", live.error);
    assert_eq!(live.values[0].signal_id, "coolant_temp");
    assert_eq!(live.data.as_ref().unwrap()["read_instead"], serde_json::json!({}));
    assert!(!live.warnings.iter().any(|w| w.code == "signal_read_from_another_pid"));
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
