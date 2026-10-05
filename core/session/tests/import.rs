//! Merging another install's database into this one.
//!
//! Two stores stand in for two devices: a phone that went to the car, and a
//! laptop that already has a history of its own. The laptop's history is what
//! makes these tests mean something: its event rows are numbered differently
//! from the phone's, so a reference carried over by number would land on the
//! wrong exchange.

use aim_session::{Finding, FindingOutcome, SessionStore};
use aim_types::{
    now, DtcRecord, DtcStatus, ErrorCode, EventKind, Measurement, Module, ModuleId, ModuleIdentity,
    ObdProtocol, SessionId, Timestamp, Vehicle,
};
use std::path::{Path, PathBuf};

const VIN: &str = "1FT7W2BT6KEC00001";

struct Bench {
    dir: tempfile::TempDir,
    phone: SessionStore,
    laptop: SessionStore,
}

impl Bench {
    fn new() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let phone = SessionStore::open(dir.path().join("phone.sqlite")).unwrap();
        let laptop = SessionStore::open(dir.path().join("laptop.sqlite")).unwrap();
        // The laptop has been used. Whatever the phone numbered 1 to 9 is
        // something else entirely here.
        let old = laptop.create_session(Some("last month".into())).unwrap();
        for i in 0..40 {
            request(&laptop, &old.id, &format!("01{i:02X}"));
        }
        Bench { dir, phone, laptop }
    }

    /// What the phone would hand to the share sheet.
    fn export(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        self.phone.snapshot_to(&path).unwrap();
        path
    }
}

fn request(store: &SessionStore, session: &SessionId, command: &str) -> i64 {
    store.append_event(session, EventKind::AdapterRequest { command: command.into() }).unwrap()
}

fn response(store: &SessionStore, session: &SessionId, command: &str, line: &str) -> i64 {
    store
        .append_event(
            session,
            EventKind::AdapterResponse {
                command: command.into(),
                lines: vec![line.into()],
                elapsed_ms: 42,
                classification: "ok".into(),
            },
        )
        .unwrap()
}

fn reading(
    store: &SessionStore,
    session: &SessionId,
    module: &ModuleId,
    signal: &str,
    at_ms: i128,
    raw: &str,
) {
    store
        .record_measurement(&Measurement {
            session_id: session.clone(),
            module_id: module.clone(),
            timestamp: Timestamp::from_unix_millis(1_790_000_000_000 + at_ms),
            signal_id: signal.into(),
            value: Some(at_ms as f64),
            text_value: None,
            unit: Some("rpm".into()),
            raw_value: raw.into(),
        })
        .unwrap();
}

fn finding(subject: &str, claim: &str, observed_at: &str) -> Finding {
    Finding {
        subject: subject.into(),
        outcome: FindingOutcome::Established,
        claim: claim.into(),
        evidence: "measured".into(),
        authority: "measured_this_session".into(),
        observed_at: observed_at.into(),
        session_id: None,
    }
}

/// A visit to the truck: identified, one module, a code with its freeze
/// frame, two readings. Left open, as a session usually is when exported.
fn a_visit(store: &SessionStore) -> (SessionId, ModuleId, i64) {
    let session = store.create_session(Some("a drive".into())).unwrap();
    let vehicle = store.upsert_vehicle(&Vehicle::from_vin(Some(VIN.into()))).unwrap();
    store.attach_vehicle(&session.id, &vehicle.id).unwrap();
    let module = store
        .upsert_module(&Module {
            id: ModuleId::new(),
            session_id: session.id.clone(),
            module_key: "ECU_7E8".into(),
            name: "Engine".into(),
            address: "7E8".into(),
            request_address: Some("7E0".into()),
            protocol: ObdProtocol::Iso15765Can11_500,
            identity: ModuleIdentity::default(),
            software_version: Some("CAL-01".into()),
            discovered_at: now(),
        })
        .unwrap();
    request(store, &session.id, "0902");
    request(store, &session.id, "03");
    let frame = response(store, &session.id, "03", "7E8 04 43 01 24 63");
    store
        .record_dtc(&DtcRecord {
            session_id: session.id.clone(),
            module_id: module.id.clone(),
            code: "P2463".into(),
            status: DtcStatus::Confirmed,
            description: Some("Diesel particulate filter restriction".into()),
            occurrence: 1,
            freeze_frame_ref: Some(frame),
            read_at: now(),
        })
        .unwrap();
    reading(store, &session.id, &module.id, "engine_rpm", 0, "0C80");
    reading(store, &session.id, &module.id, "engine_rpm", 500, "0C90");
    (session.id, module.id, frame)
}

