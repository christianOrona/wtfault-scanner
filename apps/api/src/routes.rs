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
use aim_types::{AimError, ErrorCode, SessionId, ToolResult};
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
        .route("/api/v1/profiles/preview", post(preview_profile))
        .route("/api/v1/profiles/import", post(import_profile))
        .route("/api/v1/features", get(features))
        .route("/api/v1/features/{id}", get(read_feature))
        .route("/api/v1/config/capture", post(capture_configuration))
        .route("/api/v1/config/compare-to-factory", post(compare_to_factory))
        .route("/api/v1/vehicles/knowledge", get(vehicle_knowledge).post(record_vehicle_knowledge))
        .route("/api/v1/vehicles/scorecard", get(vehicle_scorecard))
        .route("/api/v1/vehicles/scorecard/diff", post(diff_scorecards))
        .route("/api/v1/config/diff", post(diff_captures))
        .route("/api/v1/config/captures", get(list_captures))
        .route("/api/v1/catalog/signals", get(catalog_signals))
        .route("/api/v1/catalog/signals/{signal}", post(read_catalog_signal))
        .route("/api/v1/modules/{key}/write-gate", post(probe_write_gate))
        .route("/api/v1/features/{id}/preview", post(preview_feature_change))
        .route("/api/v1/features/{id}/apply", post(apply_feature_change))
        .route("/api/v1/features/{id}/write-gate", post(probe_feature_write_gate))
        .route("/api/v1/tools", get(tools))
        .route("/api/v1/tools/{name}", post(run_tool))
        // ---- adapter ----
        .route("/api/v1/adapters/ports", get(ports))
        .route("/api/v1/adapter", get(adapter))
        .route("/api/v1/adapter/connect", post(connect))
        .route("/api/v1/adapter/disconnect", post(disconnect))
        // ---- vehicle ----
        .route("/api/v1/vehicles/identify", post(identify_vehicle))
        .route("/api/v1/vehicles/identity", get(vehicle_identity))
        .route(
            "/api/v1/vehicles/as-built",
            get(as_built_status).post(import_as_built).delete(forget_as_built),
        )
        .route("/api/v1/vehicles/vpic", get(vpic_status).post(lookup_vpic))
        .route("/api/v1/vehicles/obdb", get(obdb_status).post(fetch_obdb))
        // ---- modules, live ----
        .route("/api/v1/modules", get(list_modules).post(scan_modules))
        .route("/api/v1/modules/{key}", get(module_identity))
        .route("/api/v1/modules/{key}/dtcs", get(module_dtcs))
        .route("/api/v1/modules/{key}/signals", get(module_signals))
        .route("/api/v1/modules/{key}/monitor-tests", get(module_monitor_tests))
        .route("/api/v1/modules/{key}/capabilities", get(module_capabilities))
        .route("/api/v1/modules/{key}/read", post(module_read))
        .route("/api/v1/dtcs/clear", post(clear_dtcs))
        .route("/api/v1/readiness", get(readiness))
        .route("/api/v1/procedures", get(list_procedures))
        .route("/api/v1/procedures/{id}", get(check_procedure))
        .route("/api/v1/procedures/{id}/run", post(run_procedure))
        .route("/api/v1/modules/scan-all", post(scan_all_modules))
        .route("/api/v1/export", post(export_file))
        // ---- updates ----
        .route("/api/v1/update/check", get(update_check))
        .route("/api/v1/update/apply", post(update_apply))
        .route("/api/v1/update/download", post(update_download))
        .route("/api/v1/update/download", get(update_download_status))
        // ---- reporting a problem ----
        .route("/api/v1/support/report", get(support_report))
        .route("/api/v1/support/reveal", post(support_reveal))
        .route("/api/v1/modules/{key}/freeze-frame", get(freeze_frame))
        .route("/api/v1/modules/{key}/tests/{test_id}/run", post(run_module_test))
        // ---- sessions ----
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/{id}", get(session_detail))
        .route("/api/v1/sessions/{id}/events", get(session_events))
        .route("/api/v1/sessions/{id}/modules", get(session_modules))
        .route("/api/v1/sessions/{id}/dtcs", get(session_dtcs))
        .route("/api/v1/sessions/{id}/measurements", get(session_measurements))
        .route("/api/v1/sessions/{id}/stream", get(ws::session_events_ws))
        .route("/api/v1/sessions/{before}/compare/{after}", get(compare_sessions))
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
        .route(
            "/api/v1/settings/privacy",
            get(crate::agent_routes::privacy).post(crate::agent_routes::set_privacy),
        )
        // ---- live data websocket ----
        .route("/api/v1/live", get(ws::live_data_ws))
        .with_state(state)
}

// ------------------------------------------------------------------ server

async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    // Asked before the service is touched, and reported whether or not the
    // service can be read. A full scan holds the service for minutes; a health
    // check that waited for it would go quiet exactly when somebody is checking
    // whether anything is still alive.
    let busy = state.busy_now().map(|(what, seconds)| {
        json!({
            "doing": what,
            "seconds": seconds,
            "explanation": format!(
                "The adapter is busy with {what}, {seconds}s so far. Only one conversation \
                 with the vehicle can run at a time, so anything else is waiting for this."
            ),
        })
    });

    // The service itself is only read when it is free. When it is busy the
    // health check answers from what it already knows rather than joining the
    // queue behind a scan.
    let connected = if busy.is_some() {
        None
    } else {
        state
            .peek_service(|s| {
                json!({
                    "session_id": s.session_id(),
                    "state": s.state(),
                    "adapter": s.adapter_descriptor(),
                })
            })
            .await?
    };
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
        "busy": busy,
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
        c.all(k).map(|e| json!({ "id": e.id, "easy": e.easy, "technical": e.technical })).collect()
    };
    Ok(Json(json!({
        "signals": bucket(ExplainKind::Signal),
        "codes": bucket(ExplainKind::Code),
        "concepts": bucket(ExplainKind::Concept),
    })))
}

