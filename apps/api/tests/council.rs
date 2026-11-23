//! Local agents and the Mechanic council, through the real HTTP surface.
//!
//! The one thing stubbed is the agent CLI itself: a small shell script stands
//! in for `claude`, answering the way Claude Code's `--output-format json`
//! does. Everything else is real: the router, the settings file, the process
//! runner, the text tool protocol, the agent loop, the safety-gated executor
//! and the simulated F-250 with a loaded particulate filter.

use aim_api::state::{AppState, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use serde_json::{json, Value};
use std::time::Duration;

struct Harness {
    base: String,
    client: reqwest::Client,
    dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open(dir.path().join("session.sqlite")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = ServerConfig {
            default_transport: TransportChoice::Simulator,
            default_port: None,
            default_scenario: ScenarioId::DpfRegen,
            personality: PersonalityChoice::Genuine,
            bind: addr.to_string(),
            simulator_latency: Duration::ZERO,
            settings_path: dir.path().join("providers.json"),
            profiles_dir: Some(dir.path().join("profiles")),
            replay: None,
            simulated_vehicle: Default::default(),
            calibrations_dir: Some(dir.path().join("calibrations")),
            handoff: None,
        };
        let app = aim_api::router(AppState::new(store, DecoderSet::generic_obd().unwrap(), config));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Harness { base: format!("http://{addr}/api/v1"), client: reqwest::Client::new(), dir }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.client.request(method, format!("{}{path}", self.base));
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.send(method, path, body).await;
        assert!((200..300).contains(&status), "{path} -> {status}: {value}");
        value
    }
}

fn local_agent(label: &str, executable: &str) -> Value {
    json!({ "kind": "local_agent", "label": label, "model": "", "local_agent": "claude_code",
            "executable": executable })
}

#[tokio::test]
async fn local_agents_and_the_council_are_configured_through_settings() {
    let h = Harness::start().await;
    use reqwest::Method as M;

    // Discovery answers without spending anything, whatever is installed here.
    let found = h.ok(M::GET, "/settings/local-agents", None).await;
    assert_eq!(found["supported"], true);
    assert!(found["agents"].is_array());
    assert_eq!(found["looked_for"].as_array().unwrap().len(), 3);

    // A local agent must say which one; a blank model is its CLI's default.
    let (status, _) = h
        .send(
            M::POST,
            "/settings/providers",
            Some(json!({"kind": "local_agent", "label": "x", "model": ""})),
        )
        .await;
    assert_eq!(status, 400);

    let a = h.ok(M::POST, "/settings/providers", Some(local_agent("Claude Code", "/bin/sh"))).await;
    let lead = a["added"].as_str().unwrap().to_string();
    let view = &a["providers"][0];
    assert_eq!(view["kind"], "local_agent");
    assert_eq!(view["local_agent"], "claude_code");
    assert_eq!(view["has_key"], false);
    assert_eq!(view["locality"], "somebody_else", "the CLI sends prompts to its vendor");

    let b = h
        .ok(
            M::POST,
            "/settings/providers",
            Some(json!({"kind": "ollama", "label": "Jarvis", "model": "qwen3:8b"})),
        )
        .await;
    let member = b["added"].as_str().unwrap().to_string();

    let (status, _) = h
        .send(
            M::POST,
            "/settings/council",
            Some(json!({"enabled": true, "members": ["prov_nope"]})),
        )
        .await;
    assert_eq!(status, 400, "a member must be a configured provider");

    let set = h
        .ok(M::POST, "/settings/council", Some(json!({"enabled": true, "members": [lead, member]})))
        .await;
    assert_eq!(set["sitting"].as_array().unwrap().len(), 1, "the lead is not its own member");
    assert_eq!(set["sitting"][0]["label"], "Jarvis");

    let status = h.ok(M::GET, "/agent", None).await;
    assert_eq!(status["council"][0]["label"], "Jarvis");
    let providers = h.ok(M::GET, "/settings/providers", None).await;
    assert_eq!(providers["council"]["enabled"], true);
    assert_eq!(providers["local_agents_supported"], true);
    assert!(providers["kinds"].as_array().unwrap().iter().any(|k| k["id"] == "local_agent"));

    // Deleting a member takes it off the council too.
    h.ok(M::DELETE, &format!("/settings/providers/{member}"), None).await;
    let status = h.ok(M::GET, "/agent", None).await;
    assert_eq!(status["council"], json!([]));
}

/// A stand-in for Claude Code. It answers each kind of prompt the way the
/// real CLI answers with `--output-format json`, and records its arguments.
#[cfg(unix)]
fn fake_claude(dir: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("claude");
    let log = dir.join("args.log");
    let script = format!(
        r#"#!/bin/sh
case "$1" in
  --version) echo "2.1.0 (Claude Code)"; exit 0;;
  auth) echo '{{"loggedIn":true,"authMethod":"claude.ai"}}'; exit 0;;
esac
echo "$@" >> "{log}"
env | grep -q '^ANTHROPIC_API_KEY=' && echo "LEAKED KEY" >> "{log}"
p=$(cat)
case "$p" in
  *"write the council's answer"*) r="Council: the particulate filter is loaded; let it regenerate on a highway drive.";;
  *"you are on a Mechanic council"*) r="Second opinion: agree, P2463 is soot loading.";;
  *"Tool result: read_dtcs"*) r="Draft: P2463 says the particulate filter is loaded.";;
  *) r='Reading the codes.\n<tool_calls>[{{\"name\":\"read_dtcs\",\"input\":{{}}}}]</tool_calls>';;
