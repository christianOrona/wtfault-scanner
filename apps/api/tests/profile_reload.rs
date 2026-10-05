//! A settings profile takes effect without restarting the app.
//!
//! Somebody who has worked out a mapping for their car, or been given one, was
//! told to close the app to see it. The profile is now in force for the next
//! connection, and a vehicle connected at the time keeps the definitions it
//! started with.

use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use serde_json::{json, Value};
use std::time::Duration;

/// A profile for the simulated truck's make. It claims to be verified, as a
/// file from a stranger might.
const PROFILE: &str = r#"
version: 1
profile: from-a-friend
features:
  - id: imported.courtesy_wipe
    name: "Courtesy wipe"
    easy: "One more sweep of the wipers a few seconds after washing."
    technical: "A body module configuration bit."
    risk: convenience
    modules: ["726"]
    verification: verified
    source: "a forum post"
    applies_to:
      makes: [Ford]
    mapping:
      kind: data_identifier_bits
      module: "726"
      did: 0xDE0A
      byte: 2
      mask: 0x04
      on: 0x04
      off: 0x00
"#;

struct Bench {
    base: String,
    client: reqwest::Client,
    dir: tempfile::TempDir,
}

impl Bench {
    async fn start() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: ScenarioId::Parked,
            personality: PersonalityChoice::Genuine,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            settings_path: dir.path().join("providers.json"),
            profiles_dir: Some(profiles.clone()),
            replay: None,
            simulated_vehicle: Default::default(),
            calibrations_dir: None,
            handoff: None,
        };
        let store = SessionStore::open(dir.path().join("sessions.sqlite")).unwrap();
        let decoders = DecoderSet::with_profiles(&profiles).unwrap();
        let app = aim_api::router(AppState::new(store, decoders, config));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Bench { base: format!("http://{addr}/api/v1"), client: reqwest::Client::new(), dir }
    }

    async fn get(&self, path: &str) -> Value {
        self.client.get(format!("{}{path}", self.base)).send().await.unwrap().json().await.unwrap()
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status();
        let value: Value = r.json().await.unwrap();
        assert!(status.is_success(), "POST {path} -> {status}: {value}");
        value
    }

    /// The ids of the settings the connected vehicle is offered.
    async fn offered(&self) -> Vec<String> {
        let list = self.get("/features").await;
        list["data"]["features"]
            .as_array()
            .map(|f| f.iter().filter_map(|x| x["id"].as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    async fn connect(&self) {
        self.post("/adapter/connect", json!({})).await;
        self.post("/vehicles/identify", json!({})).await;
    }
}

#[tokio::test]
async fn an_imported_profile_is_in_force_for_the_next_connection() {
    let b = Bench::start().await;
    let before = b.get("/profiles").await["feature_count"].as_u64().unwrap();

    let imported = b
        .post("/profiles/import", json!({ "text": PROFILE, "source": "from-a-friend.yaml" }))
        .await;
    assert_eq!(imported["imported"], true);
    assert_eq!(imported["reconnect_to_use"], false, "nothing was connected");

    // Without restarting anything.
    assert_eq!(b.get("/profiles").await["feature_count"], before + 1);

    b.connect().await;
    let offered = b.offered().await;
    assert!(offered.iter().any(|id| id == "imported.courtesy_wipe"), "{offered:?}");

    // And whatever the file said about itself, it arrived unverified.
    let list = b.get("/features").await;
    let wipe = list["data"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "imported.courtesy_wipe")
        .unwrap();
    assert_ne!(wipe["verification"], "verified", "{wipe}");
}

/// A vehicle connected when the profile arrives keeps what it started with:
/// definitions do not change under a session that is being recorded.
#[tokio::test]
async fn a_connected_vehicle_keeps_its_definitions_until_it_reconnects() {
    let b = Bench::start().await;
    b.connect().await;
    assert!(!b.offered().await.iter().any(|id| id == "imported.courtesy_wipe"));

    let imported = b
        .post("/profiles/import", json!({ "text": PROFILE, "source": "from-a-friend.yaml" }))
        .await;
    assert_eq!(imported["reconnect_to_use"], true);
    assert!(imported["note"].as_str().unwrap().contains("connect again"));
    assert!(!b.offered().await.iter().any(|id| id == "imported.courtesy_wipe"));

    b.post("/adapter/disconnect", json!({})).await;
    b.connect().await;
    assert!(b.offered().await.iter().any(|id| id == "imported.courtesy_wipe"));
}

/// A file put in the folder by hand is picked up by asking, not by restarting.
#[tokio::test]
async fn a_file_dropped_in_the_folder_is_found_by_checking_again() {
    let b = Bench::start().await;
    let before = b.get("/profiles").await["feature_count"].as_u64().unwrap();

    std::fs::write(b.dir.path().join("profiles").join("mine.yaml"), PROFILE).unwrap();
    assert_eq!(b.get("/profiles").await["feature_count"], before, "not looked at yet");

    let checked = b.post("/profiles/reload", json!({})).await;
    assert_eq!(checked["settings_before"], before);
    assert_eq!(checked["settings_now"], before + 1);
    assert_eq!(checked["reconnect_to_use"], false);
    assert_eq!(b.get("/profiles").await["feature_count"], before + 1);
}