/// Configurable features that could apply to the connected vehicle.
async fn features(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.list_features("user:api")).await?))
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    /// What the feature should be set to.
    desired: aim_diagnostics::DesiredValue,
}

/// Read one feature's current setting from the vehicle.
///
/// Read-only, so no confirmation. A feature with no measured mapping answers
/// with what is known and what would establish the rest, rather than a 404 -
/// the question "can my truck do this" has a useful answer even when "where
/// does the setting live" does not.
async fn read_feature(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_feature(&id, "user:api")).await?))
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
            .with_service_named("checking what a change would do", move |s| {
                s.preview_configuration_change(&id, body.desired, "user:api")
            })
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ApplyBody {
    /// What the feature should be set to.
    desired: aim_diagnostics::DesiredValue,
    /// Who authorised this, typed by a person.
    confirmation: String,
}

/// Change a vehicle setting.
///
/// Its own route rather than the generic tool dispatch, for the same reason
/// clearing codes has one: the agent reaches the vehicle exclusively through
/// that dispatch, and it must not be able to change how a vehicle is
/// configured. A model can propose a change and explain it; a person applies
/// it.
///
/// The same shape rule as the preview holds — the feature id comes from the
/// path and the value from the body, and neither has anywhere to put a module
/// address or a byte offset.
async fn apply_feature_change(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ApplyBody>,
) -> ApiResult<Json<ToolResult>> {
    if body.confirmation.trim().is_empty() {
        return Err(ApiError::bad_request(
            "changing a vehicle setting needs an explicit confirmation naming who authorised it",
        ));
    }
    Ok(Json(
        state
            .with_service_named("changing a setting on the vehicle", move |s| {
                s.apply_configuration_change(&id, body.desired, "user:api", &body.confirmation)
            })
            .await?,
    ))
}

/// Ask the module that owns a feature whether it accepts writes.
///
/// Addressed by feature, not by module, so a screen offering to change "the
/// double horn chirp" never has to know that it lives at 72E and is requested
/// at 726. Writes nothing that can land: see
/// [`aim_diagnostics::DiagnosticService::probe_write_gate_for_feature`].
async fn probe_feature_write_gate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ClearBody>,
) -> ApiResult<Json<ToolResult>> {
    if body.confirmation.trim().is_empty() {
        return Err(ApiError::bad_request(
            "asking a module whether it accepts writes needs an explicit confirmation",
        ));
    }
    Ok(Json(
        state
            .with_service_named("asking a module whether it accepts changes", move |s| {
                s.probe_write_gate_for_feature(&id, "user:api", Some(body.confirmation.as_str()))
            })
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
        body.as_ref().and_then(|b| b.initiator.clone()).unwrap_or_else(|| String::from("user:api")),
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
    let result = state.with_service(move |s| aim_tools::execute(s, &tools, &call)).await?;
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
    // This is the poll the whole window runs on, once a second. It must answer
    // while a scan is running, and a scan holds the service for minutes — so it
    // asks without waiting, and falls back to the last answer it got.
    let peeked = state
        .peek_service_now(|s| {
            json!({
                "connected": s.state().is_usable(),
                "state": s.state(),
                "descriptor": s.adapter_descriptor(),
                "health": s.health(),
                "capabilities": s.capabilities(),
                // The counters turned into something a caller can act on.
                "fitness": aim_types::AdapterFitness::assess(&s.health(), &s.capabilities()),
                "session_id": s.session_id(),
                "vehicle": s.vehicle(),
            })
        })
        .await;

    let mut snapshot = match peeked {
        Ok(Some(fresh)) => {
            if let Ok(mut slot) = state.last_adapter.lock() {
                *slot = Some(fresh.clone());
            }
            fresh
        }
        // Nothing connected. Not a stale answer — a current one.
        Ok(None) => {
            if let Ok(mut slot) = state.last_adapter.lock() {
                *slot = None;
            }
            json!({ "connected": false, "state": { "state": "disconnected" } })
        }
        // Mid-request. Answer from the last reading rather than blocking, and
        // say that is what this is, so nothing downstream mistakes a cached
        // health counter for a fresh one.
        Err(_) => match state.last_adapter.lock().ok().and_then(|s| s.clone()) {
            Some(mut cached) => {
                if let Some(obj) = cached.as_object_mut() {
                    obj.insert(String::from("stale"), json!(true));
                }
                cached
            }
            None => json!({ "connected": false, "state": { "state": "disconnected" } }),
        },
    };

    // Attached either way: a free adapter reports no work, which is itself the
    // answer to "is it stuck?".
    if let Some(obj) = snapshot.as_object_mut() {
        obj.insert(
            String::from("busy"),
            match state.busy_now() {
                Some((what, seconds)) => json!({ "doing": what, "seconds": seconds }),
                None => Value::Null,
            },
        );
    }
    Ok(Json(snapshot))
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
            .with_service_named("identifying the vehicle", |s| s.identify_vehicle("user:api"))
            .await?,
    ))
}

/// Everything established about the vehicle, with the evidence behind it.
///
/// A `GET` beside the `POST` above, and the distinction is the point: the POST
/// goes and asks the vehicle, this reports what is already known from every
/// read so far. It never touches the bus, so it is safe to call whenever a
/// screen needs to know how sure the application is about what it is plugged
/// into.
async fn vehicle_identity(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let identity = state.peek_service(|s| s.identity()).await?;
    Ok(Json(json!({ "identity": identity })))
}

