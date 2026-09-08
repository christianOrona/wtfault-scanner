//! HTTP routes for `/api/v1`.
//!
//! # Two answer shapes, on purpose
//!
//! **Tool endpoints** (anything that talks to the vehicle) answer `200 OK` with
//! a [`ToolResult`] envelope whenever the operation *ran*, whether or not it
//! succeeded. A truck answering `NO DATA` is a real diagnostic outcome with
//! evidence and warnings attached, not an HTTP failure, and the UI renders it
//! the same way it renders a success. Callers check `result.success`.
//!
//! **Structural failures** — nothing connected, no such session, a malformed
//! request — answer with a real status code and the error envelope from
//! [`crate::error`]. Those are wrong requests, not diagnostic findings.
//!
//! That split is what lets the UI have exactly one code path for "the vehicle
//! said something" and one for "I asked wrong".

use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, ConnectRequest};
use crate::ws;
use aim_types::{SessionId, ToolResult};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
// Only the serial port probe needs this, and that whole path is feature-gated.
#[cfg(feature = "serial")]
use std::time::Duration;

/// Build the versioned router.
pub fn router(state: AppState) -> Router {
    Router::new()
        // ---- server ----
        .route("/api/v1/health", get(health))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/explanations", get(explanations))
        .route("/api/v1/profiles", get(profiles))
        .route("/api/v1/features", get(features))
        .route("/api/v1/features/{id}/preview", post(preview_feature_change))
        .route("/api/v1/tools", get(tools))
        .route("/api/v1/tools/{name}", post(run_tool))
        // ---- adapter ----
        .route("/api/v1/adapters/ports", get(ports))
        .route("/api/v1/adapter", get(adapter))
        .route("/api/v1/adapter/connect", post(connect))
        .route("/api/v1/adapter/disconnect", post(disconnect))
        // ---- vehicle ----
        .route("/api/v1/vehicles/identify", post(identify_vehicle))
        // ---- modules, live ----
        .route("/api/v1/modules", get(list_modules).post(scan_modules))
        .route("/api/v1/modules/{key}", get(module_identity))
        .route("/api/v1/modules/{key}/dtcs", get(module_dtcs))
        .route("/api/v1/modules/{key}/signals", get(module_signals))
        .route("/api/v1/modules/{key}/monitor-tests", get(module_monitor_tests))
        .route("/api/v1/modules/{key}/read", post(module_read))
        .route("/api/v1/dtcs/clear", post(clear_dtcs))
        .route("/api/v1/readiness", get(readiness))
        .route("/api/v1/modules/{key}/freeze-frame", get(freeze_frame))
        .route(
            "/api/v1/modules/{key}/tests/{test_id}/run",
            post(run_module_test),
        )
        // ---- sessions ----
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/{id}", get(session_detail))
        .route("/api/v1/sessions/{id}/events", get(session_events))
        .route("/api/v1/sessions/{id}/modules", get(session_modules))
        .route("/api/v1/sessions/{id}/dtcs", get(session_dtcs))
        .route(
            "/api/v1/sessions/{id}/measurements",
            get(session_measurements),
        )
        .route("/api/v1/sessions/{id}/stream", get(ws::session_events_ws))
        // ---- agent seam ----
        .route("/api/v1/agent", get(crate::agent_routes::agent_status))
        .route("/api/v1/agent/messages", post(crate::agent_routes::messages))
        .route("/api/v1/agent/inspect", post(crate::agent_routes::inspect))
        .route(
            "/api/v1/settings/providers",
            get(crate::agent_routes::list_providers).post(crate::agent_routes::add_provider),
        )
        .route(
            "/api/v1/settings/providers/{id}",
            axum::routing::put(crate::agent_routes::update_provider)
                .delete(crate::agent_routes::delete_provider),
        )
        .route("/api/v1/settings/providers/{id}/select", post(crate::agent_routes::select_provider))
        .route("/api/v1/settings/providers/{id}/test", post(crate::agent_routes::test_provider))
        .route("/api/v1/settings/voice", post(crate::agent_routes::set_voice))
        // ---- live data websocket ----
        .route("/api/v1/live", get(ws::live_data_ws))
        .with_state(state)
}

// ------------------------------------------------------------------ server

