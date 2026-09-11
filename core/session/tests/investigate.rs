//! A forensic query over a real session database.
//!
//! Ignored by default: it needs a database that only exists on a machine that
//! has been plugged into a vehicle, and it prints rather than asserts. Run it
//! with the path to one:
//!
//! ```text
//! AIM_INVESTIGATE_DB=.../sessions.sqlite \
//!   cargo test -p aim-session --test investigate -- --ignored --nocapture
//! ```
//!
//! It exists because "611 stopped responses" is a number nobody can act on.
//! What makes it actionable is which commands produced them and how long they
//! took, and that is a query rather than a guess. Kept in the tree so the next
//! unexplained number is one command away from an answer instead of an
//! afternoon of writing this again.

use rusqlite::Connection;

fn tally(conn: &Connection, title: &str, sql: &str) {
    println!("\n--- {title} ---");
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            println!("  (query failed: {e})");
            return;
        }
    };
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))
        .and_then(|m| m.collect::<Result<Vec<_>, _>>());
    match rows {
        Ok(rows) if rows.is_empty() => println!("  (nothing)"),
        Ok(rows) => {
            for (label, n) in rows {
                println!("  {:<34} {n}", label.unwrap_or_else(|| "(none)".into()));
            }
        }
        Err(e) => println!("  (failed: {e})"),
    }
}

#[test]
#[ignore = "needs a real session database; set AIM_INVESTIGATE_DB"]
fn what_produced_the_adapter_failures() {
    let Ok(path) = std::env::var("AIM_INVESTIGATE_DB") else { return };
    let conn = Connection::open(&path).expect("open the database");

    let total: i64 =
        conn.query_row("SELECT COUNT(*) FROM session_events", [], |r| r.get(0)).unwrap();
    let sessions: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
    println!("\n{total} events across {sessions} sessions\n{path}");

    tally(
        &conn,
        "response classes",
        "SELECT json_extract(payload, '$.classification'), COUNT(*) n
         FROM session_events WHERE kind = 'adapter_response'
         GROUP BY 1 ORDER BY n DESC",
    );

    // The question the issue asks. The response event carries the command it
    // answers, so no correlation with a neighbouring row is needed — which
    // matters, because a scan interleaves requests and a neighbour is not
    // reliably the cause.
    tally(
        &conn,
        "commands that produced `stopped`",
        "SELECT json_extract(payload, '$.command'), COUNT(*) n
         FROM session_events
         WHERE kind = 'adapter_response'
           AND json_extract(payload, '$.classification') = 'stopped'
         GROUP BY 1 ORDER BY n DESC LIMIT 25",
    );

    tally(
        &conn,
        "how long a `stopped` took, bucketed",
        "SELECT CASE
                  WHEN json_extract(payload, '$.elapsed_ms') < 50 THEN 'under 50ms'
                  WHEN json_extract(payload, '$.elapsed_ms') < 250 THEN '50-250ms'
                  WHEN json_extract(payload, '$.elapsed_ms') < 1000 THEN '250ms-1s'
                  WHEN json_extract(payload, '$.elapsed_ms') < 5000 THEN '1-5s'
                  ELSE 'over 5s' END,
                COUNT(*) n
         FROM session_events
         WHERE kind = 'adapter_response'
           AND json_extract(payload, '$.classification') = 'stopped'
         GROUP BY 1 ORDER BY n DESC",
    );

    tally(
        &conn,
        "sessions the `stopped` responses fall in",
        "SELECT session_id, COUNT(*) n
         FROM session_events
         WHERE kind = 'adapter_response'
           AND json_extract(payload, '$.classification') = 'stopped'
         GROUP BY 1 ORDER BY n DESC LIMIT 10",
    );

    tally(
        &conn,
        "commands that timed out, for comparison",
        "SELECT json_extract(payload, '$.command'), COUNT(*) n
         FROM session_events
         WHERE kind = 'adapter_response'
           AND json_extract(payload, '$.classification') = 'timeout'
         GROUP BY 1 ORDER BY n DESC LIMIT 15",
    );
    println!();
}

#[test]
#[ignore = "needs a real session database; set AIM_INVESTIGATE_DB"]
fn what_the_stopped_recovery_costs() {
    let Ok(path) = std::env::var("AIM_INVESTIGATE_DB") else { return };
    let conn = Connection::open(&path).expect("open the database");

    // `STOPPED` already triggers a warm start and a retry, so the 611 are not
    // lost addresses. The question is what that recovery costs: `ATWS` is
    // followed by re-applying every setting it cleared, so one interrupted
    // command turns into several round trips.
    tally(
        &conn,
        "commands sent in the session that produced every `stopped`",
        "SELECT json_extract(payload, '$.command'), COUNT(*) n
         FROM session_events
         WHERE kind = 'adapter_request'
           AND session_id = (
               SELECT session_id FROM session_events
               WHERE kind = 'adapter_response'
                 AND json_extract(payload, '$.classification') = 'stopped'
               GROUP BY session_id ORDER BY COUNT(*) DESC LIMIT 1)
         GROUP BY 1 ORDER BY n DESC LIMIT 12",
    );

    let (events, ms): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(json_extract(payload, '$.elapsed_ms')), 0)
             FROM session_events
             WHERE kind = 'adapter_response'
               AND session_id = (
                   SELECT session_id FROM session_events
                   WHERE kind = 'adapter_response'
                     AND json_extract(payload, '$.classification') = 'stopped'
                   GROUP BY session_id ORDER BY COUNT(*) DESC LIMIT 1)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    println!(
        "\n  that session: {events} responses, {:.1}s of adapter time total",
        ms as f64 / 1000.0
    );
}

#[test]
#[ignore = "needs a real session database; set AIM_INVESTIGATE_DB"]
fn when_the_stopped_session_happened() {
    let Ok(path) = std::env::var("AIM_INVESTIGATE_DB") else { return };
    let conn = Connection::open(&path).expect("open the database");
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.started_at,
                    (SELECT COUNT(*) FROM session_events e
                     WHERE e.session_id = s.id AND e.kind = 'adapter_request'
                       AND json_extract(e.payload, '$.command') = 'ATWS') AS warm_starts,
                    (SELECT COUNT(*) FROM session_events e
                     WHERE e.session_id = s.id AND e.kind = 'adapter_response'
                       AND json_extract(e.payload, '$.classification') = 'stopped') AS stopped
             FROM sessions s ORDER BY s.started_at DESC",
        )
        .unwrap();
    println!("\n--- sessions: warm starts vs stopped ---");
    for row in stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
    {
        let (id, started, warm, stopped) = row.unwrap();
        if stopped > 0 || warm > 0 {
            println!("  {started}  {}  ATWS={warm:<5} stopped={stopped}", &id[..12]);
        }
    }
    println!();
}
