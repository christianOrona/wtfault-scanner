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

use aim_agent::report::{ConfidenceClass, Finding, Report, Severity, Source, Verdict};
use aim_agent::scripted::{say, tool_call, ScriptedProvider};
use serde_json::json;

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
