//! The SQLite session store.
//!
//! One type, [`SessionStore`], owns the database. It is cheap to clone (an
//! `Arc` around a mutex-guarded connection) and is `Send + Sync`, so the
//! diagnostic thread and the HTTP handlers can share one.
//!
//! # Why a single connection behind a mutex
//!
//! This is a local desktop app with one vehicle, one adapter and one operator.
//! Writes are serialized by physics long before they reach the database. A
//! connection pool would add configuration and failure modes to buy
//! concurrency the workload does not have.

use crate::schema;
use aim_types::{
    hex, AgentTrace, AimError, AimResult, Connection as ConnectionRecord, Diagnosis, DtcRecord,
    DtcStatus, ErrorCode, EventKind, Measurement, Module, ModuleId, SessionEvent, SessionId,
    TestRun, Timestamp, Vehicle, VehicleId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast;

/// How many events a slow websocket subscriber may fall behind before it is
/// disconnected and told to re-read from the database.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// A session with the counts a list view needs, without loading its events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The session.
    pub session: aim_types::Session,
    /// Its vehicle, when one was identified.
    pub vehicle: Option<Vehicle>,
    /// Number of events recorded.
    pub event_count: i64,
    /// Number of modules discovered.
    pub module_count: i64,
    /// Number of distinct DTCs read.
    pub dtc_count: i64,
    /// Number of measurements recorded.
    pub measurement_count: i64,
}

/// One stored configuration capture, with the context needed to compare it.
///
/// The capture itself is kept as the JSON the module produced rather than
/// re-modelled here: this crate stores it and hands it back, and the meaning of
/// the bytes belongs to whoever took them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCapture {
    /// Identifier, `cap_...`.
    pub id: String,
    /// Session it was taken in.
    pub session_id: String,
    /// Vehicle it came from, when one had been identified.
    pub vehicle_id: Option<String>,
    /// Module key, e.g. `ECU_7E0` or `ECU_18DAF110`.
    pub module_key: String,
    /// Free-text note so a person can tell "before" from "after" later.
    pub label: Option<String>,
    /// When it was taken, ISO 8601.
    pub taken_at: String,
    /// The capture, verbatim.
    pub capture: serde_json::Value,
}

/// One imported as-built file, as it was stored.
///
/// The data stays JSON here for the same reason a capture does: this crate
/// keeps it and hands it back, and what the bytes mean belongs to the decoder
/// that parsed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAsBuilt {
    /// The VIN the file was issued for, upper case.
    pub vin: String,
    /// When it was imported, ISO 8601.
    pub imported_at: String,
    /// Where it came from, so a wrong import is traceable to a file.
    pub source: Option<String>,
    /// The parsed data, verbatim.
    pub data: serde_json::Value,
}

/// Local diagnostic history.
#[derive(Clone)]
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
    events: broadcast::Sender<SessionEvent>,
    path: Option<String>,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStore")
            .field("path", &self.path.as_deref().unwrap_or(":memory:"))
            .finish()
    }
}