async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let connected = state
        .peek_service(|s| {
            json!({
                "session_id": s.session_id(),
                "state": s.state(),
                "adapter": s.adapter_descriptor(),
            })
        })
        .await?;
    Ok(Json(json!({
        "service": "ai-mechanic",
        "api_version": "v1",
        "build_version": env!("CARGO_PKG_VERSION"),
        "schema_version": state.store.schema_version(),
        "database": state.store.path(),
        "bind": state.config.bind,
        "default_transport": state.config.default_transport,
        "default_scenario": state.config.default_scenario.as_str(),
        "scenarios": aim_simulator::ScenarioId::all()
            .iter()
            .map(|s| json!({ "id": s.as_str(), "description": aim_simulator::Scenario::new(*s).description }))
            .collect::<Vec<_>>(),
        "active": connected,
    })))
}

/// Every plain-language explanation this build ships.
///
/// Sent as one document rather than looked up per item: the whole set is a few
/// tens of kilobytes, it never changes while the process runs, and a client
/// that has it can explain anything on screen without another round trip.
async fn explanations(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    use aim_decoders::ExplainKind;
    let c = &state.decoders.explanations;
    let bucket = |k: ExplainKind| -> Vec<Value> {
        c.all(k)
            .map(|e| json!({ "id": e.id, "easy": e.easy, "technical": e.technical }))
            .collect()
    };
    Ok(Json(json!({
        "signals": bucket(ExplainKind::Signal),
        "codes": bucket(ExplainKind::Code),
        "concepts": bucket(ExplainKind::Concept),
    })))
}

/// Configurable features that could apply to the connected vehicle.
async fn features(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state.with_service(|s| s.list_features("user:api")).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    /// What the feature should be set to.
    desired: aim_diagnostics::DesiredValue,
}

/// Evaluate a proposed configuration change. Sends nothing to the vehicle.
///
/// Note the shape: the feature id comes from the path and the value from the
/// body, and there is nowhere in either to put a module address or a byte
/// offset. That is the point — see `aim_diagnostics::config`.
async fn preview_feature_change(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PreviewBody>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.preview_configuration_change(&id, body.desired, "user:api"))
            .await?,
    ))
}

/// What user-supplied profile files contributed at startup.
///
/// Reported rather than silent: a user who cannot see what got loaded cannot
/// tell an app that read their file from one that ignored it.
async fn profiles(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "report": state.decoders.profiles,
        "feature_count": state.decoders.features.len(),
        "features_from_profiles": state
            .decoders
            .features
            .all()
            .filter(|f| f.source.as_deref().is_some_and(|s| s.starts_with("user:")))
            .count(),
    }))
}

async fn capabilities(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    // The gate lives inside the service, but the registry it uses is fixed for
    // the build, so the list is available whether or not anything is connected.
    let registry = aim_safety::CapabilityRegistry::phase1();
    let all: Vec<Value> = registry
        .all()
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id,
                "description": c.description,
                "level": c.level,
                "enabled": c.level <= aim_safety::MAX_ENABLED_LEVEL,
                "mutating": c.mutating,
                "preconditions": c.preconditions,
                "required_adapter_flags": c.required_adapter_flags,
            })
        })
        .collect();
    let conditions = state.peek_service(|s| *s.conditions()).await?;
    Ok(Json(json!({
        "max_enabled_level": aim_safety::MAX_ENABLED_LEVEL,
        "capabilities": all,
        "observed_conditions": conditions,
    })))
}

async fn tools(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "tools": state.tools.all(),
        "enabled": state.tools.enabled().iter().map(|t| &t.name).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct ToolBody {
    #[serde(default)]
    arguments: Value,
    #[serde(default)]
    initiator: Option<String>,
    #[serde(default)]
    confirmation: Option<String>,
}

/// Generic tool dispatch — the same path the future agent runtime will use.
async fn run_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<ToolBody>>,
) -> ApiResult<Json<ToolResult>> {
    let body = body.map(|Json(b)| b);
    let mut call = aim_tools::ToolCall::new(
        name,
        body.as_ref()
            .and_then(|b| b.initiator.clone())
            .unwrap_or_else(|| String::from("user:api")),
    );
    if let Some(b) = &body {
        if !b.arguments.is_null() {
            call = call.with_arguments(b.arguments.clone());
        }
        if let Some(c) = &b.confirmation {
            call = call.confirmed_by(c.clone());
        }
    }
    let tools = std::sync::Arc::clone(&state.tools);
    let result = state
        .with_service(move |s| aim_tools::execute(s, &tools, &call))
        .await?;
    Ok(Json(result))
}

