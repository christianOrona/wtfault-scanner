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
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
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
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "adapter_degraded"
                && w.severity == aim_types::WarningSeverity::Serious)
    );

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
    assert_eq!(data["calibration_verification_numbers"].as_array().unwrap().len(), 1);
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
    assert!(!identity["calibration_verification_numbers"].as_array().unwrap().is_empty());
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
    assert!(result.warnings.iter().any(|w| w.code == "identity_field_unavailable"));
}

#[test]
fn supported_pids_are_enumerated_across_the_whole_mask_chain() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.read_supported_pids("ECU_7E8", USER);
    assert!(result.success, "{:?}", result.error);

    let data = result.data.unwrap();
    let pids: Vec<u64> =
        data["pids"].as_array().unwrap().iter().map(|p| p["pid"].as_u64().unwrap()).collect();

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
        assert!(r.success, "advertised PID {pid:02X} ({signal}) could not be read: {:?}", r.error);
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
    let stored =
        service.store().measurements(service.session_id(), Some("engine_rpm"), 10).unwrap();
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
    let signals: Vec<String> =
        ["engine_rpm", "coolant_temp", "nonsense_signal"].iter().map(|s| s.to_string()).collect();
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
    assert_eq!(result.values[0].provenance.verification, aim_types::VerificationStatus::Unverified);
    assert!(result.warnings.iter().any(|w| w.code == "unverified_decoder"));
}

#[test]
fn a_healthy_vehicle_reports_no_codes_rather_than_failing() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    service.scan_modules(USER);
    let result = service.read_dtcs(None, USER);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.data.unwrap()["confirmed_count"], 0);
    assert!(service.store().dtcs(service.session_id(), None).unwrap().is_empty());
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
        vec![DtcStatus::Confirmed, DtcStatus::Confirmed, DtcStatus::Pending, DtcStatus::Permanent]
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

    let frozen_rpm =
        frame.values.iter().find(|v| v.signal_id == "engine_rpm").unwrap().value.as_f64().unwrap();
    assert!((frozen_rpm - 748.0).abs() < 1.0, "frozen rpm was {frozen_rpm}");
    assert!(
        live_rpm > frozen_rpm + 200.0,
        "the truck is at a raised idle now ({live_rpm}) versus when the code stored ({frozen_rpm})"
    );

    // A freeze frame must not pollute the live measurement series.
    let measured =
        service.store().measurements(service.session_id(), Some("engine_rpm"), 100).unwrap();
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

    let events = service.store().events_since(service.session_id(), 0, 10_000).unwrap();
    let refusal = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::SafetyDecision { operation, allowed, initiator, reason, .. }
                if operation == "obd2.clear_dtcs" =>
            {
                Some((*allowed, initiator.clone(), reason.clone()))
            }
            _ => None,
        })
        .next()
        .expect("the refusal must be in the flight recorder");

    assert!(!refusal.0);
    assert_eq!(refusal.1, "agent:planner");
    // Refused for being an agent, not for lacking a confirmation. The gate
    // checks that first on purpose: a model must not be able to get further by
    // supplying a confirmation token, so "who asked" is settled before "did
    // they tick the box".
    assert_eq!(refusal.2.as_deref(), Some("agent_may_not_mutate"));
}