impl SessionStore {
    /// Open (creating if needed) the database at `path` and migrate it.
    pub fn open(path: impl AsRef<Path>) -> AimResult<SessionStore> {
        let p = path.as_ref();
        if let Some(dir) = p.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    AimError::new(
                        ErrorCode::StorageError,
                        format!("cannot create {}: {e}", dir.display()),
                    )
                })?;
            }
        }
        let conn = Connection::open(p).map_err(|e| open_failure(p, e))?;
        // `Connection::open` does not read the file - it only creates a handle,
        // so a damaged database opens happily and fails later, somewhere in
        // migration, as an error that no longer knows it came from SQLite.
        // Reading the schema forces the header now, while the error is still
        // typed and can be classified as corruption rather than guessed at.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .map_err(|e| open_failure(p, e))?;
        SessionStore::from_connection(conn, Some(p.display().to_string()))
            .map_err(|e| enrich_open_failure(p, e))
    }

    /// An in-memory database. Used by tests and by `--ephemeral` runs.
    pub fn open_in_memory() -> AimResult<SessionStore> {
        SessionStore::from_connection(Connection::open_in_memory().map_err(storage)?, None)
    }

    fn from_connection(mut conn: Connection, path: Option<String>) -> AimResult<SessionStore> {
        // WAL keeps a reader (the UI polling history) from blocking the writer
        // (the diagnostic thread recording a live scan). It is unavailable for
        // in-memory databases, where it is also unnecessary.
        if path.is_some() {
            let _ = conn.pragma_update(None, "journal_mode", "WAL");
            // A second process - an older build left installed, or a copy
            // launched twice - should wait its turn rather than fail instantly.
            // Five seconds is far longer than any write this app makes and
            // still short enough that a genuine deadlock surfaces as an error
            // rather than as a hang.
            let _ = conn.pragma_update(None, "busy_timeout", 5000);
        }
        conn.pragma_update(None, "foreign_keys", "ON").map_err(storage)?;
        schema::migrate(&mut conn)?;
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(SessionStore { conn: Arc::new(Mutex::new(conn)), events, path })
    }

    /// Where this database lives, or `None` for in-memory.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Schema version currently applied.
    pub fn schema_version(&self) -> i64 {
        schema::latest_version()
    }

    /// Subscribe to events as they are appended. Used by the websocket stream.
    ///
    /// A subscriber that falls more than [`EVENT_CHANNEL_CAPACITY`] events
    /// behind is lagged by the channel; the caller should then re-read from
    /// [`SessionStore::events_since`] rather than pretend it saw everything.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    /// Read a pragma back as text.
    ///
    /// Pragmas are asked for by name rather than assumed, because "we set WAL
    /// on open" and "this database is in WAL mode" are different claims - the
    /// `pragma_update` in [`SessionStore::from_connection`] deliberately
    /// ignores its own failure.
    pub fn pragma_string(&self, name: &str) -> AimResult<String> {
        let conn = self.lock()?;
        // Pragma names cannot be bound as parameters. Only accept the ones we
        // ask for, so this cannot become a way to interpolate arbitrary SQL.
        const ALLOWED: [&str; 6] = [
            "journal_mode",
            "foreign_keys",
            "user_version",
            "page_size",
            "page_count",
            "freelist_count",
        ];
        if !ALLOWED.contains(&name) {
            return Err(AimError::new(
                ErrorCode::StorageError,
                format!("pragma {name} is not one this build reads back"),
            ));
        }
        conn.query_row(&format!("PRAGMA {name}"), [], |r| {
            r.get_ref(0).map(|v| match v {
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                other => format!("{other:?}"),
            })
        })
        .map_err(storage)
    }

    /// `PRAGMA integrity_check`, returning whatever SQLite says.
    ///
    /// Returns the string `ok` for a healthy database. This is the check that
    /// distinguishes a genuinely damaged file from one that merely failed to
    /// open - a distinction worth having before anyone deletes anything.
    pub fn integrity_check(&self) -> AimResult<String> {
        let conn = self.lock()?;
        conn.query_row("PRAGMA integrity_check(20)", [], |r| r.get::<_, String>(0)).map_err(storage)
    }

    fn lock(&self) -> AimResult<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| {
            AimError::new(
                ErrorCode::StorageError,
                "session store mutex was poisoned by a previous panic",
            )
        })
    }

    // ------------------------------------------------------------ sessions

    /// Start a new session.
    pub fn create_session(&self, label: Option<String>) -> AimResult<aim_types::Session> {
        let session = aim_types::Session {
            id: SessionId::new(),
            vehicle_id: None,
            started_at: aim_types::now(),
            ended_at: None,
            label: label.clone(),
        };
        self.lock()?
            .execute(
                "INSERT INTO sessions (id, vehicle_id, started_at, ended_at, label)
                 VALUES (?1, NULL, ?2, NULL, ?3)",
                params![session.id.as_str(), session.started_at.to_rfc3339(), session.label],
            )
            .map_err(storage)?;
        self.append_event(&session.id, EventKind::SessionStarted { label })?;
        Ok(session)
    }

    /// Mark a session ended. Idempotent: the first end time wins, because a
    /// session's history must not change after the fact.
    pub fn end_session(&self, id: &SessionId) -> AimResult<()> {
        self.append_event(id, EventKind::SessionEnded)?;
        self.lock()?
            .execute(
                "UPDATE sessions SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![id.as_str(), aim_types::now().to_rfc3339()],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Load one session.
    pub fn get_session(&self, id: &SessionId) -> AimResult<aim_types::Session> {
        self.lock()?
            .query_row(
                "SELECT id, vehicle_id, started_at, ended_at, label FROM sessions WHERE id = ?1",
                [id.as_str()],
                row_to_session,
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| AimError::not_found(format!("no session {id}")))
    }

    /// Sessions newest first, with the counts a list view shows.
    pub fn list_sessions(&self, limit: u32) -> AimResult<Vec<SessionSummary>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.vehicle_id, s.started_at, s.ended_at, s.label,
                        (SELECT COUNT(*) FROM session_events e WHERE e.session_id = s.id),
                        (SELECT COUNT(*) FROM modules m WHERE m.session_id = s.id),
                        (SELECT COUNT(*) FROM dtcs d WHERE d.session_id = s.id),
                        (SELECT COUNT(*) FROM measurements x WHERE x.session_id = s.id)
                 FROM sessions s
                 ORDER BY s.started_at DESC, s.rowid DESC
                 LIMIT ?1",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok((
                    row_to_session(r)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        drop(stmt);

        let mut out = Vec::with_capacity(rows.len());
        for (session, event_count, module_count, dtc_count, measurement_count) in rows {
            let vehicle = match &session.vehicle_id {
                Some(v) => get_vehicle(&conn, v)?,
                None => None,
            };
            out.push(SessionSummary {
                session,
                vehicle,
                event_count,
                module_count,
                dtc_count,
                measurement_count,
            });
        }
        Ok(out)
    }

    // -------------------------------------------------------------- events

    /// Append an event to the flight recorder and publish it to subscribers.
    ///
    /// Returns the row id, which is the stable `evidence_ref` other tables and
    /// the [`aim_types::ToolResult`] envelope point at.
    pub fn append_event(&self, session_id: &SessionId, kind: EventKind) -> AimResult<i64> {
        let timestamp = aim_types::now();
        let payload = serde_json::to_string(&kind).map_err(|e| {
            AimError::new(ErrorCode::StorageError, format!("cannot serialize event: {e}"))
        })?;
        let name = kind.name();

        let (id, seq) = {
            let conn = self.lock()?;
            // seq is allocated inside the same statement as the insert, so two
            // concurrent writers cannot produce a gap or a duplicate.
            conn.execute(
                "INSERT INTO session_events (session_id, seq, timestamp, kind, payload)
                 VALUES (
                    ?1,
                    (SELECT COALESCE(MAX(seq), 0) + 1 FROM session_events WHERE session_id = ?1),
                    ?2, ?3, ?4
                 )",
                params![session_id.as_str(), timestamp.to_rfc3339(), name, payload],
            )
            .map_err(storage)?;
            let id = conn.last_insert_rowid();
            let seq: i64 = conn
                .query_row("SELECT seq FROM session_events WHERE id = ?1", [id], |r| r.get(0))
                .map_err(storage)?;
            (id, seq)
        };

        let event =
            SessionEvent { id: Some(id), session_id: session_id.clone(), seq, timestamp, kind };
        // No subscribers is the normal case (headless runs); it is not an error.
        let _ = self.events.send(event);
        Ok(id)
    }

    /// Events for a session with `seq` greater than `after_seq`, in order.
    pub fn events_since(
        &self,
        session_id: &SessionId,
        after_seq: i64,
        limit: u32,
    ) -> AimResult<Vec<SessionEvent>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, seq, timestamp, kind, payload
                 FROM session_events
                 WHERE session_id = ?1 AND seq > ?2
                 ORDER BY seq
                 LIMIT ?3",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![session_id.as_str(), after_seq, limit], row_to_event)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }

    /// One event by row id — how a `raw_evidence_ref` is resolved.
    pub fn event_by_id(&self, id: i64) -> AimResult<SessionEvent> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, session_id, seq, timestamp, kind, payload FROM session_events WHERE id = ?1",
            [id],
            row_to_event,
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| AimError::not_found(format!("no event {id}")))?
    }

    /// How many events a session has.
    pub fn event_count(&self, session_id: &SessionId) -> AimResult<i64> {
        self.lock()?
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id = ?1",
                [session_id.as_str()],
                |r| r.get(0),
            )
            .map_err(storage)
    }

    // ------------------------------------------------------------ vehicles

    /// Insert a vehicle, or return the existing row with the same VIN.
    ///
    /// Vehicles without a VIN are never merged: two unidentified vehicles are
    /// two vehicles until something proves otherwise.
    pub fn upsert_vehicle(&self, vehicle: &Vehicle) -> AimResult<Vehicle> {
        let conn = self.lock()?;
        if let Some(vin) = &vehicle.vin {
            let existing: Option<Vehicle> = conn
                .query_row(
                    "SELECT id, vin, make, model, year, trim, engine, transmission, discovered_at
                     FROM vehicles WHERE vin = ?1",
                    [vin],
                    row_to_vehicle,
                )
                .optional()
                .map_err(storage)?;
            if let Some(v) = existing {
                return Ok(v);
            }
        }
        conn.execute(
            "INSERT INTO vehicles (id, vin, make, model, year, trim, engine, transmission,
                                   discovered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                vehicle.id.as_str(),
                vehicle.vin,
                vehicle.make,
                vehicle.model,
                vehicle.year,
                vehicle.trim,
                vehicle.engine,
                vehicle.transmission,
                vehicle.discovered_at.to_rfc3339(),
            ],
        )
        .map_err(storage)?;
        Ok(vehicle.clone())
    }

    /// Point a session at a vehicle.
    pub fn attach_vehicle(&self, session_id: &SessionId, vehicle_id: &VehicleId) -> AimResult<()> {
        self.lock()?
            .execute(
                "UPDATE sessions SET vehicle_id = ?2 WHERE id = ?1",
                params![session_id.as_str(), vehicle_id.as_str()],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Load a vehicle.
    pub fn get_vehicle(&self, id: &VehicleId) -> AimResult<Option<Vehicle>> {
        let conn = self.lock()?;
        get_vehicle(&conn, id)
    }

    // --------------------------------------------------------- connections

    /// Record an adapter connection.
    pub fn record_connection(&self, c: &ConnectionRecord) -> AimResult<()> {
        self.lock()?
            .execute(
                "INSERT INTO connections (id, session_id, adapter_id, transport, connected_at,
                                          disconnected_at, firmware, capabilities)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    c.id.as_str(),
                    c.session_id.as_str(),
                    c.adapter_id,
                    enum_str(&c.transport)?,
                    c.connected_at.to_rfc3339(),
                    c.disconnected_at.map(|t| t.to_rfc3339()),
                    c.firmware,
                    json_str(&c.capabilities)?,
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Mark a connection closed.
    pub fn close_connection(&self, id: &aim_types::ConnectionId) -> AimResult<()> {
        self.lock()?
            .execute(
                "UPDATE connections SET disconnected_at = ?2
                 WHERE id = ?1 AND disconnected_at IS NULL",
                params![id.as_str(), aim_types::now().to_rfc3339()],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// The protocol a module last answered on through this adapter.
    ///
    /// Used to reorder the protocol sweep, and for nothing else. A remembered
    /// protocol is a hint about where to look first, not a fact about the
    /// vehicle in front of you now - somebody may have unplugged the cable and
    /// walked to a different car, which is exactly what the sweep is for.
    ///
    /// Keyed by adapter rather than by vehicle because the vehicle is not
    /// identified until after a protocol is working, which makes it useless as
    /// a key at the moment the answer is needed.
    pub fn last_protocol_for_adapter(
        &self,
        adapter_id: &str,
    ) -> AimResult<Option<aim_types::ObdProtocol>> {
        let conn = self.lock()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT m.protocol
                 FROM modules m
                 JOIN connections c ON c.session_id = m.session_id
                 WHERE c.adapter_id = ?1 AND m.protocol IS NOT NULL AND m.protocol <> 'unknown'
                 ORDER BY m.discovered_at DESC
                 LIMIT 1",
                [adapter_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        Ok(raw.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok()))
    }

    /// The line speed this adapter was last found answering at.
    ///
    /// Recorded inside the connection capabilities, so no new table is needed.
    /// `None` for Bluetooth, where the virtual port ignores baud, and for a port
    /// nothing has connected to yet.
    pub fn last_baud_for_adapter(&self, adapter_id: &str) -> AimResult<Option<u32>> {
        let conn = self.lock()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT capabilities FROM connections
                 WHERE adapter_id = ?1
                 ORDER BY connected_at DESC
                 LIMIT 1",
                [adapter_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        Ok(raw
            .and_then(|s| serde_json::from_str::<aim_types::AdapterCapabilities>(&s).ok())
            .and_then(|c| c.baud))
    }

    /// Store one configuration capture, returning its id.
    ///
    /// Kept beyond the session it was taken in, because the point of a capture
    /// is to compare it with one taken after somebody changed something — and
    /// that may be a week later, through the vehicle's own controls, with the
    /// laptop shut in between.
    pub fn record_capture(
        &self,
        session_id: &SessionId,
        vehicle_id: Option<&str>,
        module_key: &str,
        label: Option<&str>,
        taken_at: &str,
        capture_json: &str,
    ) -> AimResult<String> {
        let id = aim_types::CaptureId::new().to_string();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO config_captures
                 (id, session_id, vehicle_id, module_key, label, taken_at, capture)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session_id.as_str(), vehicle_id, module_key, label, taken_at, capture_json],
        )
        .map_err(storage)?;
        Ok(id)
    }

    /// One stored capture, by id.
    pub fn capture(&self, id: &str) -> AimResult<StoredCapture> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, session_id, vehicle_id, module_key, label, taken_at, capture
             FROM config_captures WHERE id = ?1",
            params![id],
            row_to_capture,
        )
        .optional()
        .map_err(storage)?
        .transpose()?
        .ok_or_else(|| AimError::not_found(format!("no capture {id:?}")))
    }

    /// Captures for a vehicle, newest first.
    ///
    /// Scoped by vehicle rather than by session so that today's "after" can
    /// find last week's "before".
    pub fn captures_for_vehicle(&self, vehicle_id: &str) -> AimResult<Vec<StoredCapture>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, vehicle_id, module_key, label, taken_at, capture
                 FROM config_captures WHERE vehicle_id = ?1 ORDER BY taken_at DESC",
            )
            .map_err(storage)?;
        let rows = stmt.query_map(params![vehicle_id], row_to_capture).map_err(storage)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(storage)??);
        }
        Ok(out)
    }

    /// Keep an as-built file against the VIN it was issued for.
    ///
    /// Keyed on that VIN and nothing else, because that is what the file is
    /// keyed on and what makes it safe to use later. A second import for the
    /// same vehicle replaces the first: these are snapshots of one unchanging
    /// factory record, and keeping a history of downloads of the same document
    /// would be filing cabinet, not evidence.
    ///
    /// Caller's job to have checked the VIN against the connected vehicle
    /// first — this stores what it is given.
    pub fn store_as_built(
        &self,
        vin: &str,
        source: Option<&str>,
        data_json: &str,
    ) -> AimResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO as_built (vin, imported_at, source, data)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(vin) DO UPDATE SET
                 imported_at = excluded.imported_at,
                 source = excluded.source,
                 data = excluded.data",
            params![vin.to_ascii_uppercase(), aim_types::now().to_rfc3339(), source, data_json],
        )
        .map_err(storage)?;
        Ok(())
    }

    /// The as-built file held for one VIN, when there is one.
    pub fn as_built(&self, vin: &str) -> AimResult<Option<StoredAsBuilt>> {
        let conn = self.lock()?;
        let row: Option<(String, String, Option<String>, String)> = conn
            .query_row(
                "SELECT vin, imported_at, source, data FROM as_built WHERE vin = ?1",
                params![vin.to_ascii_uppercase()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(storage)?;

        let Some((vin, imported_at, source, data)) = row else { return Ok(None) };
        let data = serde_json::from_str(&data).map_err(|e| {
            AimError::new(ErrorCode::StorageError, format!("stored as-built is undecodable: {e}"))
        })?;
        Ok(Some(StoredAsBuilt { vin, imported_at, source, data }))
    }

    /// Forget the as-built file held for one VIN.
    ///
    /// Somebody's vehicle identity and configuration live in that row, so
    /// removing it has to be as easy as adding it. Returns whether one was
    /// there.
    pub fn forget_as_built(&self, vin: &str) -> AimResult<bool> {
        let conn = self.lock()?;
        let n = conn
            .execute("DELETE FROM as_built WHERE vin = ?1", params![vin.to_ascii_uppercase()])
            .map_err(storage)?;
        Ok(n > 0)
    }

    /// Connections recorded for a session, oldest first.
    pub fn connections(&self, session_id: &SessionId) -> AimResult<Vec<ConnectionRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, adapter_id, transport, connected_at, disconnected_at,
                        firmware, capabilities
                 FROM connections WHERE session_id = ?1 ORDER BY connected_at",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([session_id.as_str()], row_to_connection)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }

    // ------------------------------------------------------------- modules

    /// Insert or update a discovered module, keyed on `(session, module_key)`.
    pub fn upsert_module(&self, m: &Module) -> AimResult<Module> {
        let conn = self.lock()?;
        let existing: Option<Module> = conn
            .query_row(
                "SELECT id, session_id, module_key, name, address, protocol, identity,
                        software_version, discovered_at, request_address
                 FROM modules WHERE session_id = ?1 AND module_key = ?2",
                params![m.session_id.as_str(), m.module_key],
                row_to_module,
            )
            .optional()
            .map_err(storage)?
            .transpose()?;

        if let Some(prev) = existing {
            conn.execute(
                "UPDATE modules SET name = ?3, address = ?4, protocol = ?5, identity = ?6,
                                    software_version = ?7, request_address = ?8
                 WHERE session_id = ?1 AND module_key = ?2",
                params![
                    m.session_id.as_str(),
                    m.module_key,
                    m.name,
                    m.address,
                    enum_str(&m.protocol)?,
                    json_str(&m.identity)?,
                    m.software_version,
                    m.request_address,
                ],
            )
            .map_err(storage)?;
            return Ok(Module { id: prev.id, discovered_at: prev.discovered_at, ..m.clone() });
        }

        conn.execute(
            "INSERT INTO modules (id, session_id, module_key, name, address, protocol, identity,
                                  software_version, discovered_at, request_address)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                m.id.as_str(),
                m.session_id.as_str(),
                m.module_key,
                m.name,
                m.address,
                enum_str(&m.protocol)?,
                json_str(&m.identity)?,
                m.software_version,
                m.discovered_at.to_rfc3339(),
                m.request_address,
            ],
        )
        .map_err(storage)?;
        Ok(m.clone())
    }

    /// Modules discovered in a session, in discovery order.
    pub fn modules(&self, session_id: &SessionId) -> AimResult<Vec<Module>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, module_key, name, address, protocol, identity,
                        software_version, discovered_at, request_address
                 FROM modules WHERE session_id = ?1 ORDER BY module_key",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([session_id.as_str()], row_to_module)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }

    /// One module by its id.
    pub fn get_module(&self, id: &ModuleId) -> AimResult<Module> {
        self.lock()?
            .query_row(
                "SELECT id, session_id, module_key, name, address, protocol, identity,
                        software_version, discovered_at, request_address
                 FROM modules WHERE id = ?1",
                [id.as_str()],
                row_to_module,
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| AimError::not_found(format!("no module {id}")))?
    }

    // ---------------------------------------------------------------- dtcs

    /// Record a DTC. Reading the same code twice in a session increments its
    /// occurrence count rather than creating a duplicate row.
    pub fn record_dtc(&self, d: &DtcRecord) -> AimResult<()> {
        self.lock()?
            .execute(
                "INSERT INTO dtcs (session_id, module_id, code, status, description, occurrence,
                                   freeze_frame_ref, read_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
                 ON CONFLICT(session_id, module_id, code, status) DO UPDATE SET
                     occurrence = occurrence + 1,
                     read_at = excluded.read_at,
                     description = COALESCE(excluded.description, dtcs.description),
                     freeze_frame_ref = COALESCE(excluded.freeze_frame_ref, dtcs.freeze_frame_ref)",
                params![
                    d.session_id.as_str(),
                    d.module_id.as_str(),
                    d.code,
                    d.status.as_str(),
                    d.description,
                    d.freeze_frame_ref,
                    d.read_at.to_rfc3339(),
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// DTCs for a session, optionally limited to one module.
    pub fn dtcs(
        &self,
        session_id: &SessionId,
        module_id: Option<&ModuleId>,
    ) -> AimResult<Vec<DtcRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, module_id, code, status, description, occurrence,
                        freeze_frame_ref, read_at
                 FROM dtcs
                 WHERE session_id = ?1 AND (?2 IS NULL OR module_id = ?2)
                 ORDER BY module_id, code",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![session_id.as_str(), module_id.map(|m| m.as_str())], row_to_dtc)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }

    // -------------------------------------------------------- measurements

    /// Record one decoded reading.
    pub fn record_measurement(&self, m: &Measurement) -> AimResult<i64> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO measurements (session_id, module_id, timestamp, signal_id, value,
                                       text_value, unit, raw_value)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                m.session_id.as_str(),
                m.module_id.as_str(),
                m.timestamp.to_rfc3339(),
                m.signal_id,
                m.value,
                m.text_value,
                m.unit,
                m.raw_value,
            ],
        )
        .map_err(storage)?;
        Ok(conn.last_insert_rowid())
    }

    /// Measurements, newest first, optionally for one signal.
    pub fn measurements(
        &self,
        session_id: &SessionId,
        signal_id: Option<&str>,
        limit: u32,
    ) -> AimResult<Vec<Measurement>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, module_id, timestamp, signal_id, value, text_value, unit,
                        raw_value
                 FROM measurements
                 WHERE session_id = ?1 AND (?2 IS NULL OR signal_id = ?2)
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?3",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![session_id.as_str(), signal_id, limit], |r| {
                Ok(Measurement {
                    session_id: SessionId::from_string(r.get::<_, String>(0)?),
                    module_id: ModuleId::from_string(r.get::<_, String>(1)?),
                    timestamp: parse_ts(r, 2)?,
                    signal_id: r.get(3)?,
                    value: r.get(4)?,
                    text_value: r.get(5)?,
                    unit: r.get(6)?,
                    raw_value: r.get(7)?,
                })
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        Ok(rows)
    }

    // ----------------------------------------------- tests, traces, results

    /// Record a test run. Handoff §10: who asked and whether a human confirmed
    /// are not optional columns.
    pub fn record_test_run(&self, t: &TestRun) -> AimResult<()> {
        self.lock()?
            .execute(
                "INSERT INTO test_runs (id, session_id, module_id, test_id, risk_level,
                                        requested_by, confirmed_by_user, started_at, ended_at,
                                        result, evidence_ref, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                     ended_at = excluded.ended_at,
                     result = excluded.result,
                     evidence_ref = COALESCE(excluded.evidence_ref, test_runs.evidence_ref),
                     detail = COALESCE(excluded.detail, test_runs.detail)",
                params![
                    t.id.as_str(),
                    t.session_id.as_str(),
                    t.module_id.as_ref().map(|m| m.as_str()),
                    t.test_id,
                    t.risk_level.as_str(),
                    t.requested_by,
                    t.confirmed_by_user as i32,
                    t.started_at.to_rfc3339(),
                    t.ended_at.map(|x| x.to_rfc3339()),
                    enum_str(&t.result)?,
                    t.evidence_ref,
                    t.detail.as_ref().map(|d| d.to_string()),
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Test runs for a session.
    pub fn test_runs(&self, session_id: &SessionId) -> AimResult<Vec<TestRun>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, module_id, test_id, risk_level, requested_by,
                        confirmed_by_user, started_at, ended_at, result, evidence_ref, detail
                 FROM test_runs WHERE session_id = ?1 ORDER BY started_at",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([session_id.as_str()], row_to_test_run)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }

    /// Record an agent message or tool call. The agent runtime is not built
    /// yet; the table and this method are the seam it writes through.
    pub fn record_agent_trace(&self, t: &AgentTrace) -> AimResult<()> {
        self.lock()?
            .execute(
                "INSERT INTO agent_traces (session_id, message_id, role, content, tool_name,
                                           tool_args_ref, tool_result_ref, model, prompt_version,
                                           timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    t.session_id.as_str(),
                    t.message_id,
                    t.role,
                    t.content,
                    t.tool_name,
                    t.tool_args_ref,
                    t.tool_result_ref,
                    t.model,
                    t.prompt_version,
                    t.timestamp.to_rfc3339(),
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Agent traces for a session, oldest first.
    pub fn agent_traces(&self, session_id: &SessionId) -> AimResult<Vec<AgentTrace>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, message_id, role, content, tool_name, tool_args_ref,
                        tool_result_ref, model, prompt_version, timestamp
                 FROM agent_traces WHERE session_id = ?1 ORDER BY timestamp, id",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([session_id.as_str()], |r| {
                Ok(AgentTrace {
                    session_id: SessionId::from_string(r.get::<_, String>(0)?),
                    message_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    tool_name: r.get(4)?,
                    tool_args_ref: r.get(5)?,
                    tool_result_ref: r.get(6)?,
                    model: r.get(7)?,
                    prompt_version: r.get(8)?,
                    timestamp: parse_ts(r, 9)?,
                })
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        Ok(rows)
    }

    /// Record a diagnosis.
    pub fn record_diagnosis(&self, d: &Diagnosis) -> AimResult<()> {
        self.lock()?
            .execute(
                "INSERT INTO diagnoses (id, session_id, hypothesis, confidence, evidence_refs,
                                        alternatives, recommendation, unresolved_questions,
                                        created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    d.id.as_str(),
                    d.session_id.as_str(),
                    d.hypothesis,
                    d.confidence,
                    json_str(&d.evidence_refs)?,
                    json_str(&d.alternatives)?,
                    d.recommendation,
                    json_str(&d.unresolved_questions)?,
                    d.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Diagnoses for a session.
    pub fn diagnoses(&self, session_id: &SessionId) -> AimResult<Vec<Diagnosis>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, hypothesis, confidence, evidence_refs, alternatives,
                        recommendation, unresolved_questions, created_at
                 FROM diagnoses WHERE session_id = ?1 ORDER BY created_at",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([session_id.as_str()], row_to_diagnosis)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        rows.into_iter().collect()
    }
}

/// Convenience: build a [`Measurement`] from a decoded value.
pub fn measurement_from(
    session_id: &SessionId,
    module_id: &ModuleId,
    value: &aim_types::DecodedValue,
) -> Measurement {
    Measurement {
        session_id: session_id.clone(),
        module_id: module_id.clone(),
        timestamp: value.timestamp,
        signal_id: value.signal_id.clone(),
        value: value.value.as_f64(),
        text_value: value.value.as_str().map(|s| s.to_string()),
        unit: value.unit.clone(),
        // The raw bytes travel with every reading, so a stored measurement can
        // always be re-derived from the evidence rather than trusted on faith.
        raw_value: value.provenance.raw_hex.clone(),
    }
}

/// Hex helper re-exported so callers building raw evidence do not reach past
/// this crate for it.
pub fn raw_hex(bytes: &[u8]) -> String {
    hex(bytes)
}

// ------------------------------------------------------------- row mapping

fn get_vehicle(conn: &Connection, id: &VehicleId) -> AimResult<Option<Vehicle>> {
    conn.query_row(
        "SELECT id, vin, make, model, year, trim, engine, transmission, discovered_at
         FROM vehicles WHERE id = ?1",
        [id.as_str()],
        row_to_vehicle,
    )
    .optional()
    .map_err(storage)
}

fn row_to_session(r: &Row<'_>) -> rusqlite::Result<aim_types::Session> {
    Ok(aim_types::Session {
        id: SessionId::from_string(r.get::<_, String>(0)?),
        vehicle_id: r.get::<_, Option<String>>(1)?.map(VehicleId::from_string),
        started_at: parse_ts(r, 2)?,
        ended_at: parse_ts_opt(r, 3)?,
        label: r.get(4)?,
    })
}

fn row_to_vehicle(r: &Row<'_>) -> rusqlite::Result<Vehicle> {
    Ok(Vehicle {
        id: VehicleId::from_string(r.get::<_, String>(0)?),
        vin: r.get(1)?,
        make: r.get(2)?,
        model: r.get(3)?,
        year: r.get(4)?,
        trim: r.get(5)?,
        engine: r.get(6)?,
        transmission: r.get(7)?,
        discovered_at: parse_ts(r, 8)?,
    })
}

fn row_to_event(r: &Row<'_>) -> rusqlite::Result<AimResult<SessionEvent>> {
    let payload: String = r.get(5)?;
    let id: i64 = r.get(0)?;
    let session_id = SessionId::from_string(r.get::<_, String>(1)?);
    let seq: i64 = r.get(2)?;
    let timestamp = parse_ts(r, 3)?;
    Ok(match serde_json::from_str::<EventKind>(&payload) {
        Ok(kind) => Ok(SessionEvent { id: Some(id), session_id, seq, timestamp, kind }),
        Err(e) => Err(AimError::new(
            ErrorCode::StorageError,
            format!("event {id} has an undecodable payload: {e}"),
        )),
    })
}

fn row_to_connection(r: &Row<'_>) -> rusqlite::Result<AimResult<ConnectionRecord>> {
    // SQL extraction first, so rusqlite errors stay rusqlite errors; decoding
    // of stored JSON and enum spellings is a separate, fallible step.
    let id = aim_types::ConnectionId::from_string(r.get::<_, String>(0)?);
    let session_id = SessionId::from_string(r.get::<_, String>(1)?);
    let adapter_id: String = r.get(2)?;
    let transport: String = r.get(3)?;
    let connected_at = parse_ts(r, 4)?;
    let disconnected_at = parse_ts_opt(r, 5)?;
    let firmware: Option<String> = r.get(6)?;
    let caps: String = r.get(7)?;

    Ok(from_enum_str(&transport).and_then(|transport| {
        Ok(ConnectionRecord {
            id,
            session_id,
            adapter_id,
            transport,
            connected_at,
            disconnected_at,
            firmware,
            capabilities: from_json_str(&caps)?,
        })
    }))
}

fn row_to_module(r: &Row<'_>) -> rusqlite::Result<AimResult<Module>> {
    let id = ModuleId::from_string(r.get::<_, String>(0)?);
    let session_id = SessionId::from_string(r.get::<_, String>(1)?);
    let module_key: String = r.get(2)?;
    let name: String = r.get(3)?;
    let address: String = r.get(4)?;
    let protocol: String = r.get(5)?;
    let identity: String = r.get(6)?;
    let software_version: Option<String> = r.get(7)?;
    let discovered_at = parse_ts(r, 8)?;
    let request_address: Option<String> = r.get(9)?;

    Ok(from_enum_str(&protocol).and_then(|protocol| {
        Ok(Module {
            id,
            session_id,
            module_key,
            name,
            address,
            request_address,
            protocol,
            identity: from_json_str(&identity)?,
            software_version,
            discovered_at,
        })
    }))
}

fn row_to_dtc(r: &Row<'_>) -> rusqlite::Result<AimResult<DtcRecord>> {
    let session_id = SessionId::from_string(r.get::<_, String>(0)?);
    let module_id = ModuleId::from_string(r.get::<_, String>(1)?);
    let code: String = r.get(2)?;
    let status: String = r.get(3)?;
    let description: Option<String> = r.get(4)?;
    let occurrence: u32 = r.get(5)?;
    let freeze_frame_ref: Option<i64> = r.get(6)?;
    let read_at = parse_ts(r, 7)?;

    let status = match status.as_str() {
        "confirmed" => Ok(DtcStatus::Confirmed),
        "pending" => Ok(DtcStatus::Pending),
        "permanent" => Ok(DtcStatus::Permanent),
        other => Err(AimError::new(
            ErrorCode::StorageError,
            format!("unknown stored DTC status {other:?}"),
        )),
    };
    Ok(status.map(|status| DtcRecord {
        session_id,
        module_id,
        code,
        status,
        description,
        occurrence,
        freeze_frame_ref,
        read_at,
    }))
}

fn row_to_test_run(r: &Row<'_>) -> rusqlite::Result<AimResult<TestRun>> {
    let id = aim_types::TestRunId::from_string(r.get::<_, String>(0)?);
    let session_id = SessionId::from_string(r.get::<_, String>(1)?);
    let module_id = r.get::<_, Option<String>>(2)?.map(ModuleId::from_string);
    let test_id: String = r.get(3)?;
    let level: String = r.get(4)?;
    let requested_by: String = r.get(5)?;
    let confirmed_by_user = r.get::<_, i64>(6)? != 0;
    let started_at = parse_ts(r, 7)?;
    let ended_at = parse_ts_opt(r, 8)?;
    let result: String = r.get(9)?;
    let evidence_ref: Option<i64> = r.get(10)?;
    let detail: Option<String> = r.get(11)?;

    let risk_level = match level.as_str() {
        "L0" => Ok(aim_types::PermissionLevel::L0),
        "L1" => Ok(aim_types::PermissionLevel::L1),
        "L2" => Ok(aim_types::PermissionLevel::L2),
        "L3" => Ok(aim_types::PermissionLevel::L3),
        other => Err(AimError::new(
            ErrorCode::StorageError,
            format!("unknown stored permission level {other:?}"),
        )),
    };

    Ok(risk_level.and_then(|risk_level| {
        Ok(TestRun {
            id,
            session_id,
            module_id,
            test_id,
            risk_level,
            requested_by,
            confirmed_by_user,
            started_at,
            ended_at,
            result: from_enum_str(&result)?,
            evidence_ref,
            detail: match detail {
                Some(d) => Some(from_json_str(&d)?),
                None => None,
            },
        })
    }))
}

fn row_to_diagnosis(r: &Row<'_>) -> rusqlite::Result<AimResult<Diagnosis>> {
    let id = aim_types::DiagnosisId::from_string(r.get::<_, String>(0)?);
    let session_id = SessionId::from_string(r.get::<_, String>(1)?);
    let hypothesis: String = r.get(2)?;
    let confidence: f64 = r.get(3)?;
    let refs: String = r.get(4)?;
    let alts: String = r.get(5)?;
    let recommendation: String = r.get(6)?;
    let questions: String = r.get(7)?;
    let created_at = parse_ts(r, 8)?;

    Ok((|| {
        Ok(Diagnosis {
            id,
            session_id,
            hypothesis,
            confidence,
            evidence_refs: from_json_str(&refs)?,
            alternatives: from_json_str(&alts)?,
            recommendation,
            unresolved_questions: from_json_str(&questions)?,
            created_at,
        })
    })())
}

fn parse_ts(r: &Row<'_>, idx: usize) -> rusqlite::Result<Timestamp> {
    let s: String = r.get(idx)?;
    Timestamp::parse_rfc3339(&s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(format!("{s:?} is not an RFC 3339 timestamp"))),
        )
    })
}

fn parse_ts_opt(r: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Timestamp>> {
    match r.get::<_, Option<String>>(idx)? {
        None => Ok(None),
        Some(s) => Ok(Timestamp::parse_rfc3339(&s)),
    }
}

fn enum_str<T: Serialize>(v: &T) -> AimResult<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        Ok(other) => Err(AimError::new(
            ErrorCode::StorageError,
            format!("expected a string-valued enum, got {other}"),
        )),
        Err(e) => Err(AimError::new(ErrorCode::StorageError, e.to_string())),
    }
}

