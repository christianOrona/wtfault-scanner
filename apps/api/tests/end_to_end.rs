//! The whole stack, over HTTP and WebSocket, against the simulator.
//!
//! This is the handoff's "definition of done" as a test: connect → identify
//! the vehicle → enumerate what it supports → read its codes → stream live data
//! → then prove every one of those steps landed in the SQLite flight recorder.
//!
//! It runs the real axum server on a real ephemeral socket with a real
//! file-backed database. Nothing between the HTTP client and the virtual ECUs
//! is stubbed: the requests go through the router, the diagnostic service, the
//! safety gate, the ELM327 adapter, the ELM327 wire protocol and the virtual
//! ECUs, and the answers come back decoded with provenance.

use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use aim_types::{EventKind, SessionId};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

struct Harness {
    base: String,
    ws_base: String,
    client: reqwest::Client,
    store: SessionStore,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start(scenario: ScenarioId) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open(dir.path().join("session.sqlite")).unwrap();
        let decoders = DecoderSet::generic_obd().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: scenario,
            personality: PersonalityChoice::Clone,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            // A temp path: the test must never read or write the real
            // provider settings, which hold API keys.
            settings_path: dir.path().join("providers.json"),
        };
        let app = aim_api::router(AppState::new(store.clone(), decoders, config));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Harness {
            base: format!("http://{addr}/api/v1"),
            ws_base: format!("ws://{addr}/api/v1"),
            client: reqwest::Client::new(),
            store,
            _dir: dir,
        }
    }

    async fn get(&self, path: &str) -> Value {
        let r = self.client.get(format!("{}{path}", self.base)).send().await.unwrap();
        let status = r.status();
        let body: Value = r.json().await.unwrap();
        assert!(status.is_success(), "GET {path} -> {status}: {body}");
        body
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status();
        let value: Value = r.json().await.unwrap();
        assert!(status.is_success(), "POST {path} -> {status}: {value}");
        value
    }

    async fn post_raw(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    async fn get_status(&self, path: &str) -> u16 {
        self.client.get(format!("{}{path}", self.base)).send().await.unwrap().status().as_u16()
    }
}

/// Assert a tool endpoint answered with a successful envelope.
fn ok(result: &Value, what: &str) {
    assert_eq!(result["success"], true, "{what} failed: {}", result["error"]);
}