#[test]
fn a_session_arrives_whole_and_its_evidence_points_at_the_same_exchange() {
    let b = Bench::new();
    let (session, _, frame_on_phone) = a_visit(&b.phone);

    let summary = b.laptop.import_database(b.export("a.sqlite"), false).unwrap();

    assert!(summary.committed);
    assert_eq!(summary.sessions_added, 1);
    assert_eq!(summary.sessions_extended, 0);
    assert_eq!(summary.vehicles_added, 1);
    assert_eq!(summary.vehicles, vec![Some(VIN.to_string())]);
    assert_eq!(summary.events_added, 4);
    assert_eq!(summary.measurements_added, 2);
    assert_eq!(summary.dtcs_added, 1);
    assert!(summary.sessions_skipped.is_empty());

    // The recording is the phone's, event for event.
    let theirs = b.phone.events_since(&session, 0, 100).unwrap();
    let ours = b.laptop.events_since(&session, 0, 100).unwrap();
    assert_eq!(ours.len(), theirs.len());
    for (a, b) in ours.iter().zip(&theirs) {
        assert_eq!((a.seq, a.timestamp, &a.kind), (b.seq, b.timestamp, &b.kind));
    }

    // The code's freeze frame is the reply that reported it. On the laptop
    // that reply has another row number, and the phone's number is one of the
    // laptop's own requests from last month.
    let dtc = &b.laptop.dtcs(&session, None).unwrap()[0];
    let frame_here = dtc.freeze_frame_ref.expect("the freeze frame came across");
    assert_ne!(frame_here, frame_on_phone);
    let cited = b.laptop.event_by_id(frame_here).unwrap();
    assert_eq!(cited.session_id, session);
    assert!(matches!(
        &cited.kind,
        EventKind::AdapterResponse { command, lines, .. }
            if command == "03" && lines[0] == "7E8 04 43 01 24 63"
    ));

    let modules = b.laptop.modules(&session).unwrap();
    assert_eq!(modules[0].request_address.as_deref(), Some("7E0"));
    assert_eq!(modules[0].software_version.as_deref(), Some("CAL-01"));
    assert_eq!(b.laptop.measurements(&session, Some("engine_rpm"), 100).unwrap().len(), 2);
    assert_eq!(b.laptop.integrity_check().unwrap(), "ok");
}

#[test]
fn importing_the_same_file_again_adds_nothing() {
    let b = Bench::new();
    let (session, ..) = a_visit(&b.phone);
    let file = b.export("a.sqlite");
    b.laptop.import_database(&file, false).unwrap();
    // The first import migrates and reads the file it is given; a second
    // export is what a person would actually have.
    let again = b.laptop.import_database(b.export("a-again.sqlite"), false).unwrap();

    assert!(again.is_empty(), "{again:?}");
    assert_eq!(again.sessions_already_here, 1);
    assert_eq!(again.events_added, 0);
    assert_eq!(again.measurements_added, 0);
    assert_eq!(again.dtcs_added, 0);
    assert_eq!(b.laptop.event_count(&session).unwrap(), 4);
    assert_eq!(b.laptop.measurements(&session, None, 100).unwrap().len(), 2);
    assert_eq!(b.laptop.dtcs(&session, None).unwrap().len(), 1);
}