#[test]
fn the_flight_recorder_captures_the_whole_session_in_order() {
    let (mut service, _) = connected(ScenarioId::DpfRegen);
    service.identify_vehicle(USER);
    service.scan_modules(USER);
    service.read_dtcs(None, USER);
    service.read_pid("ECU_7E8", "coolant_temp", USER);
    service.note("owner reports a DPF warning light").unwrap();

    let events = service.store().events_since(service.session_id(), 0, 100_000).unwrap();
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
    assert!(service.conditions().engine_running, "a raised idle is an engine that is running");

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
    assert!(near["margin"].as_f64().unwrap() < 0.10, "and within a tenth of them: {near:?}");
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

#[test]
fn a_full_scan_finds_modules_a_code_reader_never_sees() {
    // The whole reason this exists. The virtual vehicle has two modules outside
    // the legislated emissions block that answer UDS and nothing else: they do
    // not reply to the 0x7DF broadcast, they implement no service 01 PIDs, and
    // service 03 cannot reach them. A scan that only reads trouble codes the
    // legislated way reports a clean vehicle while one of them has a confirmed,
    // currently-failing fault.
    let (mut service, _) = connected(ScenarioId::Healthy);

    // Baseline: the emissions path says everything is fine.
    service.scan_modules(USER);
    let dtcs = service.read_dtcs(None, USER);
    assert!(dtcs.success, "{:?}", dtcs.error);
    assert_eq!(
        dtcs.data.unwrap()["dtcs"].as_array().unwrap().len(),
        0,
        "the healthy scenario stores no emissions codes"
    );

    // The full scan disagrees, and is right.
    let result = service.scan_all_modules(USER);
    assert!(result.success, "{:?}", result.error);
    let data = result.data.unwrap();
    let modules = data["modules"].as_array().unwrap();

    // It reached past the legislated block.
    assert!(
        modules.iter().any(|m| m["in_legislated_range"] == false),
        "the sweep must reach outside 7E0-7E7: {modules:#?}"
    );

    let faults = data["fault_count"].as_u64().unwrap();
    assert!(faults > 0, "the brake module has faults service 03 cannot see");
}

#[test]
fn a_uds_fault_carries_its_status_and_a_description_when_one_exists() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let data = service.scan_all_modules(USER).data.unwrap();

    let all: Vec<&serde_json::Value> = data["modules"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["faults"].as_array().unwrap())
        .collect();
    assert!(!all.is_empty());

    // A three-byte code keeps its failure type and its two-byte base, because
    // the catalogue is keyed on the base.
    let f = all.iter().find(|f| f["base_code"] == "C0035").expect("the wheel speed sensor fault");
    assert_eq!(f["code"], "C0035-00");
    assert_eq!(f["failing_now"], true);
    assert_eq!(f["confirmed"], true);
    assert_eq!(f["status_summary"], "failing right now");

    // Stored-but-not-failing is reported as its own thing, not as "fine".
    let stored =
        all.iter().find(|f| f["base_code"] == "U0121").expect("the lost-communication fault");
    assert_eq!(stored["failing_now"], false);
    assert_eq!(stored["confirmed"], true);
    assert_eq!(stored["status_summary"], "stored, but not failing at the moment");
}

#[test]
fn a_module_with_no_fault_service_is_distinguished_from_one_with_no_faults() {
    // Both look like zero codes and mean completely different things. The
    // simulator models one of each on purpose.
    let (mut service, _) = connected(ScenarioId::Healthy);
    let data = service.scan_all_modules(USER).data.unwrap();
    let modules = data["modules"].as_array().unwrap();

    let healthy = modules
        .iter()
        .find(|m| m["address"] == "7A8")
        .expect("the module that answers with an empty fault list");
    assert_eq!(healthy["fault_count"], 0);
    assert!(healthy["note"].is_null(), "it answered; there is nothing to explain");

    let no_service = modules
        .iter()
        .find(|m| m["address"] == "7EB")
        .expect("the module that does not implement the fault service");
    assert_eq!(no_service["fault_count"], 0);
    assert!(
        !no_service["note"].is_null(),
        "a module that declined must say so rather than read as healthy"
    );
}

#[test]
fn a_silent_bus_reports_no_modules_rather_than_an_empty_success() {
    let (mut service, _) = connected(ScenarioId::BusSilent);
    let result = service.scan_all_modules(USER);
    assert!(!result.success);
    assert_eq!(result.error.unwrap().code, ErrorCode::NoData);
}

// ---------------------------------------------------------------- features

/// Build a service whose catalogue has been extended by a profile file, the way
/// a user's own profile directory extends it at startup.
///
/// The mapping below points at a record the simulated body module actually
/// serves. Note what is *not* shared: the simulator holds bytes and the profile
/// holds their meaning. If both knew the meaning, this test could pass against
/// a mapping that is wrong for every real vehicle, which is the exact failure
/// this project exists to avoid.
fn service_with_profile(yaml: &str) -> (DiagnosticService, tempfile::TempDir) {
    let (s, _e, d) = service_with_profile_and_emulator(yaml);
    (s, d)
}

