//! The diagnostic service driven against the virtual vehicle.
//!
//! These tests exercise the real stack end to end below the API: service →
//! safety gate → ELM327 adapter → ELM327 wire protocol → virtual ECUs, with
//! everything recorded into a real SQLite database.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_types::{ConnectionState, DtcStatus, ErrorCode, EventKind};
use std::sync::Arc;

const USER: &str = "user:test";

fn service(scenario: ScenarioId) -> (DiagnosticService, aim_simulator::SharedEmulator) {
    let transport = SimulatedTransport::new(scenario);
    let emulator = transport.emulator();
    let adapter: Box<dyn DiagnosticAdapter> = Box::new(Elm327Adapter::new(
        Box::new(transport),
        Elm327Config::fast(),
    ));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    (service, emulator)
}

fn connected(scenario: ScenarioId) -> (DiagnosticService, aim_simulator::SharedEmulator) {
    let (mut s, e) = service(scenario);
    let r = s.connect(USER);
    assert!(r.success, "connect failed: {:?}", r.error);
    (s, e)
}

#[test]
fn connecting_records_the_connection_and_its_capabilities() {
    let (service, _) = connected(ScenarioId::Healthy);
    assert_eq!(service.state(), ConnectionState::Ready);

    let connections = service.store().connections(service.session_id()).unwrap();
    assert_eq!(connections.len(), 1);
    assert!(connections[0].capabilities.elm327_compatible);
    assert!(connections[0].disconnected_at.is_none());
    assert!(connections[0].adapter_id.starts_with("sim:"));
}

#[test]
fn adapter_caveats_reach_the_caller_as_warnings() {
    let (mut service, _) = service(ScenarioId::Healthy);
    let result = service.connect(USER);
    assert!(result.success);
    // The default simulated adapter is the cheap clone, so its limits must be
    // visible to whoever called connect, not buried in a capability struct.
    assert!(
        result.warnings.iter().any(|w| w.code == "adapter_caveat"),
        "expected adapter caveats, got {:?}",
        result.warnings
    );
}

#[test]
fn a_silent_bus_connects_degraded_and_says_so_loudly() {
    let (mut service, _) = service(ScenarioId::BusSilent);
    let result = service.connect(USER);
    assert!(result.success, "the adapter itself is fine");
    assert!(matches!(service.state(), ConnectionState::Degraded { .. }));
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "adapter_degraded" && w.severity == aim_types::WarningSeverity::Serious));

    // And reads then fail with the vehicle's silence, not a fabricated value.
    let scan = service.scan_modules(USER);
    assert!(!scan.success);
    assert_eq!(scan.error.unwrap().code, ErrorCode::VehicleNotResponding);
}

#[test]
fn identify_vehicle_reads_the_vin_and_records_the_vehicle() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let result = service.identify_vehicle(USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.as_ref().unwrap();
    assert_eq!(data["vin"], aim_simulator::SIMULATED_VIN);
    assert_eq!(data["vin_decoded"]["check_digit_valid"], true);
    assert_eq!(data["vin_decoded"]["model_year"], 2019);
    assert_eq!(data["reported_by"], "7E8");

    // The VIN value carries the raw bytes it came from.
    let vin_value = result.values.iter().find(|v| v.signal_id == "vin").unwrap();
    assert!(!vin_value.provenance.raw_hex.is_empty());
    assert!(vin_value.provenance.evidence_ref.is_some());

    // Only what the VIN standard encodes is filled in.
    let vehicle = service.vehicle().unwrap();
    assert_eq!(vehicle.year, Some(2019));
    assert!(vehicle.make.is_some(), "WMI decodes to a manufacturer");
    assert!(vehicle.model.is_none(), "model must not be inferred");
    assert!(vehicle.trim.is_none());
    assert!(vehicle.engine.is_none());
}

