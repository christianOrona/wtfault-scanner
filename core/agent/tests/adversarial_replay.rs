//! What the system does when the model misbehaves.
//!
//! Every test here scripts a model that breaks one of the rules the product
//! promises, and asserts on what survives. None of them call a real model, so
//! the suite is free and deterministic and can run on every commit.
//!
//! The distinction being tested is worth stating, because it is easy to write
//! a suite that misses it entirely: proving a well-behaved model produces good
//! output tells you nothing. Prompt text is not a security boundary, and the
//! only evidence that a badly-behaved model would be caught is to script one
//! and watch.

use aim_agent::provider::{Message, ToolSpec};
use aim_agent::report::{ConfidenceClass, Finding, Report, Severity, Source, Verdict};
use aim_agent::scripted::{say, tool_call, ScriptedProvider};
use aim_agent::{Agent, NullSink, ToolExecutor};
use serde_json::{json, Value};

fn finding(source: Source, severity: Severity, refs: Vec<i64>, title: &str) -> Finding {
    Finding {
        title: title.into(),
        severity,
        plain_english: "...".into(),
        source,
        evidence: Vec::new(),
        evidence_refs: refs,
        what_to_do: None,
        estimated_cost: None,
    }
}

/// A model claiming it measured something, with nothing to point at.
///
/// The most likely way a hallucination reaches a person: not an invented code,
/// but a real-sounding claim wearing the word "measured". Confidence is derived
/// from the citation rather than the claim, so the label does not transfer.
#[test]
fn a_measurement_claimed_without_evidence_does_not_get_measured_confidence() {
    let f = finding(Source::Measured, Severity::Critical, vec![], "Turbo failing");

    assert!(!f.is_evidenced(), "nothing was cited, so nothing was measured");
    assert_eq!(
        f.confidence().class,
        ConfidenceClass::Low,
        "a claim of measurement must not inherit the authority of one"
    );
    // And the severity it asked for is preserved, because the two are separate
    // questions: this may well be critical if true.
    assert_eq!(f.severity, Severity::Critical);
}

/// A model that answers about a vehicle nobody read.
#[test]
fn model_knowledge_never_reaches_the_measured_tier() {
    for severity in [Severity::Critical, Severity::Serious, Severity::Caution, Severity::Info] {
        let f = finding(Source::ModelKnowledge, severity, vec![1, 2, 3], "Costs about $2000");
        assert_eq!(
            f.confidence().class,
            ConfidenceClass::Low,
            "evidence_refs must not launder general knowledge into a measurement"
        );
        assert!(!f.is_evidenced());
    }
}

/// The report schema has nowhere for a model to rate itself.
#[test]
fn the_schema_offered_to_the_model_has_no_confidence_field() {
    let schema = aim_agent::report::submit_report_schema();
    let text = serde_json::to_string(&schema).expect("serialises");
    assert!(
        !text.contains("\"confidence\""),
        "a model must not be able to state its own confidence: {text}"
    );
    // And the sources it may choose from are exactly the ones the type knows.
    for expected in ["measured", "indirectly_measured", "profile", "catalog", "model_knowledge"] {
        assert!(text.contains(expected), "schema is missing source {expected}");
    }
}

/// The scripted provider replays exactly what it was given, and says so when
/// it runs out rather than inventing a plausible reply.
///
/// That last part matters for every test built on it: a harness that
/// improvises when the script ends would turn "the loop ran more times than
/// expected" into a silent pass.
#[tokio::test]
async fn the_harness_replays_in_order_and_refuses_to_improvise() {
    use aim_agent::provider::{ChatRequest, LlmProvider};

    let provider =
        ScriptedProvider::new(vec![tool_call("read_dtcs", json!({})), say("no codes stored")]);
    assert_eq!(provider.remaining(), 2);

    let request = ChatRequest {
        system: "system".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 64,
    };

    let first = provider.chat(&request).await.expect("first reply");
    assert_eq!(first.tool_calls().len(), 1, "the tool call comes back first");
    let second = provider.chat(&request).await.expect("second reply");
    assert!(second.tool_calls().is_empty(), "then the prose");
    assert_eq!(provider.remaining(), 0);

    let err = provider.chat(&request).await.expect_err("script exhausted");
    assert!(
        err.to_string().contains("ran out") || format!("{err:?}").contains("ran out"),
        "running out must be loud, not improvised: {err}"
    );

    // And it recorded what it was asked, so a test can assert on the request
    // rather than only on the reply.
    assert_eq!(provider.requests().len(), 3);
}

