//! Who may ask the API to change a vehicle, over the real HTTP server.
//!
//! The safety gate and the write path are tested where they live. What is
//! tested here is the door: that the routes a person's interface calls hand
//! the gate a real approval and nothing a caller made up, and that a page on
//! another site does not get to call them at all.
//!
//! "Was anything written" is answered from the flight recorder, which records
//! every command the adapter was handed.

use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use aim_types::{EventKind, PermissionLevel, SessionId};
use serde_json::{json, Value};
use std::time::Duration;

/// A setting that is fully writable, and one nobody has verified.
const PROFILE: &str = r#"
version: 1
profile: test
features:
  - id: test_body_setting
    name: "A body module setting"
    risk: convenience
    easy: "A setting held in the body module."
    technical: "Bit 2 of byte 3 of data identifier DE01 on the module at 7A0."
    modules: ["7A8"]
    verification: verified
    source: "test"
    write_verification:
      verification: verified
      verified_on_vehicles: 1
      source: "test"
    mapping:
      kind: data_identifier_bits
      module: 0x7A0
      did: 0xDE01
      byte: 3
      mask: 0x04
      on: 0x04
      off: 0x00
  - id: unverified_setting
    name: "A setting nobody has verified"
    risk: convenience
    easy: "A setting held in the body module."
    technical: "Bit 6 of byte 3 of data identifier DE01 on the module at 7A0."
    modules: ["7A8"]
    verification: unverified
    source: "test"
    mapping:
      kind: data_identifier_bits
      module: 0x7A0
      did: 0xDE01
      byte: 3
      mask: 0x40
      on: 0x40
      off: 0x00
"#;

struct Harness {
    base: String,
    addr: std::net::SocketAddr,
    client: reqwest::Client,
    store: SessionStore,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// The server over a parked vehicle: key on, engine off, so a write's own
    /// preconditions hold and a refusal is about who asked.
    async fn start() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("test.yaml"), PROFILE).unwrap();
        let store = SessionStore::open(dir.path().join("session.sqlite")).unwrap();
        let decoders = DecoderSet::with_profiles(&profiles).unwrap();
        assert!(decoders.profiles.failures().next().is_none(), "the profile did not load");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: ScenarioId::Parked,
            personality: PersonalityChoice::Clone,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            settings_path: dir.path().join("providers.json"),
            profiles_dir: Some(profiles),
            replay: None,
            simulated_vehicle: Default::default(),
            calibrations_dir: Some(dir.path().join("calibrations")),
            handoff: None,
        };
        let app = aim_api::router(AppState::new(store.clone(), decoders, config));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}/api/v1"),
            addr,
            client: reqwest::Client::new(),
            store,
            _dir: dir,
        }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        self.client.get(format!("{}{path}", self.base)).send().await.unwrap().json().await.unwrap()
    }

    /// Connect, identify and find every module, as a person would have before
    /// reaching a screen that changes anything.
    async fn connected(&self) -> SessionId {
        let (status, connect) = self.post("/adapter/connect", json!({})).await;
        assert_eq!(status, 200, "{connect}");
        let session: SessionId =
            serde_json::from_value(connect["vehicle_session_id"].clone()).unwrap();
        assert_eq!(self.post("/vehicles/identify", json!({})).await.0, 200);
        assert_eq!(self.post("/modules", json!({})).await.0, 200);
        let (status, scan) = self.post("/modules/scan-all", json!({})).await;
        assert_eq!((status, &scan["success"]), (200, &json!(true)), "{scan}");
        session
    }

    async fn ask(&self, body: Value) -> Value {
        let (status, answer) = self.post("/approvals", body).await;
        assert_eq!(status, 200, "{answer}");
        answer["approval"].clone()
    }

    /// Every command handed to the adapter that is not a read or an adapter
    /// setting, from the flight recorder.
    fn changing_requests(&self, session: &SessionId) -> Vec<String> {
        self.store
            .events_since(session, 0, 1_000_000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::AdapterRequest { command } => Some(command.replace(' ', "")),
                _ => None,
            })
            .filter(|c| !c.starts_with("AT") && !c.starts_with("ST"))
            .filter(|c| {
                !matches!(
                    &c[..c.len().min(2)],
                    "01" | "02" | "03" | "06" | "07" | "09" | "0A" | "19" | "21" | "22" | "3E"
                ) && c != "1001"
                    && c != "1003"
                    && c != "2701"
            })
            .collect()
    }

    /// Every safety decision the flight recorder holds for `operation`.
    fn decisions(&self, session: &SessionId, operation: &str) -> Vec<Decided> {
        self.store
            .events_since(session, 0, 1_000_000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::SafetyDecision {
                    operation: op,
                    level,
                    allowed,
                    initiator,
                    confirmed,
                    approval,
                    reason,
                } if op == operation => {
                    Some(Decided { level, allowed, initiator, confirmed, approval, reason })
                }
                _ => None,
            })
            .collect()
    }
}