/// Whether an as-built file is held for this vehicle, and how to get one.
async fn as_built_status(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service_named("reading the as-built configuration", |s| {
                s.as_built_status("user:api")
            })
            .await?,
    ))
}

/// An as-built file's contents, sent whole.
///
/// The file rather than a path: the server is not given a way to read arbitrary
/// files off the machine it runs on, even though it happens to be the same
/// machine today. The client opens what the person chose and posts the text.
#[derive(Debug, Deserialize)]
struct AsBuiltBody {
    /// The file's contents.
    text: String,
    /// Where it came from, so a wrong import is traceable to a file.
    #[serde(default)]
    source: Option<String>,
}

/// Import an as-built file, if its VIN is the connected vehicle's.
async fn import_as_built(
    State(state): State<AppState>,
    Json(body): Json<AsBuiltBody>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| {
                s.import_as_built(&body.text, body.source.as_deref(), "user:api")
            })
            .await?,
    ))
}

/// Forget the as-built file held for this vehicle.
async fn forget_as_built(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.forget_as_built("user:api")).await?))
}

/// The cached vPIC reply, if any.
///
/// Never touches the network. A 200 OK with a null `cached` field means no
/// VIN has been read yet.
async fn vpic_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let cached = state.with_service(|s| s.cached_vpic()).await?;
    Ok(Json(json!({ "cached": cached.as_ref().map(cached_json) })))
}

/// Request a vPIC lookup.
///
/// If `refresh` is false (the default), and a reply is already cached, it is
/// returned without touching the network. Otherwise, the lookup proceeds.
#[derive(Debug, Default, Deserialize)]
struct VpicBody {
    /// If true, force a fresh lookup even if a reply is cached.
    #[serde(default)]
    refresh: bool,
}

/// Perform a vPIC lookup for the VIN of the connected vehicle.
///
/// The lookup is performed only if no cached reply exists or `refresh` is true.
/// Returns the decoded vehicle information.
async fn lookup_vpic(
    State(state): State<AppState>,
    Json(body): Json<VpicBody>,
) -> ApiResult<Json<Value>> {
    let (cached, request) = state.with_service(|s| (s.cached_vpic(), s.vpic_request())).await?;
    let url = request?;

    // Each VIN is looked up once. Asking again goes to the network only when
    // somebody explicitly asks for a refresh.
    if let (false, Some(reply)) = (body.refresh, &cached) {
        let mut held = cached_json(reply);
        held["from_cache"] = json!(true);
        return Ok(Json(held));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ApiError::bad_request(format!("could not build an http client: {e}")))?;
    let response = client.get(&url).send().await.map_err(|e| {
        ApiError::bad_request(format!("could not reach NHTSA vPIC: {e}. Nothing was changed."))
    })?;
    if !response.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "NHTSA vPIC answered {} rather than the vehicle information",
            response.status()
        )));
    }
    // Checked against the declared length first where there is one, and again
    // after reading, because a declared length is a claim by the server.
    if response.content_length().is_some_and(|n| n as usize > 512 * 1024) {
        return Err(ApiError::bad_request("NHTSA vPIC reply is too large"));
    }
    let text = response
        .text()
        .await
        .map_err(|e| ApiError::bad_request(format!("could not read NHTSA vPIC reply: {e}")))?;
    if text.len() > 512 * 1024 {
        return Err(ApiError::bad_request("NHTSA vPIC reply is too large"));
    }

    let url_for_service = url.clone();
    let decode =
        state.with_service(move |s| s.record_vpic_reply(&text, &url_for_service)).await??;
    let held = state.with_service(|s| s.cached_vpic()).await?;
    Ok(Json(json!({
        "from_cache": false,
        "vin": held.as_ref().map(|r| r.vin.clone()),
        "fetched_at": held.as_ref().map(|r| r.fetched_at.clone()),
        "source_url": url,
        "decode": decode,
    })))
}

/// Which OBDb signal set belongs to the connected vehicle, and whether it is kept.
///
/// Never touches the network.
async fn obdb_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let repo = match obdb_repository(&state).await? {
        Err(why) => return Ok(Json(json!({ "repository": null, "why_not": why }))),
        Ok(repo) => repo,
    };
    let kept = state
        .config
        .profiles_dir
        .as_deref()
        .and_then(|d| aim_decoders::obdb::cached_signalset(d, &repo))
        .is_some();
    Ok(Json(json!({
        "repository": repo,
        "kept": kept,
        "source": aim_decoders::obdb::repository_url(&repo),
    })))
}

/// Ask for the connected vehicle's OBDb signal set.
#[derive(Debug, Default, Deserialize)]
struct ObdbBody {
    /// Fetch again even when a copy is kept.
    #[serde(default)]
    refresh: bool,
}

