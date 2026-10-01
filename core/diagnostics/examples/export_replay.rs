//! Turn a real session into a replay fixture for CI (#59).
//!
//! Works on a copy of the database, never the original: opening a store
//! migrates it, and the app may be writing to it. The copy is thrown away.
//!
//!     cargo run -p aim-diagnostics --example export_replay -- <sessions.sqlite>
//!         lists the sessions, newest first
//!
//!     cargo run -p aim-diagnostics --example export_replay -- <sessions.sqlite> <session id> <name>
//!         writes core/diagnostics/tests/replays/<name>.transcript
//!
//! The VIN is replaced by an anonymous one, and the file is not written at all
//! if any trace of the real one is left. The baseline is recorded separately:
//!
//!     cargo test -p aim-diagnostics --test replay_baselines -- --ignored record_missing_baselines

use aim_diagnostics::transcript::{anonymise, anonymous_vin, export_transcript};
use aim_session::SessionStore;
use aim_types::SessionId;
use std::path::{Path, PathBuf};
use std::process::exit;

const REPLAYS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/replays");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (database, export) = match args.as_slice() {
        [database] => (database, None),
        [database, session, name] => (database, Some((session, name))),
        _ => {
            eprintln!("usage: export_replay <sessions.sqlite> [<session id> <name>]");
            exit(2);
        }
    };

    let scratch = tempfile::tempdir().unwrap_or_else(|e| fail(format!("no temp dir: {e}")));
    let store = open_copy(Path::new(database), scratch.path());

    match export {
        None => list(&store),
        Some((session, name)) => write(&store, &SessionId::from_string(session.as_str()), name),
    }
}

/// Copy the database and its write-ahead log, which holds whatever the app
/// has not checkpointed yet, and open the copy.
fn open_copy(database: &Path, scratch: &Path) -> SessionStore {
    if !database.is_file() {
        fail(format!("no database at {}", database.display()));
    }
    let copy = scratch.join("sessions.sqlite");
    std::fs::copy(database, &copy)
        .unwrap_or_else(|e| fail(format!("cannot copy {}: {e}", database.display())));
    let wal = PathBuf::from(format!("{}-wal", database.display()));
    if wal.is_file() {
        std::fs::copy(&wal, scratch.join("sessions.sqlite-wal"))
            .unwrap_or_else(|e| fail(format!("cannot copy {}: {e}", wal.display())));
    }
    SessionStore::open(&copy).unwrap_or_else(|e| fail(format!("cannot open the copy: {e}")))
}

fn list(store: &SessionStore) {
    let sessions = store.list_sessions(200).unwrap_or_else(|e| fail(e.to_string()));
    println!(
        "{:<36} {:<19} {:<17} {:>7} {:>7}  label",
        "session", "started", "vehicle", "events", "modules"
    );
    for row in sessions {
        // Enough of the VIN to tell vehicles apart, not enough to identify one.
        let vehicle = match row.vehicle.and_then(|v| v.vin) {
            Some(vin) if vin.len() == 17 => format!("{}******", &vin[..11]),
            Some(vin) => vin,
            None => String::from("-"),
        };
        println!(
            "{:<36} {:<19} {:<17} {:>7} {:>7}  {}",
            row.session.id,
            row.session.started_at.to_rfc3339().get(..19).unwrap_or_default(),
            vehicle,
            row.event_count,
            row.module_count,
            row.session.label.unwrap_or_default()
        );
    }
}

fn write(store: &SessionStore, session_id: &SessionId, name: &str) {
    if name.is_empty()
        || name.starts_with("simulator-")
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        fail(format!(
            "name {name:?}: use letters, digits and dashes, e.g. 2019-f250-full-scan; \
             `simulator-` is kept for the simulator's own recordings"
        ));
    }

    let session = store.get_session(session_id).unwrap_or_else(|e| fail(e.to_string()));
    let vin = session
        .vehicle_id
        .as_ref()
        .and_then(|id| store.get_vehicle(id).ok().flatten())
        .and_then(|vehicle| vehicle.vin)
        .unwrap_or_else(|| {
            // Without the VIN there is no checking that it is gone, and a
            // replay that never settles a VIN has no scorecard to compare.
            fail(format!("session {session_id} has no VIN recorded, so it cannot be anonymised"))
        });
    let anonymous = anonymous_vin(&vin).unwrap_or_else(|e| fail(e.to_string()));

    let recorded = export_transcript(
        store,
        session_id,
        &format!(
            "{name}: a real session from {}, VIN anonymised, exported by export_replay",
            session.started_at.to_rfc3339().get(..10).unwrap_or("an unknown date")
        ),
    )
    .unwrap_or_else(|e| fail(e.to_string()));
    let text = anonymise(&recorded, &vin).unwrap_or_else(|e| fail(format!("not written: {e}")));

    let exchanges = text.lines().filter(|line| line.starts_with("> ")).count();
    if exchanges == 0 {
        fail(format!("session {session_id} recorded no adapter exchanges"));
    }

    let path = Path::new(REPLAYS).join(format!("{name}.transcript"));
    if path.exists() {
        fail(format!("{} already exists; remove it first to replace it", path.display()));
    }
    std::fs::write(&path, text).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    println!("wrote {} ({exchanges} exchanges, VIN now {anonymous})", path.display());
    println!(
        "next: cargo test -p aim-diagnostics --test replay_baselines -- --ignored record_missing_baselines"
    );
}

fn fail(message: String) -> ! {
    eprintln!("export_replay: {message}");
    exit(1);
}