// ----------------------------------------------------------------- adapter

#[derive(Debug, Deserialize)]
struct PortsQuery {
    /// Send `ATZ`/`ATI` to each port to see which one answers. Slower, and it
    /// briefly opens every port, so it is opt-in.
    #[serde(default)]
    probe: bool,
}

async fn ports(Query(q): Query<PortsQuery>) -> ApiResult<Json<Value>> {
    let probe = q.probe;
    let listed = tokio::task::spawn_blocking(move || list_ports_blocking(probe))
        .await
        .map_err(|e| ApiError::internal(format!("port enumeration failed: {e}")))?;
    Ok(Json(listed))
}

#[cfg(feature = "serial")]
fn list_ports_blocking(probe: bool) -> Value {
    let ports = aim_transport::list_ports();
    if !probe {
        return json!({ "ports": ports, "probed": false });
    }
    let probed: Vec<Value> = ports
        .into_iter()
        .map(|info| {
            let identification =
                aim_adapter::probe::probe_port(&info.name, Duration::from_millis(2500));
            match identification {
                Ok(id) => json!({ "port": info, "identification": id }),
                // A port that will not open is reported with its reason. "COM3
                // is in use by another program" is exactly what a user needs.
                Err(e) => json!({ "port": info, "error": e }),
            }
        })
        .collect();
    json!({ "ports": probed, "probed": true })
}

#[cfg(not(feature = "serial"))]
fn list_ports_blocking(probe: bool) -> Value {
    json!({
        "ports": [],
        "probed": probe,
        "note": "this build has no serial support compiled in; only the simulator transport is available",
    })
}

async fn adapter(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let snapshot = state
        .peek_service(|s| {
            json!({
                "connected": s.state().is_usable(),
                "state": s.state(),
                "descriptor": s.adapter_descriptor(),
                "health": s.health(),
                "capabilities": s.capabilities(),
                "session_id": s.session_id(),
                "vehicle": s.vehicle(),
            })
        })
        .await?;
    Ok(Json(snapshot.unwrap_or_else(|| {
        json!({ "connected": false, "state": { "state": "disconnected" } })
    })))
}

async fn connect(
    State(state): State<AppState>,
    body: Option<Json<ConnectRequest>>,
) -> ApiResult<Json<ToolResult>> {
    let request = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(state.connect(request).await?))
}

async fn disconnect(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.disconnect().await?))
}

// ----------------------------------------------------------------- vehicle

async fn identify_vehicle(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(|s| s.identify_vehicle("user:api"))
            .await?,
    ))
}

// ----------------------------------------------------------------- modules

async fn scan_modules(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.scan_modules("user:api")).await?))
}

async fn list_modules(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let modules = state
        .with_service(|s| s.store().modules(s.session_id()))
        .await??;
    Ok(Json(json!({ "modules": modules })))
}

async fn module_identity(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.get_module_identity(&key, "user:api"))
            .await?,
    ))
}

async fn module_dtcs(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.read_dtcs(Some(&key), "user:api"))
            .await?,
    ))
}

async fn module_signals(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.read_supported_pids(&key, "user:api"))
            .await?,
    ))
}

async fn module_monitor_tests(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.read_monitor_tests(&key, "user:api"))
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ClearBody {
    /// Who authorised it. Recorded in the audit trail verbatim.
    ///
    /// Required, and deliberately not defaulted: the safety gate refuses an L1
    /// operation without one, and a route that supplied its own would be
    /// forging the consent the gate exists to check for.
    confirmation: String,
    /// One module, or every module that answers when absent.
    #[serde(default)]
    module: Option<String>,
}

/// Emissions readiness from every module that keeps it.
///
/// Not per-module in the path, deliberately: readiness is a whole-vehicle
/// question and the modules can disagree, so the answer has to include all of
/// them or it is picking a winner without saying so.
async fn readiness(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state.with_service(|s| s.read_readiness("user:api")).await?,
    ))
}