/// Most sessions are exported while still open: nobody disconnects before
/// pressing Export. The rest of the session has to be able to follow.
#[test]
fn a_later_export_finishes_a_session_that_was_brought_in_open() {
    let b = Bench::new();
    let (session, module, _) = a_visit(&b.phone);
    b.laptop.import_database(b.export("parked.sqlite"), false).unwrap();
    assert!(b.laptop.get_session(&session).unwrap().ended_at.is_none());

    // The drive itself, recorded after the first export.
    request(&b.phone, &session, "010C");
    let second = response(&b.phone, &session, "07", "7E8 04 47 01 20 02");
    reading(&b.phone, &session, &module, "engine_rpm", 1000, "0CA0");
    reading(&b.phone, &session, &module, "vehicle_speed", 1000, "32");
    b.phone
        .record_dtc(&DtcRecord {
            session_id: session.clone(),
            module_id: module.clone(),
            code: "P2002".into(),
            status: DtcStatus::Pending,
            description: None,
            occurrence: 1,
            freeze_frame_ref: Some(second),
            read_at: now(),
        })
        .unwrap();
    b.phone.end_session(&session).unwrap();

    let summary = b.laptop.import_database(b.export("driven.sqlite"), false).unwrap();

    assert_eq!(summary.sessions_added, 0);
    assert_eq!(summary.sessions_extended, 1);
    assert_eq!(summary.measurements_added, 2);
    assert_eq!(summary.dtcs_added, 1);
    let theirs = b.phone.events_since(&session, 0, 100).unwrap();
    let ours = b.laptop.events_since(&session, 0, 100).unwrap();
    assert_eq!(summary.events_added as usize, theirs.len() - 4);
    assert_eq!(
        ours.iter().map(|e| (e.seq, &e.kind)).collect::<Vec<_>>(),
        theirs.iter().map(|e| (e.seq, &e.kind)).collect::<Vec<_>>()
    );
    assert_eq!(b.laptop.measurements(&session, None, 100).unwrap().len(), 4);
    assert!(b.laptop.get_session(&session).unwrap().ended_at.is_some());

    // The new code cites an event that only arrived with this import.
    let dtcs = b.laptop.dtcs(&session, None).unwrap();
    let pending = dtcs.iter().find(|d| d.code == "P2002").unwrap();
    let cited = b.laptop.event_by_id(pending.freeze_frame_ref.unwrap()).unwrap();
    assert!(matches!(&cited.kind, EventKind::AdapterResponse { command, .. } if command == "07"));

    // And the older export, brought in after the newer, takes nothing back.
    let stale = b.laptop.import_database(b.dir.path().join("parked.sqlite"), false).unwrap();
    assert!(stale.is_empty(), "{stale:?}");
    assert_eq!(stale.sessions_already_here, 1);
    assert_eq!(b.laptop.events_since(&session, 0, 100).unwrap().len(), theirs.len());
}

#[test]
fn a_preview_says_what_would_happen_and_keeps_nothing() {
    let b = Bench::new();
    let (session, ..) = a_visit(&b.phone);
    b.phone
        .record_finding(VIN, &finding("bus.secondary.rate", "500 kbit/s", "2026-10-01T10:00:00Z"))
        .unwrap();
    let before = b.laptop.list_sessions(100).unwrap().len();

    let preview = b.laptop.import_database(b.export("a.sqlite"), true).unwrap();

    assert!(!preview.committed);
    assert_eq!(preview.sessions_added, 1);
    assert_eq!(preview.findings_added, 1);
    assert_eq!(b.laptop.list_sessions(100).unwrap().len(), before);
    assert!(b.laptop.get_session(&session).is_err());
    assert!(b.laptop.vehicle_by_vin(VIN).unwrap().is_none());
    assert!(b.laptop.knowledge(VIN).unwrap().is_empty());

    // Doing it for real gives the numbers the preview promised.
    let real = b.laptop.import_database(b.export("b.sqlite"), false).unwrap();
    assert_eq!(aim_session::ImportSummary { committed: false, ..real }, preview);
}