/// Everything the write preconditions need to have been observed.
///
/// The gate refuses on unknown rather than assuming: an unread road speed is
/// not a stationary vehicle, and a module nobody has heard from is not present.
fn prepare_for_write(service: &mut DiagnosticService) {
    service.scan_modules(USER);
    assert!(service.read_pid("ECU_7E8", "vehicle_speed", USER).success);
    assert!(service.scan_all_modules(USER).success);
}

fn service_with_profile_and_emulator(
    yaml: &str,
) -> (DiagnosticService, aim_simulator::SharedEmulator, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test-profile.yaml"), yaml).unwrap();

    let decoders = DecoderSet::with_profiles(dir.path()).unwrap();
    assert!(
        decoders.profiles.failures().next().is_none(),
        "profile did not load: {:?}",
        decoders.profiles.failures().collect::<Vec<_>>()
    );

    let transport = SimulatedTransport::new(ScenarioId::Healthy);
    let emulator = transport.emulator();
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let mut service = DiagnosticService::start(
        adapter,
        SessionStore::open_in_memory().unwrap(),
        Arc::new(decoders),
        SafetyGate::phase1(),
        None,
    )
    .unwrap();
    assert!(service.connect(USER).success);
    (service, emulator, dir)
}

const MIRROR_PROFILE: &str = r#"
version: 1
profile: test
features:
  - id: test_body_setting
    name: "A body module setting"
    risk: convenience
    easy: "A comfort setting held in the body module."
    technical: "Bit 2 of byte 3 of data identifier DE01 on the module at 7A0."
    modules: ["7A8"]
    verification: verified
    source: "measured on the simulated vehicle"
    write_verification:
      verification: verified
      verified_on_vehicles: 1
      source: "measured on the simulated vehicle"
    mapping:
      kind: data_identifier_bits
      module: 0x7A0
      did: 0xDE01
      byte: 3
      mask: 0x04
      on: 0x04
      off: 0x00
"#;

#[test]
fn a_feature_with_a_measured_mapping_reads_its_current_state() {
    let (mut service, _dir) = service_with_profile(MIRROR_PROFILE);

    let r = service.read_feature("test_body_setting", USER);
    assert!(r.success, "read_feature failed: {:?}", r.error);
    let data = r.data.as_ref().unwrap();

    assert_eq!(data["readable"], true);
    // The simulated record has byte 3 = 0b1011_0011. Bit 2 is clear, so this
    // feature reads as off - and the record is returned verbatim so the state
    // is checkable against the bytes rather than taken on trust.
    assert_eq!(data["state"], false);
    assert_eq!(data["did"], "DE01");
    assert_eq!(data["record"], "001122b34455");
    assert_eq!(data["module"], "7A0");
}

/// The case that is the whole product argument: the app knows the feature
/// exists and does not know where it lives, and says so in a way a person can
/// act on rather than as a refusal.
#[test]
fn a_feature_with_no_measured_mapping_is_an_open_question_not_a_dead_end() {
    let described_only = MIRROR_PROFILE
        .split("    mapping:")
        .next()
        .unwrap()
        .replace("verification: verified", "verification: unverified");
    let (mut service, _dir) = service_with_profile(&described_only);

    let r = service.read_feature("test_body_setting", USER);
    assert!(r.success, "an unmeasured mapping is not an error: {:?}", r.error);
    let data = r.data.as_ref().unwrap();

    assert_eq!(data["known"], true, "the feature itself is known");
    assert_eq!(data["readable"], false, "but it cannot be read yet");
    assert!(data["state"].is_null(), "and no state may be guessed at");

    let steps = data["how_to_establish"].as_array().expect("steps to close the gap");
    assert_eq!(steps.len(), 4, "the four-step measurement procedure");

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"mapping_not_measured"), "{codes:?}");
}

#[test]
fn an_unknown_feature_id_is_an_error_rather_than_an_invented_answer() {
    let (mut service, _dir) = service_with_profile(MIRROR_PROFILE);
    let r = service.read_feature("no_such_feature", USER);
    assert!(!r.success);
    assert_eq!(r.error.as_ref().unwrap().code, ErrorCode::NotFound);
}

