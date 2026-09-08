//! Read back what actually happened in a recorded session.
//!
//!     cargo run -p aim-session --example dump_session -- [session-id]
//!
//! With no argument it dumps the most recent session. The flight recorder is
//! this product's claim to honesty; being able to read it back outside the app
//! is what makes that claim checkable rather than merely asserted.

fn main() {
    let base = std::env::var("APPDATA").unwrap_or_default();
    let path = match std::env::var("AIM_DB") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => std::path::Path::new(&base).join("ai-mechanic").join("data").join("sessions.sqlite"),
    };
    let store = match aim_session::SessionStore::open(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open {}: {}", path.display(), e.message);
            std::process::exit(1);
        }
    };

    let sessions = store.list_sessions(8).expect("list sessions");
    println!("--- recent sessions ---");
    for s in &sessions {
        println!(
            "  {}  started={}  events={}  label={:?}",
            s.session.id.0, s.session.started_at, s.event_count, s.session.label
        );
    }

    let Some(target) = std::env::args()
        .nth(1)
        .or_else(|| sessions.first().map(|s| s.session.id.0.clone()))
    else {
        println!("no sessions recorded");
        return;
    };
    let id = aim_types::SessionId(target.clone());
    println!("\n================ session {target} ================");

    println!("\n--- modules ---");
    for m in store.modules(&id).unwrap_or_default() {
        println!("  {} {} {:?}", m.module_key, m.address, m.name);
    }

    println!("\n--- readings ---");
    for m in store.measurements(&id, None, 300).unwrap_or_default() {
        println!(
            "  {} {:<34} value={:?} text={:?} unit={:?} raw={:?}",
            m.timestamp, m.signal_id, m.value, m.text_value, m.unit, m.raw_value
        );
    }

    println!("\n--- events ---");
    let events = store.events_since(&id, 0, 4000).unwrap_or_default();
    for e in &events {
        println!("  #{:<5} {}", e.seq, format!("{:?}", e.kind).replace('\n', " "));
    }
    println!("\n{} events total", events.len());
}