/// The VIN is the vehicle. Each device gave the truck a row id of its own.
#[test]
fn a_vehicle_both_devices_have_met_is_one_vehicle() {
    let b = Bench::new();
    let (on_laptop, ..) = a_visit(&b.laptop);
    let (on_phone, ..) = a_visit(&b.phone);
    let here = b.laptop.vehicle_by_vin(VIN).unwrap().unwrap();
    assert_ne!(here.id, b.phone.vehicle_by_vin(VIN).unwrap().unwrap().id);
    b.phone
        .record_capture(&on_phone, Some(VIN), "72E", Some("before"), "2026-10-01T10:00:00Z", "{}")
        .unwrap();
    // A module only the phone's visit found.
    b.phone
        .upsert_module(&Module {
            id: ModuleId::new(),
            session_id: on_phone.clone(),
            module_key: "ECU_72E".into(),
            name: "Body control".into(),
            address: "72E".into(),
            request_address: Some("726".into()),
            protocol: ObdProtocol::Iso15765Can11_500,
            identity: ModuleIdentity::default(),
            software_version: None,
            discovered_at: now(),
        })
        .unwrap();

    let summary = b.laptop.import_database(b.export("a.sqlite"), false).unwrap();

    assert_eq!(summary.vehicles_added, 0);
    assert_eq!(summary.captures_added, 1);
    assert_eq!(b.laptop.known_vins().unwrap(), vec![VIN.to_string()]);
    for session in [&on_laptop, &on_phone] {
        assert_eq!(b.laptop.get_session(session).unwrap().vehicle_id, Some(here.id.clone()));
    }
    // Which is what lets a second visit start from the first, whichever
    // device made it.
    let known: Vec<String> =
        b.laptop.modules_for_vehicle(&here.id).unwrap().into_iter().map(|m| m.module_key).collect();
    assert_eq!(known, vec!["ECU_72E", "ECU_7E8"]);
    assert_eq!(b.laptop.captures_for_vehicle(VIN).unwrap().len(), 1);
}

