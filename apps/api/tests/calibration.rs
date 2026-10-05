//! Calibration discovery over HTTP, against the simulated 2023 Odyssey.
//!
//! The workflow as an interface drives it: read a module's software identity
//! from the vehicle, then look for a file that is that software. The second
//! half never touches the vehicle, and finding nothing is an answer.

use aim_api::state::{
    AppState, PersonalityChoice, ServerConfig, SimulatedVehicle, TransportChoice,
};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use serde_json::{json, Value};
use std::time::Duration;

struct Bench {
    base: String,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Bench {
    async fn start() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: ScenarioId::Healthy,
            personality: PersonalityChoice::Genuine,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            settings_path: dir.path().join("providers.json"),
            profiles_dir: None,
            replay: None,
            simulated_vehicle: SimulatedVehicle::Odyssey,
            calibrations_dir: Some(dir.path().join("calibrations")),
            handoff: None,
        };
        let store = SessionStore::open(dir.path().join("sessions.sqlite")).unwrap();
        let app = aim_api::router(AppState::new(store, DecoderSet::generic_obd().unwrap(), config));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Bench { base: format!("http://{addr}/api/v1"), client: reqwest::Client::new(), _dir: dir }
    }

    async fn get(&self, path: &str) -> Value {
        let r = self.client.get(format!("{}{path}", self.base)).send().await.unwrap();
        assert!(r.status().is_success(), "GET {path} -> {}", r.status());
        r.json().await.unwrap()
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status();
        let value: Value = r.json().await.unwrap();
        assert!(status.is_success(), "POST {path} -> {status}: {value}");
        value
    }

    /// Add a file the way the screen does: its name, and its bytes as the body.
    async fn add_file(&self, name: &str, bytes: &[u8]) -> (reqwest::StatusCode, Value) {
        let r = self
            .client
            .post(format!("{}/calibration/files", self.base))
            .query(&[("name", name)])
            .header("content-type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .unwrap();
        let status = r.status();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

fn source<'a>(sources: &'a Value, id: &str) -> &'a Value {
    sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id || s["source"]["id"] == id)
        .unwrap_or_else(|| panic!("no source called {id} in {sources}"))
}

#[tokio::test]
async fn a_modules_software_is_identified_and_a_file_is_judged_against_it() {
    let b = Bench::start().await;
    b.post("/adapter/connect", json!({})).await;
    b.post("/vehicles/identify", json!({})).await;
    b.post("/modules", json!({})).await;

    // ---- identify the ECU ------------------------------------------------
    let read = b.get("/modules/ECU_18DAF110/calibration").await;
    assert_eq!(read["success"], true, "{}", read["error"]);
    assert_eq!(read["tool"], "read_calibration_identity");
    let identity = read["data"]["identity"].clone();
    let calibration = &identity["fields"]["calibration_id"][0];
    assert_eq!(calibration["value"], "37805-5MR-C120");
    assert_eq!(calibration["source"]["kind"], "obd_info_type");
    assert!(calibration["evidence_ref"].is_i64());
    assert!(calibration["raw_hex"].as_str().unwrap().len() > 20);
    // Asked for and refused, with the refusal kept.
    let refused = identity["unanswered"].as_array().unwrap();
    assert!(refused.iter().any(|u| u["source"]["did"] == 0xF191
        && u["raw_hex"].as_str().is_some_and(|r| r.starts_with("7f22"))));

    // ---- where files are looked for ---------------------------------------
    let status = b.get("/calibration").await;
    assert_eq!(status["uses_network"], false);
    assert_eq!(status["writes_to_vehicle"], false);
    let sources = status["sources"].as_array().unwrap();
    assert!(sources.iter().all(|s| s["uses_network"] == false));
    // The person's folder, the kept files, and a manufacturer tool's folder,
    // each saying what kind of place it is.
    assert_eq!(source(&status["sources"], "folder")["kind"], "user_folder");
    assert_eq!(source(&status["sources"], "cache")["kind"], "kept");
    assert_eq!(source(&status["sources"], "honda-j2534-rewrite")["kind"], "tool_installation");
    let folder = std::path::PathBuf::from(status["folder"].as_str().unwrap());
    assert!(folder.join("README.txt").is_file(), "the folder explains itself");

    // ---- find calibration: nothing supplied -------------------------------
    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(found["resolution"]["outcome"], "NO_ARTIFACT_FOUND");
    assert_eq!(found["module"], "ECU_18DAF110");
    assert_eq!(found["resolution"]["matches"].as_array().unwrap().len(), 0);

    // ---- find calibration: a file declared to be it ------------------------
    let bytes = b"stand-in bytes; not a real calibration";
    std::fs::write(folder.join("pcm.bin"), bytes).unwrap();
    std::fs::write(
        folder.join("pcm.bin.json"),
        format!(
            r#"{{ "sha256": "{}", "calibration_id": "37805-5MR-C120" }}"#,
            aim_calibration::sha256_hex(bytes)
        ),
    )
    .unwrap();
    // And one that is only named like it: a package by its shape, made up.
    let package = aim_calibration::rwd::fixture::z(&[&[b"TEST-OTHER-0001"]], &[0x9C, 0x41, 0x07]);
    std::fs::write(folder.join("37805-5MR-C120.rwd"), &package).unwrap();

    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(found["resolution"]["outcome"], "ARTIFACT_FOUND");
    let matches = found["resolution"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0]["artifact"]["filename"], "pcm.bin");
    assert_eq!(matches[0]["matching"]["status"], "EXACT_MATCH");
    // Exact on the person's own declaration, and nobody's word on who made it.
    assert_eq!(matches[0]["matching"]["rests_on"], "user_declared");
    assert_eq!(matches[0]["origin"]["manufacturer"], "NOT_ESTABLISHED");
    assert_eq!(matches[0]["inspection"]["software"], "PAYLOAD_OPAQUE");
    assert_eq!(matches[0]["validation"]["status"], "VALID");
    assert_eq!(matches[1]["matching"]["rests_on"], "filename");
    assert_eq!(matches[1]["inspection"]["rwd"]["layout"], "z");
    assert_eq!(matches[0]["artifact"]["sha256"], aim_calibration::sha256_hex(bytes));
    assert_eq!(matches[1]["matching"]["status"], "PARTIAL_MATCH");
    assert_eq!(matches[1]["validation"]["status"], "PARTIALLY_VALIDATED");
    // The README is not a calibration and was not offered as one.
    let offered = found["resolution"]["sources"][0]["offered"].as_u64().unwrap();
    assert_eq!(offered, 2);

    // What matched is kept, and still found with the folder emptied.
    for name in ["pcm.bin", "pcm.bin.json", "37805-5MR-C120.rwd"] {
        std::fs::remove_file(folder.join(name)).unwrap();
    }
    assert_eq!(b.get("/calibration").await["kept"].as_array().unwrap().len(), 2);
    let again = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(again["resolution"]["matches"][0]["matching"]["status"], "EXACT_MATCH");
}

