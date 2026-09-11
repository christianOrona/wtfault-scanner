//! Session store behaviour.

use aim_session::{measurement_from, SessionStore};
use aim_types::{
    now, AdapterCapabilities, AgentTrace, ConnectionId, ConnectionState, DecodedValue, Diagnosis,
    DiagnosisId, DtcRecord, DtcStatus, ErrorCode, EventKind, Module, ModuleId, ModuleIdentity,
    ObdProtocol, PermissionLevel, Provenance, SessionId, TestOutcome, TestRun, TestRunId,
    Timestamp, TransportKind, Value, Vehicle, VerificationStatus,
};

fn store() -> SessionStore {
    SessionStore::open_in_memory().unwrap()
}

fn module(session: &SessionId, key: &str, address: &str) -> Module {
    Module {
        id: ModuleId::new(),
        session_id: session.clone(),
        module_key: key.to_string(),
        name: format!("OBD module at {address}"),
        address: address.to_string(),
        request_address: None,
        protocol: ObdProtocol::Iso15765Can11_500,
        identity: ModuleIdentity::default(),
        software_version: None,
        discovered_at: now(),
    }
}

#[test]
fn creating_a_session_records_its_first_event() {
    let s = store();
    let session = s.create_session(Some("bench test".into())).unwrap();
    assert!(session.id.as_str().starts_with("ses_"));
    assert_eq!(s.event_count(&session.id).unwrap(), 1);

    let events = s.events_since(&session.id, 0, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert!(matches!(
        &events[0].kind,
        EventKind::SessionStarted { label } if label.as_deref() == Some("bench test")
    ));
}

#[test]
fn event_sequence_numbers_are_gap_free_and_ordered() {
    let s = store();
    let session = s.create_session(None).unwrap();
    for i in 0..50 {
        s.append_event(&session.id, EventKind::AdapterRequest { command: format!("010{i:X}") })
            .unwrap();
    }
    let events = s.events_since(&session.id, 0, 1000).unwrap();
    assert_eq!(events.len(), 51);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as i64 + 1, "sequence must be gap-free");
    }
}

#[test]
fn events_are_isolated_per_session() {
    let s = store();
    let a = s.create_session(None).unwrap();
    let b = s.create_session(None).unwrap();
    s.append_event(&a.id, EventKind::SessionEnded).unwrap();

    assert_eq!(s.event_count(&a.id).unwrap(), 2);
    assert_eq!(s.event_count(&b.id).unwrap(), 1);
    // Each session numbers its own events from 1.
    assert_eq!(s.events_since(&b.id, 0, 10).unwrap()[0].seq, 1);
}

#[test]
fn events_can_be_read_incrementally_which_is_what_the_websocket_needs() {
    let s = store();
    let session = s.create_session(None).unwrap();
    for _ in 0..5 {
        s.append_event(&session.id, EventKind::SessionEnded).unwrap();
    }
    let first = s.events_since(&session.id, 0, 3).unwrap();
    assert_eq!(first.len(), 3);
    let rest = s.events_since(&session.id, first.last().unwrap().seq, 100).unwrap();
    assert_eq!(rest.len(), 3);
    assert_eq!(rest[0].seq, 4);
}

#[test]
fn an_event_row_id_resolves_back_to_the_event_it_is_evidence_for() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let id = s
        .append_event(
            &session.id,
            EventKind::AdapterResponse {
                command: "0100".into(),
                lines: vec!["7E8 06 41 00 BE 3F A8 13".into()],
                elapsed_ms: 31,
                classification: "data".into(),
            },
        )
        .unwrap();

    let event = s.event_by_id(id).unwrap();
    match event.kind {
        EventKind::AdapterResponse { command, lines, .. } => {
            assert_eq!(command, "0100");
            assert_eq!(lines[0], "7E8 06 41 00 BE 3F A8 13");
        }
        other => panic!("wrong event: {other:?}"),
    }
    assert_eq!(s.event_by_id(999_999).unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn subscribers_receive_events_as_they_are_appended() {
    let s = store();
    let mut rx = s.subscribe();
    let session = s.create_session(None).unwrap();

    let first = rx.recv().await.unwrap();
    assert_eq!(first.session_id, session.id);
    assert_eq!(first.seq, 1);
    assert!(first.id.is_some(), "published events carry their row id");

    s.append_event(&session.id, EventKind::UserNote { text: "unplugged the sensor".into() })
        .unwrap();
    let second = rx.recv().await.unwrap();
    assert_eq!(second.seq, 2);
}

#[test]
fn ending_a_session_is_idempotent_and_history_does_not_change() {
    let s = store();
    let session = s.create_session(None).unwrap();
    s.end_session(&session.id).unwrap();
    let first_end = s.get_session(&session.id).unwrap().ended_at.unwrap();
    s.end_session(&session.id).unwrap();
    assert_eq!(
        s.get_session(&session.id).unwrap().ended_at.unwrap(),
        first_end,
        "the first end time must win"
    );
}

#[test]
fn a_missing_session_is_not_found_rather_than_an_empty_one() {
    let s = store();
    let err = s.get_session(&SessionId::from_string("ses_nope")).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn vehicles_are_deduplicated_by_vin_but_never_merged_without_one() {
    let s = store();
    let a = s.upsert_vehicle(&Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()))).unwrap();
    let b = s.upsert_vehicle(&Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()))).unwrap();
    assert_eq!(a.id, b.id, "the same VIN is the same vehicle");

    let x = s.upsert_vehicle(&Vehicle::from_vin(None)).unwrap();
    let y = s.upsert_vehicle(&Vehicle::from_vin(None)).unwrap();
    assert_ne!(x.id, y.id, "unidentified vehicles must not be merged");
}

