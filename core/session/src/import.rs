//! Taking in another install's database.
//!
//! A vehicle is scanned by whichever device is at the car, and what the app
//! knows about it is kept against its VIN. Without this, that knowledge is
//! split by device: a drive recorded on a phone never reaches the truck's
//! history on the laptop, and a second visit starts no further ahead than the
//! first. [`SessionStore::import_database`] merges a copy made by
//! [`SessionStore::snapshot_to`] into this database.
//!
//! # What it promises
//!
//! - **Nothing already here is replaced by something older.** A session this
//!   database already holds in full is left alone. A finding, an as-built file
//!   or a VIN lookup is replaced only by a newer one.
//! - **Importing twice adds nothing the second time.** A session is known by
//!   its id and an event by its place in the session, so the same file, or a
//!   later export from the same phone, is safe to bring in again.
//! - **A session exported while it was still open can be finished later.** A
//!   later export holds the same session with more in it, and only the part
//!   this database lacks is added.
//! - **The recorded exchanges are copied as they are.** No event is edited on
//!   the way in. The places that point at an event by row number (a code's
//!   freeze frame, a test's evidence) are pointed at the same event's new
//!   row, or at nothing when that event did not come across. Never at another.
//! - **All of it or none of it.** The merge is one transaction.

use crate::schema;
use crate::store::{storage, SessionStore};
use aim_types::{AimError, AimResult, ErrorCode};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

/// Every table an import has to account for, and the columns it carries.
///
/// Spelled out rather than `SELECT *` so that a column added by a later
/// migration cannot be dropped on the way in without anyone noticing: a test
/// compares this list with the schema and fails when they part.
const CARRIED: &[(&str, &[&str])] = &[
    (
        "vehicles",
        &["id", "vin", "make", "model", "year", "trim", "engine", "transmission", "discovered_at"],
    ),
    ("sessions", &["id", "vehicle_id", "started_at", "ended_at", "label"]),
    (
        "connections",
        &[
            "id",
            "session_id",
            "adapter_id",
            "transport",
            "connected_at",
            "disconnected_at",
            "firmware",
            "capabilities",
        ],
    ),
    (
        "modules",
        &[
            "id",
            "session_id",
            "module_key",
            "name",
            "address",
            "protocol",
            "identity",
            "software_version",
            "discovered_at",
            "request_address",
        ],
    ),
    ("session_events", &["id", "session_id", "seq", "timestamp", "kind", "payload"]),
    (
        "dtcs",
        &[
            "id",
            "session_id",
            "module_id",
            "code",
            "status",
            "description",
            "occurrence",
            "freeze_frame_ref",
            "read_at",
        ],
    ),
    (
        "measurements",
        &[
            "id",
            "session_id",
            "module_id",
            "timestamp",
            "signal_id",
            "value",
            "text_value",
            "unit",
            "raw_value",
        ],
    ),
    (
        "test_runs",
        &[
            "id",
            "session_id",
            "module_id",
            "test_id",
            "risk_level",
            "requested_by",
            "confirmed_by_user",
            "started_at",
            "ended_at",
            "result",
            "evidence_ref",
            "detail",
        ],
    ),
    (
        "agent_traces",
        &[
            "id",
            "session_id",
            "message_id",
            "role",
            "content",
            "tool_name",
            "tool_args_ref",
            "tool_result_ref",
            "model",
            "prompt_version",
            "timestamp",
        ],
    ),
    (
        "diagnoses",
        &[
            "id",
            "session_id",
            "hypothesis",
            "confidence",
            "evidence_refs",
            "alternatives",
            "recommendation",
            "unresolved_questions",
            "created_at",
        ],
    ),
    (
        "config_captures",
        &["id", "session_id", "vehicle_id", "module_key", "label", "taken_at", "capture"],
    ),
    ("as_built", &["vin", "imported_at", "source", "data"]),
    (
        "vehicle_knowledge",
        &[
            "vin",
            "subject",
            "outcome",
            "claim",
            "evidence",
            "authority",
            "observed_at",
            "session_id",
        ],
    ),
    ("vpic_replies", &["vin", "fetched_at", "source_url", "body"]),
];