/// Looking for a file needs no vehicle: an identity read last week will do.
#[tokio::test]
async fn finding_a_file_does_not_need_a_vehicle_connected() {
    let b = Bench::start().await;
    let identity = json!({
        "module_key": "ECU_18DAF110",
        "address": "18DAF110",
        "protocol": null,
        "fields": {},
        "unanswered": []
    });
    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(found["resolution"]["outcome"], "NO_ARTIFACT_FOUND");
}

/// A source that was not searched is told apart, over HTTP, from one that was
/// searched and holds nothing.
#[tokio::test]
async fn each_source_says_how_its_search_ended() {
    let b = Bench::start().await;
    let identity = json!({
        "module_key": "ECU_18DAF110", "address": "18DAF110", "protocol": null,
        "fields": {}, "unanswered": []
    });
    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    let sources = &found["resolution"]["sources"];

    assert_eq!(source(sources, "folder")["status"], "no_match");
    assert_eq!(source(sources, "folder")["searched"], true);
    assert_eq!(source(sources, "cache")["status"], "no_match");
    // No Honda tool is installed where the tests run: nowhere to look.
    let tool = source(sources, "honda-j2534-rewrite");
    assert_eq!(tool["status"], "not_configured");
    assert_eq!(tool["searched"], false);
    assert!(tool["error"].as_str().is_some_and(|e| e.contains("not found on this computer")));
    assert_eq!(found["resolution"]["incomplete"], false);

    // A sources.json that cannot be read is a failure, and the search says it
    // could not look everywhere.
    let root = std::path::PathBuf::from(found["folder"].as_str().unwrap());
    std::fs::write(root.parent().unwrap().join("sources.json"), "{ not json").unwrap();
    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(found["resolution"]["outcome"], "NO_ARTIFACT_FOUND");
    assert_eq!(found["resolution"]["incomplete"], true);
    assert_eq!(source(&found["resolution"]["sources"], "sources.json")["status"], "failed");
}