fn from_enum_str<T: for<'de> Deserialize<'de>>(s: &str) -> AimResult<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| {
        AimError::new(
            ErrorCode::StorageError,
            format!("stored value {s:?} is not a known variant: {e}"),
        )
    })
}

fn json_str<T: Serialize>(v: &T) -> AimResult<String> {
    serde_json::to_string(v).map_err(|e| AimError::new(ErrorCode::StorageError, e.to_string()))
}

fn from_json_str<T: for<'de> Deserialize<'de>>(s: &str) -> AimResult<T> {
    serde_json::from_str(s).map_err(|e| {
        AimError::new(ErrorCode::StorageError, format!("stored JSON is undecodable: {e}"))
    })
}

fn row_to_capture(r: &Row<'_>) -> rusqlite::Result<AimResult<StoredCapture>> {
    let capture: String = r.get(6)?;
    Ok(Ok(StoredCapture {
        id: r.get(0)?,
        session_id: r.get(1)?,
        vehicle_id: r.get(2)?,
        module_key: r.get(3)?,
        label: r.get(4)?,
        taken_at: r.get(5)?,
        capture: match serde_json::from_str(&capture) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Err(AimError::new(
                    ErrorCode::StorageError,
                    format!("stored capture is undecodable: {e}"),
                )))
            }
        },
    }))
}

