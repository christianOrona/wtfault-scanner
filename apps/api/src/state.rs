//! Shared server state.
//!
//! # Why the service sits behind a blocking mutex
//!
//! There is exactly one adapter and one vehicle. Vehicle I/O is blocking by
//! nature — an ELM327 exchange is a write followed by a wait for a `>` prompt —
//! and two overlapping requests on one serial port would interleave into
//! garbage. So the [`DiagnosticService`] lives behind a `std::sync::Mutex` and
//! every handler that touches it goes through [`AppState::with_service`], which
//! hops onto a blocking thread first. The async runtime is never blocked, and
//! the vehicle only ever sees one conversation at a time.

use crate::error::{ApiError, ApiResult};
use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{AdapterPersonality, ScenarioId, SimulatedTransport};
use aim_tools::ToolRegistry;
use aim_types::{AimError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Which transport a connection should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportChoice {
    /// A real serial port. On Windows a paired Bluetooth ELM327 is an outgoing
    /// COM port, so this is also the Bluetooth path.
    Serial,
    /// The in-process virtual vehicle.
    Simulator,
}

/// Which emulated adapter the simulator should pretend to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalityChoice {
    /// A genuine ELM327 v1.5.
    Genuine,
    /// The cheap Bluetooth clone. The default, because it is the harsher test
    /// and it is what is actually plugged into the development truck.
    Clone,
}

impl PersonalityChoice {
    fn build(self) -> AdapterPersonality {
        match self {
            PersonalityChoice::Genuine => AdapterPersonality::genuine_v1_5(),
            PersonalityChoice::Clone => AdapterPersonality::cheap_clone_v2_1(),
        }
    }
}

/// How the server was launched. Supplies the defaults a connect request omits.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Default transport for a connect request that does not name one.
    pub default_transport: TransportChoice,
    /// Default serial port.
    pub default_port: Option<String>,
    /// Default simulator scenario.
    pub default_scenario: ScenarioId,
    /// Emulated adapter personality for simulator connections.
    pub personality: PersonalityChoice,
    /// Address the server is bound to.
    pub bind: String,
    /// Simulated per-command latency, so live data moves at a believable rate.
    pub simulator_latency: Duration,
    /// Where model-provider settings are stored.
    pub settings_path: std::path::PathBuf,
}

/// What a connect request asks for.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectRequest {
    /// Transport to use. Defaults to how the server was launched.
    pub transport: Option<TransportChoice>,
    /// Serial port name, e.g. `COM5` or `/dev/rfcomm0`.
    pub port: Option<String>,
    /// Simulator scenario name, e.g. `dpf-regen`.
    pub scenario: Option<String>,
    /// Label recorded on the session.
    pub label: Option<String>,
}

/// Everything the handlers share.
#[derive(Clone)]
pub struct AppState {
    /// The active diagnostic service, or `None` when nothing is connected.
    pub service: Arc<Mutex<Option<DiagnosticService>>>,
    /// Session history. Available whether or not an adapter is connected.
    pub store: SessionStore,
    /// Decoder set, shared by every session.
    pub decoders: Arc<DecoderSet>,
    /// Tool schemas.
    pub tools: Arc<ToolRegistry>,
    /// Launch configuration.
    pub config: Arc<ServerConfig>,
    /// Model-provider settings. Kept out of the session database on purpose:
    /// that file holds VINs and vehicle history, and an API key has no business
    /// travelling with it.
    pub settings: Arc<aim_agent::SettingsStore>,
}

impl AppState {
    /// Build the shared state.
    pub fn new(store: SessionStore, decoders: DecoderSet, config: ServerConfig) -> Self {
        AppState {
            service: Arc::new(Mutex::new(None)),
            store,
            decoders: Arc::new(decoders),
            tools: Arc::new(ToolRegistry::phase1()),
            settings: Arc::new(aim_agent::SettingsStore::new(config.settings_path.clone())),
            config: Arc::new(config),
        }
    }

    /// Run `f` against the active service on a blocking thread.
    ///
    /// Fails with [`ErrorCode::NoActiveSession`] when nothing is connected,
    /// which is a distinct, actionable answer rather than an empty result.
    pub async fn with_service<F, T>(&self, f: F) -> ApiResult<T>
    where
        F: FnOnce(&mut DiagnosticService) -> T + Send + 'static,
        T: Send + 'static,
    {
        let service = Arc::clone(&self.service);
        tokio::task::spawn_blocking(move || {
            let mut guard = service
                .lock()
                .map_err(|_| ApiError::internal("diagnostic service lock was poisoned"))?;
            match guard.as_mut() {
                Some(s) => Ok(f(s)),
                None => Err(ApiError::no_session()),
            }
        })
        .await
        .map_err(|e| ApiError::internal(format!("diagnostic task failed: {e}")))?
    }

    /// Read something from the active service without requiring one.
    pub async fn peek_service<F, T>(&self, f: F) -> ApiResult<Option<T>>
    where
        F: FnOnce(&DiagnosticService) -> T + Send + 'static,
        T: Send + 'static,
    {
        let service = Arc::clone(&self.service);
        tokio::task::spawn_blocking(move || {
            let guard = service
                .lock()
                .map_err(|_| ApiError::internal("diagnostic service lock was poisoned"))?;
            Ok(guard.as_ref().map(f))
        })
        .await
        .map_err(|e| ApiError::internal(format!("diagnostic task failed: {e}")))?
    }

