//! *Ask* does not report on trouble codes it has not read.
//!
//! The whole path with nothing stubbed but the model: the simulated F-250 with
//! a clogged exhaust filter, the diagnostic service, the tool registry, the
//! safety gate and the agent loop. The model is a script, so this costs
//! nothing to run and says the same thing every time.
//!
//! Reproduced on 2026-10-05 with a real model. An inspection had read
//! everything. *Ask* was then asked whether it was safe to drive home, made
//! one read (the full scan the inspection had left behind) and answered "No
//! engine or airbag faults anywhere" about an engine holding P2463, P242F and
//! P2002. The scan holds each module's UDS fault memory, which on this engine
//! is empty, and none of the emissions codes.

use aim_agent::scripted::{say, tool_call, ScriptedProvider};
use aim_agent::{Agent, AgentEvent, AgentOutcome, Content, Message};
use aim_api::agent::{CoreExecutor, RecordingSink};
use aim_api::state::{AppState, ConnectRequest, PersonalityChoice, ServerConfig, TransportChoice};
use aim_decoders::DecoderSet;
use aim_session::SessionStore;
use aim_simulator::ScenarioId;
use serde_json::{json, Value};
use std::time::Duration;

const ASSISTANT: &str = "agent:agent";

/// What the assistant answered, word for word.
const NOTHING_IN_THE_ENGINE: &str = "Nothing in the engine module, nothing in the body module. \
                                     No engine or airbag faults anywhere.";

/// The truck as the inspection left it: connected, its modules found, its
/// trouble codes read once and a full scan kept.
async fn the_truck_after_an_inspection() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        default_transport: TransportChoice::Simulator,
        default_port: None,
        default_scenario: ScenarioId::DpfRegen,
        personality: PersonalityChoice::Genuine,
        bind: String::from("127.0.0.1:0"),
        simulator_latency: Duration::ZERO,
        // A temp path: the test must never read the real provider settings.
        settings_path: dir.path().join("providers.json"),
        profiles_dir: Some(dir.path().join("profiles")),
        replay: None,
        simulated_vehicle: Default::default(),
        calibrations_dir: Some(dir.path().join("calibrations")),
        handoff: None,
    };
    let store = SessionStore::open(dir.path().join("session.sqlite")).unwrap();
    let state = AppState::new(store, DecoderSet::generic_obd().unwrap(), config);

    let connected = state.connect(ConnectRequest::default()).await.expect("connects");
    assert!(connected.success, "{:?}", connected.error);

    let inspection = state
        .with_service(|s| {
            [
                s.identify_vehicle(ASSISTANT),
                s.scan_modules(ASSISTANT),
                s.read_dtcs(None, ASSISTANT),
                s.scan_all_modules(ASSISTANT),
            ]
        })
        .await
        .expect("a session");
    for read in &inspection {
        assert!(read.success, "{}: {:?}", read.tool, read.error);
    }
    (state, dir)
}

/// One *Ask* turn: the suggested question, the assistant's question back, and
/// the answer to it.
async fn ask(state: &AppState, model: &ScriptedProvider) -> (AgentOutcome, RecordingSink) {
    let mut executor = CoreExecutor::new(state);
    let mut sink = RecordingSink::default();
    let conversation = vec![
        Message::user("Is it safe to drive home?"),
        Message::assistant("Which warning lights are lit?"),
        Message::user("ABS light only"),
    ];
    let outcome = Agent::new(model)
        .chat("system", conversation, &mut executor, &mut sink)
        .await
        .expect("the turn completes");
    (outcome, sink)
}

/// What a tool call returned to the model, as the model was given it.
fn returned(outcome: &AgentOutcome, tool: &str) -> Value {
    let id = format!("call_{tool}");
    outcome
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|c| match c {
            Content::ToolResult { tool_use_id, content, .. } if *tool_use_id == id => {
                Some(serde_json::from_str(content).expect("a tool result is JSON"))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{tool} returned nothing to the model"))
}

fn ran(sink: &RecordingSink, tool: &str) -> bool {
    sink.events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolFinished { name, success: true, .. } if name == tool))
}

#[tokio::test]
async fn ask_reads_the_engine_codes_before_it_says_there_are_none() {
    let (state, _dir) = the_truck_after_an_inspection().await;
    let model = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        say(NOTHING_IN_THE_ENGINE),
        tool_call("read_dtcs", json!({})),
        say("The ABS light is the wheel speed fault at the brake module. The engine module \
             also has P2463 stored: the exhaust filter is clogging."),
    ]);

    let (outcome, sink) = ask(&state, &model).await;

    // The one read it made first: the scan the inspection left, in which the
    // engine module has an empty list. The entry says whose codes it is not.
    let scan = returned(&outcome, "scan_all_modules");
    let reused =
        scan["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "full_scan_reused");
    assert!(reused, "it was handed the kept scan: {}", scan["warnings"]);
    let engine = scan["data"]["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["module_key"] == "ECU_7E8")
        .expect("the engine module is in the scan");
    assert_eq!(engine["fault_count"], 0);
    assert_eq!(engine["emissions_codes_read"], false);

    // "No engine faults" on the strength of that was handed back, naming the
    // modules whose trouble codes had not been read and the read that does it.
    let requests = model.requests();
    let told = requests[2].messages.last().expect("a message").text();
    assert!(told.contains("was not shown to the person"), "{told}");
    assert!(told.contains("ECU_7E8"), "{told}");
    assert!(told.contains("`read_dtcs`"), "{told}");
    assert!(!told.contains("ECU_768"), "the brake module keeps no emissions codes: {told}");

    // The read was then made against the vehicle, and found what is there.
    assert!(ran(&sink, "read_dtcs"));
    let codes = returned(&outcome, "read_dtcs");
    let found: Vec<&str> = codes["data"]["dtcs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    for code in ["P2463", "P242F", "P2002"] {
        assert!(found.contains(&code), "{code} was read: {found:?}");
    }

    // And the answer the person gets is the one written after it.
    assert!(outcome.text.contains("P2463"), "{}", outcome.text);
    assert!(!outcome.text.contains("No engine or airbag faults"), "{}", outcome.text);
    assert_eq!(model.remaining(), 0);
}

/// A model that is told and says it again. It never reads the codes, so the
/// application says so ahead of its answer, in its own words.
#[tokio::test]
async fn an_assistant_that_will_not_read_them_does_not_get_no_faults_through() {
    let (state, _dir) = the_truck_after_an_inspection().await;
    let model = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        say(NOTHING_IN_THE_ENGINE),
        say(NOTHING_IN_THE_ENGINE),
    ]);

    let (outcome, sink) = ask(&state, &model).await;

    assert!(!ran(&sink, "read_dtcs"), "it made one read, the full scan");
    assert_ne!(outcome.text, NOTHING_IN_THE_ENGINE);
    assert!(outcome.text.starts_with("A note from the app, not the assistant"), "{}", outcome.text);
    assert!(outcome.text.contains("ECU_7E8"), "{}", outcome.text);
    assert!(outcome.text.ends_with(NOTHING_IN_THE_ENGINE), "its own words are not rewritten");
    assert_eq!(model.remaining(), 0);
}