/// What an import added, or would add.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportSummary {
    /// False for a preview: everything below is what would happen.
    pub committed: bool,
    /// Sessions this database did not have.
    pub sessions_added: u64,
    /// Sessions it had the start of, now with the rest.
    pub sessions_extended: u64,
    /// Sessions it already had in full, or had more of than the file does.
    pub sessions_already_here: u64,
    /// Sessions left out, and why.
    pub sessions_skipped: Vec<SkippedSession>,
    /// Vehicles this database had never seen.
    pub vehicles_added: u64,
    /// The VINs of the sessions added or extended. A vehicle that never gave
    /// one is listed as `null`.
    pub vehicles: Vec<Option<String>>,
    /// Recorded events added.
    pub events_added: u64,
    /// Readings added.
    pub measurements_added: u64,
    /// Fault code records added.
    pub dtcs_added: u64,
    /// Configuration captures added.
    pub captures_added: u64,
    /// Findings about a vehicle that this database did not have.
    pub findings_added: u64,
    /// Findings replaced by a newer one from the file.
    pub findings_updated: u64,
    /// Findings left alone because the one here is the newer.
    pub findings_kept: u64,
    /// As-built files and VIN lookups taken, new or newer.
    pub vehicle_records_taken: u64,
}

impl ImportSummary {
    /// True when the file holds nothing this database lacks.
    pub fn is_empty(&self) -> bool {
        self.sessions_added == 0
            && self.sessions_extended == 0
            && self.vehicles_added == 0
            && self.captures_added == 0
            && self.findings_added == 0
            && self.findings_updated == 0
            && self.vehicle_records_taken == 0
    }
}

/// A session an import left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedSession {
    /// The session.
    pub id: String,
    /// Why, in words a person can act on.
    pub reason: String,
}

/// The same event in this database, for an event's row number in the file.
/// Null when that event is not here, which leaves the reference empty rather
/// than pointing at whatever happens to have that number.
fn event_here(src_ref: &str) -> String {
    format!(
        "(SELECT l.id FROM src.session_events s
          JOIN main.session_events l ON l.session_id = s.session_id AND l.seq = s.seq
          WHERE s.id = {src_ref})"
    )
}

fn refused(message: impl Into<String>) -> AimError {
    AimError::new(ErrorCode::BadRequest, message)
}

impl SessionStore {
    /// Merge the database at `file` into this one.
    ///
    /// `file` has to be a copy the caller owns: it is brought up to this
    /// build's schema in place before it is read. With `dry_run` the merge is
    /// worked out in full and then undone, so the summary is exact and
    /// nothing is kept.
    pub fn import_database(
        &self,
        file: impl AsRef<Path>,
        dry_run: bool,
    ) -> AimResult<ImportSummary> {
        let file = file.as_ref();
        let target = file
            .to_str()
            .ok_or_else(|| refused(format!("{} is not a UTF-8 path", file.display())))?;
        if self.path().is_some_and(|own| same_file(Path::new(own), file)) {
            return Err(refused("that file is this app's own database"));
        }
        prepare(file)?;

        let conn = self.lock()?;
        // The file is somebody's upload. Nothing in its schema gets to run
        // anything of ours.
        conn.pragma_update(None, "trusted_schema", "OFF").map_err(storage)?;
        conn.execute("ATTACH DATABASE ?1 AS src", [target]).map_err(storage)?;
        let merged = merge(&conn, dry_run);
        // Outside the transaction, which has ended either way by now.
        let _ = conn.execute_batch("DETACH DATABASE src");
        merged
    }
}

/// True when both paths are one file. Paths that cannot be resolved are
/// treated as different, which is what lets a missing file fail later with a
/// message about that file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Check that `file` is one of this app's databases and bring it up to the
/// schema this build reads.
fn prepare(file: &Path) -> AimResult<()> {
    if !file.is_file() {
        return Err(refused(format!("there is no file at {}", file.display())));
    }
    let not_ours = || refused("that file is not a WTFault database");
    let mut conn = Connection::open(file).map_err(|_| not_ours())?;
    let version: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))
        .map_err(|_| not_ours())?;
    if version == 0 {
        return Err(not_ours());
    }
    if version > schema::latest_version() {
        return Err(refused(format!(
            "that database was written by a newer version of the app (schema {version}; this \
             build reads up to {}). Update this app, then import it again.",
            schema::latest_version()
        )));
    }
    schema::migrate(&mut conn)?;

    // Every name read below has to be a plain table. A view under one of
    // these names would have this import run a query the file wrote.
    for (table, _) in CARRIED {
        let kind: Option<String> = conn
            .query_row("SELECT type FROM sqlite_master WHERE name = ?1", [table], |r| r.get(0))
            .optional()
            .map_err(|_| not_ours())?;
        if kind.as_deref() != Some("table") {
            return Err(not_ours());
        }
    }
    Ok(())
}

