//! The desktop shell.
//!
//! The shell hosts the diagnostic core **in-process** rather than launching
//! `aim-api` as a sidecar. That is a deliberate choice: a sidecar for a process
//! that holds an open serial port is a process that can be orphaned, and an
//! orphaned one keeps the COM port locked until it is killed by hand — which on
//! a garage laptop looks exactly like a broken adapter. One process cannot leak
//! one.
//!
//! The UI stays a plain HTTP client of the same `/api/v1` contract in
//! `docs/API.md`, so nothing here is a private back door: whatever the desktop
//! can do, `curl` can do.
//!
//! # Why the shell also serves the UI
//!
//! The window loads the interface from the diagnostic core's own port, so the
//! page and the API share an origin. The alternative — a page on one origin
//! calling an API on another — needs CORS headers, and a CORS-permissive
//! loopback server that can talk to a vehicle is one any web page you happen to
//! visit could read your truck through. Same-origin removes the question
//! instead of answering it badly.

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::time::Duration;

use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// The built UI, embedded so the release binary is self-contained.
static UI: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../dist");

/// Preferred port. Documented in `docs/API.md`, so try it first and only fall
/// back when something already holds it.
const PREFERRED_PORT: u16 = 8787;

/// Where sessions are kept. Unlike `cargo run -p aim-api`, the desktop app
/// persists by default: someone scanning a truck in a driveway expects the
/// history to still be there tomorrow.
/// The one directory this application keeps anything in.
///
/// `None` when it cannot be created, which every caller treats as "carry on
/// without that": a diagnostic tool that will not open because it could not
/// make a folder is worse than one running without a log.
fn data_dir() -> Option<std::path::PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "ai-mechanic")?;
    let dir = dirs.data_dir().to_path_buf();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, path = %dir.display(), "cannot create the data directory");
        return None;
    }
    Some(dir)
}

fn database_path() -> Option<std::path::PathBuf> {
    Some(data_dir()?.join("sessions.sqlite"))
}

/// Where user-supplied vehicle profiles live.
///
/// Sits beside the session database so there is one place a person has to know
/// about. Returned even when it does not exist yet — the loader creates it with
/// its README, which is what makes the mechanism discoverable by someone poking
/// around the filesystem rather than only by someone reading documentation.
fn profile_dir() -> Option<std::path::PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "ai-mechanic")?;
    Some(dirs.data_dir().join("profiles"))
}

/// Start logging to the console and, when there is somewhere to put it, to a
/// file on disk.
///
/// A release build is a Windows GUI binary with no console attached, so until
/// there was a file this application's log went nowhere at all — a build that
/// ran for eleven minutes and froze left a hash in the event log and nothing
/// else.
///
/// The file writer is deliberately not the buffered one. Buffering is faster
/// and loses whatever had not been flushed when the process dies, which is
/// exactly the part worth reading.
fn start_logging(log_dir: Option<&std::path::Path>) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,aim_=debug"));

    let file = log_dir.and_then(|dir| {
        tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("wtfault")
            .filename_suffix("log")
            // A week is long enough to cover "it did it again last Tuesday"
            // without turning into something a person has to clean up.
            .max_log_files(7)
            .build(dir)
            .map_err(|e| tracing::warn!(error = %e, "cannot open a log file; logging to the console only"))
            .ok()
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(
            file.map(|f| {
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(f)
            }),
        )
        .init();

    // A panic already prints to a stderr nobody can see. This puts it in the
    // file as well, which is the difference between "it closed by itself" and
    // a line naming the source file it happened in.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "the application panicked");
        default(info);
    }));
}

/// Bind the preferred port, or let the OS choose one if it is taken.
///
/// Returning the real address matters: the UI is told where the core actually
/// is instead of assuming, so a second instance or an unrelated program on 8787
/// degrades into "a different port" rather than "the app is broken".
/// Serve the embedded UI for anything the API router did not claim.
///
/// Unknown paths fall back to `index.html` so the single-page app keeps working
/// on a reload. `/api/` is deliberately excluded: a mistyped endpoint must stay
/// a 404 from the API rather than quietly becoming a page, or a broken client
/// would look like a working one.
async fn serve_ui(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');

    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "no such endpoint").into_response();
    }

    let file = UI
        .get_file(path)
        .or_else(|| UI.get_file("index.html"));

    match file {
        Some(f) => {
            let mime = mime_guess::from_path(f.path()).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], Body::from(f.contents())).into_response()
        }
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the UI was not built into this binary; run `npm run build` and rebuild",
        )
            .into_response(),
    }
}