/// Fetch the connected vehicle's OBDb signal set, on request, and keep it.
///
/// OBDb is organised by make and model, which come from the NHTSA vPIC lookup,
/// so that has to have happened first. A kept set loads on the next start.
async fn fetch_obdb(
    State(state): State<AppState>,
    Json(body): Json<ObdbBody>,
) -> ApiResult<Json<Value>> {
    let repo = match obdb_repository(&state).await? {
        Ok(r) => r,
        Err(why) => return Err(ApiError::from(AimError::new(ErrorCode::PreconditionFailed, why))),
    };

    let Some(dir) = state.config.profiles_dir.clone() else {
        return Err(ApiError::bad_request(
            "this build has no profiles directory, so there is nowhere to keep a signal set",
        ));
    };

    if !body.refresh {
        if let Some(path) = aim_decoders::obdb::cached_signalset(&dir, &repo) {
            return Ok(Json(json!({
                "from_cache": true,
                "repository": repo,
                "path": path.display().to_string(),
                "source": aim_decoders::obdb::repository_url(&repo),
                "loads_on_next_start": true
            })));
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ApiError::bad_request(format!("could not build an http client: {e}")))?;
    let response =
        client.get(aim_decoders::obdb::signalset_url(&repo)).send().await.map_err(|e| {
            ApiError::bad_request(format!("could not reach OBDb: {e}. Nothing was changed."))
        })?;

    if response.status() == 404 {
        return Err(ApiError::from(AimError::not_found(format!(
            "OBDb has no signal set for {repo} yet. Nothing was changed."
        ))));
    }

    if !response.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "OBDb answered {} rather than the signal set. Nothing was changed.",
            response.status()
        )));
    }

    // Checked against the declared length first where there is one, and again
    // after reading, because a declared length is a claim by the server.
    if response
        .content_length()
        .is_some_and(|n| n as usize > aim_decoders::obdb::MAX_SIGNALSET_BYTES)
    {
        return Err(ApiError::bad_request("OBDb signal set reply is too large"));
    }

    let text = response
        .text()
        .await
        .map_err(|e| ApiError::bad_request(format!("could not read OBDb signal set reply: {e}")))?;

    if text.len() > aim_decoders::obdb::MAX_SIGNALSET_BYTES {
        return Err(ApiError::bad_request("OBDb signal set reply is too large"));
    }

    // OBDb has a repository for many models whose signal set is still empty
    // (Ford-F-250 was, on 2026-09-17). Keeping that would look like a vehicle
    // with nothing to offer, so it is said plainly instead and nothing is kept.
    let commands = aim_decoders::signalset::SignalSet::from_json(&text).map(|s| s.commands.len());
    if commands == Ok(0) {
        return Err(ApiError::from(AimError::not_found(format!(
            "OBDb has a page for {repo} but no signals recorded for it yet. Nothing was changed."
        ))));
    }

    // Refuses anything that is not a signal set, with its own error code, before writing.
    let path = aim_decoders::obdb::cache_signalset(&dir, &repo, &text)?;

    Ok(Json(json!({
        "from_cache": false,
        "commands": commands.unwrap_or(0),
        "repository": repo,
        "path": path.display().to_string(),
        "source": aim_decoders::obdb::repository_url(&repo),
        "loads_on_next_start": true
    })))
}

/// Helper to determine the OBDb repository name for the connected vehicle.
async fn obdb_repository(state: &AppState) -> ApiResult<Result<String, String>> {
    let cached = state.with_service(|s| s.cached_vpic()).await?;

    let reply = match cached {
        Some(r) => r,
        None => {
            return Ok(Err(
                "look the vehicle up with NHTSA first: OBDb is organised by make and model"
                    .to_string(),
            ))
        }
    };

    let Ok(decode) = aim_decoders::parse_decode_vin_values(&reply.body) else {
        return Ok(Err(String::from(
            "the kept NHTSA reply for this VIN no longer reads; look the vehicle up again",
        )));
    };

    let repo = aim_decoders::obdb::repository_name(
        decode.make.as_deref().unwrap_or(""),
        decode.model.as_deref().unwrap_or(""),
    );

    match repo {
        Some(r) => Ok(Ok(r)),
        None => Ok(Err("NHTSA did not give a make and model for this VIN, so there is no OBDb signal set to look for".to_string())),
    }
}

/// Helper to format a cached vPIC reply for JSON output.
fn cached_json(reply: &aim_session::StoredVpicReply) -> Value {
    json!({
        "vin": reply.vin,
        "fetched_at": reply.fetched_at,
        "source_url": reply.source_url,
        "decode": aim_decoders::parse_decode_vin_values(&reply.body).ok()
    })
}

// ----------------------------------------------------------------- modules

async fn scan_modules(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service_named("a scan for modules", |s| s.scan_modules("user:api")).await?))
}

/// Every module found, with which bus it answered on and what it can be asked.
///
/// The second fact is why this is not a plain list any more. A vehicle with two
/// buses is mostly *not* emissions modules: a 2019 F-250 answers with 7 on the
/// primary bus and 29 on the secondary, and those 29 implement UDS and nothing
/// else. Offering them a live-data picker built on OBD-II service 01 produces a
/// screen of empty rows and a person wondering what they did wrong.
///
/// Derived from the module key rather than stored, so sessions recorded before
/// there was a second bus need no migration.
async fn list_modules(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let modules = state.with_service(|s| s.store().modules(s.session_id())).await??;
    let described: Vec<Value> = modules
        .into_iter()
        .map(|m| {
            let bus = aim_adapter::VehicleBus::of_module_key(&m.module_key);
            let mut v = serde_json::to_value(&m).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut v {
                map.insert("bus".into(), serde_json::to_value(bus).unwrap_or(Value::Null));
                map.insert("bus_label".into(), Value::String(bus.label().into()));
                map.insert("answers_obd2".into(), Value::Bool(bus.answers_obd2()));
            }
            v
        })
        .collect();
    Ok(Json(json!({ "modules": described })))
}

async fn module_identity(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.get_module_identity(&key, "user:api")).await?))
}

async fn module_dtcs(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_dtcs(Some(&key), "user:api")).await?))
}

async fn module_signals(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_supported_pids(&key, "user:api")).await?))
}

/// Measure what one module supports. Reads only.
async fn module_capabilities(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service_named("a capability probe of one module", move |s| {
                s.probe_module_capabilities(&key, "user:api")
            })
            .await?,
    ))
}

async fn module_monitor_tests(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_monitor_tests(&key, "user:api")).await?))
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

/// Sweep the diagnostic address range and read every module's fault memory.
async fn scan_all_modules(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service_named("a full scan of every module", |s| s.scan_all_modules("user:api"))
            .await?,
    ))
}