fn count(conn: &Connection, table: &str) -> AimResult<u64> {
    conn.query_row(&format!("SELECT COUNT(*) FROM main.{table}"), [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
        .map_err(storage)
}

/// The whole merge, in one transaction that a preview rolls back.
fn merge(conn: &Connection, dry_run: bool) -> AimResult<ImportSummary> {
    let tx = conn.unchecked_transaction().map_err(storage)?;
    let mut summary = ImportSummary::default();

    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.import_vehicle;
         DROP TABLE IF EXISTS temp.import_take;
         -- A vehicle row in the file, and the row here that is the same vehicle.
         CREATE TEMP TABLE import_vehicle (src_id TEXT PRIMARY KEY, local_id TEXT NOT NULL);
         -- The sessions being brought in, and how far this database already
         -- has each one: 0 for a session it has never seen.
         CREATE TEMP TABLE import_take (session_id TEXT PRIMARY KEY, local_max INTEGER NOT NULL);",
    )
    .map_err(storage)?;

    // ---- vehicles ---------------------------------------------------------
    // The VIN is the vehicle. Two devices that each met the same truck gave it
    // two row ids, and everything from the file is filed under the one here.
    let before = count(&tx, "vehicles")?;
    tx.execute_batch(
        "INSERT INTO import_vehicle (src_id, local_id)
             SELECT s.id, m.id FROM src.vehicles s
             JOIN main.vehicles m ON m.vin = s.vin
             WHERE s.vin IS NOT NULL;

         -- What the other device worked out and this one has not: a model
         -- settled by a VIN lookup made there, say. Never the other way.
         UPDATE main.vehicles SET
             make         = COALESCE(make,         (SELECT s.make         FROM src.vehicles s WHERE s.vin = main.vehicles.vin)),
             model        = COALESCE(model,        (SELECT s.model        FROM src.vehicles s WHERE s.vin = main.vehicles.vin)),
             year         = COALESCE(year,         (SELECT s.year         FROM src.vehicles s WHERE s.vin = main.vehicles.vin)),
             trim         = COALESCE(trim,         (SELECT s.trim         FROM src.vehicles s WHERE s.vin = main.vehicles.vin)),
             engine       = COALESCE(engine,       (SELECT s.engine       FROM src.vehicles s WHERE s.vin = main.vehicles.vin)),
             transmission = COALESCE(transmission, (SELECT s.transmission FROM src.vehicles s WHERE s.vin = main.vehicles.vin))
         WHERE vin IN (SELECT vin FROM src.vehicles WHERE vin IS NOT NULL);

         INSERT INTO main.vehicles
                (id, vin, make, model, year, trim, engine, transmission, discovered_at)
             SELECT id, vin, make, model, year, trim, engine, transmission, discovered_at
             FROM src.vehicles s
             WHERE s.id NOT IN (SELECT src_id FROM import_vehicle)
               AND s.id NOT IN (SELECT id FROM main.vehicles);

         INSERT OR IGNORE INTO import_vehicle (src_id, local_id)
             SELECT id, id FROM src.vehicles;",
    )
    .map_err(storage)?;
    summary.vehicles_added = count(&tx, "vehicles")? - before;

    // ---- which sessions ---------------------------------------------------
    let candidates: Vec<(String, bool, i64, i64)> = {
        let mut stmt = tx
            .prepare(
                "SELECT s.id,
                        EXISTS (SELECT 1 FROM main.sessions m WHERE m.id = s.id),
                        COALESCE((SELECT MAX(seq) FROM main.session_events e WHERE e.session_id = s.id), 0),
                        COALESCE((SELECT MAX(seq) FROM src.session_events e WHERE e.session_id = s.id), 0)
                 FROM src.sessions s ORDER BY s.started_at",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)?
    };
    for (id, here, local_max, src_max) in candidates {
        if !here {
            summary.sessions_added += 1;
            tx.execute("INSERT INTO import_take VALUES (?1, 0)", [&id]).map_err(storage)?;
            continue;
        }
        if local_max > src_max {
            // An older export of a session this database has since been
            // given more of.
            summary.sessions_already_here += 1;
            continue;
        }
        // The session here has to be the opening of the one in the file. A
        // session id is unique to the device that recorded it, so anything
        // else means one of the two is not what it says it is.
        let same_so_far: bool = tx
            .query_row(
                "SELECT (SELECT timestamp || '|' || kind FROM main.session_events
                         WHERE session_id = ?1 AND seq = ?2)
                     IS (SELECT timestamp || '|' || kind FROM src.session_events
                         WHERE session_id = ?1 AND seq = ?2)",
                params![id, local_max],
                |r| r.get(0),
            )
            .map_err(storage)?;
        if !same_so_far {
            summary.sessions_skipped.push(SkippedSession {
                id,
                reason: "a session with this id is already here and its recording differs \
                         from the file's, so neither was changed"
                    .into(),
            });
        } else if src_max == local_max {
            summary.sessions_already_here += 1;
        } else {
            summary.sessions_extended += 1;
            tx.execute("INSERT INTO import_take VALUES (?1, ?2)", params![id, local_max])
                .map_err(storage)?;
        }
    }

    // ---- sessions and what hangs off them ---------------------------------
    let events_before = count(&tx, "session_events")?;
    let measurements_before = count(&tx, "measurements")?;
    let dtcs_before = count(&tx, "dtcs")?;

    let sessions_and_events = "
        INSERT INTO main.sessions (id, vehicle_id, started_at, ended_at, label)
            SELECT s.id, v.local_id, s.started_at, s.ended_at, s.label
            FROM src.sessions s
            JOIN import_take t ON t.session_id = s.id
            LEFT JOIN import_vehicle v ON v.src_id = s.vehicle_id
            WHERE t.local_max = 0
              AND NOT EXISTS (SELECT 1 FROM main.sessions m WHERE m.id = s.id);

        -- A session this database had the start of may since have ended, or
        -- have had its vehicle identified.
        UPDATE main.sessions SET
            ended_at   = COALESCE(ended_at,
                (SELECT s.ended_at FROM src.sessions s WHERE s.id = main.sessions.id)),
            label      = COALESCE(label,
                (SELECT s.label FROM src.sessions s WHERE s.id = main.sessions.id)),
            vehicle_id = COALESCE(vehicle_id,
                (SELECT v.local_id FROM src.sessions s
                 JOIN import_vehicle v ON v.src_id = s.vehicle_id
                 WHERE s.id = main.sessions.id))
        WHERE id IN (SELECT session_id FROM import_take);

        INSERT INTO main.connections
               (id, session_id, adapter_id, transport, connected_at, disconnected_at,
                firmware, capabilities)
            SELECT c.id, c.session_id, c.adapter_id, c.transport, c.connected_at,
                   c.disconnected_at, c.firmware, c.capabilities
            FROM src.connections c JOIN import_take t ON t.session_id = c.session_id
            WHERE true
            ON CONFLICT(id) DO UPDATE SET
                disconnected_at = excluded.disconnected_at,
                firmware        = excluded.firmware,
                capabilities    = excluded.capabilities;

        INSERT INTO main.modules
               (id, session_id, module_key, name, address, protocol, identity,
                software_version, discovered_at, request_address)
            SELECT m.id, m.session_id, m.module_key, m.name, m.address, m.protocol,
                   m.identity, m.software_version, m.discovered_at, m.request_address
            FROM src.modules m JOIN import_take t ON t.session_id = m.session_id
            WHERE true
            ON CONFLICT(id) DO UPDATE SET
                name             = excluded.name,
                address          = excluded.address,
                protocol         = excluded.protocol,
                identity         = excluded.identity,
                software_version = excluded.software_version,
                request_address  = excluded.request_address;

        -- The recording, exactly as recorded. Only the part past what is
        -- already here, in the order it was written.
        INSERT INTO main.session_events (session_id, seq, timestamp, kind, payload)
            SELECT e.session_id, e.seq, e.timestamp, e.kind, e.payload
            FROM src.session_events e JOIN import_take t ON t.session_id = e.session_id
            WHERE e.seq > t.local_max
            ORDER BY e.id;

        INSERT INTO main.measurements
               (session_id, module_id, timestamp, signal_id, value, text_value, unit, raw_value)
            SELECT x.session_id, x.module_id, x.timestamp, x.signal_id, x.value,
                   x.text_value, x.unit, x.raw_value
            FROM src.measurements x JOIN import_take t ON t.session_id = x.session_id
            WHERE NOT EXISTS (
                SELECT 1 FROM main.measurements h
                WHERE h.session_id = x.session_id AND h.signal_id = x.signal_id
                  AND h.timestamp = x.timestamp AND h.module_id = x.module_id
                  AND h.raw_value = x.raw_value)
            ORDER BY x.id;

        INSERT INTO main.agent_traces
               (session_id, message_id, role, content, tool_name, tool_args_ref,
                tool_result_ref, model, prompt_version, timestamp)
            SELECT a.session_id, a.message_id, a.role, a.content, a.tool_name,
                   {args_ref}, {result_ref}, a.model, a.prompt_version, a.timestamp
            FROM src.agent_traces a JOIN import_take t ON t.session_id = a.session_id
            WHERE NOT EXISTS (
                SELECT 1 FROM main.agent_traces h
                WHERE h.session_id = a.session_id AND h.message_id = a.message_id
                  AND h.role = a.role AND h.timestamp = a.timestamp)
            ORDER BY a.id;

        INSERT INTO main.dtcs
               (session_id, module_id, code, status, description, occurrence,
                freeze_frame_ref, read_at)
            SELECT d.session_id, d.module_id, d.code, d.status, d.description, d.occurrence,
                   {freeze_ref}, d.read_at
            FROM src.dtcs d JOIN import_take t ON t.session_id = d.session_id
            WHERE true
            ORDER BY d.id
            ON CONFLICT(session_id, module_id, code, status) DO UPDATE SET
                description      = excluded.description,
                occurrence       = excluded.occurrence,
                freeze_frame_ref = excluded.freeze_frame_ref,
                read_at          = excluded.read_at;

        INSERT OR IGNORE INTO main.test_runs
               (id, session_id, module_id, test_id, risk_level, requested_by,
                confirmed_by_user, started_at, ended_at, result, evidence_ref, detail)
            SELECT r.id, r.session_id, r.module_id, r.test_id, r.risk_level, r.requested_by,
                   r.confirmed_by_user, r.started_at, r.ended_at, r.result, {test_ref}, r.detail
            FROM src.test_runs r JOIN import_take t ON t.session_id = r.session_id;

        INSERT OR IGNORE INTO main.diagnoses
               (id, session_id, hypothesis, confidence, evidence_refs, alternatives,
                recommendation, unresolved_questions, created_at)
            SELECT g.id, g.session_id, g.hypothesis, g.confidence,
                   (SELECT json_group_array(l.id)
                    FROM json_each(g.evidence_refs) j
                    JOIN src.session_events s ON s.id = j.value
                    JOIN main.session_events l
                      ON l.session_id = s.session_id AND l.seq = s.seq),
                   g.alternatives, g.recommendation, g.unresolved_questions, g.created_at
            FROM src.diagnoses g JOIN import_take t ON t.session_id = g.session_id;
        ";
    tx.execute_batch(
        &sessions_and_events
            .replace("{args_ref}", &event_here("a.tool_args_ref"))
            .replace("{result_ref}", &event_here("a.tool_result_ref"))
            .replace("{freeze_ref}", &event_here("d.freeze_frame_ref"))
            .replace("{test_ref}", &event_here("r.evidence_ref")),
    )
    .map_err(storage)?;
    summary.events_added = count(&tx, "session_events")? - events_before;
    summary.measurements_added = count(&tx, "measurements")? - measurements_before;
    summary.dtcs_added = count(&tx, "dtcs")? - dtcs_before;

    summary.vehicles = {
        let mut stmt = tx
            .prepare(
                "SELECT DISTINCT v.vin FROM import_take t
                 JOIN main.sessions s ON s.id = t.session_id
                 LEFT JOIN main.vehicles v ON v.id = s.vehicle_id
                 ORDER BY v.vin",
            )
            .map_err(storage)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)?
    };

    // ---- what is kept against a vehicle, not a session --------------------
    // A capture belongs to a session and outlives it in use, so it comes for
    // every session this database now has, not only the ones just added.
    let before = count(&tx, "config_captures")?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO main.config_captures
                (id, session_id, vehicle_id, module_key, label, taken_at, capture)
             SELECT c.id, c.session_id,
                    COALESCE((SELECT v.local_id FROM import_vehicle v WHERE v.src_id = c.vehicle_id),
                             c.vehicle_id),
                    c.module_key, c.label, c.taken_at, c.capture
             FROM src.config_captures c
             WHERE EXISTS (SELECT 1 FROM main.sessions m WHERE m.id = c.session_id);",
    )
    .map_err(storage)?;
    summary.captures_added = count(&tx, "config_captures")? - before;

    // A finding about the same thing supersedes the older one, which is the
    // rule this table already keeps on one device. Across two, the newer is
    // whichever was observed later, wherever that was.
    let (newer, older): (i64, i64) = tx
        .query_row(
            "SELECT COALESCE(SUM(julianday(s.observed_at) >  julianday(m.observed_at)), 0),
                    COALESCE(SUM(julianday(s.observed_at) <= julianday(m.observed_at)), 0)
             FROM src.vehicle_knowledge s
             JOIN main.vehicle_knowledge m ON m.vin = s.vin AND m.subject = s.subject",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(storage)?;
    let before = count(&tx, "vehicle_knowledge")?;
    let records_before = count(&tx, "as_built")? + count(&tx, "vpic_replies")?;
    let records_newer: i64 = tx
        .query_row(
            "SELECT (SELECT COUNT(*) FROM src.as_built s JOIN main.as_built m ON m.vin = s.vin
                     WHERE julianday(s.imported_at) > julianday(m.imported_at))
                  + (SELECT COUNT(*) FROM src.vpic_replies s JOIN main.vpic_replies m ON m.vin = s.vin
                     WHERE julianday(s.fetched_at) > julianday(m.fetched_at))",
            [],
            |r| r.get(0),
        )
        .map_err(storage)?;
    tx.execute_batch(
        "INSERT INTO main.vehicle_knowledge
                (vin, subject, outcome, claim, evidence, authority, observed_at, session_id)
             SELECT vin, subject, outcome, claim, evidence, authority, observed_at, session_id
             FROM src.vehicle_knowledge
             WHERE true
             ON CONFLICT(vin, subject) DO UPDATE SET
                 outcome     = excluded.outcome,
                 claim       = excluded.claim,
                 evidence    = excluded.evidence,
                 authority   = excluded.authority,
                 observed_at = excluded.observed_at,
                 session_id  = excluded.session_id
             WHERE julianday(excluded.observed_at) > julianday(vehicle_knowledge.observed_at);

         INSERT INTO main.as_built (vin, imported_at, source, data)
             SELECT vin, imported_at, source, data FROM src.as_built
             WHERE true
             ON CONFLICT(vin) DO UPDATE SET
                 imported_at = excluded.imported_at,
                 source      = excluded.source,
                 data        = excluded.data
             WHERE julianday(excluded.imported_at) > julianday(as_built.imported_at);

         INSERT INTO main.vpic_replies (vin, fetched_at, source_url, body)
             SELECT vin, fetched_at, source_url, body FROM src.vpic_replies
             WHERE true
             ON CONFLICT(vin) DO UPDATE SET
                 fetched_at = excluded.fetched_at,
                 source_url = excluded.source_url,
                 body       = excluded.body
             WHERE julianday(excluded.fetched_at) > julianday(vpic_replies.fetched_at);

         DROP TABLE temp.import_vehicle;
         DROP TABLE temp.import_take;",
    )
    .map_err(storage)?;
    summary.findings_added = count(&tx, "vehicle_knowledge")? - before;
    summary.findings_updated = newer as u64;
    summary.findings_kept = older as u64;
    summary.vehicle_records_taken = count(&tx, "as_built")? + count(&tx, "vpic_replies")?
        - records_before
        + records_newer as u64;

    if dry_run {
        tx.rollback().map_err(storage)?;
    } else {
        tx.commit().map_err(storage)?;
        summary.committed = true;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migration that adds a column, or a table, has to say what an import
    /// does with it. Until it does, this fails.
    #[test]
    fn every_table_and_column_in_the_schema_is_accounted_for() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock().unwrap();

        let mut tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations' ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut carried: Vec<String> = CARRIED.iter().map(|(t, _)| t.to_string()).collect();
        tables.sort();
        carried.sort();
        assert_eq!(tables, carried, "a table is missing from the import, or is no longer real");

        for (table, columns) in CARRIED {
            let mut real: Vec<String> = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let mut listed: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
            real.sort();
            listed.sort();
            assert_eq!(real, listed, "{table}: the import's column list is out of date");
        }
    }
}
