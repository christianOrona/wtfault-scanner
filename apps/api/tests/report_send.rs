//! Send, end to end: the app posts a report to a running endpoint, which keeps
//! exactly the text the person saw (#50).

use std::net::SocketAddr;
use std::time::Duration;

/// Both tests point the app somewhere through one process-wide variable, so
/// they take turns rather than race.
static ENDPOINT_VARIABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn endpoint(dir: &std::path::Path) -> SocketAddr {
    let config = aim_report_endpoint::Config {
        dir: dir.to_path_buf(),
        per_address: 5,
        window: Duration::from_secs(3600),
        per_day: 100,
        trust_forwarded_for: false,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app =
        aim_report_endpoint::router(config).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

#[tokio::test]
async fn a_withheld_report_reaches_the_endpoint_as_it_was_shown() {
    let _turn = ENDPOINT_VARIABLE.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let addr = endpoint(dir.path()).await;
    std::env::set_var("AIM_REPORT_ENDPOINT", format!("http://{addr}"));

    let shown = aim_api::support::withhold_identifiers(
        "connected to 1FT7W2BT6KEC00001 and it froze",
        &[String::from("1FT7W2BT6KEC00001")],
    )
    .0;
    let outcome = aim_api::support::send(&shown).await;
    assert!(outcome.sent, "{outcome:?}");
    assert!(outcome.reference.is_some());

    let day = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
    let file = std::fs::read_dir(day).unwrap().next().unwrap().unwrap().path();
    let kept = std::fs::read_to_string(file).unwrap();
    assert!(kept.ends_with(&shown), "{kept}");
    assert!(!kept.contains("1FT7W2BT6KEC00001"), "{kept}");
}

/// Nothing listening is an answer, not an error: copy and save still work.
#[tokio::test]
async fn an_unreachable_endpoint_fails_quietly() {
    let _turn = ENDPOINT_VARIABLE.lock().await;
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = unused.local_addr().unwrap();
    drop(unused);
    std::env::set_var("AIM_REPORT_ENDPOINT", format!("http://{addr}"));
    let outcome = aim_api::support::send("anything").await;
    assert!(!outcome.sent);
    assert!(outcome.reason.unwrap().contains("could not reach"));
}