fn storage(e: rusqlite::Error) -> AimError {
    AimError::new(ErrorCode::StorageError, e.to_string())
}

/// True when SQLite is claiming the file itself is damaged.
///
/// These are the two codes that produce the bare string "database disk image is
/// malformed", which tells a person nothing they can act on.
fn is_corruption(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt) | Some(rusqlite::ErrorCode::NotADatabase)
    )
}

/// Turn a failed open into something a person can act on.
///
/// "database disk image is malformed" is SQLite telling the truth in a way that
/// helps nobody: it names no file, and it is reported identically whether the
/// data is destroyed or a write-ahead log was simply left behind by a crash.
/// The second case is far more common and is recoverable, so the message says
/// which file, and what to do, in that order.
fn open_failure(path: &Path, e: rusqlite::Error) -> AimError {
    if !is_corruption(&e) {
        return AimError::new(
            ErrorCode::StorageError,
            format!("cannot open {}: {e}", path.display()),
        );
    }
    let wal = path.with_extension("sqlite-wal");
    let stale_wal = wal.exists();
    let mut msg = format!(
        "{} could not be opened ({e}). The history is not necessarily lost",
        path.display()
    );
    if stale_wal {
        msg.push_str(
            ": a write-ahead log is still sitting beside it, which is what an app \
             that was killed rather than closed leaves behind. Make sure no other \
             copy of the app is running and open it again",
        );
    } else {
        msg.push_str(
            ". Close any other copy of the app and try again; if it still fails, \
             move the file aside and a fresh database will be created",
        );
    }
    AimError::new(ErrorCode::StorageError, msg)
}