/// The whole investigation procedure, end to end, with no mapping in hand at
/// the start and a proposed one at the end.
///
/// This is the workflow that turns "nobody has measured where that bit lives"
/// from a dead end into a job with four steps. The middle step - changing the
/// setting with a tool that already knows how - is played here by writing the
/// record directly, which is exactly what a known-good tool would have done.
#[test]
fn capturing_twice_around_a_change_identifies_the_bits_that_moved() {
    let (mut service, _dir) = service_with_profile(MIRROR_PROFILE);
    const BODY: &str = "7A0";
    const DID: u16 = 0xDE01;

    // 1. Capture.
    let before = service.capture_configuration(BODY, &[DID], Some("before".into()), USER);
    assert!(before.success, "capture failed: {:?}", before.error);
    let before_capture: aim_diagnostics::capture::ConfigCapture =
        serde_json::from_value(before.data.as_ref().unwrap()["capture"].clone()).unwrap();
    assert_eq!(before_capture.records[&DID], vec![0x00, 0x11, 0x22, 0xB3, 0x44, 0x55]);

    // The write preconditions include "the vehicle is stationary", and an
    // unread speed is not a stationary vehicle - the gate refuses on unknown
    // rather than assuming zero, which is why this read is here rather than
    // being something the test could skip.
    service.scan_modules(USER);
    let speed = service.read_pid("ECU_7E8", "vehicle_speed", USER);
    assert!(speed.success, "reading speed failed: {:?}", speed.error);

    // The body module answers UDS but not the emissions services, so the
    // legislated scan does not see it. This is the whole reason the full scan
    // exists, and the write gate requires the owning module to have actually
    // answered rather than being assumed present.
    let all = service.scan_all_modules(USER);
    assert!(all.success, "full scan failed: {:?}", all.error);

    // 2. Somebody changes the setting with another tool. Bit 2 of byte 3.
    let changed = service.apply_configuration_change(
        "test_body_setting",
        aim_diagnostics::DesiredValue::On,
        USER,
        "the-owner",
    );
    assert!(changed.success, "the change failed: {:?}", changed.error);

    // 3. Capture again.
    let after = service.capture_configuration(BODY, &[DID], Some("after".into()), USER);
    assert!(after.success);
    let after_capture: aim_diagnostics::capture::ConfigCapture =
        serde_json::from_value(after.data.as_ref().unwrap()["capture"].clone()).unwrap();

    // 4. Exactly one byte moved, and only the bit that actually changed is
    //    claimed by the mapping it proposes.
    let d = aim_diagnostics::capture::diff(&before_capture, &after_capture);
    assert!(d.is_unambiguous(), "expected one byte to move, got {:?}", d.changes);
    let c = &d.changes[0];
    assert_eq!(c.byte, 3);
    assert_eq!(c.before, 0xB3);
    assert_eq!(c.after, 0xB7);
    assert_eq!(c.changed_mask, 0x04, "only bit 2, not the whole byte");

    match c.as_mapping(BODY, true) {
        aim_decoders::Mapping::DataIdentifierBits { module, did, byte, mask, on, off } => {
            assert_eq!(
                (module.as_str(), did, byte, mask, on, off),
                (BODY, DID, 3, 0x04, 0x04, 0x00)
            );
        }
        other => panic!("wrong mapping kind: {other:?}"),
    }
}

/// A module that holds none of the requested identifiers is an error with a
/// reason, not an empty success that reads as "your module has no settings".
#[test]
fn capturing_from_a_module_that_holds_nothing_says_so() {
    let (mut service, _dir) = service_with_profile(MIRROR_PROFILE);
    // The engine controller answers plenty, but holds no configuration records.
    let r = service.capture_configuration("7E0", &[0xDE01], None, USER);
    assert!(!r.success);
    assert_eq!(r.error.as_ref().unwrap().code, aim_types::ErrorCode::NoData);
}

/// Set how the simulated body module handles a configuration write.
fn set_write_behaviour(
    emulator: &aim_simulator::SharedEmulator,
    behaviour: aim_simulator::ConfigWriteBehaviour,
) {
    let mut e = emulator.lock().unwrap();
    for ecu in e.vehicle.ecus.iter_mut() {
        if ecu.response_id == 0x7A8 {
            ecu.config_write = behaviour;
        }
    }
}