#[tokio::test]
async fn the_full_read_only_diagnostic_flow_over_http_and_websocket() {
    let h = Harness::start(ScenarioId::DpfRegen).await;

    // ---- the server describes itself before anything is connected --------
    let health = h.get("/health").await;
    assert_eq!(health["api_version"], "v1");
    assert_eq!(health["active"], Value::Null);
    assert!(health["scenarios"].as_array().unwrap().len() >= 3);

    // Vehicle operations before connecting are refused with a specific,
    // actionable code rather than an empty success.
    let (status, body) = h.post_raw("/vehicles/identify", json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["code"], "no_active_session");

    // ---- connect ---------------------------------------------------------
    let connect = h.post("/adapter/connect", json!({ "label": "end to end" })).await;
    ok(&connect, "connect");
    assert_eq!(connect["data"]["state"]["state"], "ready");
    assert_eq!(connect["data"]["protocol_label"], "ISO 15765-4 CAN 11/500");
    // The cheap-clone adapter's limits are surfaced, not buried.
    let caveats: Vec<&str> = connect["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(caveats.contains(&"adapter_caveat"));

    let session_id =
        SessionId::from_string(h.get("/adapter").await["session_id"].as_str().unwrap().to_string());

    // ---- identify the vehicle -------------------------------------------
    let identify = h.post("/vehicles/identify", json!({})).await;
    ok(&identify, "identify_vehicle");
    assert_eq!(identify["data"]["vin"], aim_simulator::SIMULATED_VIN);
    assert_eq!(identify["data"]["vin_decoded"]["check_digit_valid"], true);
    assert_eq!(identify["data"]["vin_decoded"]["model_year"], 2019);
    assert_eq!(identify["data"]["calibration_ids"][0], "SIMULATED-CAL-01");
    // Every value points back at the adapter exchange that produced it.
    let vin_value = &identify["values"][0];
    assert!(vin_value["provenance"]["evidence_ref"].is_i64());
    assert!(!vin_value["provenance"]["raw_hex"].as_str().unwrap().is_empty());

    // ---- scan modules ----------------------------------------------------
    let scan = h.post("/modules", json!({})).await;
    ok(&scan, "scan_modules");
    let modules: Vec<&str> = scan["data"]["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["module_key"].as_str().unwrap())
        .collect();
    assert_eq!(modules, vec!["ECU_7E8", "ECU_7EA", "ECU_7EB"]);

    // ---- scan supported PIDs --------------------------------------------
    let signals = h.get("/modules/ECU_7E8/signals").await;
    ok(&signals, "read_supported_pids");
    let pids = signals["data"]["pids"].as_array().unwrap();
    assert!(pids.len() > 30, "expected a full mask walk, got {}", pids.len());
    let signal_ids: Vec<&str> = pids.iter().filter_map(|p| p["signal_id"].as_str()).collect();
    assert!(signal_ids.contains(&"engine_rpm"));
    assert!(signal_ids.contains(&"dpf_temp_bank1_inlet"));

    // ---- read DTCs -------------------------------------------------------
    let dtcs = h.get("/modules/ECU_7E8/dtcs").await;
    ok(&dtcs, "read_dtcs");
    let codes: Vec<&str> = dtcs["data"]["dtcs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec!["P2463", "P242F", "P2002", "P2463"]);
    assert_eq!(dtcs["data"]["confirmed_count"], 2);
    // Descriptions come from the catalog; none is invented.
    for d in dtcs["data"]["dtcs"].as_array().unwrap() {
        assert!(d["description"].is_string(), "{d} has no description");
        assert!(!d["structural_summary"].as_str().unwrap().is_empty());
    }

    // ---- freeze frame ----------------------------------------------------
    let frame = h.get("/modules/ECU_7E8/freeze-frame?frame=0").await;
    ok(&frame, "read_freeze_frame");
    assert_eq!(frame["data"]["dtc"], "P2463");
    assert!(!frame["values"].as_array().unwrap().is_empty());

    // ---- stream live data over the websocket -----------------------------
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{}/live", h.ws_base)).await.unwrap();

    let hello: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
    assert_eq!(hello["type"], "hello");

    socket
        .send(Message::Text(
            json!({
                "type": "subscribe",
                "module": "ECU_7E8",
                "signals": ["engine_rpm", "coolant_temp", "dpf_temp_bank1_inlet"],
                "interval_ms": 50
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let subscribed: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
    assert_eq!(subscribed["type"], "subscribed");
    assert_eq!(subscribed["interval_ms"], 50);

    let mut rpm_samples = Vec::new();
    for _ in 0..4 {
        let message: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
        assert_eq!(message["type"], "sample", "{message}");
        let result = &message["result"];
        ok(result, "live sample");
        assert_eq!(result["values"].as_array().unwrap().len(), 3);

        let rpm = result["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["signal_id"] == "engine_rpm")
            .unwrap()["value"]["value"]
            .as_f64()
            .unwrap();
        rpm_samples.push(rpm);

        // The unverified DPF decoder taints its value all the way out to the
        // websocket, which is the whole point of carrying verification status.
        let warnings: Vec<&str> = result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["code"].as_str().unwrap())
            .collect();
        assert!(
            warnings.contains(&"unverified_decoder"),
            "the DPF temperature is unverified and must say so: {warnings:?}"
        );
    }
    // The truck is at a raised idle during a regeneration, and the values move.
    assert!(rpm_samples.iter().all(|r| (900.0..1300.0).contains(r)));
    assert!(
        rpm_samples.windows(2).any(|w| w[0] != w[1]),
        "live values should change between samples: {rpm_samples:?}"
    );

    socket.send(Message::Text(json!({"type":"unsubscribe"}).to_string().into())).await.unwrap();
    let unsub: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
    assert_eq!(unsub["type"], "unsubscribed");
    drop(socket);

    // ---- the write path stays shut ---------------------------------------
    let (status, cleared) = h
        .post_raw(
            "/tools/clear_dtcs",
            json!({
                "arguments": { "module": "ECU_7E8" },
                "initiator": "agent:planner",
                "confirmation": "the-owner"
            }),
        )
        .await;
    assert_eq!(status, 200, "the tool ran and refused; that is a 200 envelope");
    assert_eq!(cleared["success"], false);
    assert_eq!(
        cleared["error"]["code"], "permission_level_disabled",
        "an agent must never be able to clear codes, even though a person can"
    );
    // The codes are still there.
    let after = h.get("/modules/ECU_7E8/dtcs").await;
    assert_eq!(after["data"]["confirmed_count"], 2);

    // =====================================================================
    // Everything above must now be in the SQLite flight recorder.
    // =====================================================================
    let store = &h.store;

    let session = store.get_session(&session_id).unwrap();
    assert_eq!(session.label.as_deref(), Some("end to end"));

    // Vehicle
    let vehicle = store.get_vehicle(session.vehicle_id.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(vehicle.vin.as_deref(), Some(aim_simulator::SIMULATED_VIN));
    assert_eq!(vehicle.year, Some(2019));
    assert!(vehicle.model.is_none(), "model must never be inferred");

    // Connection, with the capability snapshot taken at the time.
    let connections = store.connections(&session_id).unwrap();
    assert_eq!(connections.len(), 1);
    assert!(connections[0].capabilities.elm327_compatible);
    assert!(!connections[0].capabilities.caveats.is_empty());

    // Modules
    let stored_modules = store.modules(&session_id).unwrap();
    assert_eq!(stored_modules.len(), 3);
    assert_eq!(
        stored_modules.iter().find(|m| m.module_key == "ECU_7E8").unwrap().name,
        "SIM ENGINE CONTROL"
    );

    // DTCs: four distinct (code, status) observations.
    let stored_dtcs = store.dtcs(&session_id, None).unwrap();
    assert_eq!(stored_dtcs.len(), 4);
    assert!(stored_dtcs.iter().all(|d| d.description.is_some()));

    // Measurements: every live sample was recorded with its raw bytes.
    let rpm_rows = store.measurements(&session_id, Some("engine_rpm"), 1000).unwrap();
    assert!(
        rpm_rows.len() >= 4,
        "expected the streamed samples to be recorded, got {}",
        rpm_rows.len()
    );
    for row in &rpm_rows {
        assert!(row.value.is_some());
        assert_eq!(row.unit.as_deref(), Some("rpm"));
        assert!(!row.raw_value.is_empty(), "a measurement must keep the bytes it was decoded from");
    }

    // Events: the complete, ordered, gap-free trace.
    let events = store.events_since(&session_id, 0, 100_000).unwrap();
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as i64 + 1, "the event log must be gap-free");
    }
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.name()).collect();
    for expected in [
        "session_started",
        "connection_state_changed",
        "adapter_identified",
        "adapter_request",
        "adapter_response",
        "tool_invoked",
        "tool_completed",
        "safety_decision",
        "vehicle_identified",
        "module_discovered",
        "dtc_read",
        "measurement_recorded",
    ] {
        assert!(kinds.contains(&expected), "no {expected} event was recorded");
    }

    // The refusal is in the trace, with who asked for it.
    let refusal = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::SafetyDecision { operation, allowed, initiator, reason, .. }
                if operation == "obd2.clear_dtcs" =>
            {
                Some((*allowed, initiator.clone(), reason.clone()))
            }
            _ => None,
        })
        .expect("the refused clear_dtcs must be recorded");
    assert!(!refusal.0);
    assert_eq!(refusal.1, "agent:planner");
    assert_eq!(refusal.2.as_deref(), Some("permission_level_disabled"));

    // A decoded value's evidence_ref resolves to the exact adapter exchange.
    let evidence_ref = identify["values"][0]["provenance"]["evidence_ref"].as_i64().unwrap();
    match store.event_by_id(evidence_ref).unwrap().kind {
        EventKind::AdapterResponse { command, lines, .. } => {
            assert_eq!(command, "0902", "the VIN request");
            assert!(!lines.is_empty());
        }
        other => panic!("evidence_ref pointed at {other:?}"),
    }

    // ---- and the history is readable back over HTTP ----------------------
    let listed = h.get("/sessions").await;
    let summary = &listed["sessions"][0];
    assert_eq!(summary["session"]["id"], session_id.as_str());
    assert_eq!(summary["module_count"], 3);
    assert_eq!(summary["dtc_count"], 4);
    assert!(summary["measurement_count"].as_i64().unwrap() >= 4);
    assert_eq!(summary["vehicle"]["vin"], aim_simulator::SIMULATED_VIN);

    let detail = h.get(&format!("/sessions/{session_id}")).await;
    assert_eq!(detail["modules"].as_array().unwrap().len(), 3);
    assert_eq!(detail["dtcs"].as_array().unwrap().len(), 4);
    assert_eq!(detail["event_count"].as_i64().unwrap(), events.len() as i64);

    let page = h.get(&format!("/sessions/{session_id}/events?after_seq=0&limit=5")).await;
    assert_eq!(page["events"].as_array().unwrap().len(), 5);
    assert_eq!(page["events"][0]["seq"], 1);
    assert_eq!(page["total"].as_i64().unwrap(), events.len() as i64);

    // ---- disconnect closes the record ------------------------------------
    let disconnect = h.post("/adapter/disconnect", json!({})).await;
    ok(&disconnect, "disconnect");
    assert!(store.connections(&session_id).unwrap()[0].disconnected_at.is_some());
    assert!(store.get_session(&session_id).unwrap().ended_at.is_some());
}

#[tokio::test]
async fn the_session_event_websocket_replays_history_then_follows_live() {
    let h = Harness::start(ScenarioId::Healthy).await;
    ok(&h.post("/adapter/connect", json!({})).await, "connect");
    let session_id = h.get("/adapter").await["session_id"].as_str().unwrap().to_string();

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{}/sessions/{session_id}/stream", h.ws_base))
            .await
            .unwrap();

    // Backlog first, terminated by hello.
    let mut backlog = 0usize;
    let from_seq = loop {
        let message: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
        match message["type"].as_str().unwrap() {
            "event" => backlog += 1,
            "hello" => break message["from_seq"].as_i64().unwrap(),
            other => panic!("unexpected message type {other}"),
        }
    };
    assert!(backlog > 5, "the connect trace should be replayed");
    assert_eq!(from_seq, backlog as i64);

    // Now cause something and watch it arrive live.
    let scan = h.post("/modules", json!({})).await;
    ok(&scan, "scan_modules");

    let mut live_kinds = Vec::new();
    for _ in 0..12 {
        let message: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
        assert_eq!(message["type"], "event");
        let seq = message["event"]["seq"].as_i64().unwrap();
        assert!(seq > from_seq, "live events must continue the sequence");
        live_kinds.push(message["event"]["kind"]["kind"].as_str().unwrap().to_string());
    }
    assert!(
        live_kinds.iter().any(|k| k == "tool_invoked"),
        "expected the scan to stream through: {live_kinds:?}"
    );
}

#[tokio::test]
async fn a_silent_bus_is_reported_as_degraded_rather_than_as_a_working_truck() {
    let h = Harness::start(ScenarioId::BusSilent).await;

    let connect = h.post("/adapter/connect", json!({})).await;
    ok(&connect, "connect");
    assert_eq!(connect["data"]["state"]["state"], "degraded");
    let severities: Vec<&str> = connect["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["severity"].as_str().unwrap())
        .collect();
    assert!(severities.contains(&"serious"));

    // And a read fails with the vehicle's silence, carrying capability state.
    let scan = h.post("/modules", json!({})).await;
    assert_eq!(scan["success"], false);
    assert_eq!(scan["error"]["code"], "vehicle_not_responding");
    assert!(
        scan["error"]["capability_state"]["elm327_compatible"] == true,
        "the error must carry what the adapter is known to be"
    );
}

#[tokio::test]
async fn connecting_twice_is_refused_so_a_live_session_is_never_orphaned() {
    let h = Harness::start(ScenarioId::Healthy).await;
    ok(&h.post("/adapter/connect", json!({})).await, "connect");

    let (status, body) = h.post_raw("/adapter/connect", json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["code"], "adapter_busy");
}

#[tokio::test]
async fn a_scenario_can_be_chosen_per_connection() {
    let h = Harness::start(ScenarioId::Healthy).await;
    let connect = h.post("/adapter/connect", json!({ "scenario": "dpf-regen" })).await;
    ok(&connect, "connect");
    h.post("/modules", json!({})).await;
    let dtcs = h.get("/modules/ECU_7E8/dtcs").await;
    assert_eq!(dtcs["data"]["confirmed_count"], 2, "the regen scenario was used");

    let (status, body) = h.post_raw("/adapter/disconnect", json!({})).await;
    assert_eq!(status, 200, "{body}");

    let (status, body) =
        h.post_raw("/adapter/connect", json!({ "scenario": "not-a-scenario" })).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn unbuilt_features_answer_with_a_reason_rather_than_a_404() {
    let h = Harness::start(ScenarioId::Healthy).await;

    // The agent exists now, but no model provider is configured in a fresh
    // profile. That must say so rather than looking like a broken endpoint.
    let (status, body) = h
        .post_raw(
            "/agent/messages",
            json!({ "messages": [{ "role": "user", "content": "what is wrong" }] }),
        )
        .await;
    assert_eq!(status, 409, "no configured provider should be a precondition failure");
    assert_eq!(body["error"]["details"]["agent_code"], "no_provider");

    let (status, body) = h.post_raw("/modules/ECU_7E8/tests/turbo_actuator/run", json!({})).await;
    assert_eq!(status, 501);
    assert_eq!(body["error"]["code"], "not_implemented");

    // A genuinely unknown path is still a 404.
    assert_eq!(h.get_status("/does-not-exist").await, 404);
}

#[tokio::test]
async fn the_tool_registry_is_published_for_the_future_agent() {
    let h = Harness::start(ScenarioId::Healthy).await;
    let tools = h.get("/tools").await;

    let enabled: Vec<&str> =
        tools["enabled"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert!(enabled.contains(&"read_dtcs"));
    assert!(!enabled.contains(&"clear_dtcs"));

    // Plain JSON Schema, no vendor wrapper.
    let read_pid =
        tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == "read_pid").unwrap();
    assert_eq!(read_pid["parameters"]["type"], "object");
    assert_eq!(read_pid["parameters"]["additionalProperties"], false);
    assert_eq!(read_pid["permission_level"], "L0");

    let capabilities = h.get("/capabilities").await;
    // L2 is configuration writes, which this build performs behind a typed
    // confirmation. L3 (programming) stays off.
    assert_eq!(capabilities["max_enabled_level"], "L2");
    let disabled: Vec<&str> = capabilities["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["enabled"] == false)
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    // clear_dtcs is enabled as a *capability* - a person may do it - and
    // disabled as a *tool*, so the agent may not. Checked below on the tool
    // list rather than here.
    assert!(disabled.contains(&"program.program_module"));
}

async fn next_text<S>(socket: &mut S) -> String
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("timed out waiting for a websocket message")
        {
            Some(Ok(Message::Text(t))) => return t.to_string(),
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            other => panic!("unexpected websocket frame: {other:?}"),
        }
    }
}