/// A report whose findings are all unevidenced must not read as a clean bill of
/// health, and must not read as a confident diagnosis either.
#[test]
fn an_inconclusive_report_is_representable_and_says_so() {
    let report = Report {
        verdict: Verdict::Inconclusive,
        headline: "Not enough answered to say".into(),
        summary: "The vehicle answered too little to draw a conclusion.".into(),
        findings: vec![finding(Source::Unknown, Severity::Info, vec![], "Unclear")],
        watch_items: Vec::new(),
        not_checked: vec!["Most modules did not answer".into()],
        next_steps: Vec::new(),
    };

    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_eq!(report.findings[0].confidence().class, ConfidenceClass::None);
    assert!(!report.findings.iter().any(|f| f.is_evidenced()), "nothing here rests on a reading");
    assert!(
        !report.not_checked.is_empty(),
        "an inconclusive report has to say what it could not check"
    );
}

// ------------------------------------------------- "no faults", unread

/// Answers the two fault reads the way the simulated F-250 does with a
/// clogged exhaust filter: the codes are in the engine module's emissions
/// services, and its UDS fault memory, which is all a full scan asks for, is
/// empty.
struct TruckWithEngineCodes {
    codes: Vec<&'static str>,
    calls: Vec<String>,
}

impl TruckWithEngineCodes {
    fn new() -> Self {
        TruckWithEngineCodes { codes: vec!["P2463", "P242F", "P2002"], calls: Vec::new() }
    }

    fn with_nothing_stored() -> Self {
        TruckWithEngineCodes { codes: Vec::new(), calls: Vec::new() }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for TruckWithEngineCodes {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    async fn run(&mut self, name: &str, _arguments: &Value, _initiator: &str) -> Value {
        self.calls.push(name.to_string());
        match name {
            "scan_all_modules" => json!({
                "tool": "scan_all_modules", "success": true, "raw_evidence_ref": 210,
                "data": { "module_count": 3, "fault_count": 1, "modules": [
                    { "module_key": "ECU_7E8", "address": "7E8", "name": "SIM ENGINE CONTROL",
                      "in_legislated_range": true, "faults": [], "fault_count": 0,
                      "emissions_codes_read": false, "note": null },
                    { "module_key": "ECU_768", "address": "768", "name": "Module at 768",
                      "in_legislated_range": false, "fault_count": 1,
                      "faults": [{ "code": "C0035-00", "failing_now": true }],
                      "emissions_codes_read": null, "note": null },
                    { "module_key": "ECU_7A8", "address": "7A8", "name": "BODY CONTROL MODULE",
                      "in_legislated_range": false, "faults": [], "fault_count": 0,
                      "emissions_codes_read": null, "note": null },
                ] }
            }),
            "read_dtcs" => json!({
                "tool": "read_dtcs", "success": true, "raw_evidence_ref": 233,
                "data": {
                    "dtcs": self.codes.iter().map(|code| json!({
                        "code": code, "status": "confirmed", "module": "ECU_7E8"
                    })).collect::<Vec<_>>(),
                    "confirmed_count": self.codes.len(),
                    "modules_read": ["ECU_7E8", "ECU_768", "ECU_7A8"],
                }
            }),
            other => json!({
                "tool": other, "success": false,
                "error": { "code": "operation_not_allowed", "message": "not in this script" }
            }),
        }
    }
}

/// What the assistant answered, word for word, on 2026-10-05.
const NOTHING_IN_THE_ENGINE: &str = "Nothing in the engine module, nothing in the body module. \
                                     No engine or airbag faults anywhere.";

fn asked_about_driving_home() -> Vec<Message> {
    vec![
        Message::user("Is it safe to drive home?"),
        Message::assistant("Which warning lights are lit?"),
        Message::user("ABS light only"),
    ]
}

/// A model that reads only the full scan and reports on the engine's codes.
///
/// The full scan holds each module's UDS fault memory and none of the
/// emissions codes, so the engine module's empty list there says nothing
/// about P2463. The answer is handed back, the codes are read, and what
/// reaches the person is the answer written after reading them.
#[tokio::test]
async fn no_engine_faults_after_only_a_full_scan_is_handed_back_for_the_read() {
    let provider = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        say(NOTHING_IN_THE_ENGINE),
        tool_call("read_dtcs", json!({})),
        say("The ABS light is the wheel speed fault, C0035. The engine module also has P2463 \
             stored: the exhaust filter is clogging."),
    ]);
    let mut truck = TruckWithEngineCodes::new();

    let out = Agent::new(&provider)
        .chat("system", asked_about_driving_home(), &mut truck, &mut NullSink)
        .await
        .expect("the turn completes");

    assert_eq!(truck.calls, vec!["scan_all_modules", "read_dtcs"]);
    assert!(out.text.contains("P2463"), "{}", out.text);
    assert!(!out.text.contains("No engine or airbag faults"), "{}", out.text);
    assert_eq!(provider.remaining(), 0);