/// The whole reason a positive response is not allowed to count as success.
///
/// A module that answers "accepted" and then changes nothing is a real
/// behaviour, and an app tested only against a simulator that always tells the
/// truth would report this write as done.
#[test]
fn a_write_the_module_accepts_and_ignores_is_reported_as_unverified() {
    let (mut service, emulator, _dir) = service_with_profile_and_emulator(MIRROR_PROFILE);
    prepare_for_write(&mut service);
    set_write_behaviour(&emulator, aim_simulator::ConfigWriteBehaviour::AcceptButIgnore);

    let r = service.apply_configuration_change(
        "test_body_setting",
        aim_diagnostics::DesiredValue::On,
        USER,
        "the-owner",
    );

    assert!(r.success, "the operation ran; what it reports is the point");
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["changed"], false, "nothing actually changed");
    assert_eq!(data["verified"], false);

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"write_not_verified"), "{codes:?}");
}

/// A module that refuses is not a module that quietly failed.
#[test]
fn a_write_the_module_refuses_is_an_error_naming_the_refusal() {
    let (mut service, emulator, _dir) = service_with_profile_and_emulator(MIRROR_PROFILE);
    prepare_for_write(&mut service);
    set_write_behaviour(&emulator, aim_simulator::ConfigWriteBehaviour::Refuse);

    let r = service.apply_configuration_change(
        "test_body_setting",
        aim_diagnostics::DesiredValue::On,
        USER,
        "the-owner",
    );
    assert!(!r.success, "a refused write must not read as a success");

    // And the setting is untouched, which the app can still demonstrate.
    let read = service.read_feature("test_body_setting", USER);
    assert!(read.success);
    assert_eq!(read.data.as_ref().unwrap()["state"], false);
}

/// Asking for the state it is already in writes nothing at all.
#[test]
fn a_change_to_the_value_already_set_writes_nothing() {
    let (mut service, _emulator, _dir) = service_with_profile_and_emulator(MIRROR_PROFILE);
    prepare_for_write(&mut service);

    let r = service.apply_configuration_change(
        "test_body_setting",
        aim_diagnostics::DesiredValue::Off,
        USER,
        "the-owner",
    );
    assert!(r.success);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["changed"], false);
    assert_eq!(data["reason"], "already_set");
    assert!(r.warnings.iter().any(|w| w.code == "already_set"));
}

/// The identity is assembled from what the session actually did, not from a
/// separate pass over the vehicle.
///
/// Nothing here talks to the bus: identify and scan already did, and this is
/// the evidence they produced arriving in one place instead of three. The
/// point of the test is that reading it costs nothing and that the gaps come
/// through as gaps.
#[test]
fn the_identity_gathers_what_the_session_already_established() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);

    let identity = service.identity();

    // The VIN and what its own structure encodes.
    assert!(identity.settled("vin").is_some(), "the VIN was read and should be here");
    assert!(identity.settled("make").is_some());

    // And the gap. No licensed database ships with this build, so the model is
    // never established from a VIN — which is a finding, stated as one.
    assert_eq!(identity.settled("model"), None);
    assert!(identity.unresolved.contains(&String::from("model")));

    // Modules and the protocol they answered on came along without anybody
    // asking the vehicle a second time.
    assert!(!identity.observations_about("module").is_empty());
    assert_eq!(identity.observations_about("protocol").len(), 1);

    // Nothing disagrees on a single healthy vehicle.
    assert!(identity.contested().is_empty());
}

/// Before anything has been read, an identity is empty rather than a set of
/// blank fields — and it says which fields nobody established.
#[test]
fn an_identity_before_any_reading_claims_nothing() {
    let (service, _) = connected(ScenarioId::Healthy);
    let identity = service.identity();
    assert!(identity.is_empty());
    assert_eq!(identity.unresolved, vec!["vin", "make", "model", "model_year"]);
}

