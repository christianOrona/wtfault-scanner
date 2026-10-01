//! The other end of "Send" on the app's problem report (#50).
//!
//! # What it is for
//!
//! The app can already assemble a report and let a person copy or save it.
//! Most problems are still never reported, because moving the text anywhere
//! is a second job. This is the place a Send button can post it to.
//!
//! # What it will not become
//!
//! It is an attack surface by existing, so it is kept dumb on purpose:
//!
//! - **A bounded body.** A report is text the person has read on screen.
//!   Anything larger than [`MAX_TEXT_BYTES`] is refused before it is parsed.
//! - **A rate limit per address**, and a ceiling per day across everybody, so
//!   a loop in somebody's client or a deliberate flood fills nothing.
//! - **Files, not a database.** Each report is one text file under a dated
//!   folder. There is nothing to migrate, nothing to inject into, and nothing
//!   to run it beside.
//! - **Nothing it receives is executed, interpreted or fetched.** The text is
//!   written as it arrived. No field names a path: the file name is made here.
//! - **It keeps no more than it needs**: the report, the version and platform
//!   the app says it is, and when it arrived. Not the address it came from,
//!   which is only held in memory for the rate limit.

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// The most report text accepted, in bytes. The app's report is a header and
/// the last few hundred log lines; this is several times that.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

/// The whole request body: the text, escaped as JSON, and a little around it.
///
/// Escaping grows text: every newline, backslash and quote takes two bytes, so
/// a log of Windows paths can nearly double. Four times the text is generous
/// for that and still bounded; the text itself is held to
/// [`MAX_TEXT_BYTES`] once parsed.
pub const MAX_BODY_BYTES: usize = 4 * MAX_TEXT_BYTES + 4 * 1024;

/// How long `version` and `platform` may be.
const MAX_LABEL_CHARS: usize = 64;

/// How the endpoint is run.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where reports are written, one file each, under a folder per day.
    pub dir: PathBuf,
    /// Reports accepted from one address per [`Config::window`].
    pub per_address: u32,
    /// The window [`Config::per_address`] counts over.
    pub window: Duration,
    /// Reports accepted from everybody per day, so the disk cannot be filled.
    pub per_day: u32,
    /// Take the client address from `X-Forwarded-For`. Only behind a proxy
    /// that sets it; anywhere else a client could name any address it liked.
    pub trust_forwarded_for: bool,
}

/// One report, as the app posts it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// The app's version.
    pub version: String,
    /// The operating system and architecture the app says it runs on.
    pub platform: String,
    /// The report, exactly as the person saw it.
    pub text: String,
}

/// What a sender is told.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    /// Whether it was kept.
    pub accepted: bool,
    /// A reference the person can quote, when it was kept.
    pub reference: Option<String>,
    /// Why not, when it was not.
    pub reason: Option<String>,
}

impl Receipt {
    fn refused(reason: impl Into<String>) -> Receipt {
        Receipt { accepted: false, reference: None, reason: Some(reason.into()) }
    }
}

#[derive(Default)]
struct Counters {
    by_address: HashMap<IpAddr, (Instant, u32)>,
    day: u64,
    today: u32,
    sequence: u64,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    counters: Arc<Mutex<Counters>>,
}

/// The routes, ready to serve.
pub fn router(config: Config) -> Router {
    let state =
        AppState { config: Arc::new(config), counters: Arc::new(Mutex::new(Counters::default())) };
    Router::new()
        .route("/v1/health", get(|| async { "ok" }))
        .route("/v1/reports", post(receive))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

fn unix_now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
}

/// `YYYY-MM-DD` for a number of days since the epoch, without a date library.
fn date_of(days: u64) -> String {
    // Howard Hinnant's civil-from-days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Printable labels only: these end up in a file someone reads.
fn label_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= MAX_LABEL_CHARS
        && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')
}

/// Who sent this, for the rate limit.
///
/// Behind a proxy, the last `X-Forwarded-For` entry: the one the proxy itself
/// appended. Every entry before it came from the client, which can write any
/// address it likes there and so dodge the limit with each request.
fn client_address(headers: &HeaderMap, peer: SocketAddr, trust_forwarded_for: bool) -> IpAddr {
    if trust_forwarded_for {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|last| last.trim().parse().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.ip()
}

async fn receive(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<Report>, axum::extract::rejection::JsonRejection>,
) -> (StatusCode, Json<Receipt>) {
    let report = match body {
        Ok(Json(report)) => report,
        // Over the body limit: refused before anything was parsed.
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(Receipt::refused(format!("a report is at most {MAX_TEXT_BYTES} bytes"))),
            )
        }
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(Receipt::refused(
                    "the body is not a report: expected version, platform and text",
                )),
            )
        }
    };
    if report.text.trim().is_empty() || report.text.len() > MAX_TEXT_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(Receipt::refused(format!(
                "the text must be between 1 and {MAX_TEXT_BYTES} bytes"
            ))),
        );
    }
    if !label_ok(&report.version) || !label_ok(&report.platform) {
        return (
            StatusCode::BAD_REQUEST,
            Json(Receipt::refused("version and platform must be short printable text")),
        );
    }

    let address = client_address(&headers, peer, state.config.trust_forwarded_for);
    let now = Instant::now();
    let today = unix_now().as_secs() / 86_400;
    let sequence = {
        let mut c = state.counters.lock().await;
        if c.day != today {
            c.day = today;
            c.today = 0;
        }
        if c.today >= state.config.per_day {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(Receipt::refused("too many reports today; please try again tomorrow")),
            );
        }
        let window = state.config.window;
        // Forget addresses whose window has passed, so the map stays small.
        c.by_address.retain(|_, (since, _)| now.duration_since(*since) < window);
        let entry = c.by_address.entry(address).or_insert((now, 0));
        if entry.1 >= state.config.per_address {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(Receipt::refused("too many reports from this address; please wait a while")),
            );
        }
        entry.1 += 1;
        c.today += 1;
        c.sequence += 1;
        c.sequence
    };

    let stamp = unix_now();
    let reference = format!("{}-{:03}-{sequence}", stamp.as_secs(), stamp.subsec_millis());
    let folder = state.config.dir.join(date_of(today));
    let contents = format!(
        "version  : {}\nplatform : {}\nreceived : {}\n\n{}",
        report.version,
        report.platform,
        stamp.as_secs(),
        report.text
    );
    let written = async {
        tokio::fs::create_dir_all(&folder).await?;
        tokio::fs::write(folder.join(format!("{reference}.txt")), contents).await
    }
    .await;

    match written {
        Ok(()) => {
            tracing::info!(reference, "report kept");
            (
                StatusCode::CREATED,
                Json(Receipt { accepted: true, reference: Some(reference), reason: None }),
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "could not write a report");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Receipt::refused("the report could not be kept; nothing was stored")),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_since_the_epoch_become_dates() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(20_727), "2026-10-01");
        assert_eq!(date_of(11_016), "2000-02-29");
    }

    #[test]
    fn labels_are_short_and_printable() {
        assert!(label_ok("0.5.0"));
        assert!(label_ok("windows x86_64"));
        assert!(!label_ok(""));
        assert!(!label_ok("line\nbreak"));
        assert!(!label_ok(&"x".repeat(65)));
    }
}