#[test]
fn a_vehicle_attached_to_a_session_shows_up_in_the_session_list() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let vehicle = s.upsert_vehicle(&Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()))).unwrap();
    s.attach_vehicle(&session.id, &vehicle.id).unwrap();

    let summary = &s.list_sessions(10).unwrap()[0];
    assert_eq!(summary.session.id, session.id);
    assert_eq!(summary.vehicle.as_ref().unwrap().vin.as_deref(), Some("1FT7W2BT6KEC00001"));
}

#[test]
fn connections_round_trip_with_their_capability_snapshot() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let mut caps = AdapterCapabilities::unknown(TransportKind::Bluetooth);
    caps.elm327_compatible = true;
    caps.add_caveat("clone firmware");

    let c = aim_types::Connection {
        id: ConnectionId::new(),
        session_id: session.id.clone(),
        adapter_id: "COM5".into(),
        transport: TransportKind::Bluetooth,
        connected_at: now(),
        disconnected_at: None,
        firmware: Some("ELM327 v2.1".into()),
        capabilities: caps.clone(),
    };
    s.record_connection(&c).unwrap();

    let back = s.connections(&session.id).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].capabilities, caps);
    assert_eq!(back[0].transport, TransportKind::Bluetooth);
    assert!(back[0].disconnected_at.is_none());

    s.close_connection(&c.id).unwrap();
    assert!(s.connections(&session.id).unwrap()[0].disconnected_at.is_some());
}

#[test]
fn rediscovering_a_module_updates_it_instead_of_duplicating_it() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let m = s.upsert_module(&module(&session.id, "ECU_7E8", "7E8")).unwrap();

    let mut updated = module(&session.id, "ECU_7E8", "7E8");
    updated.identity = ModuleIdentity {
        ecu_name: Some("SIM ENGINE CONTROL".into()),
        calibration_ids: vec!["SIMULATED-CAL-01".into()],
        calibration_verification_numbers: vec!["1a2b3c4d".into()],
    };
    let again = s.upsert_module(&updated).unwrap();

    assert_eq!(again.id, m.id, "the module keeps its identity across reads");
    let stored = s.modules(&session.id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].identity.ecu_name.as_deref(), Some("SIM ENGINE CONTROL"));
    assert_eq!(s.get_module(&m.id).unwrap().module_key, "ECU_7E8");
}

#[test]
fn reading_the_same_code_twice_counts_occurrences_rather_than_duplicating() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let m = s.upsert_module(&module(&session.id, "ECU_7E8", "7E8")).unwrap();

    let dtc = DtcRecord {
        session_id: session.id.clone(),
        module_id: m.id.clone(),
        code: "P2463".into(),
        status: DtcStatus::Confirmed,
        description: Some("Diesel particulate filter restriction: soot accumulation".into()),
        occurrence: 1,
        freeze_frame_ref: None,
        read_at: now(),
    };
    s.record_dtc(&dtc).unwrap();
    s.record_dtc(&dtc).unwrap();
    s.record_dtc(&dtc).unwrap();

    let stored = s.dtcs(&session.id, None).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].occurrence, 3);

    // The same code with a different status is a different observation.
    let mut pending = dtc.clone();
    pending.status = DtcStatus::Pending;
    s.record_dtc(&pending).unwrap();
    assert_eq!(s.dtcs(&session.id, None).unwrap().len(), 2);
    assert_eq!(s.dtcs(&session.id, Some(&m.id)).unwrap().len(), 2);
}

