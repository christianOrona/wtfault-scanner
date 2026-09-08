//! Open a session database the way the app does and report what happens.
//!
//! Written because a real database reported "database disk image is malformed"
//! in the app while `PRAGMA integrity_check` on the same bytes said `ok`. When
//! those two disagree, the fault is in how the file is opened rather than in
//! the file, and this reproduces the open rather than the check.
//!
//!     cargo run -p aim-session --example db_doctor -- "C:\path\to\sessions.sqlite"

use aim_session::SessionStore;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: db_doctor <path to sessions.sqlite>");
            std::process::exit(2);
        }
    };

    println!("opening {path}");

    let store = match SessionStore::open(&path) {
        Ok(s) => {
            println!("  open: ok");
            s
        }
        Err(e) => {
            println!("  open: FAILED — {e}");
            std::process::exit(1);
        }
    };

    // The pragmas the app relies on, read back rather than assumed.
    for pragma in ["journal_mode", "foreign_keys", "user_version", "page_size"] {
        match store.pragma_string(pragma) {
            Ok(v) => println!("  {pragma} = {v}"),
            Err(e) => println!("  {pragma}: FAILED — {e}"),
        }
    }

    match store.integrity_check() {
        Ok(v) => println!("  integrity_check = {v}"),
        Err(e) => println!("  integrity_check: FAILED — {e}"),
    }

    // The query the Sessions pane actually runs.
    match store.list_sessions(50) {
        Ok(rows) => println!("  list_sessions(50): ok, {} rows", rows.len()),
        Err(e) => println!("  list_sessions(50): FAILED — {e}"),
    }
}