esac
printf '{{"type":"result","subtype":"success","is_error":false,"result":"%s","usage":{{"input_tokens":3,"output_tokens":2}}}}' "$r"
"#,
        log = log.display()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

#[cfg(unix)]
#[tokio::test]
async fn ask_runs_as_a_council_of_local_agents_against_the_simulated_truck() {
    use reqwest::Method as M;
    let h = Harness::start().await;
    let exe = fake_claude(h.dir.path());
    std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-must-not-reach-the-cli");

    let lead = h.ok(M::POST, "/settings/providers", Some(local_agent("Claude Code", &exe))).await;
    let lead = lead["added"].as_str().unwrap().to_string();
    let member =
        h.ok(M::POST, "/settings/providers", Some(local_agent("Second Claude", &exe))).await;
    let member = member["added"].as_str().unwrap().to_string();
    h.ok(M::POST, &format!("/settings/providers/{lead}/select"), None).await;
    h.ok(M::POST, "/settings/council", Some(json!({"enabled": true, "members": [member]}))).await;

    // Testing the provider checks the install and sign-in, and sends no prompt.
    let probe = h.ok(M::POST, &format!("/settings/providers/{lead}/test"), None).await;
    assert_eq!(probe["reachable"], true, "{probe}");
    assert!(probe["detail"].as_str().unwrap().contains("Signed in"), "{probe}");

    h.ok(M::POST, "/adapter/connect", Some(json!({}))).await;
    let answer = h
        .ok(
            M::POST,
            "/agent/messages",
            Some(json!({"messages": [{"role": "user", "content": "Why is it smoking?"}]})),
        )
        .await;

    assert!(answer["text"].as_str().unwrap().starts_with("Council:"), "{answer}");
    let council = &answer["council"];
    assert_eq!(council["merged"], true, "{answer}");
    assert_eq!(council["lead"], "Claude Code");
    assert_eq!(council["opinions"][0]["member"], "Second Claude");
    assert!(council["opinions"][0]["text"].as_str().unwrap().contains("P2463"));

    // The lead read the truck through the ordinary, safety-gated tool path.
    let trace = answer["trace"].as_array().unwrap();
    assert!(trace.iter().any(|t| t["type"] == "tool" && t["name"] == "read_dtcs"), "{trace:?}");
    assert!(trace.iter().any(|t| t["type"] == "council_opinion" && t["success"] == true));

    let log = std::fs::read_to_string(h.dir.path().join("args.log")).unwrap();
    assert!(!log.contains("LEAKED KEY"), "an API key reached the CLI");
    assert!(log.contains("--tools"), "the CLI's own tools are switched off: {log}");
    std::env::remove_var("ANTHROPIC_API_KEY");
}