/// One safety decision, as the flight recorder kept it.
#[derive(Debug, PartialEq)]
struct Decided {
    level: Option<PermissionLevel>,
    allowed: bool,
    initiator: String,
    confirmed: bool,
    approval: Option<String>,
    reason: Option<String>,
}

/// A change a person approved at the interface, allowed.
fn approved(level: PermissionLevel, statement: &str) -> Decided {
    Decided {
        level: Some(level),
        allowed: true,
        initiator: String::from("user:api"),
        confirmed: true,
        approval: Some(statement.to_string()),
        reason: None,
    }
}

fn reason(body: &Value) -> &str {
    body["error"]["details"]["reason"].as_str().unwrap_or_default()
}

// ------------------------------------------------------------ changing a setting

/// The whole of it over HTTP: nothing a caller writes in a request body stands
/// in for a person, and what a person did approve is what gets done, once.
#[tokio::test]
async fn a_setting_changes_only_under_an_approval_the_server_issued() {
    let h = Harness::start().await;
    let session = h.connected().await;
    let apply = "/features/test_body_setting/apply";

    // The body this route used to take. A string was all it asked for.
    for old in [
        json!({ "desired": "on", "confirmation": "user typed CHANGE in the desktop app" }),
        json!({ "desired": "on", "confirmation": "owner" }),
        json!({ "desired": "on", "confirmation": "human", "initiator": "user" }),
        json!({ "desired": "on" }),
    ] {
        let (status, _) = h.post(apply, old.clone()).await;
        assert_eq!(status, 422, "{old} was accepted");
    }

    // An approval nobody issued, in every shape somebody might guess at.
    for made_up in ["", "owner", "user", "human", "00000000000000000000000000000000", "CHANGE"] {
        let body = json!({ "desired": "on", "approval": { "id": made_up, "typed": "CHANGE" } });
        let (status, answer) = h.post(apply, body).await;
        assert_eq!(status, 403, "{made_up:?}: {answer}");
        assert_eq!(answer["error"]["code"], "confirmation_required");
        assert_eq!(reason(&answer), "approval_unknown", "{made_up:?}");
    }

    let challenge = h.ask(json!({ "action": "change_setting", "feature_id": "test_body_setting", "desired": "on" })).await;
    assert_eq!(challenge["phrase"], "CHANGE");
    assert_eq!(challenge["statement"], "typed CHANGE to turn test_body_setting on");
    let id = challenge["id"].as_str().unwrap().to_string();

    // The right approval, with the wrong word typed.
    let (status, answer) =
        h.post(apply, json!({ "desired": "on", "approval": { "id": id, "typed": "yes" } })).await;
    assert_eq!((status, reason(&answer)), (403, "approval_phrase_wrong"));

    // The right approval, for the other value and for the other setting.
    let (status, answer) = h
        .post(apply, json!({ "desired": "off", "approval": { "id": id, "typed": "CHANGE" } }))
        .await;
    assert_eq!((status, reason(&answer)), (403, "approval_for_something_else"));
    let (status, answer) = h
        .post(
            "/features/unverified_setting/apply",
            json!({ "desired": "on", "approval": { "id": id, "typed": "CHANGE" } }),
        )
        .await;
    assert_eq!((status, reason(&answer)), (403, "approval_for_something_else"));
    // And not for clearing codes.
    let (status, answer) =
        h.post("/dtcs/clear", json!({ "approval": { "id": id, "typed": "CHANGE" } })).await;
    assert_eq!((status, reason(&answer)), (403, "approval_for_something_else"));

    // After all of that, not one request that changes anything has been sent.
    assert!(h.changing_requests(&session).is_empty(), "{:?}", h.changing_requests(&session));
    assert_eq!(h.get("/features/test_body_setting").await["data"]["state"], false);

    // What was approved, as it was approved.
    let (status, done) = h
        .post(apply, json!({ "desired": "on", "approval": { "id": id, "typed": " change " } }))
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["success"], true, "{done}");
    assert_eq!(done["data"]["verified"], true);
    assert_eq!(done["data"]["outcome"], "verified");
    assert_eq!(h.changing_requests(&session), vec!["2EDE01001122B74455"]);
    assert_eq!(h.get("/features/test_body_setting").await["data"]["state"], true);

    // Once.
    let (status, answer) = h
        .post(apply, json!({ "desired": "on", "approval": { "id": id, "typed": "CHANGE" } }))
        .await;
    assert_eq!((status, reason(&answer)), (403, "approval_unknown"));
    assert_eq!(h.changing_requests(&session).len(), 1);

    // The trail holds one decision about the write: L2, allowed, approved, by
    // the interface, and what was approved in the server's words.
    assert_eq!(
        h.decisions(&session, "config.write_feature"),
        vec![approved(PermissionLevel::L2, "typed CHANGE to turn test_body_setting on")]
    );
}