    // What the model was told, after the answer that was not delivered.
    let requests = provider.requests();
    let told = requests[2].messages.last().expect("a message").text();
    assert!(told.contains("was not shown to the person"), "{told}");
    assert!(told.contains("`read_dtcs`"), "{told}");
    assert!(told.contains("ECU_7E8"), "the engine module is named: {told}");
    assert!(!told.contains("ECU_768"), "a module that keeps no emissions codes is not: {told}");
}

/// A model that is told, and says it again without reading anything.
///
/// It has had its turn to make the read. The claim still does not go out on
/// its own: the application says first, in its own words, what was not read,
/// and the model's answer follows exactly as it wrote it.
#[tokio::test]
async fn a_model_that_says_it_again_does_not_get_it_through_unchanged() {
    let provider = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        say(NOTHING_IN_THE_ENGINE),
        say(NOTHING_IN_THE_ENGINE),
    ]);
    let mut truck = TruckWithEngineCodes::new();

    let out = Agent::new(&provider)
        .chat("system", asked_about_driving_home(), &mut truck, &mut NullSink)
        .await
        .expect("the turn completes");

    assert_eq!(truck.calls, vec!["scan_all_modules"], "it never read the codes");
    assert_ne!(out.text, NOTHING_IN_THE_ENGINE);
    assert!(out.text.starts_with("A note from the app, not the assistant"), "{}", out.text);
    assert!(out.text.contains("ECU_7E8"), "{}", out.text);
    assert!(out.text.ends_with(NOTHING_IN_THE_ENGINE), "the answer itself is not rewritten");
    assert_eq!(out.steps, 3, "handed back once, not until the budget is gone");
}

/// The same answer on the last step, with no turn left to hand it back in.
#[tokio::test]
async fn the_claim_is_caught_with_no_turn_left_to_hand_it_back() {
    let provider = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        say(NOTHING_IN_THE_ENGINE),
    ]);
    let mut truck = TruckWithEngineCodes::new();

    let out = Agent::new(&provider)
        .with_max_steps(2)
        .chat("system", asked_about_driving_home(), &mut truck, &mut NullSink)
        .await
        .expect("the turn completes");

    assert!(out.text.starts_with("A note from the app, not the assistant"), "{}", out.text);
    assert_eq!(provider.remaining(), 0);
}

/// No read at all, and "no faults" from the conversation so far.
///
/// Only prose comes back between turns, so what an earlier answer said is not
/// in front of the model as a tool result and is not a read.
#[tokio::test]
async fn no_faults_with_nothing_read_this_turn_is_handed_back_too() {
    let provider = ScriptedProvider::new(vec![
        say("As I said, there are no stored codes, so yes."),
        tool_call("read_dtcs", json!({})),
        say("I read them again: the engine module has P2463."),
    ]);
    let mut truck = TruckWithEngineCodes::new();

    let out = Agent::new(&provider)
        .chat("system", vec![Message::user("So it is fine to drive?")], &mut truck, &mut NullSink)
        .await
        .expect("the turn completes");

    assert_eq!(truck.calls, vec!["read_dtcs"]);
    assert!(out.text.contains("P2463"), "{}", out.text);
}

/// The check is on the claim, not on the scan. What a full scan did find is
/// the model's to report, and so is "no codes" once the codes were read.
#[tokio::test]
async fn an_answer_with_the_read_behind_it_goes_through_as_written() {
    // A full scan, and an answer that says what it found and no more.
    let found = "The ABS light is real: the module at 768 has C0035 failing right now.";
    let provider =
        ScriptedProvider::new(vec![tool_call("scan_all_modules", json!({})), say(found)]);
    let out = Agent::new(&provider)
        .chat("system", asked_about_driving_home(), &mut TruckWithEngineCodes::new(), &mut NullSink)
        .await
        .expect("the turn completes");
    assert_eq!(out.text, found);
    assert_eq!(out.steps, 2, "nothing was handed back");

    // Both reads, on a vehicle with nothing stored, and "no codes" said of it.
    let clean = "I read every module's trouble codes and there are no stored codes.";
    let provider = ScriptedProvider::new(vec![
        tool_call("scan_all_modules", json!({})),
        tool_call("read_dtcs", json!({})),
        say(clean),
    ]);
    let mut truck = TruckWithEngineCodes::with_nothing_stored();
    let out = Agent::new(&provider)
        .chat("system", vec![Message::user("Any codes?")], &mut truck, &mut NullSink)
        .await
        .expect("the turn completes");
    assert_eq!(out.text, clean);
    assert_eq!(out.steps, 3);
}