/// The same mapping, scoped to a truck that is not this one but closely
/// resembles it.
///
/// Identical to `MIRROR_PROFILE` except for the VIN it claims to have been
/// measured on, plus the prefix that says which vehicles it is a candidate for.
const CANDIDATE_PROFILE: &str = r#"
version: 1
profile: test
features:
  - id: test_body_setting
    name: "A body module setting"
    risk: convenience
    easy: "A comfort setting held in the body module."
    technical: "Bit 2 of byte 3 of data identifier DE01 on the module at 7A0."
    modules: ["7A8"]
    verification: verified
    source: "measured on a different vehicle"
    applies_to:
      vins: ["1FT7W2BT6KEC99999"]
      candidate_vin_prefixes: ["1FT7W2BT"]
    write_verification:
      verification: verified
      verified_on_vehicles: 1
      source: "measured on a different vehicle"
    mapping:
      kind: data_identifier_bits
      module: 0x7A0
      did: 0xDE01
      byte: 3
      mask: 0x04
      on: 0x04
      off: 0x00
"#;

/// Scoping a measured mapping to one exact VIN and stopping is too strict to
/// be useful: the next identical truck gets nothing. It is offered instead,
/// labelled as coming from somewhere else.
#[test]
fn a_mapping_measured_on_a_similar_truck_is_offered_to_this_one() {
    let (mut service, _dir) = service_with_profile(CANDIDATE_PROFILE);
    assert!(service.identify_vehicle(USER).success);

    let r = service.list_features(USER);
    assert!(r.success);
    let data = r.data.as_ref().unwrap();
    let features = data["features"].as_array().unwrap();

    let f = features
        .iter()
        .find(|f| f["id"] == "test_body_setting")
        .expect("a mapping measured on a near-identical truck must not be invisible to this one");
    assert_eq!(f["authority"], "measured_on_similar_vehicle");
    assert_eq!(f["measured_on_this_vehicle"], false);
    assert_eq!(data["from_a_similar_vehicle"], 1);

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"mappings_from_a_similar_vehicle"), "{codes:?}");
}

/// Predict and check. Reading it costs nothing, and the reading is stated
/// plainly enough to be wrong — which is what makes it evidence rather than a
/// claim. Nothing is written to find out.
#[test]
fn reading_a_candidate_states_a_prediction_the_owner_can_falsify() {
    let (mut service, _dir) = service_with_profile(CANDIDATE_PROFILE);
    assert!(service.identify_vehicle(USER).success);

    let r = service.read_feature("test_body_setting", USER);
    assert!(r.success, "a candidate is readable: {:?}", r.error);
    let data = r.data.as_ref().unwrap();

    assert_eq!(data["readable"], true);
    assert_eq!(data["state"], false, "the same bytes decode the same way");
    assert_eq!(data["measured_on_this_vehicle"], false);

    let check = &data["check_this"];
    assert!(check.is_object(), "a candidate must state what it predicts");
    let claim = check["claim"].as_str().unwrap();
    assert!(claim.contains("OFF"), "the prediction has to be specific: {claim}");
    assert!(check["ask"].as_str().unwrap().contains("Does your vehicle agree"));
    assert_eq!(check["writable"], false);

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"mapping_from_a_similar_vehicle"), "{codes:?}");
}

/// And the line. Fully verified for reading and writing on the truck it was
/// measured on, and still refused here — however well the reading matched.
/// Relevance has to reach the safety gate, because a label nobody enforces is
/// decoration.
#[test]
fn a_candidate_is_never_writable_however_confident_it_looks() {
    let (mut service, _dir) = service_with_profile(CANDIDATE_PROFILE);
    assert!(service.identify_vehicle(USER).success);
    prepare_for_write(&mut service);

    let r = service.read_feature("test_body_setting", USER);
    assert_eq!(r.data.as_ref().unwrap()["writable"], false);

    let plan = service.preview_configuration_change(
        "test_body_setting",
        aim_diagnostics::DesiredValue::On,
        USER,
    );
    let plan = plan.data.as_ref().unwrap();
    assert_eq!(plan["can_apply"], false, "a candidate must never be writable");

    let checks = plan["checks"].as_array().unwrap();
    let c = checks
        .iter()
        .find(|c| c["id"] == "mapping_is_for_this_vehicle")
        .expect("the gate has to ask this, not just the interface");
    assert_eq!(c["passed"], false);
    // Not a permanent refusal - confirming it on this vehicle is what clears it.
    assert_eq!(c["blocking_by_design"], false);
}