fn bind_loopback() -> std::io::Result<StdTcpListener> {
    match StdTcpListener::bind(SocketAddr::from(([127, 0, 0, 1], PREFERRED_PORT))) {
        Ok(l) => Ok(l),
        Err(e) => {
            tracing::warn!(error = %e, port = PREFERRED_PORT, "preferred port is taken; asking the OS for one");
            StdTcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        }
    }
}

/// Start the shell.
///
/// # Panics
///
/// Panics if the diagnostic core cannot be started at all — no loopback socket,
/// no session store, or no decoder definitions. There is no useful degraded
/// mode: a diagnostic tool that cannot open its own database would have to
/// either invent data or show an empty window, and both are worse than a clear
/// failure at startup.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let support = data_dir().and_then(|dir| aim_api::support::init(&dir));
    start_logging(support.map(|s| s.log_dir.as_path()));

    // What the last run did, before this one overwrites the record of it. A run
    // that froze or was killed never removed its marker, and this is the only
    // place that failure can be noticed — by definition it could not report
    // itself while it was happening.
    if let Some(previous) = aim_api::support::begin_run() {
        tracing::warn!(
            previous_version = %previous.version,
            previous_started = %previous.started,
            "starting after a run that did not end cleanly"
        );
    }

    let listener = bind_loopback().expect("cannot bind a loopback socket for the diagnostic core");
    listener
        .set_nonblocking(true)
        .expect("cannot set the listener non-blocking");
    let addr = listener
        .local_addr()
        .expect("the bound listener has no address");
    let origin = format!("http://{addr}");

    let db = database_path();
    let store = match &db {
        Some(path) => SessionStore::open(path).expect("cannot open the session database"),
        None => SessionStore::open_in_memory().expect("cannot open an in-memory session database"),
    };
    // User profiles are merged over the shipped data every start, so a mapping
    // somebody measured on their own vehicle takes effect without a rebuild.
    let decoders = match profile_dir() {
        Some(dir) => DecoderSet::with_profiles(&dir),
        None => DecoderSet::generic_obd(),
    }
    .expect("cannot load the OBD-II decoder definitions");

    let config = ServerConfig {
        // The desktop app is for a real truck; the simulator stays one click
        // away in the connect dialog rather than being the default.
        default_transport: TransportChoice::Serial,
        default_port: None,
        default_scenario: ScenarioId::DpfRegen,
        personality: PersonalityChoice::Clone,
        bind: addr.to_string(),
        simulator_latency: Duration::from_millis(8),
        // Beside the session database, in the user's own profile. Model
        // provider settings hold API keys, so they belong with the user rather
        // than next to the binary.
        settings_path: db
            .as_ref()
            .and_then(|p| p.parent())
            .map(|dir| dir.join("providers.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("providers.json")),
        profiles_dir: profile_dir(),
    };

    let state = AppState::new(store, decoders, config);

    tracing::info!(
        %addr,
        database = db.as_ref().map_or("(memory)".into(), |p| p.display().to_string()),
        "starting the diagnostic core in-process"
    );

    // Debug builds load from the Vite dev server, which proxies /api back here,
    // so the page is same-origin there too and hot reload keeps working.
    // Release builds load from this server, which serves the embedded UI.
    let window_url = if cfg!(debug_assertions) {
        WebviewUrl::default()
    } else {
        WebviewUrl::External(
            origin
                .parse()
                .expect("the loopback origin is not a valid URL"),
        )
    };

    tauri::Builder::default()
        .setup(move |app| {
            let router = aim_api::routes::router(state.clone()).fallback(serve_ui);
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .expect("cannot adopt the loopback listener into the async runtime");
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!(error = %e, "the diagnostic core stopped");
                }
            });

            WebviewWindowBuilder::new(app, "main", window_url)
                .title("WTFault Scanner")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                .build()?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the desktop shell failed to start")
        .run(|_app, event| {
            // The core lives in this process, so it goes when the window does,
            // and the open serial port goes with it. The one thing that has to
            // happen on the way out is saying so: a run that leaves its marker
            // behind is read as a crash by the next launch.
            if let tauri::RunEvent::Exit = event {
                tracing::info!("shutting down");
                aim_api::support::end_run();
            }
        });
}