#[test]
fn measurements_keep_the_raw_bytes_they_were_decoded_from() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let m = s.upsert_module(&module(&session.id, "ECU_7E8", "7E8")).unwrap();

    let decoded = DecodedValue::new(
        "engine_rpm",
        "Engine speed",
        Value::Number(1094.0),
        Some("rpm".into()),
        None,
        Provenance::decoded(
            &[0x41, 0x0C, 0x11, 0x18],
            "obd2.mode01.pid0C",
            "3",
            VerificationStatus::Verified,
            now(),
        ),
    );
    s.record_measurement(&measurement_from(&session.id, &m.id, &decoded)).unwrap();

    let stored = s.measurements(&session.id, Some("engine_rpm"), 10).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].value, Some(1094.0));
    assert_eq!(stored[0].unit.as_deref(), Some("rpm"));
    assert_eq!(stored[0].raw_value, "410c1118");
    // Filtering by a signal that was never read returns nothing, not everything.
    assert!(s.measurements(&session.id, Some("coolant_temp"), 10).unwrap().is_empty());
}

#[test]
fn test_runs_record_who_asked_and_whether_a_human_confirmed() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let run = TestRun {
        id: TestRunId::new(),
        session_id: session.id.clone(),
        module_id: None,
        test_id: "obd2.clear_dtcs".into(),
        risk_level: PermissionLevel::L2,
        requested_by: "agent:planner".into(),
        confirmed_by_user: false,
        started_at: now(),
        ended_at: Some(now()),
        result: TestOutcome::Rejected,
        evidence_ref: None,
        detail: Some(serde_json::json!({ "reason": "permission_level_disabled" })),
    };
    s.record_test_run(&run).unwrap();

    let stored = s.test_runs(&session.id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].requested_by, "agent:planner");
    assert!(!stored[0].confirmed_by_user);
    assert_eq!(stored[0].risk_level, PermissionLevel::L2);
    assert_eq!(stored[0].result, TestOutcome::Rejected);
    assert_eq!(stored[0].detail.as_ref().unwrap()["reason"], "permission_level_disabled");
}

#[test]
fn the_agent_trace_and_diagnosis_seams_persist_even_with_no_agent_built() {
    let s = store();
    let session = s.create_session(None).unwrap();

    s.record_agent_trace(&AgentTrace {
        session_id: session.id.clone(),
        message_id: "msg_1".into(),
        role: "user".into(),
        content: Some("why is the DPF light on".into()),
        tool_name: None,
        tool_args_ref: None,
        tool_result_ref: None,
        model: None,
        prompt_version: None,
        timestamp: now(),
    })
    .unwrap();
    assert_eq!(s.agent_traces(&session.id).unwrap().len(), 1);

    s.record_diagnosis(&Diagnosis {
        id: DiagnosisId::new(),
        session_id: session.id.clone(),
        hypothesis: "particulate filter loading".into(),
        confidence: 0.4,
        evidence_refs: vec![1, 2],
        alternatives: vec!["sensor fault".into()],
        recommendation: "observe a completed regeneration".into(),
        unresolved_questions: vec!["has a regeneration completed recently?".into()],
        created_at: now(),
    })
    .unwrap();
    let d = s.diagnoses(&session.id).unwrap();
    assert_eq!(d[0].evidence_refs, vec![1, 2]);
    assert_eq!(d[0].unresolved_questions.len(), 1);
}

#[test]
fn session_summaries_count_what_the_session_contains() {
    let s = store();
    let session = s.create_session(Some("scan".into())).unwrap();
    let m = s.upsert_module(&module(&session.id, "ECU_7E8", "7E8")).unwrap();
    s.record_dtc(&DtcRecord {
        session_id: session.id.clone(),
        module_id: m.id.clone(),
        code: "P2463".into(),
        status: DtcStatus::Confirmed,
        description: None,
        occurrence: 1,
        freeze_frame_ref: None,
        read_at: now(),
    })
    .unwrap();
    s.record_measurement(&aim_types::Measurement {
        session_id: session.id.clone(),
        module_id: m.id.clone(),
        timestamp: now(),
        signal_id: "engine_rpm".into(),
        value: Some(700.0),
        text_value: None,
        unit: Some("rpm".into()),
        raw_value: "410c0af0".into(),
    })
    .unwrap();

    let summary = &s.list_sessions(10).unwrap()[0];
    assert_eq!(summary.module_count, 1);
    assert_eq!(summary.dtc_count, 1);
    assert_eq!(summary.measurement_count, 1);
    assert_eq!(summary.event_count, 1);
}

#[test]
fn history_survives_reopening_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("history.sqlite");

    let session_id = {
        let s = SessionStore::open(&path).unwrap();
        let session = s.create_session(Some("first run".into())).unwrap();
        s.append_event(
            &session.id,
            EventKind::ConnectionStateChanged {
                from: "connecting".into(),
                to: ConnectionState::Ready,
            },
        )
        .unwrap();
        s.end_session(&session.id).unwrap();
        session.id
    };

    let reopened = SessionStore::open(&path).unwrap();
    let session = reopened.get_session(&session_id).unwrap();
    assert_eq!(session.label.as_deref(), Some("first run"));
    assert!(session.ended_at.is_some());
    assert_eq!(reopened.event_count(&session_id).unwrap(), 3);
    assert_eq!(reopened.list_sessions(10).unwrap().len(), 1);
}