/// Clear stored codes. The one write a person can make in this build.
///
/// Its own route rather than the generic tool dispatch, because the agent
/// reaches the vehicle exclusively through that dispatch and must not be able
/// to do this. Erasing the readiness monitors is a decision for whoever owns
/// the vehicle, taken in front of a dialog that says what it costs — not
/// something a model talks itself into halfway through an inspection.
async fn clear_dtcs(
    State(state): State<AppState>,
    Json(body): Json<ClearBody>,
) -> ApiResult<Json<ToolResult>> {
    if body.confirmation.trim().is_empty() {
        return Err(ApiError::bad_request(
            "clearing codes needs an explicit confirmation naming who authorised it",
        ));
    }
    Ok(Json(
        state
            .with_service(move |s| {
                s.clear_dtcs(body.module.as_deref(), "user:api", Some(&body.confirmation))
            })
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ReadBody {
    signals: Vec<String>,
}

async fn module_read(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<ReadBody>,
) -> ApiResult<Json<ToolResult>> {
    if body.signals.is_empty() {
        return Err(ApiError::bad_request("signals must not be empty"));
    }
    Ok(Json(
        state
            .with_service(move |s| s.read_live_data(&key, &body.signals, "user:api"))
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct FrameQuery {
    #[serde(default)]
    frame: u8,
}

async fn freeze_frame(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(q): Query<FrameQuery>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| s.read_freeze_frame(&key, q.frame, "user:api"))
            .await?,
    ))
}

/// Active tests are handoff §13's `POST /modules/{id}/tests/{testId}/run`.
///
/// No L1 test is implemented in this build. The endpoint exists and answers
/// with a specific code so the UI can show "not in this build" rather than
/// inferring it from a 404.
async fn run_module_test(Path((key, test_id)): Path<(String, String)>) -> ApiError {
    ApiError::not_implemented(format!(
        "no active tests are implemented in this build; {test_id:?} on {key:?} is not available. \
         Active tests are L1 operations requiring confirmation and preconditions, and are \
         scheduled for the bidirectional-diagnostics phase."
    ))
}

/// The agent runtime seam.
///
/// Deliberately unbuilt: this build ships the deterministic core, the safety

// ---------------------------------------------------------------- sessions

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    50
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<Value>> {
    let sessions = state.store.list_sessions(q.limit.min(500))?;
    Ok(Json(json!({ "sessions": sessions })))
}

async fn session_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = SessionId::from_string(id);
    let session = state.store.get_session(&id)?;
    let vehicle = match &session.vehicle_id {
        Some(v) => state.store.get_vehicle(v)?,
        None => None,
    };
    Ok(Json(json!({
        "session": session,
        "vehicle": vehicle,
        "connections": state.store.connections(&id)?,
        "modules": state.store.modules(&id)?,
        "dtcs": state.store.dtcs(&id, None)?,
        "test_runs": state.store.test_runs(&id)?,
        "diagnoses": state.store.diagnoses(&id)?,
        "agent_traces": state.store.agent_traces(&id)?,
        "event_count": state.store.event_count(&id)?,
    })))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after_seq: i64,
    #[serde(default = "default_event_limit")]
    limit: u32,
}

fn default_event_limit() -> u32 {
    500
}

async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Value>> {
    let id = SessionId::from_string(id);
    // Confirm the session exists so an unknown id is a 404 rather than an
    // empty page that looks like a session with no history.
    state.store.get_session(&id)?;
    let events = state.store.events_since(&id, q.after_seq, q.limit.min(5000))?;
    let total = state.store.event_count(&id)?;
    Ok(Json(json!({
        "events": events,
        "after_seq": q.after_seq,
        "total": total,
    })))
}

async fn session_modules(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = SessionId::from_string(id);
    state.store.get_session(&id)?;
    Ok(Json(json!({ "modules": state.store.modules(&id)? })))
}

async fn session_dtcs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = SessionId::from_string(id);
    state.store.get_session(&id)?;
    Ok(Json(json!({ "dtcs": state.store.dtcs(&id, None)? })))
}

#[derive(Debug, Deserialize)]
struct MeasurementQuery {
    signal: Option<String>,
    #[serde(default = "default_event_limit")]
    limit: u32,
}

async fn session_measurements(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<MeasurementQuery>,
) -> ApiResult<Json<Value>> {
    let id = SessionId::from_string(id);
    state.store.get_session(&id)?;
    let measurements =
        state
            .store
            .measurements(&id, q.signal.as_deref(), q.limit.min(10_000))?;
    Ok(Json(json!({ "measurements": measurements })))
}