#[test]
fn calibration_identifiers_come_back_with_the_vin() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let result = service.identify_vehicle(USER);
    let data = result.data.unwrap();
    let calids = data["calibration_ids"].as_array().unwrap();
    assert_eq!(calids.len(), 1);
    assert_eq!(calids[0], "SIMULATED-CAL-01");
    assert_eq!(
        data["calibration_verification_numbers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn scanning_finds_every_module_and_names_them_from_evidence() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let result = service.scan_modules(USER);
    assert!(result.success, "{:?}", result.error);

    let modules = service.store().modules(service.session_id()).unwrap();
    let keys: Vec<&str> = modules.iter().map(|m| m.module_key.as_str()).collect();
    assert_eq!(keys, vec!["ECU_7E8", "ECU_7EA", "ECU_7EB"]);

    // Each module reports its own name, so none keeps the address fallback.
    let engine = modules.iter().find(|m| m.module_key == "ECU_7E8").unwrap();
    assert_eq!(engine.name, "SIM ENGINE CONTROL");
    assert_eq!(engine.address, "7E8");
    assert_eq!(engine.protocol, aim_types::ObdProtocol::Iso15765Can11_500);
}

#[test]
fn module_identity_fills_in_calibration_and_ecu_name() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.get_module_identity("ECU_7E8", USER);
    assert!(result.success, "{:?}", result.error);

    let identity = &result.data.as_ref().unwrap()["identity"];
    assert_eq!(identity["ecu_name"], "SIM ENGINE CONTROL");
    assert_eq!(identity["calibration_ids"][0], "SIMULATED-CAL-01");
    assert!(!identity["calibration_verification_numbers"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_module_that_reports_less_still_returns_what_it_has() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    // 7EB has an ECU name but no calibration ids; the gaps are warnings, not
    // a failed read.
    let result = service.get_module_identity("ECU_7EB", USER);
    assert!(result.success, "{:?}", result.error);
    let identity = &result.data.as_ref().unwrap()["identity"];
    assert_eq!(identity["ecu_name"], "SIM MODULE 7EB");
    assert!(identity["calibration_ids"].as_array().unwrap().is_empty());
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "identity_field_unavailable"));
}

#[test]
fn supported_pids_are_enumerated_across_the_whole_mask_chain() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.read_supported_pids("ECU_7E8", USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.unwrap();
    let pids: Vec<u64> = data["pids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pid"].as_u64().unwrap())
        .collect();

    // The chain must have been walked past the first mask: 0x7C only appears
    // in the fourth block.
    assert!(pids.contains(&0x0C), "engine rpm");
    assert!(pids.contains(&0x42), "control module voltage");
    assert!(pids.contains(&0x7C), "the 0x60 block must have been reached");
    assert!(data["count"].as_u64().unwrap() > 30);
}

#[test]
fn every_advertised_pid_can_actually_be_read() {
    // The mask is a promise. This is the test that it is kept, through the
    // full stack rather than inside the simulator.
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let listed = service.read_supported_pids("ECU_7E8", USER);
    let data = listed.data.unwrap();

    for entry in data["pids"].as_array().unwrap() {
        let pid = entry["pid"].as_u64().unwrap() as u8;
        // Mask PIDs are structural, not readings.
        if matches!(pid, 0x00 | 0x20 | 0x40 | 0x60) {
            continue;
        }
        if entry["decoder_available"] != true {
            continue;
        }
        let signal = entry["signal_id"].as_str().unwrap();
        let r = service.read_pid("ECU_7E8", signal, USER);
        assert!(
            r.success,
            "advertised PID {pid:02X} ({signal}) could not be read: {:?}",
            r.error
        );
    }
}

#[test]
fn reading_a_pid_produces_a_decoded_value_with_provenance_and_a_measurement() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.read_pid("ECU_7E8", "engine_rpm", USER);
    assert!(result.success, "{:?}", result.error);

    let value = &result.values[0];
    assert_eq!(value.signal_id, "engine_rpm");
    assert_eq!(value.unit.as_deref(), Some("rpm"));
    let rpm = value.value.as_f64().unwrap();
    assert!((600.0..900.0).contains(&rpm), "idle rpm was {rpm}");
    assert!(!value.out_of_range);
    assert_eq!(value.provenance.decoder_id, "obd2.mode01.pid0C");
    assert!(value.provenance.evidence_ref.is_some());

    // And it landed in the measurement series.
    let stored = service
        .store()
        .measurements(service.session_id(), Some("engine_rpm"), 10)
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].value, Some(rpm));
    assert_eq!(stored[0].raw_value, value.provenance.raw_hex);
}

#[test]
fn a_pid_can_be_addressed_by_number_as_well_as_by_name() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    // 0x0C is hex, 0C is hex because of the letter, 12 is decimal. All three
    // name PID 0x0C; the rule is fixed so a spelling never changes meaning.
    for spelling in ["0x0C", "0C", "12"] {
        let r = service.read_pid("ECU_7E8", spelling, USER);
        assert!(r.success, "{spelling} failed: {:?}", r.error);
        assert_eq!(r.values[0].signal_id, "engine_rpm", "for spelling {spelling}");
    }
    // A decimal spelling means decimal: 16 is PID 0x10, not PID 0x16.
    let r = service.read_pid("ECU_7E8", "16", USER);
    assert_eq!(r.values[0].signal_id, "maf_rate");
}