/// Emissions readiness from every module that keeps it.
///
/// Not per-module in the path, deliberately: readiness is a whole-vehicle
/// question and the modules can disagree, so the answer has to include all of
/// them or it is picking a winner without saying so.
async fn readiness(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.read_readiness("user:api")).await?))
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
    Ok(Json(state.with_service(move |s| s.read_live_data(&key, &body.signals, "user:api")).await?))
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
    Ok(Json(state.with_service(move |s| s.read_freeze_frame(&key, q.frame, "user:api")).await?))
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
    let measurements = state.store.measurements(&id, q.signal.as_deref(), q.limit.min(10_000))?;
    Ok(Json(json!({ "measurements": measurements })))
}

/// A file the user asked to keep.
#[derive(Debug, Deserialize)]
struct ExportBody {
    /// Base file name. Path separators are stripped; see `safe_name`.
    filename: String,
    /// The whole file, as text.
    content: String,
}

/// Write an export to the user's Downloads folder and say where it went.
///
/// The obvious implementation — a blob URL and an `<a download>` — does nothing
/// inside the desktop shell. A Tauri webview has no download manager, so the
/// click is swallowed silently: the button appeared to work, no file appeared,
/// and there was no error anywhere to explain it.
///
/// Doing it server-side sidesteps the whole problem and is better anyway,
/// because it can report the actual path. The UI shows where the file is rather
/// than leaving the user to guess which folder their browser chose.
async fn export_file(
    State(_state): State<AppState>,
    Json(body): Json<ExportBody>,
) -> ApiResult<Json<Value>> {
    if body.content.is_empty() {
        return Err(ApiError::bad_request("nothing to export"));
    }
    // A quarter of a megabyte of text is a very large report. The cap exists so
    // a bug upstream cannot fill a disk through this endpoint.
    const MAX_BYTES: usize = 256 * 1024;
    if body.content.len() > MAX_BYTES {
        return Err(ApiError::bad_request(format!(
            "export is {} bytes; the limit is {MAX_BYTES}",
            body.content.len()
        )));
    }

    let name = safe_name(&body.filename)?;
    let dir = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .ok_or_else(|| ApiError::internal("cannot locate the Downloads folder"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;

    let path = dir.join(&name);
    std::fs::write(&path, body.content.as_bytes())
        .map_err(|e| ApiError::internal(format!("cannot write {}: {e}", path.display())))?;

    tracing::info!(path = %path.display(), "wrote an export");
    Ok(Json(json!({
        "path": path.display().to_string(),
        "directory": dir.display().to_string(),
        "filename": name,
    })))
}

/// Reduce a caller-supplied name to a bare filename inside the target folder.
///
/// The caller is this app's own UI, but that is not a reason to trust the
/// string: a path traversal here writes anywhere the user can write, and the
/// check is three lines.
fn safe_name(raw: &str) -> ApiResult<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let ok = !base.is_empty()
        && base != "."
        && base != ".."
        && !base.contains("..")
        && base.len() <= 120
        && base.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
    if ok {
        Ok(base.to_string())
    } else {
        Err(ApiError::bad_request(format!("unusable file name {raw:?}")))
    }
}

/// What changed between two recorded scans of the same vehicle.
///
/// The most useful diagnostic question a single scan cannot answer. Every scan
/// this app has ever done is already on disk; nothing compared them, so a fuel
/// trim that has been creeping for six months looked exactly like one that was
/// always where it is.
async fn compare_sessions(
    State(state): State<AppState>,
    Path((before, after)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let cmp = aim_session::compare_sessions(&state.store, &SessionId(before), &SessionId(after))?;
    // `notable` is the store's own judgement about what is worth a person's
    // attention. Sent alongside rather than used to filter, so the UI can show
    // everything if asked and the API never silently drops data.
    let notable: Vec<&str> =
        cmp.signals.iter().filter(|s| s.notable()).map(|s| s.signal_id.as_str()).collect();
    Ok(Json(json!({ "comparison": cmp, "notable_signals": notable })))
}

/// Is there a newer release? Harmless, so it needs no confirmation.
async fn update_check() -> Json<Value> {
    Json(serde_json::to_value(crate::update::check().await).unwrap_or(Value::Null))
}

/// Download the newest installer and run it.
///
/// Behind an explicit POST, and behind a button in the interface, because this
/// downloads an executable and starts it. The check happens on its own; this
/// does not.
async fn update_apply() -> ApiResult<Json<Value>> {
    match crate::update::apply().await {
        Ok(path) => Ok(Json(serde_json::json!({
            "started": true,
            "installer": path,
            "note": "The installer is running silently. This app closes and comes back updated.",
        }))),
        Err(e) => Err(ApiError::bad_request(e)),
    }
}

/// Start fetching the installer without installing it.
///
/// Returns immediately with whatever the download is doing; the work continues
/// in the background and [`update_download_status`] reports on it. Asking twice
/// is harmless.
async fn update_download() -> Json<Value> {
    tokio::spawn(async { crate::update::download().await });
    Json(serde_json::to_value(crate::update::download_state()).unwrap_or(Value::Null))
}

/// How far along the background download is.
async fn update_download_status() -> Json<Value> {
    Json(serde_json::to_value(crate::update::download_state()).unwrap_or(Value::Null))
}

/// What this application has established about the connected vehicle.
///
/// Separate from the session log, which records what the vehicle said. This is
/// what was concluded from it, including the things that were ruled out — which
/// are the expensive findings and the ones most easily lost.
#[derive(Debug, Deserialize)]
struct KnowledgeQuery {
    /// Which vehicle to report on. Omitted means the connected one.
    #[serde(default)]
    vin: Option<String>,
}

async fn vehicle_knowledge(
    State(state): State<AppState>,
    Query(q): Query<KnowledgeQuery>,
) -> ApiResult<Json<Value>> {
    // By VIN when asked, which is what makes this readable with nothing
    // plugged in. A garage reviewing six trucks at a desk is the ordinary case
    // for reporting, and requiring a connection to read what is already known
    // would make the store useless for exactly that.
    if let Some(vin) = q.vin {
        let findings = state.store.knowledge(&vin).map_err(ApiError::from)?;
        return Ok(Json(json!({
            "vin": vin,
            "count": findings.len(),
            "findings": findings,
        })));
    }

    // Connected vehicle, when there is one.
    if let Ok(findings) = state.with_service(|s| s.knowledge()).await {
        return Ok(Json(json!({
            "count": findings.len(),
            "findings": findings,
        })));
    }

    // Otherwise: which vehicles anything is known about at all. The answer a
    // person opening the app with nothing plugged in actually wants.
    let vehicles = state.store.vehicles_with_knowledge().map_err(ApiError::from)?;
    Ok(Json(json!({
        "vehicles": vehicles
            .into_iter()
            .map(|(vin, count)| json!({ "vin": vin, "findings": count }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
struct ScorecardQuery {
    #[serde(default)]
    vin: Option<String>,
}

/// Read a vehicle's scorecard by VIN with nothing plugged in.
///
/// An unknown VIN gives an empty scorecard. The scorecard is built from the
/// store's knowledge and does not touch any vehicle bus.
async fn vehicle_scorecard(
    State(state): State<AppState>,
    Query(q): Query<ScorecardQuery>,
) -> ApiResult<Json<Value>> {
    let vin = q
        .vin
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("a vin query parameter is required"))?;
    let card = aim_diagnostics::scorecard::scorecard(&state.store, vin).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(card).map_err(|e| ApiError::internal(e.to_string()))?))
}

/// Two saved scorecards to compare.
#[derive(Debug, Deserialize)]
struct ScorecardDiffBody {
    /// The earlier snapshot.
    before: aim_diagnostics::Scorecard,
    /// The later snapshot.
    after: aim_diagnostics::Scorecard,
}

/// Compares two saved scorecards; `before` is the earlier snapshot.
async fn diff_scorecards(Json(body): Json<ScorecardDiffBody>) -> ApiResult<Json<Value>> {
    Ok(Json(
        serde_json::to_value(body.before.diff(&body.after))
            .map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

#[derive(Debug, Deserialize)]
struct FindingBody {
    /// Which vehicle this is about.
    ///
    /// Optional: when absent the connected vehicle is used, which is the
    /// ordinary case. Supplying one explicitly is for recording what was
    /// learned about a vehicle that is not plugged in right now — seeding a
    /// history from a session that has already ended, or correcting an entry
    /// at a desk. The VIN is the key either way, so nothing can be filed
    /// against a vehicle by accident.
    #[serde(default)]
    vin: Option<String>,
    subject: String,
    outcome: String,
    claim: String,
    evidence: String,
    #[serde(default)]
    authority: Option<String>,
}

/// Record something established about the connected vehicle.
///
/// A POST rather than something the app infers: a finding is a claim, and a
/// claim needs somebody or something willing to stand behind it. Evidence is
/// required by the type, because a finding nobody can attribute is a rumour.
async fn record_vehicle_knowledge(
    State(state): State<AppState>,
    Json(body): Json<FindingBody>,
) -> ApiResult<Json<Value>> {
    if body.claim.trim().is_empty() || body.evidence.trim().is_empty() {
        return Err(ApiError::bad_request(
            "a finding needs both a claim and the evidence behind it",
        ));
    }
    let outcome = match body.outcome.as_str() {
        "established" => aim_session::FindingOutcome::Established,
        "ruled_out" => aim_session::FindingOutcome::RuledOut,
        "observed" => aim_session::FindingOutcome::Observed,
        other => {
            return Err(ApiError::bad_request(format!(
                "{other:?} is not an outcome; use established, ruled_out or observed"
            )))
        }
    };
    let authority = body.authority.unwrap_or_else(|| String::from("measured_this_session"));

    match body.vin {
        // Named explicitly: goes straight to the store, because there may be no
        // session and no vehicle connected at all.
        Some(vin) => {
            let finding = aim_session::Finding {
                subject: body.subject,
                outcome,
                claim: body.claim,
                evidence: body.evidence,
                authority,
                observed_at: aim_types::now().to_rfc3339(),
                session_id: None,
            };
            state.store.record_finding(&vin, &finding).map_err(ApiError::from)?;
        }
        None => {
            state
                .with_service(move |s| {
                    s.record_finding(
                        &body.subject,
                        outcome,
                        &body.claim,
                        &body.evidence,
                        &authority,
                    )
                })
                .await??;
        }
    }
    Ok(Json(json!({ "recorded": true })))
}

/// What on this vehicle is no longer how the factory built it.
///
/// A POST because it reads every module the as-built file describes, which is
/// real traffic on a vehicle bus rather than a lookup.
async fn compare_to_factory(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service_named("a comparison against the factory configuration", |s| {
                s.compare_to_factory("user:api")
            })
            .await?,
    ))
}

/// Everything a person would be asked for when reporting a problem, gathered in
/// one place so nobody has to be talked through finding it.
///
/// A read: it assembles what this machine already knows and sends nothing
/// anywhere. What happens to the text afterwards is the person's decision.
async fn support_report() -> Json<Value> {
    Json(serde_json::to_value(crate::support::report()).unwrap_or(Value::Null))
}

/// Open the log folder in the desktop's own file manager.
///
/// A POST because it starts a program, even though it reads nothing and changes
/// nothing. It takes no path: the folder is this process's own.
async fn support_reveal() -> ApiResult<Json<Value>> {
    match crate::support::reveal_logs() {
        Ok(dir) => Ok(Json(serde_json::json!({ "opened": dir }))),
        Err(e) => Err(ApiError::bad_request(e)),
    }
}

#[derive(Debug, Deserialize)]
struct CaptureBody {
    /// Module diagnostic request address, e.g. 1830 for 0x726.
    module: String,
    /// Data identifiers to read.
    identifiers: Vec<u16>,
    #[serde(default)]
    label: Option<String>,
}

/// Read a set of configuration records off one module.
///
/// The first half of turning an unmapped feature into a mapped one. Read-only.
async fn capture_configuration(
    State(state): State<AppState>,
    Json(body): Json<CaptureBody>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| {
                s.capture_configuration(&body.module, &body.identifiers, body.label, "user:api")
            })
            .await?,
    ))
}

/// Find out whether a module accepts writes, without writing anything.
///
/// Issues `WriteDataByIdentifier` to an identifier verified absent on that
/// module immediately before, so there is nowhere for it to land. The refusal
/// is the measurement. A person confirms this; an agent may never initiate it.
async fn probe_write_gate(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<ClearBody>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(
        state
            .with_service(move |s| {
                s.probe_write_gate(&key, "user:api", Some(body.confirmation.as_str()))
            })
            .await?,
    ))
}

/// Community signal definitions that might apply to this vehicle.
///
/// A lookup: this touches no vehicle. Everything it returns is somebody else's
/// recorded claim, and an empty answer is the common case.
async fn catalog_signals(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.list_catalog_signals("user:api")).await?))
}

