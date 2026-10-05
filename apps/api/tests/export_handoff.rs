//! Exports on a shell that hands files over itself, which is a phone.
//!
//! The real server on a real socket, with a stand-in for the share sheet. What
//! matters is that a session recorded there can leave: the database copy opens
//! as a database with the session in it, and the transcript carries the
//! exchanges without the VIN.

use aim_api::handoff::FileHandoff;
use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use aim_types::SessionId;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Records what it was asked to offer instead of offering it.
#[derive(Debug)]
struct ShareSheet {
    dir: PathBuf,
    offered: Mutex<Vec<(PathBuf, String)>>,
}

impl FileHandoff for ShareSheet {
    fn staging_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    fn offer(&self, path: &Path, mime: &str) -> Result<(), String> {
        self.offered.lock().unwrap().push((path.to_path_buf(), mime.to_string()));
        Ok(())
    }
}

struct Phone {
    base: String,
    client: reqwest::Client,
    sheet: Arc<ShareSheet>,
    _dir: tempfile::TempDir,
}

impl Phone {
    async fn start() -> Phone {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open(dir.path().join("sessions.sqlite")).unwrap();
        let sheet = Arc::new(ShareSheet {
            dir: dir.path().join("cache").join("exports"),
            offered: Mutex::new(Vec::new()),
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: ScenarioId::DpfRegen,
            personality: PersonalityChoice::Clone,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            settings_path: dir.path().join("providers.json"),
            profiles_dir: None,
            replay: None,
            simulated_vehicle: Default::default(),
            handoff: Some(sheet.clone()),
        };
        let app = aim_api::router(AppState::new(store, DecoderSet::generic_obd().unwrap(), config));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Phone {
            base: format!("http://{addr}/api/v1"),
            client: reqwest::Client::new(),
            sheet,
            _dir: dir,
        }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    /// Connect to the virtual truck and read its VIN, so there is a session
    /// with a vehicle and some exchanges in it. Returns the session id.
    async fn scan(&self) -> String {
        let (status, connect) = self.post("/adapter/connect", json!({ "label": "a drive" })).await;
        assert_eq!(status, 200, "{connect}");
        let (status, identify) = self.post("/vehicles/identify", json!({})).await;
        assert_eq!(status, 200, "{identify}");
        assert_eq!(identify["data"]["vin"], aim_simulator::SIMULATED_VIN);
        let adapter: Value = self
            .client
            .get(format!("{}/adapter", self.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        adapter["session_id"].as_str().unwrap().to_string()
    }

    fn offered(&self) -> Vec<(PathBuf, String)> {
        self.sheet.offered.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn the_database_copy_is_offered_and_holds_the_session() {
    let phone = Phone::start().await;
    let session = phone.scan().await;

    let (status, body) =
        phone.post("/export/database", json!({ "filename": "wtfault-database.sqlite" })).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["shared"], true);
    assert_eq!(body["filename"], "wtfault-database.sqlite");

    let staged = phone.sheet.dir.join("wtfault-database.sqlite");
    assert_eq!(phone.offered(), vec![(staged.clone(), "application/octet-stream".to_string())]);
    assert_eq!(body["bytes"], std::fs::metadata(&staged).unwrap().len());

    // The point of the copy: somewhere else, it is the same history.
    let copy = SessionStore::open(&staged).unwrap();
    let id = SessionId::from_string(session);
    assert!(copy.event_count(&id).unwrap() > 10);
    assert!(copy.vehicle_by_vin(aim_simulator::SIMULATED_VIN).unwrap().is_some());
    drop(copy);

    // A second export under the same name replaces the first.
    let (status, body) =
        phone.post("/export/database", json!({ "filename": "wtfault-database.sqlite" })).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(phone.offered().len(), 2);
}

#[tokio::test]
async fn the_transcript_is_offered_without_the_vin() {
    let phone = Phone::start().await;
    let session = phone.scan().await;

    let (status, body) = phone
        .post(&format!("/sessions/{session}/transcript"), json!({ "filename": "drive.transcript" }))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["shared"], true);

    let staged = phone.sheet.dir.join("drive.transcript");
    assert_eq!(phone.offered(), vec![(staged.clone(), "text/plain".to_string())]);

    let text = std::fs::read_to_string(&staged).unwrap();
    assert!(text.contains("> 0902"), "the VIN request should be in the transcript");
    assert!(!text.contains(aim_simulator::SIMULATED_VIN));
    // The same text the read-only route answers with.
    let served = phone
        .client
        .get(format!("{}/sessions/{session}/transcript", phone.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, served);
}

#[tokio::test]
async fn an_ordinary_export_goes_to_the_share_sheet_too() {
    let phone = Phone::start().await;

    let (status, body) = phone
        .post("/export", json!({ "filename": "wtfault-codes.csv", "content": "code\r\nP2463\r\n" }))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["shared"], true);
    assert_eq!(
        phone.offered(),
        vec![(phone.sheet.dir.join("wtfault-codes.csv"), "text/csv".to_string())]
    );
}

#[tokio::test]
async fn a_refused_name_or_session_offers_nothing() {
    let phone = Phone::start().await;

    let (status, _) = phone.post("/export/database", json!({ "filename": ".." })).await;
    assert_eq!(status, 400);
    let (status, _) = phone
        .post("/sessions/ses_nope/transcript", json!({ "filename": "drive.transcript" }))
        .await;
    assert_eq!(status, 404);

    assert!(phone.offered().is_empty());
    let left: Vec<_> = std::fs::read_dir(&phone.sheet.dir).unwrap().collect();
    assert!(left.is_empty(), "nothing should have been written: {left:?}");
}

/// Post a file as the body, the way the interface sends an import.
async fn upload(phone: &Phone, path: &str, content_type: &str, bytes: Vec<u8>) -> (u16, Value) {
    let r = phone
        .client
        .post(format!("{}{path}", phone.base))
        .header("content-type", content_type)
        .body(bytes)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

/// The round trip the two halves exist for: recorded on one device, read on
/// another, as part of the same vehicle's history.
#[tokio::test]
async fn what_one_device_exports_another_imports() {
    let phone = Phone::start().await;
    let session = phone.scan().await;
    let (status, body) =
        phone.post("/export/database", json!({ "filename": "from-the-phone.sqlite" })).await;
    assert_eq!(status, 200, "{body}");
    let file = std::fs::read(phone.sheet.dir.join("from-the-phone.sqlite")).unwrap();

    let desk = Phone::start().await;
    let known = |v: Value| v["sessions"].as_array().unwrap().len();
    let sessions = || async {
        let r = desk.client.get(format!("{}/sessions", desk.base)).send().await.unwrap();
        r.json::<Value>().await.unwrap()
    };
    assert_eq!(known(sessions().await), 0);

    // Asked first: what would this add?
    let (status, preview) =
        upload(&desk, "/import/database?dry_run=true", "application/octet-stream", file.clone())
            .await;
    assert_eq!(status, 200, "{preview}");
    assert_eq!(preview["committed"], false);
    assert_eq!(preview["sessions_added"], 1);
    assert_eq!(preview["vehicles"][0], aim_simulator::SIMULATED_VIN);
    assert_eq!(known(sessions().await), 0, "a preview keeps nothing");

    let (status, done) =
        upload(&desk, "/import/database", "application/octet-stream", file.clone()).await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["committed"], true);
    assert_eq!(done["events_added"], preview["events_added"]);

    let listed = sessions().await;
    assert_eq!(known(listed.clone()), 1);
    assert_eq!(listed["sessions"][0]["session"]["id"], session);
    assert_eq!(listed["sessions"][0]["vehicle"]["vin"], aim_simulator::SIMULATED_VIN);

    // The same file again changes nothing.
    let (status, again) = upload(&desk, "/import/database", "application/octet-stream", file).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["sessions_added"], 0);
    assert_eq!(again["sessions_already_here"], 1);
    assert_eq!(again["events_added"], 0);

    // Nothing staged for the import is left behind.
    let left: Vec<_> = std::fs::read_dir(&desk.sheet.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn an_import_is_refused_unless_it_is_plainly_a_database_upload() {
    let desk = Phone::start().await;

    // The types a page on another site can send without asking.
    for unasked in ["text/plain", "application/x-www-form-urlencoded", "multipart/form-data"] {
        let (status, body) = upload(&desk, "/import/database", unasked, b"SQLite".to_vec()).await;
        assert_eq!(status, 400, "{unasked}: {body}");
    }
    let (status, _) = upload(&desk, "/import/database", "application/octet-stream", vec![]).await;
    assert_eq!(status, 400);
    let (status, body) =
        upload(&desk, "/import/database", "application/octet-stream", b"not a database".to_vec())
            .await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("not a WTFault database"));
}