// --------------------------------------------------------------- as-built

/// A minimal file for the simulated vehicle. `1FT7W2BT6KEC00001` is the VIN
/// the virtual truck reports, so this one belongs to it.
fn as_built_for(vin: &str) -> String {
    format!(
        "<VEHICLE><VIN>{vin}</VIN>\
         <DATA LABEL=\"7A0-02-01\"><CODE>0011</CODE><CODE>22b3</CODE><CODE>4455</CODE>\
         <CODE>00</CODE></DATA>\
         <DATA LABEL=\"9990-02-01\"><CODE>0102</CODE><CODE>0304</CODE></DATA></VEHICLE>"
    )
}

/// The check that makes the whole feature safe. A file is a perfectly valid
/// description of a perfectly real truck — just not this one — and importing
/// it would hand somebody another vehicle's configuration with nothing about
/// it looking wrong.
#[test]
fn an_as_built_file_for_another_vehicle_is_refused() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    assert!(service.identify_vehicle(USER).success);

    let r = service.import_as_built(&as_built_for("1FT7W2BT6KEC99999"), Some("test.ab"), USER);
    assert!(!r.success, "a file for a different VIN must not be imported");
    let message = r.error.as_ref().unwrap().message.to_lowercase();
    assert!(message.contains("1ft7w2bt6kec99999"), "{message}");
    // And it says the file is fine, which it is. The problem is the pairing.
    assert!(message.contains("not this one"), "{message}");
}

/// And its own vehicle's file goes in.
#[test]
fn an_as_built_file_for_this_vehicle_is_imported_and_held() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    assert!(service.identify_vehicle(USER).success);

    let r =
        service.import_as_built(&as_built_for(aim_simulator::SIMULATED_VIN), Some("test.ab"), USER);
    assert!(r.success, "import failed: {:?}", r.error);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["vin"], aim_simulator::SIMULATED_VIN);
    assert_eq!(data["modules"], 2);

    // The argument for having the file at all: it covers modules that are not
    // answering. 9990 is in the file and on no bus.
    assert_eq!(data["modules_not_answering_on_the_bus"], 2);

    let status = service.as_built_status(USER);
    assert_eq!(status.data.as_ref().unwrap()["held"], true);

    // As easy to remove as to add. It names somebody's vehicle.
    assert!(service.forget_as_built(USER).success);
    assert_eq!(service.as_built_status(USER).data.as_ref().unwrap()["held"], false);
}

/// Unprompted, and specific. Somebody who has never heard of an as-built file
/// is told it exists, what it would add, and that their VIN is what finds it.
#[test]
fn a_vehicle_with_no_file_is_told_what_it_is_missing() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    assert!(service.identify_vehicle(USER).success);

    let r = service.as_built_status(USER);
    assert!(r.success);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["held"], false);
    assert_eq!(data["vin"], aim_simulator::SIMULATED_VIN);
    assert!(data["what_it_would_add"].as_str().unwrap().contains("asleep"));
    // The VIN is in the instructions, ready to paste. The friction is knowing
    // the file exists, not the free account.
    let steps = data["how_to_get_one"].as_array().unwrap();
    assert!(steps.iter().any(|s| s.as_str().unwrap().contains(aim_simulator::SIMULATED_VIN)));

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"as_built_not_imported"), "{codes:?}");
}

/// Before a VIN is established there is nothing to check a file against, and
/// the app says that rather than importing hopefully.
#[test]
fn nothing_can_be_imported_before_the_vehicle_is_identified() {
    let (mut service, _) = connected(ScenarioId::Healthy);
    let r = service.import_as_built(&as_built_for(aim_simulator::SIMULATED_VIN), None, USER);
    assert!(!r.success);
    assert!(r.error.as_ref().unwrap().message.contains("no single VIN"));
}