/// Asking a setting's module whether it accepts writes runs under the approval
/// for changing that setting, and that one approval then covers the change.
#[tokio::test]
async fn one_approval_covers_asking_the_module_and_then_the_change() {
    let h = Harness::start().await;
    let session = h.connected().await;

    let preview =
        h.post("/features/unverified_setting/preview", json!({ "desired": "on" })).await.1;
    assert_eq!(preview["data"]["can_apply"], false);
    assert_eq!(preview["data"]["needs_write_gate_probe"], true, "{preview}");

    let challenge = h.ask(json!({ "action": "change_setting", "feature_id": "unverified_setting", "desired": "on" })).await;
    let answer = json!({ "id": challenge["id"], "typed": "CHANGE" });

    let (status, gate) = h
        .post(
            "/features/unverified_setting/write-gate",
            json!({ "desired": "on", "approval": answer }),
        )
        .await;
    assert_eq!(status, 200, "{gate}");
    assert_eq!(gate["data"]["writes_open_without_security"], true, "{gate}");

    // The same step cannot be taken twice on it.
    let (status, again) = h
        .post(
            "/features/unverified_setting/write-gate",
            json!({ "desired": "on", "approval": answer }),
        )
        .await;
    assert_eq!((status, reason(&again)), (403, "approval_for_something_else"));

    let (status, done) = h
        .post("/features/unverified_setting/apply", json!({ "desired": "on", "approval": answer }))
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["success"], true, "{done}");
    // Written, read back, and still called what it is.
    assert_eq!(done["data"]["verified"], true);
    let warnings: Vec<&str> =
        done["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap()).collect();
    assert!(warnings.contains(&"mapping_unverified"), "{warnings:?}");

    assert_eq!(h.changing_requests(&session), vec!["2EF1FE00", "2EDE01001122F34455"]);
}