/// A person can hand the app a file they have. It goes in their folder as it
/// is, and is then judged like any other file there.
#[tokio::test]
async fn a_file_a_person_has_can_be_added_and_is_then_judged() {
    let b = Bench::start().await;
    b.post("/adapter/connect", json!({})).await;
    b.post("/vehicles/identify", json!({})).await;
    b.post("/modules", json!({})).await;
    let identity = b.get("/modules/ECU_18DAF110/calibration").await["data"]["identity"].clone();

    let bytes = b"stand-in bytes; not a real calibration";
    let (status, added) = b.add_file("37805-5MR-C120.bin", bytes).await;
    assert!(status.is_success(), "{status}: {added}");
    assert_eq!(added["sha256"], aim_calibration::sha256_hex(bytes));
    assert_eq!(added["already_there"], false);

    // Found, by its name and nothing more: adding a file declares nothing.
    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    let one = &found["resolution"]["matches"][0];
    assert_eq!(one["artifact"]["filename"], "37805-5MR-C120.bin");
    assert_eq!(one["matching"]["status"], "PARTIAL_MATCH");
    assert_eq!(one["matching"]["rests_on"], "filename");
    assert_eq!(one["artifact"]["sources"][0]["kind"], "user_folder");
    assert_eq!(one["origin"]["manufacturer"], "NOT_ESTABLISHED");

    // The same file again is the same file. A different one is not let in
    // under its name.
    let (status, again) = b.add_file("37805-5MR-C120.bin", bytes).await;
    assert!(status.is_success());
    assert_eq!(again["already_there"], true);
    let (status, refused) = b.add_file("37805-5MR-C120.bin", b"something else").await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{refused}");

    // A name that is not a file in the folder, and a file that is not a
    // calibration, are refused before anything is written.
    for name in ["../outside.bin", "sub/inside.bin", "C:\\x.bin", "notes.txt", "x.bin.json"] {
        let (status, body) = b.add_file(name, bytes).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{name}: {body}");
    }
    let folder = std::path::PathBuf::from(found["folder"].as_str().unwrap());
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["37805-5MR-C120.bin", "README.txt"]);
    assert!(!folder.parent().unwrap().join("outside.bin").exists());
}

/// The tool a model calls and the route the screen calls give the same
/// answer, because they are the same search.
#[tokio::test]
async fn a_model_and_the_screen_get_the_same_answer() {
    let b = Bench::start().await;
    b.post("/adapter/connect", json!({})).await;
    b.post("/vehicles/identify", json!({})).await;
    b.post("/modules", json!({})).await;

    // The tool is one a model is handed.
    let tools = b.get("/tools").await;
    assert!(tools["enabled"].as_array().unwrap().iter().any(|t| t == "find_calibration"));

    let bytes = b"stand-in bytes; not a real calibration";
    let (status, _) = b.add_file("pcm.bin", bytes).await;
    assert!(status.is_success());
    let folder = b.get("/calibration").await["folder"].as_str().unwrap().to_string();
    std::fs::write(
        std::path::Path::new(&folder).join("pcm.bin.json"),
        r#"{ "calibration_id": "37805-5MR-C120" }"#,
    )
    .unwrap();

    // As a model asks: a module, and nothing else.
    let by_tool = b
        .post(
            "/tools/find_calibration",
            json!({ "arguments": { "module": "ECU_18DAF110" }, "initiator": "agent:test" }),
        )
        .await;
    assert_eq!(by_tool["success"], true, "{}", by_tool["error"]);
    assert_eq!(by_tool["tool"], "find_calibration");
    let tool_search = &by_tool["data"]["search"]["resolution"];

    // As the screen asks: read the identity, then look.
    let identity = b.get("/modules/ECU_18DAF110/calibration").await["data"]["identity"].clone();
    let by_screen = b.post("/calibration/find", json!({ "identity": identity })).await;
    let screen_search = &by_screen["resolution"];

    assert_eq!(tool_search["outcome"], "ARTIFACT_FOUND");
    assert_eq!(tool_search["outcome"], screen_search["outcome"]);
    for key in ["status", "rests_on", "reason", "checks", "warnings"] {
        assert_eq!(
            tool_search["matches"][0]["matching"][key],
            screen_search["matches"][0]["matching"][key],
            "{key}"
        );
    }
    assert_eq!(tool_search["matches"][0]["origin"], screen_search["matches"][0]["origin"]);
    assert_eq!(tool_search["matches"][0]["validation"], screen_search["matches"][0]["validation"]);
    assert_eq!(by_tool["data"]["limits"]["manufacturer_origin"], "NOT_ESTABLISHED");

    // And a model cannot hand it a place to look or an identity of its own.
    for arguments in [
        json!({ "module": "ECU_18DAF110", "url": "https://example.com/x.rwd.gz" }),
        json!({ "module": "ECU_18DAF110", "path": "C:/" }),
        json!({ "module": "ECU_18DAF110", "identity": {} }),
    ] {
        let refused = b.post("/tools/find_calibration", json!({ "arguments": arguments })).await;
        assert_eq!(refused["success"], false, "{arguments}");
    }
}
