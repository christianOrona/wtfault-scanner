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
    assert_eq!(sources.len(), 2);
    assert!(sources.iter().all(|s| s["uses_network"] == false));
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
    // And one that is only named like it.
    std::fs::write(folder.join("37805-5MR-C120.rwd"), b"other bytes").unwrap();

    let found = b.post("/calibration/find", json!({ "identity": identity })).await;
    assert_eq!(found["resolution"]["outcome"], "ARTIFACT_FOUND");
    let matches = found["resolution"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0]["artifact"]["filename"], "pcm.bin");
    assert_eq!(matches[0]["matching"]["status"], "EXACT_MATCH");
    assert_eq!(matches[0]["validation"]["status"], "VALID");
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