/// Add the file path to a failure that happened after the handle was open.
///
/// Migration and the first pragma both touch pages, so corruption frequently
/// surfaces here rather than at `open`.
fn enrich_open_failure(path: &Path, e: AimError) -> AimError {
    if e.message.contains(&path.display().to_string()) {
        return e;
    }
    AimError::new(
        ErrorCode::StorageError,
        format!("{} could not be prepared: {}", path.display(), e.message),
    )
}

#[cfg(test)]
mod open_failure_tests {
    use super::*;

    /// A file that is not a database at all must not be reported with SQLite's
    /// own wording. "database disk image is malformed" names no file and
    /// suggests no action, which is exactly how a person ends up believing
    /// their history is gone when it is not.
    #[test]
    fn a_non_database_file_reports_which_file_and_what_to_do() {
        let dir = std::env::temp_dir().join(format!("aim-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-database.sqlite");
        std::fs::write(&path, b"this is plainly not a SQLite file").unwrap();

        let err = SessionStore::open(&path).expect_err("must not open");

        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("not-a-database.sqlite"),
            "the message must name the file: {}",
            err.message
        );
        assert!(
            msg.contains("try again") || msg.contains("open it again"),
            "the message must suggest an action: {}",
            err.message
        );
        assert_eq!(err.code, ErrorCode::StorageError);

        let _ = std::fs::remove_file(&path);
    }

    /// The happy path must keep working, and must actually be in WAL mode with
    /// a busy timeout - both are set with `let _ =`, so a regression would
    /// otherwise be silent.
    #[test]
    fn a_fresh_database_is_wal_with_a_busy_timeout() {
        let dir = std::env::temp_dir().join(format!("aim-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fresh.sqlite");
        let _ = std::fs::remove_file(&path);

        let store = SessionStore::open(&path).expect("opens");
        assert_eq!(store.pragma_string("journal_mode").unwrap(), "wal");
        assert_eq!(store.integrity_check().unwrap(), "ok");

        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
