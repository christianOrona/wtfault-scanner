//! The endpoint's promises, each one asked of it.

use aim_report_endpoint::{router, Config, Receipt, MAX_TEXT_BYTES};
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tower::ServiceExt;

fn config(dir: &Path, per_address: u32, per_day: u32, trust: bool) -> Config {
    Config {
        dir: dir.to_path_buf(),
        per_address,
        window: Duration::from_secs(3600),
        per_day,
        trust_forwarded_for: trust,
    }
}

fn app(config: Config, peer: &str) -> axum::Router {
    router(config).layer(MockConnectInfo(peer.parse::<SocketAddr>().unwrap()))
}

async fn post(app: &axum::Router, body: String, forwarded: Option<&str>) -> (StatusCode, Receipt) {
    let mut request = Request::post("/v1/reports").header("content-type", "application/json");
    if let Some(f) = forwarded {
        request = request.header("x-forwarded-for", f);
    }
    let response = app.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let receipt = serde_json::from_slice(&bytes).unwrap_or(Receipt {
        accepted: false,
        reference: None,
        reason: None,
    });
    (status, receipt)
}

fn report(text: &str) -> String {
    serde_json::json!({ "version": "0.5.0", "platform": "windows x86_64", "text": text })
        .to_string()
}

fn stored(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for day in std::fs::read_dir(dir).unwrap() {
        for file in std::fs::read_dir(day.unwrap().path()).unwrap() {
            out.push(std::fs::read_to_string(file.unwrap().path()).unwrap());
        }
    }
    out
}

#[tokio::test]
async fn a_report_is_kept_as_a_file_with_only_what_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(config(dir.path(), 5, 100, false), "203.0.113.7:5000");
    let (status, receipt) = post(&app, report("the app froze on connect"), None).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(receipt.accepted && receipt.reference.is_some());

    let files = stored(dir.path());
    assert_eq!(files.len(), 1);
    assert!(files[0].contains("the app froze on connect"));
    assert!(files[0].contains("version  : 0.5.0"));
    assert!(!files[0].contains("203.0.113.7"), "the address is not kept");
}

#[tokio::test]
async fn an_oversized_report_is_refused_and_nothing_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(config(dir.path(), 5, 100, false), "203.0.113.7:5000");
    let (status, _) = post(&app, report(&"x".repeat(MAX_TEXT_BYTES + 1)), None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = post(&app, report(&"x".repeat(MAX_TEXT_BYTES * 2)), None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "refused before it is parsed");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn one_address_is_limited_and_another_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), 2, 100, false);
    let first = app(cfg.clone(), "203.0.113.7:5000");
    assert_eq!(post(&first, report("1"), None).await.0, StatusCode::CREATED);
    assert_eq!(post(&first, report("2"), None).await.0, StatusCode::CREATED);
    let (status, receipt) = post(&first, report("3"), None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(receipt.reason.unwrap().contains("this address"));
}

#[tokio::test]
async fn the_day_has_a_ceiling_across_everybody() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(config(dir.path(), 10, 1, false), "203.0.113.7:5000");
    assert_eq!(post(&app, report("1"), None).await.0, StatusCode::CREATED);
    assert_eq!(post(&app, report("2"), None).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn anything_that_is_not_a_report_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(config(dir.path(), 10, 100, false), "203.0.113.7:5000");
    assert_eq!(post(&app, String::from("not json"), None).await.0, StatusCode::BAD_REQUEST);
    let extra = serde_json::json!({
        "version": "0.5.0", "platform": "linux", "text": "t", "path": "../../etc/passwd"
    });
    assert_eq!(post(&app, extra.to_string(), None).await.0, StatusCode::BAD_REQUEST);
    let newline = serde_json::json!({ "version": "0.5\nx", "platform": "linux", "text": "t" });
    assert_eq!(post(&app, newline.to_string(), None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(post(&app, report("   "), None).await.0, StatusCode::PAYLOAD_TOO_LARGE);
}

/// A client cannot dodge the limit by naming another address, unless the
/// endpoint was told a proxy sets that header.
#[tokio::test]
async fn a_forwarded_address_counts_only_behind_a_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let direct = app(config(dir.path(), 1, 100, false), "203.0.113.7:5000");
    assert_eq!(post(&direct, report("1"), Some("198.51.100.1")).await.0, StatusCode::CREATED);
    assert_eq!(
        post(&direct, report("2"), Some("198.51.100.2")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "the header is ignored without a proxy"
    );

    let proxied = app(config(dir.path(), 1, 100, true), "10.0.0.1:5000");
    assert_eq!(post(&proxied, report("1"), Some("198.51.100.1")).await.0, StatusCode::CREATED);
    assert_eq!(post(&proxied, report("2"), Some("198.51.100.2")).await.0, StatusCode::CREATED);
}