#[test]
fn the_newer_finding_wins_wherever_it_was_made() {
    let b = Bench::new();
    a_visit(&b.phone);
    // Known to both, observed later on the phone.
    b.laptop
        .record_finding(VIN, &finding("tire.front_left", "52 psi", "2026-09-28T10:00:00Z"))
        .unwrap();
    b.phone
        .record_finding(VIN, &finding("tire.front_left", "49 psi", "2026-10-04T18:30:00.5Z"))
        .unwrap();
    // Known to both, observed later on the laptop.
    b.laptop.record_finding(VIN, &finding("def.level", "41 %", "2026-10-04T09:00:00Z")).unwrap();
    b.phone.record_finding(VIN, &finding("def.level", "60 %", "2026-09-01T09:00:00Z")).unwrap();
    // Only the phone knows.
    b.phone
        .record_finding(VIN, &finding("bus.secondary.rate", "500 kbit/s", "2026-10-04T18:00:00Z"))
        .unwrap();
    b.phone.store_vpic_reply(VIN, "https://vpic.example/decode", "{}").unwrap();
    b.phone.store_as_built(VIN, Some("phone.ab"), "{}").unwrap();

    let summary = b.laptop.import_database(b.export("a.sqlite"), false).unwrap();

    assert_eq!(
        (summary.findings_added, summary.findings_updated, summary.findings_kept),
        (1, 1, 1)
    );
    assert_eq!(summary.vehicle_records_taken, 2);
    let claim = |subject: &str| {
        let all = b.laptop.knowledge(VIN).unwrap();
        all.into_iter().find(|f| f.subject == subject).unwrap().claim
    };
    assert_eq!(claim("tire.front_left"), "49 psi");
    assert_eq!(claim("def.level"), "41 %");
    assert_eq!(claim("bus.secondary.rate"), "500 kbit/s");
    assert!(b.laptop.vpic_reply(VIN).unwrap().is_some());
    assert_eq!(b.laptop.as_built(VIN).unwrap().unwrap().source.as_deref(), Some("phone.ab"));
}

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn a_session_whose_recording_differs_is_left_out_and_nothing_else_is_lost() {
    let b = Bench::new();
    let (session, ..) = a_visit(&b.phone);
    b.laptop.import_database(b.export("a.sqlite"), false).unwrap();

    // The same session id with a different history behind it, and more of it.
    let other = b.phone.create_session(Some("another visit".into())).unwrap();
    request(&b.phone, &session, "010D");
    let tampered = b.export("tampered.sqlite");
    raw(&tampered)
        .execute_batch(&format!(
            "DROP TRIGGER session_events_are_append_only_update;
             UPDATE session_events SET timestamp = '2020-01-01T00:00:00Z'
             WHERE session_id = '{session}' AND seq = 4;"
        ))
        .unwrap();

    let summary = b.laptop.import_database(&tampered, false).unwrap();

    assert_eq!(summary.sessions_skipped.len(), 1);
    assert_eq!(summary.sessions_skipped[0].id, session.as_str());
    assert!(summary.sessions_skipped[0].reason.contains("differs"));
    assert_eq!(summary.sessions_extended, 0);
    assert_eq!(b.laptop.event_count(&session).unwrap(), 4);
    // The rest of the file still came in.
    assert_eq!(summary.sessions_added, 1);
    assert!(b.laptop.get_session(&other.id).is_ok());
}

#[test]
fn a_file_that_is_not_one_of_ours_is_refused() {
    let b = Bench::new();
    let before = b.laptop.list_sessions(100).unwrap().len();

    let text = b.dir.path().join("notes.sqlite");
    std::fs::write(&text, b"this is not a database at all").unwrap();
    let empty = b.dir.path().join("empty.sqlite");
    raw(&empty).execute_batch("CREATE TABLE something (x);").unwrap();
    for file in [&text, &empty, &b.dir.path().join("missing.sqlite")] {
        let err = b.laptop.import_database(file, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{err:?}");
    }

    // Its own database, by another spelling of the path.
    let own = b.dir.path().join(".").join("laptop.sqlite");
    let err = b.laptop.import_database(own, false).unwrap_err();
    assert!(err.message.contains("own database"), "{err:?}");

    assert_eq!(b.laptop.list_sessions(100).unwrap().len(), before);
}

#[test]
fn a_database_from_a_newer_app_is_refused_with_what_to_do() {
    let b = Bench::new();
    a_visit(&b.phone);
    let file = b.export("future.sqlite");
    raw(&file)
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (9999, 'later', 'x')",
            [],
        )
        .unwrap();

    let err = b.laptop.import_database(&file, false).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(err.message.contains("newer version"), "{err:?}");
    assert!(b.laptop.vehicle_by_vin(VIN).unwrap().is_none());
}

/// A view under a table's name would have the import run a query the file
/// wrote. It has to be a table.
#[test]
fn a_file_with_a_view_where_a_table_belongs_is_refused() {
    let b = Bench::new();
    a_visit(&b.phone);
    let file = b.export("view.sqlite");
    raw(&file)
        .execute_batch(
            "DROP TABLE vpic_replies;
             CREATE VIEW vpic_replies AS
                 SELECT vin, observed_at AS fetched_at, claim AS source_url, evidence AS body
                 FROM vehicle_knowledge;",
        )
        .unwrap();

    let err = b.laptop.import_database(&file, false).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(b.laptop.vehicle_by_vin(VIN).unwrap().is_none());
}