#[test]
fn timestamps_survive_a_round_trip_exactly() {
    let s = store();
    let session = s.create_session(None).unwrap();
    let event = &s.events_since(&session.id, 0, 1).unwrap()[0];
    let text = event.timestamp.to_rfc3339();
    assert_eq!(Timestamp::parse_rfc3339(&text).unwrap(), event.timestamp);
}

/// The address a module *listens* on survives a round trip.
///
/// `address` is where a module answered, and outside the legislated block there
/// is no formula relating the two. A full scan learns both halves by
/// construction; discarding the half it sent to meant body and chassis modules
/// could be discovered and then never spoken to again.
#[test]
fn a_modules_request_address_is_remembered() {
    let store = SessionStore::open_in_memory().unwrap();
    let session = store.create_session(None).unwrap();

    // A body module, outside the legislated block: 72B answers, 723 listens.
    let mut body = module(&session.id, "ECU_72B", "72B");
    body.request_address = Some(String::from("723"));
    store.upsert_module(&body).unwrap();

    // And one recorded without it, which is a normal state for rows written by
    // an earlier build.
    store.upsert_module(&module(&session.id, "ECU_7E8", "7E8")).unwrap();

    let saved = store.modules(&session.id).unwrap();
    let found = saved.iter().find(|m| m.module_key == "ECU_72B").expect("body module");
    assert_eq!(found.request_address.as_deref(), Some("723"));
    let legacy = saved.iter().find(|m| m.module_key == "ECU_7E8").expect("engine module");
    assert_eq!(legacy.request_address, None, "not knowing is a fact, not a default");
}

/// A connect that degrades tells somebody the vehicle is not answering. It
/// does not tell them the same adapter read the same truck twenty minutes ago
/// — which is the difference between "it is broken" and "it is intermittent",
/// and an entirely different thing to go and check.
#[test]
fn a_previous_successful_session_is_findable_afterwards() {
    let store = SessionStore::open_in_memory().unwrap();

    // A session where the vehicle genuinely answered: a module row exists only
    // because something on the bus replied to an addressed request.
    let first = store.create_session(Some("yesterday".into())).unwrap();
    let vehicle = store
        .upsert_vehicle(&aim_types::Vehicle::from_vin(Some("1FT7W2BT7KEF78036".into())))
        .unwrap();
    store.attach_vehicle(&first.id, &vehicle.id).unwrap();
    store.record_connection(&conn(&first.id, "COM4")).expect("a connection on COM4");
    store.upsert_module(&module(&first.id, "ECU_7E8", "7E8")).unwrap();

    // Matched on the adapter.
    let found = store.last_successful_contact("COM4", None).unwrap().expect("found by adapter");
    assert_eq!(found.session_id, first.id.as_str());
    assert_eq!(found.modules, 1);
    assert!(found.same_adapter);
    assert!(!found.same_vehicle, "no VIN was offered to match on");

    // And on the VIN, which is the stronger claim of the two.
    let found = store
        .last_successful_contact("COM9", Some("1FT7W2BT7KEF78036"))
        .unwrap()
        .expect("found by VIN even through a different adapter");
    assert!(found.same_vehicle);
    assert!(!found.same_adapter);
}

/// A session where nothing ever answered is not evidence that anything works.
/// The bar is a module row, which an adapter talking to itself cannot produce.
#[test]
fn a_session_where_nothing_answered_does_not_count_as_contact() {
    let store = SessionStore::open_in_memory().unwrap();
    let empty = store.create_session(Some("silent bus".into())).unwrap();
    store.record_connection(&conn(&empty.id, "COM4")).unwrap();

    assert!(
        store.last_successful_contact("COM4", None).unwrap().is_none(),
        "a connection that found nothing proves nothing"
    );
}

/// Nothing has ever been plugged in. The common case on a first run, and it
/// must not invent a history to be encouraging about.
#[test]
fn an_unknown_adapter_has_no_history() {
    let store = SessionStore::open_in_memory().unwrap();
    assert!(store.last_successful_contact("COM99", None).unwrap().is_none());
    assert!(store.last_successful_contact("COM99", Some("1FT7W2BT7KEF00000")).unwrap().is_none());
}

/// A connection record for a given session and adapter.
fn conn(session: &SessionId, adapter: &str) -> aim_types::Connection {
    aim_types::Connection {
        id: ConnectionId::new(),
        session_id: session.clone(),
        adapter_id: adapter.to_string(),
        transport: TransportKind::Bluetooth,
        connected_at: now(),
        disconnected_at: None,
        firmware: Some("ELM327 v1.4b".into()),
        capabilities: AdapterCapabilities::unknown(TransportKind::Bluetooth),
    }
}