/// Ask the vehicle one community-defined signal and report what it answered.
///
/// `POST` rather than `GET` because it puts a request on the bus, even though
/// that request is a read.
async fn read_catalog_signal(
    State(state): State<AppState>,
    Path(signal): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_catalog_signal(&signal, "user:api")).await?))
}

/// Captures already stored for this vehicle, newest first.
///
/// The index that makes a baseline findable later, rather than something the
/// caller had to have kept a copy of.
async fn list_captures(State(state): State<AppState>) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(|s| s.list_captures("user:api")).await?))
}

#[derive(Debug, Deserialize)]
struct DiffBody {
    /// The earlier capture, inline.
    before: Option<aim_diagnostics::capture::ConfigCapture>,
    /// The later capture, inline.
    after: Option<aim_diagnostics::capture::ConfigCapture>,
    /// The earlier capture, by stored id. The usual way once a baseline has
    /// been taken on an earlier day.
    before_id: Option<String>,
    /// The later capture, by stored id.
    after_id: Option<String>,
    /// Whether the later capture is the one with the setting enabled. Stated by
    /// whoever flipped the switch, because the bytes do not say.
    #[serde(default)]
    after_is_on: bool,
}

/// Compare two captures and, when exactly one byte moved, propose the mapping.
///
/// Captures may be given inline or by stored id. The id form is what makes the
/// loop work across days: take a baseline, change the setting with the
/// vehicle's own controls whenever that happens, capture again, then compare —
/// without anybody having kept a JSON blob in a text file in between.
///
/// The comparison itself touches no vehicle.
async fn diff_captures(
    State(state): State<AppState>,
    Json(body): Json<DiffBody>,
) -> ApiResult<Json<Value>> {
    fn resolve(
        state: &AppState,
        inline: Option<aim_diagnostics::capture::ConfigCapture>,
        id: Option<String>,
        which: &str,
    ) -> ApiResult<aim_diagnostics::capture::ConfigCapture> {
        if let Some(c) = inline {
            return Ok(c);
        }
        let Some(id) = id else {
            return Err(aim_types::AimError::new(
                aim_types::ErrorCode::BadRequest,
                format!("the {which} capture must be given either inline or as {which}_id"),
            )
            .into());
        };
        let stored = state.store.capture(&id)?;
        serde_json::from_value(stored.capture).map_err(|e| {
            aim_types::AimError::new(
                aim_types::ErrorCode::StorageError,
                format!("the stored {which} capture could not be read back: {e}"),
            )
            .into()
        })
    }

    let before = resolve(&state, body.before, body.before_id, "before")?;
    let after = resolve(&state, body.after, body.after_id, "after")?;
    let after_is_on = body.after_is_on;
    let d = aim_diagnostics::capture::diff(&before, &after);
    let proposal = d.is_unambiguous().then(|| d.changes[0].as_mapping(&before.module, after_is_on));
    Ok(Json(json!({
        "diff": d,
        "comparable": d.is_comparable(),
        "unambiguous": d.is_unambiguous(),
        "proposed_mapping": proposal,
        "note": if proposal.is_some() {
            "One byte moved. This mapping describes only the bits that changed. Verify it by \
             reading the feature back, and record where it came from before relying on it."
        } else if !d.is_comparable() {
            "These captures are not comparable - they hold different identifiers or different \
             record lengths, which usually means they came from different modules or vehicles."
        } else if d.changes.is_empty() {
            "Nothing moved between these captures. Either the setting was not changed, or it \
             does not live in the identifiers that were read."
        } else {
            "More than one byte moved, so which one holds this setting is not established. \
             Repeat the capture changing only the one setting."
        },
    })))
}