    /// Open an adapter, start a session and connect.
    ///
    /// Refuses when something is already connected: silently dropping a live
    /// session would orphan its flight recorder mid-scan.
    pub async fn connect(&self, request: ConnectRequest) -> ApiResult<aim_types::ToolResult> {
        let already = self.peek_service(|s| s.state().is_usable()).await?.unwrap_or(false);
        if already {
            return Err(ApiError::new(AimError::new(
                ErrorCode::AdapterBusy,
                "an adapter is already connected; disconnect before connecting again",
            )));
        }

        let config = Arc::clone(&self.config);
        let transport_choice = request.transport.unwrap_or(config.default_transport);
        let scenario = match &request.scenario {
            Some(name) => ScenarioId::parse(name).ok_or_else(|| {
                ApiError::bad_request(format!(
                    "unknown scenario {name:?}; known scenarios are {:?}",
                    ScenarioId::all().map(|s| s.as_str())
                ))
            })?,
            None => config.default_scenario,
        };
        let port = request.port.clone().or_else(|| config.default_port.clone());

        let mut adapter = build_adapter(transport_choice, port.clone(), scenario, &config)?;

        // A protocol that answered through this adapter before is tried first.
        // Only a reordering: if it does not answer, the full sweep runs exactly
        // as it would have. Worth doing because a failing sweep measured eleven
        // seconds on a real vehicle, and a succeeding one is not free either.
        if let Some(p) = port.as_deref() {
            if let Ok(Some(previous)) = self.store.last_protocol_for_adapter(p) {
                adapter.prefer_protocol(Some(previous));
            }
        }

        let store = self.store.clone();
        let decoders = Arc::clone(&self.decoders);
        let label = request.label.clone();

        let service_slot = Arc::clone(&self.service);
        tokio::task::spawn_blocking(move || {
            let mut service =
                DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), label)
                    .map_err(ApiError::new)?;
            let result = service.connect("user:api");
            let mut guard = service_slot
                .lock()
                .map_err(|_| ApiError::internal("diagnostic service lock was poisoned"))?;
            // The service is kept even when connecting failed: its session
            // holds the flight recorder trace of exactly how it failed, which
            // is the most useful thing to have after a failed connection.
            *guard = Some(service);
            Ok(result)
        })
        .await
        .map_err(|e| ApiError::internal(format!("connect task failed: {e}")))?
    }

    /// Disconnect and end the session.
    pub async fn disconnect(&self) -> ApiResult<aim_types::ToolResult> {
        let result = self.with_service(|s| s.disconnect("user:api")).await?;
        let _ = self.with_service(|s| s.end_session()).await;
        let service = Arc::clone(&self.service);
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(mut guard) = service.lock() {
                *guard = None;
            }
        })
        .await;
        Ok(result)
    }
}

fn build_adapter(
    choice: TransportChoice,
    port: Option<String>,
    scenario: ScenarioId,
    config: &ServerConfig,
) -> ApiResult<Box<dyn DiagnosticAdapter>> {
    match choice {
        TransportChoice::Simulator => {
            let transport =
                SimulatedTransport::with_personality(scenario, config.personality.build())
                    .with_latency(config.simulator_latency);
            Ok(Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast())))
        }
        TransportChoice::Serial => {
            let port = port.ok_or_else(|| {
                ApiError::bad_request(
                    "a serial connection needs a port name; GET /api/v1/adapters/ports lists them",
                )
            })?;
            build_serial_adapter(&port)
        }
    }
}

/// Open a serial adapter, finding its line speed first.
///
/// This used to hardcode the Bluetooth defaults for every serial port. That is
/// invisible over Bluetooth — a virtual COM port ignores baud — but a wired USB
/// cable set to a different speed opens cleanly, accepts every write, and never
/// answers, which surfaces as `adapter_init_failed: ATZ was answered with
/// timeout` and looks exactly like a broken adapter.
#[cfg(feature = "serial")]
fn build_serial_adapter(port: &str) -> ApiResult<Box<dyn DiagnosticAdapter>> {
    use aim_adapter::probe::find_baud;
    use aim_transport::serial::{SerialConfig, SerialTransport};
    use std::time::Duration;

    let kind = aim_transport::list_ports()
        .into_iter()
        .find(|p| p.name == port)
        .map(|p| p.kind.transport_kind())
        .unwrap_or(aim_types::TransportKind::Usb);

    let mut config = SerialConfig::for_port(port, kind);
    // Bluetooth ports ignore baud, so there is nothing to find. For a wired
    // cable, a speed that answers is worth the few seconds it takes to find; if
    // none does, the configured default stands so the failure the caller sees
    // is the adapter's own rather than a guess of ours.
    if kind != aim_types::TransportKind::Bluetooth {
        if let Ok((_, Some(baud))) = find_baud(port, Duration::from_millis(1_200)) {
            tracing::info!(port, baud, "serial adapter answered at this line speed");
            config = config.with_baud(baud);
        }
    }

    let transport = SerialTransport::new(config);
    Ok(Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::default())))
}

#[cfg(not(feature = "serial"))]
fn build_serial_adapter(port: &str) -> ApiResult<Box<dyn DiagnosticAdapter>> {
    Err(ApiError::new(AimError::new(
        ErrorCode::TransportUnsupported,
        format!("this build has no serial support compiled in; cannot open {port}"),
    )))
}