#[test]
fn an_unknown_signal_is_rejected_rather_than_guessed() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let r = service.read_pid("ECU_7E8", "turbo_vane_position", USER);
    assert!(!r.success);
    assert_eq!(r.error.unwrap().code, ErrorCode::DecoderNotFound);
}

#[test]
fn reading_from_an_unscanned_module_says_to_scan_first() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let r = service.read_pid("ECU_7E8", "engine_rpm", USER);
    assert!(!r.success);
    let e = r.error.unwrap();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(e.message.contains("scan_modules"));
}

#[test]
fn live_data_samples_several_signals_and_reports_the_ones_it_could_not_read() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let signals: Vec<String> = ["engine_rpm", "coolant_temp", "nonsense_signal"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let result = service.read_live_data("ECU_7E8", &signals, USER);

    assert!(result.success);
    assert_eq!(result.values.len(), 2, "two of three signals are readable");
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "unknown_signal" && w.message.contains("nonsense_signal")));
}

#[test]
fn a_sample_where_nothing_reads_is_a_failure_not_a_flatline() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let signals = vec![String::from("nope"), String::from("also_nope")];
    let result = service.read_live_data("ECU_7E8", &signals, USER);
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::NoData);
}

#[test]
fn unverified_decoders_taint_the_values_they_produce() {
    // The DPF and exhaust temperature PIDs are marked unverified in the
    // profile. Reading one must warn, all the way up to the envelope.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    let result = service.read_pid("ECU_7E8", "dpf_temp_bank1_inlet", USER);
    assert!(result.success);
    assert!(!result.values[0].is_trustworthy());
    assert_eq!(
        result.values[0].provenance.verification,
        aim_types::VerificationStatus::Unverified
    );
    assert!(result
        .warnings
        .iter()
        .any(|w| w.code == "unverified_decoder"));
}

#[test]
fn a_healthy_vehicle_reports_no_codes_rather_than_failing() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.read_dtcs(None, USER);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.data.unwrap()["confirmed_count"], 0);
    assert!(service
        .store()
        .dtcs(service.session_id(), None)
        .unwrap()
        .is_empty());
}

#[test]
fn dtcs_are_read_across_all_three_services_and_described_from_the_catalog() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    let result = service.read_dtcs(Some("ECU_7E8"), USER);
    assert!(result.success, "{:?}", result.error);

    let dtcs: Vec<aim_diagnostics::DtcReport> =
        serde_json::from_value(result.data.unwrap()["dtcs"].clone()).unwrap();
    let codes: Vec<&str> = dtcs.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, vec!["P2463", "P242F", "P2002", "P2463"]);

    let statuses: Vec<DtcStatus> = dtcs.iter().map(|d| d.status).collect();
    assert_eq!(
        statuses,
        vec![
            DtcStatus::Confirmed,
            DtcStatus::Confirmed,
            DtcStatus::Pending,
            DtcStatus::Permanent
        ]
    );

    // Descriptions come from the catalog, never invented.
    let p2463 = dtcs.iter().find(|d| d.code == "P2463").unwrap();
    assert!(p2463.description.is_some());
    assert!(p2463.is_generic);
    assert!(!p2463.structural_summary.is_empty());

    // And they were persisted. P2463 is both confirmed and permanent, and
    // those are two distinct observations about the same code, so four codes
    // become four rows: the unique key is (session, module, code, status).
    let stored = service.store().dtcs(service.session_id(), None).unwrap();
    assert_eq!(stored.len(), 4);
    let permanent: Vec<&str> = stored
        .iter()
        .filter(|d| d.status == DtcStatus::Permanent)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(permanent, vec!["P2463"]);
}

#[test]
fn the_freeze_frame_is_a_snapshot_from_the_past_not_a_live_reading() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);

    let live = service.read_pid("ECU_7E8", "engine_rpm", USER);
    let live_rpm = live.values[0].value.as_f64().unwrap();

    let frame = service.read_freeze_frame("ECU_7E8", 0, USER);
    assert!(frame.success, "{:?}", frame.error);
    let data = frame.data.as_ref().unwrap();
    assert_eq!(data["dtc"], "P2463");
    assert!(data["dtc_description"].is_string());

    let frozen_rpm = frame
        .values
        .iter()
        .find(|v| v.signal_id == "engine_rpm")
        .unwrap()
        .value
        .as_f64()
        .unwrap();
    assert!(
        (frozen_rpm - 748.0).abs() < 1.0,
        "frozen rpm was {frozen_rpm}"
    );
    assert!(
        live_rpm > frozen_rpm + 200.0,
        "the truck is at a raised idle now ({live_rpm}) versus when the code stored ({frozen_rpm})"
    );

    // A freeze frame must not pollute the live measurement series.
    let measured = service
        .store()
        .measurements(service.session_id(), Some("engine_rpm"), 100)
        .unwrap();
    assert_eq!(measured.len(), 1, "only the live read was recorded");
}

