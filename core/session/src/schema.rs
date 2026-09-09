//! Schema migrations.
//!
//! Migrations are an ordered, append-only list. Each runs once inside a
//! transaction and is recorded in `schema_migrations`; the applied version is
//! also kept in SQLite's own `user_version` so a database can be identified
//! without reading a table. Editing a shipped migration is forbidden — add a
//! new one — because the garage laptop will be carrying databases from earlier
//! builds.
//!
//! The tables implement handoff §12 plus one addition the handoff calls for in
//! §9: `session_events`, the append-only chronological log that the Diagnostic
//! Flight Recorder replays.

use aim_types::{AimError, AimResult, ErrorCode};
use rusqlite::Connection;

/// One numbered schema step.
pub struct Migration {
    /// Version this migration produces. Strictly increasing, never reused.
    pub version: i64,
    /// What it does, for the migration log and for humans reading `.schema`.
    pub name: &'static str,
    /// The SQL. Runs inside a transaction.
    pub sql: &'static str,
}

/// Every migration, in order. Append only.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial core data model",
    sql: r#"
-- ---------------------------------------------------------------- sessions
-- A session is one connection lifecycle and everything read during it.
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY,
    vehicle_id   TEXT REFERENCES vehicles(id),
    started_at   TEXT NOT NULL,
    ended_at     TEXT,
    label        TEXT
);
CREATE INDEX idx_sessions_started ON sessions(started_at DESC);

CREATE TABLE vehicles (
    id            TEXT PRIMARY KEY,
    vin           TEXT,
    make          TEXT,
    model         TEXT,
    year          INTEGER,
    trim          TEXT,
    engine        TEXT,
    transmission  TEXT,
    discovered_at TEXT NOT NULL
);
-- A VIN identifies a vehicle across sessions; vehicles without one stay
-- distinct rows rather than being merged on a guess.
CREATE UNIQUE INDEX idx_vehicles_vin ON vehicles(vin) WHERE vin IS NOT NULL;

CREATE TABLE connections (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id),
    adapter_id      TEXT NOT NULL,
    transport       TEXT NOT NULL,
    connected_at    TEXT NOT NULL,
    disconnected_at TEXT,
    firmware        TEXT,
    capabilities    TEXT NOT NULL   -- JSON AdapterCapabilities
);
CREATE INDEX idx_connections_session ON connections(session_id);

CREATE TABLE modules (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL REFERENCES sessions(id),
    module_key       TEXT NOT NULL,
    name             TEXT NOT NULL,
    address          TEXT NOT NULL,
    protocol         TEXT NOT NULL,
    identity         TEXT NOT NULL,  -- JSON ModuleIdentity
    software_version TEXT,
    discovered_at    TEXT NOT NULL,
    UNIQUE(session_id, module_key)
);
CREATE INDEX idx_modules_session ON modules(session_id);

CREATE TABLE dtcs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id       TEXT NOT NULL REFERENCES sessions(id),
    module_id        TEXT NOT NULL REFERENCES modules(id),
    code             TEXT NOT NULL,
    status           TEXT NOT NULL,
    description      TEXT,
    occurrence       INTEGER NOT NULL DEFAULT 1,
    freeze_frame_ref INTEGER REFERENCES session_events(id),
    read_at          TEXT NOT NULL,
    UNIQUE(session_id, module_id, code, status)
);
CREATE INDEX idx_dtcs_session ON dtcs(session_id);

CREATE TABLE measurements (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    module_id  TEXT NOT NULL REFERENCES modules(id),
    timestamp  TEXT NOT NULL,
    signal_id  TEXT NOT NULL,
    value      REAL,
    text_value TEXT,
    unit       TEXT,
    raw_value  TEXT NOT NULL   -- hex of the bytes this was decoded from
);
CREATE INDEX idx_measurements_signal ON measurements(session_id, signal_id, timestamp);

CREATE TABLE test_runs (
    id                TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES sessions(id),
    module_id         TEXT REFERENCES modules(id),
    test_id           TEXT NOT NULL,
    risk_level        TEXT NOT NULL,
    requested_by      TEXT NOT NULL,
    confirmed_by_user INTEGER NOT NULL DEFAULT 0,
    started_at        TEXT NOT NULL,
    ended_at          TEXT,
    result            TEXT NOT NULL,
    evidence_ref      INTEGER REFERENCES session_events(id),
    detail            TEXT
);
CREATE INDEX idx_test_runs_session ON test_runs(session_id);