/// The import works from the store's public face on an in-memory database
/// too, which is what `aim-api` runs on without `--db`.
#[test]
fn an_in_memory_database_can_take_an_import() {
    let b = Bench::new();
    let (session, ..) = a_visit(&b.phone);
    let memory = SessionStore::open_in_memory().unwrap();

    let summary = memory.import_database(b.export("a.sqlite"), false).unwrap();

    assert_eq!(summary.sessions_added, 1);
    assert_eq!(memory.event_count(&session).unwrap(), 4);
}

/// The same promises, checked against a database a person actually made.
///
///     AIM_IMPORT_CHECK_DB=<a copy of sessions.sqlite> \
///         cargo test -p aim-session --test import -- --ignored --nocapture
///
/// Give it a copy, never the app's own file: the import migrates what it reads.
#[test]
#[ignore = "needs AIM_IMPORT_CHECK_DB, a copy of a real sessions.sqlite"]
fn a_real_database_comes_across_complete() {
    let source = std::env::var("AIM_IMPORT_CHECK_DB").expect("set AIM_IMPORT_CHECK_DB");
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("source.sqlite");
    // Through a store, so whatever is still in the write-ahead log comes too.
    SessionStore::open(&source).unwrap().snapshot_to(&copy).unwrap();

    let fresh_path = dir.path().join("fresh.sqlite");
    let fresh = SessionStore::open(&fresh_path).unwrap();
    // Something of its own first, so no row keeps its number by accident.
    let own = fresh.create_session(None).unwrap();
    for i in 0..500 {
        request(&fresh, &own.id, &format!("01{:02X}", i % 256));
    }

    let started = std::time::Instant::now();
    let summary = fresh.import_database(&copy, false).unwrap();
    println!("imported in {:?}: {summary:#?}", started.elapsed());
    assert!(summary.sessions_skipped.is_empty());
    assert_eq!(fresh.integrity_check().unwrap(), "ok");

    let again = fresh.import_database(&copy, false).unwrap();
    assert!(again.is_empty(), "a second import added something: {again:#?}");
    drop(fresh);

    let conn = raw(&fresh_path);
    conn.execute("ATTACH DATABASE ?1 AS src", [copy.to_str().unwrap()]).unwrap();
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };

    for table in [
        "vehicles",
        "connections",
        "modules",
        "dtcs",
        "measurements",
        "test_runs",
        "agent_traces",
        "diagnoses",
        "config_captures",
        "as_built",
        "vehicle_knowledge",
        "vpic_replies",
    ] {
        assert_eq!(
            one(&format!("SELECT COUNT(*) FROM main.{table}")),
            one(&format!("SELECT COUNT(*) FROM src.{table}")),
            "{table}"
        );
    }
    assert_eq!(
        one("SELECT COUNT(*) FROM main.sessions"),
        one("SELECT COUNT(*) FROM src.sessions") + 1
    );

    // Every event is here, in its place, byte for byte.
    assert_eq!(
        one("SELECT COUNT(*) FROM src.session_events s
             LEFT JOIN main.session_events m ON m.session_id = s.session_id AND m.seq = s.seq
             WHERE m.id IS NULL OR m.payload <> s.payload OR m.timestamp <> s.timestamp
                OR m.kind <> s.kind"),
        0
    );
    // Every freeze frame cites the event it cited before.
    let cited = one("SELECT COUNT(*) FROM src.dtcs WHERE freeze_frame_ref IS NOT NULL");
    println!("{cited} freeze frame references checked");
    assert_eq!(
        one("SELECT COUNT(*) FROM src.dtcs sd
             JOIN main.dtcs md ON md.session_id = sd.session_id AND md.module_id = sd.module_id
                              AND md.code = sd.code AND md.status = sd.status
             LEFT JOIN src.session_events se ON se.id = sd.freeze_frame_ref
             LEFT JOIN main.session_events me ON me.id = md.freeze_frame_ref
             WHERE NOT (se.session_id IS me.session_id AND se.seq IS me.seq)"),
        0
    );
}