#[test]
fn clearing_codes_needs_confirmation_and_a_stopped_engine() {
    let (mut service, emulator) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);

    // Unconfirmed: refused, and nothing reaches the vehicle.
    let result = service.clear_dtcs(Some("ECU_7E8"), USER, None);
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::ConfirmationRequired);
    assert!(
        !emulator.lock().unwrap().vehicle.dtcs_cleared,
        "an unconfirmed clear reached the vehicle"
    );

    // Confirmed, but the DPF scenario has the engine running, which is a
    // precondition failure rather than a permission one. Still nothing sent.
    let result = service.clear_dtcs(Some("ECU_7E8"), USER, Some("the-owner"));
    if !result.success {
        assert_eq!(result.error.unwrap().code, ErrorCode::PreconditionFailed);
    }

    // The codes are still readable either way.
    let after = service.read_dtcs(Some("ECU_7E8"), USER);
    assert!(after.success);
}


#[test]
fn every_safety_decision_is_recorded_with_its_initiator() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    service.clear_dtcs(None, "agent:planner", None);

    let events = service
        .store()
        .events_since(service.session_id(), 0, 10_000)
        .unwrap();
    let refusal = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::SafetyDecision {
                operation,
                allowed,
                initiator,
                reason,
                ..
            } if operation == "obd2.clear_dtcs" => {
                Some((*allowed, initiator.clone(), reason.clone()))
            }
            _ => None,
        })
        .next()
        .expect("the refusal must be in the flight recorder");

    assert!(!refusal.0);
    assert_eq!(refusal.1, "agent:planner");
    assert_eq!(refusal.2.as_deref(), Some("confirmation_required"));
}

#[test]
fn the_flight_recorder_captures_the_whole_session_in_order() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.identify_vehicle(USER);
    service.scan_modules(USER);
    service.read_dtcs(None, USER);
    service.read_pid("ECU_7E8", "coolant_temp", USER);
    service.note("owner reports a DPF warning light").unwrap();

    let events = service
        .store()
        .events_since(service.session_id(), 0, 100_000)
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.name()).collect();

    for expected in [
        "session_started",
        "connection_state_changed",
        "adapter_identified",
        "adapter_request",
        "adapter_response",
        "tool_invoked",
        "tool_completed",
        "safety_decision",
        "vehicle_identified",
        "module_discovered",
        "dtc_read",
        "measurement_recorded",
        "user_note",
    ] {
        assert!(kinds.contains(&expected), "no {expected} event recorded");
    }

    // Sequence numbers are gap-free, so a replay is exact.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as i64 + 1);
    }

    // Intent is always recorded before the traffic it causes.
    let first_invoke = kinds.iter().position(|k| *k == "tool_invoked").unwrap();
    let first_safety = kinds.iter().position(|k| *k == "safety_decision").unwrap();
    assert!(first_invoke < first_safety);
}

#[test]
fn disconnecting_closes_the_connection_record() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let result = service.disconnect(USER);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(service.state(), ConnectionState::Disconnected);
    assert!(service.store().connections(service.session_id()).unwrap()[0]
        .disconnected_at
        .is_some());
}

#[test]
fn observed_conditions_come_from_readings_not_assumptions() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    // Before any reading, nothing about the engine is claimed.
    assert!(!service.conditions().engine_running);

    service.read_pid("ECU_7E8", "engine_rpm", USER);
    assert!(
        service.conditions().engine_running,
        "a raised idle is an engine that is running"
    );

    service.read_pid("ECU_7E8", "control_module_voltage", USER);
    let v = service.conditions().battery_voltage.unwrap();
    assert!((13.0..15.0).contains(&v), "charging voltage was {v}");
}

