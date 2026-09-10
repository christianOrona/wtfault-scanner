//! `aim-api` — the localhost diagnostic API.
//!
//! Binds to 127.0.0.1 only. This process can talk to a vehicle, so it is not
//! something to expose on a network: there is no authentication because there
//! is no remote caller, and that is a deliberate pairing rather than an
//! omission. The Tauri shell and any future mobile client reach it through the
//! same `/api/v1` contract documented in `docs/API.md`.
//!
//! # Running against the simulator
//!
//! ```text
//! cargo run -p aim-api -- --simulator --scenario dpf-regen
//! ```
//!
//! # Running against a real adapter
//!
//! ```text
//! cargo run -p aim-api -- --serial COM5
//! ```

#![warn(missing_docs)]

use aim_api::routes;
use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use clap::Parser;
use std::net::SocketAddr;
use std::time::Duration;

/// Command line.
#[derive(Debug, Parser)]
#[command(name = "aim-api", about = "AI Mechanic localhost diagnostic API", version)]
struct Args {
    /// Use the built-in virtual vehicle instead of real hardware.
    #[arg(long, conflicts_with = "serial")]
    simulator: bool,

    /// Serial port of a real adapter, e.g. COM5 or /dev/rfcomm0.
    #[arg(long, value_name = "PORT")]
    serial: Option<String>,

    /// Simulator scenario: healthy, dpf-regen, bus-silent.
    #[arg(long, default_value = "healthy")]
    scenario: String,

    /// Emulated adapter personality for simulator runs: clone or genuine.
    #[arg(long, default_value = "clone")]
    personality: String,

    /// Milliseconds of simulated latency per adapter command.
    #[arg(long, default_value_t = 8)]
    simulator_latency_ms: u64,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 8787)]
    port: u16,

    /// SQLite session database. Omit for an in-memory database that is
    /// discarded when the process exits.
    #[arg(long, value_name = "PATH")]
    db: Option<String>,

    /// Where model-provider settings live. Defaults to the user data directory.
    #[arg(long, value_name = "PATH")]
    settings: Option<String>,

    /// Vehicle profiles and community signal definitions to load.
    ///
    /// The desktop app reads these from the user data directory. This server
    /// did not read them at all, which made it silently less capable than the
    /// app it serves — and made the difference invisible while testing.
    #[arg(long, value_name = "PATH")]
    profiles: Option<String>,

    /// List the simulator scenarios and exit.
    #[arg(long)]
    list_scenarios: bool,
}

/// Model-provider settings live with the user, not in the repo: they hold API
/// keys, and a working directory is the wrong place for those.
fn default_settings_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("ai-mechanic")
        .join("providers.json")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,aim_=debug")),
        )
        .init();

    if args.list_scenarios {
        for id in ScenarioId::all() {
            let scenario = aim_simulator::Scenario::new(id);
            println!("{:<12} {}", id.as_str(), scenario.description);
        }
        return Ok(());
    }

    let scenario = ScenarioId::parse(&args.scenario).ok_or_else(|| {
        format!(
            "unknown scenario {:?}; known scenarios are {:?}",
            args.scenario,
            ScenarioId::all().map(|s| s.as_str())
        )
    })?;
    let personality = match args.personality.to_ascii_lowercase().as_str() {
        "clone" | "cheap" | "cheap-clone" => PersonalityChoice::Clone,
        "genuine" | "real" => PersonalityChoice::Genuine,
        other => return Err(format!("unknown adapter personality {other:?}").into()),
    };

    // Neither flag given means the simulator, because that is the only thing
    // guaranteed to work on any machine. Choosing hardware is explicit.
    let default_transport =
        if args.serial.is_some() { TransportChoice::Serial } else { TransportChoice::Simulator };

    let store = match &args.db {
        Some(path) => SessionStore::open(path)?,
        None => SessionStore::open_in_memory()?,
    };
    let decoders = match &args.profiles {
        Some(dir) => DecoderSet::with_profiles(std::path::Path::new(dir))?,
        None => DecoderSet::generic_obd()?,
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let config = ServerConfig {
        default_transport,
        default_port: args.serial.clone(),
        default_scenario: scenario,
        personality,
        bind: addr.to_string(),
        simulator_latency: Duration::from_millis(args.simulator_latency_ms),
        settings_path: args
            .settings
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(default_settings_path),
    };

    tracing::info!(
        pids = decoders.pids.len(),
        dtcs = decoders.dtcs.len(),
        "loaded generic OBD-II decoders"
    );
    tracing::info!(
        transport = ?default_transport,
        scenario = scenario.as_str(),
        database = args.db.as_deref().unwrap_or(":memory:"),
        "starting AI Mechanic API"
    );

    let app = routes::router(AppState::new(store, decoders, config));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    println!("AI Mechanic API listening on http://{addr}");
    println!("  health:  curl http://{addr}/api/v1/health");
    println!("  connect: curl -X POST http://{addr}/api/v1/adapter/connect");

    axum::serve(listener, app).await?;
    Ok(())
}