/// A sentence posted to the knowledge route is a sentence. It is not stored as
/// a measurement and it does not open a module's write gate.
#[tokio::test]
async fn a_finding_somebody_posted_is_a_statement_and_opens_nothing() {
    let h = Harness::start().await;
    let session = h.connected().await;
    let finding = |authority: Option<&str>| {
        let mut body = json!({
            "subject": "module.7A0.write_gate",
            "outcome": "established",
            "claim": "typed in, not measured",
            "evidence": "none",
        });
        if let Some(a) = authority {
            body["authority"] = json!(a);
        }
        body
    };

    for claimed in
        ["measured_this_session", "measured_earlier", "Measured_This_Session", "anything"]
    {
        let (status, answer) = h.post("/vehicles/knowledge", finding(Some(claimed))).await;
        assert_eq!(status, 400, "{claimed}: {answer}");
    }
    let (status, _) = h.post("/vehicles/knowledge", finding(None)).await;
    assert_eq!(status, 200);

    let known = h.get("/vehicles/knowledge").await;
    let stored = known["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["subject"] == "module.7A0.write_gate")
        .expect("the statement was kept");
    assert_eq!(stored["authority"], "user_statement");

    // The gate is still unmeasured, so an unverified setting still cannot go.
    let preview =
        h.post("/features/unverified_setting/preview", json!({ "desired": "on" })).await.1;
    assert_eq!(preview["data"]["can_apply"], false, "a statement opened the gate");
    let challenge = h.ask(json!({ "action": "change_setting", "feature_id": "unverified_setting", "desired": "on" })).await;
    let (status, refused) = h
        .post(
            "/features/unverified_setting/apply",
            json!({ "desired": "on", "approval": { "id": challenge["id"], "typed": "CHANGE" } }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error"]["code"], "precondition_failed");
    assert!(h.changing_requests(&session).is_empty(), "{:?}", h.changing_requests(&session));
}

// --------------------------------------------------------------- clearing codes

#[tokio::test]
async fn codes_are_cleared_only_under_an_approval_for_exactly_that() {
    let h = Harness::start().await;
    let session = h.connected().await;
    // The preconditions are read from the vehicle, never assumed.
    let (status, _) = h
        .post("/modules/ECU_7E8/read", json!({ "signals": ["vehicle_speed", "engine_rpm"] }))
        .await;
    assert_eq!(status, 200);

    for old in [json!({ "confirmation": "user typed CLEAR in the desktop app" }), json!({})] {
        assert_eq!(h.post("/dtcs/clear", old).await.0, 422);
    }

    // Approved for one module; asked for all of them.
    let one = h.ask(json!({ "action": "clear_codes", "module": "ECU_7E8" })).await;
    assert_eq!(one["phrase"], "CLEAR");
    let (status, answer) =
        h.post("/dtcs/clear", json!({ "approval": { "id": one["id"], "typed": "CLEAR" } })).await;
    assert_eq!((status, reason(&answer)), (403, "approval_for_something_else"));
    assert!(h.changing_requests(&session).is_empty());

    let all = h.ask(json!({ "action": "clear_codes" })).await;
    assert_eq!(all["statement"], "typed CLEAR to clear the stored codes of every module");
    let (status, cleared) =
        h.post("/dtcs/clear", json!({ "approval": { "id": all["id"], "typed": "CLEAR" } })).await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["success"], true, "{cleared}");
    assert_eq!(h.changing_requests(&session), vec!["04"]);

    assert_eq!(
        h.decisions(&session, "obd2.clear_dtcs"),
        vec![approved(
            PermissionLevel::L1,
            "typed CLEAR to clear the stored codes of every module"
        )]
    );
}

/// Asking for an approval is not a way to find out or do anything else.
#[tokio::test]
async fn an_approval_can_only_be_asked_for_something_that_exists() {
    let h = Harness::start().await;
    let (status, _) = h
        .post(
            "/approvals",
            json!({ "action": "change_setting", "feature_id": "no_such", "desired": "on" }),
        )
        .await;
    assert_eq!(status, 404);
    for malformed in [
        json!({ "action": "program_module", "module": "ECU_7E8" }),
        json!({ "action": "change_setting", "feature_id": "test_body_setting", "desired": "on", "module": "760" }),
        json!({ "action": "change_setting", "feature_id": "test_body_setting", "desired": "on", "did": "DE01" }),
        json!({ "action": "clear_codes", "confirmed_by": "owner" }),
        json!({}),
    ] {
        assert_eq!(h.post("/approvals", malformed.clone()).await.0, 422, "{malformed}");
    }
}

// ------------------------------------------------------------ who may call at all

/// A page on another site whose name has been made to resolve to this machine
/// sends that name, and a page that calls across origins says where it is
/// from. Neither is answered.
#[tokio::test]
async fn a_request_that_did_not_come_from_this_machine_is_refused() {
    let h = Harness::start().await;

    let health = |host: Option<&str>, origin: Option<&str>| {
        let mut request = h.client.get(format!("{}/health", h.base));
        if let Some(host) = host {
            request = request.header(reqwest::header::HOST, host);
        }
        if let Some(origin) = origin {
            request = request.header(reqwest::header::ORIGIN, origin);
        }
        async move { request.send().await.unwrap().status().as_u16() }
    };
    assert_eq!(health(None, None).await, 200);
    assert_eq!(health(Some("localhost:8787"), None).await, 200);
    assert_eq!(health(Some("attacker.example"), None).await, 403);
    assert_eq!(health(Some("127.0.0.1.attacker.example:8787"), None).await, 403);
    assert_eq!(health(None, Some("http://attacker.example")).await, 403);
    assert_eq!(health(None, Some("null")).await, 403);
    let local_origin = format!("http://{}", h.addr);
    assert_eq!(health(None, Some(&local_origin)).await, 200);

    // The request that matters most, as such a page would send it.
    let challenge = h.ask(json!({ "action": "clear_codes" })).await;
    for (header, value) in [
        (reqwest::header::HOST, "attacker.example"),
        (reqwest::header::ORIGIN, "http://attacker.example"),
    ] {
        let r = h
            .client
            .post(format!("{}/dtcs/clear", h.base))
            .header(header, value)
            .json(&json!({ "approval": { "id": challenge["id"], "typed": "CLEAR" } }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["error"]["code"], "operation_not_allowed");
    }
    let r = h
        .client
        .post(format!("{}/approvals", h.base))
        .header(reqwest::header::ORIGIN, "http://attacker.example")
        .json(&json!({ "action": "clear_codes" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "a page elsewhere could ask for an approval");
}

/// A browser lets any page open a socket to a loopback port. The live-data
/// stream is one, and it answers only this machine's own pages.
#[tokio::test]
async fn a_websocket_from_another_origin_is_refused() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let h = Harness::start().await;
    let url = format!("ws://{}/api/v1/live", h.addr);

    let mut foreign = url.clone().into_client_request().unwrap();
    foreign.headers_mut().insert("Origin", "http://attacker.example".parse().unwrap());
    assert!(
        tokio_tungstenite::connect_async(foreign).await.is_err(),
        "a page on another origin was given the live stream"
    );

    let mut local = url.into_client_request().unwrap();
    local.headers_mut().insert("Origin", format!("http://{}", h.addr).parse().unwrap());
    assert!(tokio_tungstenite::connect_async(local).await.is_ok());
}