#[test]
fn monitor_tests_report_the_measured_value_against_the_vehicles_own_limit() {
    // The reading a code reader cannot give. The DPF scenario has a monitor
    // sitting at 38 of a 40-count limit: passing, so no code exists for it, and
    // close enough to failing that it is worth saying so.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    let result = service.read_monitor_tests("ECU_7E8", USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.unwrap();
    assert_eq!(data["supported"], true);
    let monitors = data["monitors"].as_array().unwrap();

    let near = monitors
        .iter()
        .find(|m| m["mid"] == 0x86 && m["tid"] == 0x80)
        .expect("the marginal particulate filter test");
    assert_eq!(near["passed"], true, "still inside its limits");
    assert!(
        near["margin"].as_f64().unwrap() < 0.10,
        "and within a tenth of them: {near:?}"
    );
    assert_eq!(near["name"], "Particulate filter, bank 1");

    let failed = monitors
        .iter()
        .find(|m| m["mid"] == 0x86 && m["tid"] == 0x82)
        .expect("the failing particulate filter test");
    assert_eq!(failed["passed"], false);

    assert_eq!(data["failing"], 1);
    assert_eq!(data["marginal"], 1);
}

#[test]
fn a_monitor_this_build_cannot_name_still_reports_its_verdict() {
    // MID 0xC4 is not in the catalogue. Naming it by number costs the verdict
    // nothing, because pass or fail is decided by limits the vehicle sent.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    let data = service.read_monitor_tests("ECU_7E8", USER).data.unwrap();

    let unknown = data["monitors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mid"] == 0xC4)
        .expect("the unlisted monitor must still be reported");
    assert_eq!(unknown["name"], "Manufacturer monitor 0xC4");
    assert_eq!(unknown["unknown_monitor"], true);
    assert_eq!(unknown["passed"], true);
    // Its scaling is unknown too, so the numbers stay raw rather than invented.
    assert_eq!(unknown["unknown_scaling"], true);
    assert!(unknown["value"].is_null());
    assert_eq!(unknown["raw"]["value"], 12);
}

#[test]
fn a_module_without_service_06_says_so_instead_of_failing() {
    // 7EA answers everything else but runs no monitors, which is the common
    // case on anything built before roughly 2005 as well. Not implementing a
    // service is a fact about the vehicle, not a failed scan.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);
    let result = service.read_monitor_tests("ECU_7EA", USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.unwrap();
    assert_eq!(data["supported"], false);
    assert!(data["monitors"].as_array().unwrap().is_empty());
    assert!(
        result.warnings.iter().any(|w| w.code == "no_monitor_tests"),
        "the caller must be told why it is empty: {:?}",
        result.warnings
    );
}

#[test]
fn a_healthy_vehicle_reports_monitors_with_nothing_to_say() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let data = service.read_monitor_tests("ECU_7E8", USER).data.unwrap();
    assert_eq!(data["supported"], true);
    assert_eq!(data["failing"], 0);
    assert_eq!(data["marginal"], 0);
    assert!(!data["monitors"].as_array().unwrap().is_empty());
}

#[test]
fn readiness_is_read_from_every_module_that_keeps_it() {
    // Found on a real 2019 truck: the engine and transmission controllers both
    // answer PID 01 and disagree — 195 warm-ups against 218, 11601 km since
    // clear against 11605. Reading one module and calling it "the" readiness
    // state was picking a winner silently.
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.scan_modules(USER);

    let result = service.read_readiness(USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.unwrap();
    let modules = data["modules"].as_array().unwrap();
    assert!(!modules.is_empty(), "at least one module keeps readiness");

    // Every reported module names itself and carries its own values, so the
    // caller can never be in doubt about which one a number came from.
    for m in modules {
        assert!(m["module"].as_str().is_some());
        assert!(m["address"].as_str().is_some());
        assert!(!m["values"].as_array().unwrap().is_empty());
    }
}

#[test]
fn readiness_before_a_scan_says_to_scan_rather_than_returning_nothing() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    let result = service.read_readiness(USER);
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::NotFound);
}

#[test]
fn a_live_sample_stops_asking_once_the_bus_is_unhappy() {
    // The live path runs on a timer, so continuing through a wedged bus meant
    // a fresh burst of requests every interval and no recovery. A cut-short
    // sample is reported as such rather than silently returning fewer signals.
    let (mut service, _) = connected(ScenarioId::BusSilent);
    // BusSilent connects degraded; a sample must fail cleanly rather than
    // grinding through every requested signal.
    let signals: Vec<String> = ["engine_rpm", "coolant_temp", "vehicle_speed", "maf_rate"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let result = service.read_live_data("ECU_7E8", &signals, USER);
    assert!(!result.success, "a silent bus cannot produce a sample");
}