CREATE TABLE agent_traces (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(id),
    message_id      TEXT NOT NULL,
    role            TEXT NOT NULL,
    content         TEXT,
    tool_name       TEXT,
    tool_args_ref   INTEGER REFERENCES session_events(id),
    tool_result_ref INTEGER REFERENCES session_events(id),
    model           TEXT,
    prompt_version  TEXT,
    timestamp       TEXT NOT NULL
);
CREATE INDEX idx_agent_traces_session ON agent_traces(session_id, timestamp);

CREATE TABLE diagnoses (
    id                   TEXT PRIMARY KEY,
    session_id           TEXT NOT NULL REFERENCES sessions(id),
    hypothesis           TEXT NOT NULL,
    confidence           REAL NOT NULL,
    evidence_refs        TEXT NOT NULL,  -- JSON array of session_events.id
    alternatives         TEXT NOT NULL,  -- JSON array
    recommendation       TEXT NOT NULL,
    unresolved_questions TEXT NOT NULL,  -- JSON array
    created_at           TEXT NOT NULL
);
CREATE INDEX idx_diagnoses_session ON diagnoses(session_id);

-- --------------------------------------------------------- flight recorder
-- The append-only event log. Every adapter request and response, every
-- connection transition, every safety decision and every decoded reading lands
-- here in order. `seq` is per-session and gap-free, so a replay is exact and a
-- missing event is detectable.
CREATE TABLE session_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    seq        INTEGER NOT NULL,
    timestamp  TEXT NOT NULL,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,   -- JSON EventKind
    UNIQUE(session_id, seq)
);
CREATE INDEX idx_events_session_seq ON session_events(session_id, seq);
CREATE INDEX idx_events_kind ON session_events(session_id, kind);

-- Events are a record of what happened. Rewriting history would make the
-- flight recorder worthless as evidence, so the database refuses.
CREATE TRIGGER session_events_are_append_only_update
BEFORE UPDATE ON session_events
BEGIN
    SELECT RAISE(ABORT, 'session_events is append-only');
END;
CREATE TRIGGER session_events_are_append_only_delete
BEFORE DELETE ON session_events
BEGIN
    SELECT RAISE(ABORT, 'session_events is append-only');
END;
"#,
}];

/// Bring `conn` up to the latest schema version, returning that version.
pub fn migrate(conn: &mut Connection) -> AimResult<i64> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at TEXT NOT NULL
         );",
    )
    .map_err(storage)?;

    let applied: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))
        .map_err(storage)?;

    for m in MIGRATIONS {
        if m.version <= applied {
            continue;
        }
        let tx = conn.transaction().map_err(storage)?;
        tx.execute_batch(m.sql).map_err(|e| {
            AimError::new(
                ErrorCode::StorageError,
                format!("migration {} ({}) failed: {e}", m.version, m.name),
            )
        })?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.version, m.name, aim_types::now().to_rfc3339()],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
    }

    let current = MIGRATIONS.last().map(|m| m.version).unwrap_or(0);
    conn.pragma_update(None, "user_version", current).map_err(storage)?;
    Ok(current)
}

/// The schema version this build produces.
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
}

fn storage(e: rusqlite::Error) -> AimError {
    AimError::new(ErrorCode::StorageError, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_versions_are_unique_and_ascending() {
        let mut last = 0;
        for m in MIGRATIONS {
            assert!(m.version > last, "migration {} is out of order", m.version);
            last = m.version;
        }
    }

    #[test]
    fn migrating_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        let applied: i64 =
            conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    #[test]
    fn the_user_version_pragma_identifies_the_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, latest_version());
    }

    #[test]
    fn every_table_in_the_handoff_data_model_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        for table in [
            "sessions",
            "vehicles",
            "connections",
            "modules",
            "dtcs",
            "measurements",
            "test_runs",
            "agent_traces",
            "diagnoses",
            "session_events",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} is missing");
        }
    }

    #[test]
    fn the_event_log_refuses_updates_and_deletes() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, started_at) VALUES ('ses_1', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_events (session_id, seq, timestamp, kind, payload)
             VALUES ('ses_1', 1, '2026-01-01T00:00:00Z', 'session_started', '{}')",
            [],
        )
        .unwrap();

        let update = conn.execute("UPDATE session_events SET kind = 'tampered'", []);
        assert!(update.is_err(), "the event log must reject updates");
        let delete = conn.execute("DELETE FROM session_events", []);
        assert!(delete.is_err(), "the event log must reject deletes");

        let still_there: i64 =
            conn.query_row("SELECT COUNT(*) FROM session_events", [], |r| r.get(0)).unwrap();
        assert_eq!(still_there, 1);
    }
}