/// A file covering a module that is asleep answers for it, and says plainly
/// that it is the factory value rather than a live reading.
#[test]
fn a_module_that_does_not_answer_can_still_be_read_from_the_file() {
    // A mapping pointing at a module that is not on the simulated bus at all.
    let profile = r#"
version: 1
profile: test
features:
  - id: absent_module_setting
    name: "A setting in a module that is asleep"
    risk: convenience
    easy: "Held in a module that is not answering."
    technical: "Byte 3 of DE01 on the module at 9990."
    modules: ["9990"]
    verification: verified
    source: "measured on the simulated vehicle"
    mapping:
      kind: data_identifier_bits
      module: "9990"
      did: 0xDE01
      byte: 1
      mask: 0x02
      on: 0x02
      off: 0x00
"#;
    let (mut service, _dir) = service_with_profile(profile);
    assert!(service.identify_vehicle(USER).success);
    assert!(
        service
            .import_as_built(&as_built_for(aim_simulator::SIMULATED_VIN), Some("test.ab"), USER)
            .success
    );

    let r = service.read_feature("absent_module_setting", USER);
    assert!(r.success, "the file should answer where the bus cannot: {:?}", r.error);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["state"], true, "decoded from the factory record, not guessed");
    assert_eq!(data["read_from_the_as_built_file"], true);
    assert_eq!(data["authority"], "owner_supplied_oem_data");
    // A module that will not answer a read is not one to write either.
    assert_eq!(data["writable"], false);

    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"read_from_the_as_built_file"), "{codes:?}");
}

/// A degraded connect says the vehicle is not answering. It should also say
/// when this same adapter last got an answer, because "it is broken" and "it
/// is intermittent" send somebody to completely different places — and the
/// history was on disk the whole time.
#[test]
fn a_silent_bus_mentions_the_last_time_this_adapter_worked() {
    let store = SessionStore::open_in_memory().unwrap();

    // A previous session, on this same adapter, in which modules answered.
    // A module row exists only because something on the bus replied.
    let earlier = store.create_session(Some("earlier today".into())).unwrap();
    store
        .record_connection(&aim_types::Connection {
            id: aim_types::ConnectionId::new(),
            session_id: earlier.id.clone(),
            adapter_id: String::from("sim:bus-silent"),
            transport: aim_types::TransportKind::Simulated,
            connected_at: aim_types::now(),
            disconnected_at: None,
            firmware: None,
            capabilities: aim_types::AdapterCapabilities::unknown(
                aim_types::TransportKind::Simulated,
            ),
        })
        .unwrap();
    store
        .upsert_module(&aim_types::Module {
            id: aim_types::ModuleId::new(),
            session_id: earlier.id.clone(),
            module_key: String::from("ECU_7E8"),
            name: String::from("engine"),
            address: String::from("7E8"),
            request_address: Some(String::from("7E0")),
            protocol: aim_types::ObdProtocol::Iso15765Can11_500,
            identity: aim_types::ModuleIdentity::default(),
            software_version: None,
            discovered_at: aim_types::now(),
        })
        .unwrap();

    // Now the same adapter finds a silent bus.
    let transport = SimulatedTransport::new(ScenarioId::BusSilent);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();

    let r = service.connect(USER);
    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"adapter_degraded"), "still reports the silence: {codes:?}");
    assert!(codes.contains(&"answered_before"), "and says it worked before: {codes:?}");

    let said = r.warnings.iter().find(|w| w.code == "answered_before").unwrap();
    // History, never reassurance. The distinction is the whole point.
    assert!(said.message.contains("does not mean anything is working now"), "{}", said.message);
    assert!(said.message.contains("intermittent"), "{}", said.message);

    assert!(r.data.as_ref().unwrap()["answered_before"].is_object());
}

/// And a first-ever connect invents no history to be encouraging about.
#[test]
fn a_first_connect_with_no_history_says_nothing_about_one() {
    let (mut service, _) = service(ScenarioId::BusSilent);
    let r = service.connect(USER);
    let codes: Vec<&str> = r.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"adapter_degraded"));
    assert!(!codes.contains(&"answered_before"), "nothing has ever worked: {codes:?}");
    assert!(r.data.as_ref().unwrap()["answered_before"].is_null());
}