// -------------------------------------------------------- profile import

/// A profile offered for import: its text, or a URL to fetch it from.
#[derive(Debug, Deserialize)]
struct ImportBody {
    /// The profile itself, pasted or read from a file by the client.
    #[serde(default)]
    text: Option<String>,
    /// Where to fetch it from instead.
    #[serde(default)]
    url: Option<String>,
    /// A name for the record, when the client knows one.
    #[serde(default)]
    source: Option<String>,
}

/// Resolve a body into the profile text and a label for where it came from.
///
/// The fetch is deliberately plain: `https` only, no redirects followed across
/// hosts by us, a size ceiling, and no authentication ever attached. A profile
/// is public data by definition — anything needing a credential to read is not
/// something this should be fetching on somebody's behalf.
async fn profile_text(body: &ImportBody) -> Result<(String, String), ApiError> {
    if let Some(text) = &body.text {
        let source = body.source.clone().unwrap_or_else(|| String::from("pasted"));
        return Ok((text.clone(), source));
    }
    let Some(url) = &body.url else {
        return Err(ApiError::bad_request(
            "send either the profile text or a url to fetch it from",
        ));
    };
    if !url.starts_with("https://") {
        return Err(ApiError::bad_request(
            "profiles are fetched over https only, so that what arrives is what was published",
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ApiError::bad_request(format!("could not build an http client: {e}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::bad_request(format!("could not fetch {url}: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "{url} answered {} rather than the profile",
            response.status()
        )));
    }
    // Checked against the declared length first where there is one, and again
    // after reading, because a declared length is a claim by the server.
    if response
        .content_length()
        .is_some_and(|n| n as usize > aim_decoders::import::MAX_PROFILE_BYTES)
    {
        return Err(ApiError::bad_request("that file is far too large to be a profile"));
    }
    let text = response
        .text()
        .await
        .map_err(|e| ApiError::bad_request(format!("could not read {url}: {e}")))?;
    if text.len() > aim_decoders::import::MAX_PROFILE_BYTES {
        return Err(ApiError::bad_request("that file is far too large to be a profile"));
    }
    Ok((text, body.source.clone().unwrap_or_else(|| url.clone())))
}

/// `POST /api/v1/profiles/preview`
///
/// What importing this would add and what it would override. Changes nothing.
async fn preview_profile(
    State(state): State<AppState>,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<Value>> {
    let (text, source) = profile_text(&body).await?;
    let preview = aim_decoders::import::preview(&text, &source, &state.decoders.features);
    Ok(Json(json!({ "preview": preview })))
}

/// `POST /api/v1/profiles/import`
///
/// Accept a profile, after previewing it. Written to the profiles directory
/// and loaded on the next start, which is deliberate: swapping definitions
/// under a live session would change what a reading means halfway through one.
async fn import_profile(
    State(state): State<AppState>,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<Value>> {
    let Some(dir) = state.config.profiles_dir.clone() else {
        return Err(ApiError::bad_request(
            "this build has no profiles directory, so there is nowhere to put an imported file",
        ));
    };
    let (text, source) = profile_text(&body).await?;
    let preview = aim_decoders::import::preview(&text, &source, &state.decoders.features);
    if !preview.acceptable {
        return Err(ApiError::bad_request(format!(
            "this profile was not accepted: {}",
            preview
                .findings
                .iter()
                .filter(|f| f.severity == aim_decoders::import::Severity::Blocking)
                .map(|f| f.detail.clone())
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }

    // Every claim the file makes about having been verified is removed before
    // it is written, so there is no window in which a trusted-looking
    // definition exists on disk. Verification is re-earned on this vehicle.
    let mut sanitised = aim_decoders::FeatureCatalog::default();
    sanitised
        .load_yaml(&text, &source)
        .map_err(|e| ApiError::bad_request(format!("could not read the profile: {}", e.message)))?;
    aim_decoders::import::strip_verification_claims(&mut sanitised);

    let yaml = serde_yaml_ng::to_string(&serde_json::json!({
        "version": 1,
        "profile": "imported",
        "features": sanitised.all().collect::<Vec<_>>(),
    }))
    .map_err(|e| ApiError::bad_request(format!("could not re-serialise the profile: {e}")))?;

    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::bad_request(format!("could not create {}: {e}", dir.display())))?;
    let name = format!("imported-{}.yaml", safe_stem(&source));
    let path = dir.join(&name);
    std::fs::write(&path, yaml.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("could not write {}: {e}", path.display())))?;

    Ok(Json(json!({
        "imported": true,
        "path": path.display().to_string(),
        "features": preview.changes.len(),
        "preview": preview,
        "note": "Imported unverified, whatever the file claimed about itself. It takes effect \
                 when the app next starts: swapping definitions under a live session would \
                 change what a reading means halfway through one.",
    })))
}

/// A filename fragment that cannot escape the profiles directory.
///
/// Everything that is not a letter, digit or dash becomes a dash. A source is
/// a URL or a filename somebody else chose, and it is never allowed to decide
/// where a file lands.
fn safe_stem(source: &str) -> String {
    let cleaned: String =
        source.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let trimmed = cleaned.trim_matches('-');
    let stem = if trimmed.is_empty() { "profile" } else { trimmed };
    stem.chars().take(60).collect()
}

// ------------------------------------------------------------ procedures

/// Every guided procedure this build can walk somebody through.
///
/// Includes the ones it will refuse, with the reason. A person wondering why
/// there is no road-speed test deserves to see that it exists and what it
/// would take, rather than finding an absence.
async fn list_procedures(State(_state): State<AppState>) -> ApiResult<Json<Value>> {
    let all: Vec<Value> = aim_diagnostics::Procedure::built_in()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "purpose": p.purpose,
                "hold_seconds": p.hold_seconds,
                "safety_notes": p.safety_notes,
                "measures": p.measure,
                "instructions": p.conditions.iter().map(|c| c.instruction()).collect::<Vec<_>>(),
                "can_run_alone": p.safe_for_one_person(),
                "why_not_alone": p.why_not_alone(),
            })
        })
        .collect();
    Ok(Json(json!({ "procedures": all })))
}

/// Where the vehicle is against what a procedure needs. Read-only.
async fn check_procedure(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.check_procedure(&id, "user:api")).await?))
}

/// Take the readings, having confirmed the conditions hold.
async fn run_procedure(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.run_procedure(&id, "user:api")).await?))
}
