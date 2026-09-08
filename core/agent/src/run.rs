//! The agent loop.
//!
//! Handoff §7: request → hypothesis → lowest-risk evidence → tool call →
//! validate → more evidence or conclude. The loop itself is deliberately dull;
//! all the judgement is in the model, and all the safety is in the executor.
//!
//! Two properties matter more than elegance here:
//!
//! * **It always terminates.** A model that keeps asking for reads on a slow
//!   Bluetooth adapter can burn a long time and a lot of money. The step limit
//!   is a hard stop, and hitting it produces a partial answer rather than an
//!   exception.
//! * **A failed tool call is fed back, not thrown.** The vehicle not answering
//!   is the single most common thing that happens in a garage. The model has to
//!   see it, say so, and carry on with what it can read.

use crate::error::AgentError;
use crate::provider::{ChatRequest, Content, LlmProvider, Message, Role, StopReason, ToolSpec, Usage};
use crate::report::{self, Report};
use crate::tools::{summarise_result, ToolExecutor};
use serde_json::Value;

/// How many model turns one run may take before it is stopped.
///
/// Each step is at least one model call and usually several adapter round
/// trips, and every step also grows the context the next one has to reason
/// over. Fourteen covers a thorough inspection of a multi-module vehicle while
/// keeping the conversation short enough that a small local model is still
/// coherent when it comes to write the report.
pub const DEFAULT_MAX_STEPS: usize = 14;

/// Something that happened during a run, for the UI to show as it goes.
///
/// A scan takes minutes over Bluetooth. Without progress the user is watching a
/// spinner and cannot tell a slow adapter from a hung one.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The model said something on its way to an answer.
    Thinking(String),
    /// A tool is about to run.
    ToolStarted {
        /// Tool name.
        name: String,
        /// Arguments as the model gave them.
        arguments: Value,
    },
    /// A tool finished.
    ToolFinished {
        /// Tool name.
        name: String,
        /// Whether the envelope reported success.
        success: bool,
        /// Row id of the adapter exchange, when there was one.
        evidence_ref: Option<i64>,
    },
    /// The run is over.
    Finished,
}

/// Receives [`AgentEvent`]s as they happen.
pub trait EventSink: Send {
    /// Handle one event. Must not block for long: the loop is waiting.
    fn emit(&mut self, event: AgentEvent);
}

/// An event sink that discards everything, for tests and non-interactive runs.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: AgentEvent) {}
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct AgentOutcome {
    /// The model's prose answer. Empty for an inspection, which reports
    /// through [`AgentOutcome::report`] instead.
    pub text: String,
    /// The structured report, when the run was an inspection that finished one.
    pub report: Option<Report>,
    /// The full conversation, so a follow-up question can continue it.
    pub messages: Vec<Message>,
    /// Model turns taken.
    pub steps: usize,
    /// Tokens spent across every turn.
    pub usage: Usage,
    /// True when the step limit stopped the run before the model was done.
    pub truncated: bool,
}

/// Drives a model through a task.
pub struct Agent<'a> {
    provider: &'a dyn LlmProvider,
    max_steps: usize,
    max_tokens: u32,
}

impl<'a> Agent<'a> {
    /// Build an agent over a provider.
    pub fn new(provider: &'a dyn LlmProvider) -> Self {
        Agent { provider, max_steps: DEFAULT_MAX_STEPS, max_tokens: crate::DEFAULT_MAX_TOKENS }
    }

    /// Override the step limit.
    pub fn with_max_steps(mut self, steps: usize) -> Self {
        self.max_steps = steps.max(1);
        self
    }

    /// Override the per-turn output ceiling.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Run a conversational turn: gather evidence as needed, answer in prose.
    pub async fn chat(
        &self,
        system: &str,
        history: Vec<Message>,
        executor: &mut dyn ToolExecutor,
        sink: &mut dyn EventSink,
    ) -> Result<AgentOutcome, AgentError> {
        self.drive(system, history, executor.specs(), executor, sink, false).await
    }

    /// Run an inspection: gather evidence, then submit a structured report.
    pub async fn inspect(
        &self,
        system: &str,
        request: &str,
        executor: &mut dyn ToolExecutor,
        sink: &mut dyn EventSink,
    ) -> Result<AgentOutcome, AgentError> {
        let mut specs = executor.specs();
        specs.push(ToolSpec {
            name: report::SUBMIT_TOOL.to_string(),
            description: "Deliver the finished inspection report. Call this exactly once, \
                          when you have gathered enough evidence. This is the only way your \
                          findings reach the user."
                .to_string(),
            input_schema: report::submit_report_schema(),
        });
        self.drive(system, vec![Message::user(request)], specs, executor, sink, true).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &self,
        system: &str,
        mut messages: Vec<Message>,
        specs: Vec<ToolSpec>,
        executor: &mut dyn ToolExecutor,
        sink: &mut dyn EventSink,
        expect_report: bool,
    ) -> Result<AgentOutcome, AgentError> {
        let mut usage = Usage::default();
        let mut report: Option<Report> = None;
        let mut text = String::new();
        let mut steps = 0;
        let mut truncated = false;
        // Results already produced this run, keyed by call + arguments.
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        // Tool names actually executed, for the baseline check below.
        let mut ran_tools: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Distinct trouble codes the vehicle reported, so a report that omits
        // them can be caught. Keyed by code+status: the same code confirmed and
        // permanent is one fault seen twice, not two faults.
        let mut codes_found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // Code -> catalog description, for the evidence-only fallback report.
        let mut code_details: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();

        while steps < self.max_steps {
            steps += 1;

            // Warn before the budget runs out, not at the end of it.
            //
            // Measured on a 7B model against the simulator: left to itself it
            // re-read modules it had already read, and by step 20 was carrying
            // 76k tokens of context and had lost the thread entirely. Told
            // three steps early that it is running out, it converges. A model
            // cannot budget for a limit nobody mentioned.
            let warn_at = self.max_steps.saturating_sub(3);
            if expect_report && report.is_none() && steps == warn_at && warn_at > 1 {
                let left = self.max_steps - steps;
                messages.push(Message::user(format!(
                    "You have {left} steps left before you must report. Stop gathering new \
                     evidence unless it would change your conclusion, and do not re-read \
                     anything you have already read. Start deciding what to tell the buyer."
                )));
            }

            // Reserve the last step for the conclusion.
            //
            // On the final step the evidence tools are withdrawn and only
            // `submit_report` is offered, so the one remaining move is to
            // conclude with what it already has.
            let final_step = expect_report && report.is_none() && steps == self.max_steps;
            if final_step {
                messages.push(Message::user(format!(
                    "You are out of investigation steps. Do not call any more read tools. \
                     Call `{}` now with what you have already found. If the evidence was thin, \
                     say so in `not_checked` and use verdict \"inconclusive\" — an honest \
                     partial answer is what is wanted here, not a guess.",
                    report::SUBMIT_TOOL
                )));
            }

            let turn_specs: Vec<ToolSpec> = if final_step {
                specs.iter().filter(|s| s.name == report::SUBMIT_TOOL).cloned().collect()
            } else {
                specs.clone()
            };

            let request = ChatRequest {
                system: system.to_string(),
                messages: messages.clone(),
                tools: turn_specs,
                max_tokens: self.max_tokens,
            };

            let response = self.provider.chat(&request).await?;
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            usage.cache_read_tokens += response.usage.cache_read_tokens;

            let said = response.text();
            if !said.is_empty() {
                sink.emit(AgentEvent::Thinking(said.clone()));
                text = said;
            }

            let calls: Vec<(String, String, Value)> = response
                .tool_calls()
                .into_iter()
                .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
                .collect();

            if calls.is_empty() {
                // No tools wanted. For a chat turn that is the answer. For an
                // inspection the model has skipped the one thing it was told to
                // do, so ask once rather than silently returning prose that the
                // report UI has nowhere to put.
                if expect_report && report.is_none() && steps < self.max_steps {
                    messages.push(Message { role: Role::Assistant, content: response.content });
                    messages.push(Message::user(format!(
                        "You have not submitted the report. Call `{}` now with what you found. \
                         If you could not read enough, submit it with verdict \"inconclusive\" \
                         and say so in `not_checked`.",
                        report::SUBMIT_TOOL
                    )));
                    continue;
                }
                break;
            }

            messages.push(Message { role: Role::Assistant, content: response.content });

            // All results for one assistant turn go back in a single message —
            // splitting them teaches the model to stop batching, and each extra
            // turn is another slow round trip.
            let mut results = Vec::new();
            for (id, name, arguments) in calls {
                if name == report::SUBMIT_TOOL {
                    // An inspection that never read the trouble codes is not an
                    // inspection.
                    //
                    // Measured on qwen3:8b: it called `scan_modules` and
                    // `identify_vehicle`, never called `read_dtcs`, and then
                    // reported "no active diagnostic trouble codes were
                    // detected during the scan" on a truck with three stored
                    // codes — while its own `not_checked` said the codes had
                    // not been scanned. Telling a buyer a car is clean without
                    // looking is the exact failure this project exists to
                    // prevent, so it is refused here rather than argued about
                    // in the prompt.
                    if !final_step && !ran_tools.contains("read_dtcs") {
                        results.push(Content::ToolResult {
                            tool_use_id: id,
                            content: serde_json::json!({
                                "accepted": false,
                                "error": "You have not called `read_dtcs` yet, so you do not know \
                                          whether this vehicle has any stored faults. Read the \
                                          codes from every module you found, then submit. Do not \
                                          state that no codes were found unless you have read them."
                            })
                            .to_string(),
                            is_error: true,
                        });
                        continue;
                    }
                    match serde_json::from_value::<Report>(arguments.clone()) {
                        // Valid JSON is not the same as a usable report. An
                        // incoherent one is handed back once, exactly like a
                        // schema error, so the model can fix it rather than the
                        // user receiving it.
                        Ok(mut r) => match r.incoherence_against(&codes_found) {
                            Some(why) if !final_step => {
                                results.push(Content::ToolResult {
                                    tool_use_id: id,
                                    content: serde_json::json!({
                                        "accepted": false,
                                        "error": why,
                                    })
                                    .to_string(),
                                    is_error: true,
                                });
                            }
                            // On the final step there is no turn left to
                            // correct it. A flawed report beats none: the UI
                            // says plainly when findings are missing.
                            _ => {
                                r.normalise();
                                report = Some(r);
                                results.push(Content::ToolResult {
                                    tool_use_id: id,
                                    content: r#"{"accepted":true}"#.to_string(),
                                    is_error: false,
                                });
                            }
                        },
                        Err(e) => {
                            // Hand the schema error back so it can correct itself.
                            results.push(Content::ToolResult {
                                tool_use_id: id,
                                content: format!(
                                    r#"{{"accepted":false,"error":"report did not match the schema: {e}"}}"#
                                ),
                                is_error: true,
                            });
                        }
                    }
                    continue;
                }

                // Do not run the same call twice.
                //
                // Measured against a 7B model: it re-ran identify_vehicle,
                // scan_modules, read_dtcs and read_freeze_frame, spending half
                // its budget re-reading things it already knew. Each repeat is
                // also a real round trip over a slow Bluetooth adapter. Serving
                // the previous answer costs nothing and says plainly that the
                // work is already done.
                let key = format!("{name}:{arguments}");
                if let Some(previous) = seen.get(&key) {
                    results.push(Content::ToolResult {
                        tool_use_id: id,
                        content: format!(
                            r#"{{"already_run":true,"note":"You already ran this exact call. \
                               Here is what it returned. Do not run it again; use this, or read \
                               something different.","result":{previous}}}"#
                        ),
                        is_error: false,
                    });
                    continue;
                }

                sink.emit(AgentEvent::ToolStarted { name: name.clone(), arguments: arguments.clone() });

                let raw = executor.run(&name, &arguments, "agent").await;
                let success = raw.get("success").and_then(Value::as_bool).unwrap_or(false);
                let evidence_ref = raw.get("raw_evidence_ref").and_then(Value::as_i64);
                sink.emit(AgentEvent::ToolFinished { name: name.clone(), success, evidence_ref });

                let summary = summarise_result(&raw).to_string();
                seen.insert(key, summary.clone());
                if success {
                    ran_tools.insert(name.clone());
                    if let Some(list) = raw.get("data").and_then(|d| d.get("dtcs")).and_then(|d| d.as_array()) {
                        for d in list {
                            if let Some(code) = d.get("code").and_then(|c| c.as_str()) {
                                let status = d.get("status").and_then(|s| s.as_str()).unwrap_or("");
                                let _ = status;
                                let up = code.to_ascii_uppercase();
                                let desc = d.get("description").and_then(|x| x.as_str()).map(String::from);
                                // Keep the first description seen; the same code
                                // confirmed and permanent carries the same text.
                                code_details.entry(up.clone()).or_insert(desc);
                                codes_found.insert(up);
                            }
                        }
                    }
                }

                results.push(Content::ToolResult {
                    tool_use_id: id,
                    content: summary,
                    // `is_error` reflects the envelope. A vehicle that did not
                    // answer is a fact the model must see, not one to smooth over.
                    is_error: !success,
                });
            }

            messages.push(Message::tool_results(results));

            if report.is_some() {
                break;
            }

            if response.stop_reason == StopReason::MaxTokens {
                truncated = true;
                break;
            }
        }

        if steps >= self.max_steps && report.is_none() && expect_report {
            truncated = true;
            // The model never submitted anything. Rather than discard several
            // minutes of real adapter reads, hand back what was actually
            // measured, clearly labelled as unfinished. This carries no model
            // judgement at all - only codes the vehicle reported and the
            // descriptions this build ships.
            let codes: Vec<(String, Option<String>)> =
                code_details.into_iter().collect();
            report = Some(Report::from_evidence(&codes, steps));
        }

        sink.emit(AgentEvent::Finished);

        Ok(AgentOutcome { text, report, messages, steps, usage, truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatResponse, ProviderInfo};
    use serde_json::json;
    use std::sync::Mutex;

    /// A provider that replays a fixed script of turns.
    struct ScriptedProvider {
        turns: Mutex<Vec<ChatResponse>>,
    }

    impl ScriptedProvider {
        fn new(turns: Vec<ChatResponse>) -> Self {
            ScriptedProvider { turns: Mutex::new(turns) }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ScriptedProvider {
        fn id(&self) -> &str { "scripted" }
        fn label(&self) -> &str { "Scripted" }
        fn model(&self) -> &str { "scripted" }
        async fn chat(&self, _r: &ChatRequest) -> Result<ChatResponse, AgentError> {
            let mut t = self.turns.lock().unwrap();
            if t.is_empty() {
                // Never end a script by hanging: an exhausted script means the
                // loop asked for more turns than the test expected.
                return Ok(ChatResponse {
                    model: "scripted".into(),
                    content: vec![Content::text("done")],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                });
            }
            Ok(t.remove(0))
        }
        async fn probe(&self) -> Result<ProviderInfo, AgentError> {
            Ok(ProviderInfo { reachable: true, models: vec![], elapsed_ms: 0, detail: None })
        }
    }

    /// An executor that answers with canned envelopes and records what ran.
    struct FakeExecutor {
        calls: Vec<String>,
        answer: Value,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for FakeExecutor {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "read_dtcs".into(),
                description: "Read codes.".into(),
                input_schema: json!({"type":"object","properties":{"module":{"type":"string"}}}),
            }]
        }
        async fn run(&mut self, name: &str, _a: &Value, _i: &str) -> Value {
            self.calls.push(name.to_string());
            self.answer.clone()
        }
    }

    fn turn(content: Vec<Content>, stop: StopReason) -> ChatResponse {
        ChatResponse { model: "m".into(), content, stop_reason: stop, usage: Usage::default() }
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Content {
        Content::ToolUse { id: id.into(), name: name.into(), input }
    }

    fn ok_envelope() -> Value {
        json!({ "tool":"read_dtcs", "success":true, "raw_evidence_ref":103,
                "values":[], "warnings":[],
                "data":{"dtcs":[{"code":"P2463","status":"confirmed"}]} })
    }

    fn valid_report() -> Value {
        json!({
            "verdict": "negotiate",
            "headline": "The exhaust filter is clogging and will need work.",
            "summary": "One confirmed fault.",
            "findings": [{
                "title": "Exhaust filter clogging",
                "severity": "serious",
                "plain_english": "The filter that traps soot is blocked.",
                "source": "measured",
                "evidence": ["P2463 confirmed"],
                "evidence_refs": [103]
            }]
        })
    }

    #[tokio::test]
    async fn a_chat_turn_runs_tools_then_answers() {
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![Content::text("The engine module has one stored fault.")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider).chat("sys", vec![Message::user("what is wrong?")], &mut exec, &mut sink)
            .await
            .unwrap();

        assert_eq!(exec.calls, vec!["read_dtcs"]);
        assert!(out.text.contains("one stored fault"));
        assert_eq!(out.steps, 2);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn an_inspection_captures_the_submitted_report() {
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![tool_use("t2", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider).inspect("sys", "should I buy it?", &mut exec, &mut sink)
            .await
            .unwrap();

        let r = out.report.expect("no report");
        assert_eq!(r.verdict, report::Verdict::Negotiate);
        assert_eq!(r.findings[0].evidence_refs, vec![103]);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn a_failed_read_is_fed_back_rather_than_thrown() {
        let failed = json!({ "tool":"read_dtcs", "success":false, "raw_evidence_ref":441,
                             "values":[], "warnings":[],
                             "error":{"code":"vehicle_not_responding","message":"UNABLE TO CONNECT"} });
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({}))], StopReason::ToolUse),
            turn(vec![Content::text("The vehicle did not answer, so I could not read it.")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: failed };
        let mut sink = NullSink;

        let out = Agent::new(&provider).chat("sys", vec![Message::user("scan it")], &mut exec, &mut sink)
            .await
            .unwrap();

        assert!(out.text.contains("did not answer"));
        // The failure reached the model as an error-flagged tool result.
        let fed_back = out.messages.iter().any(|m| {
            m.content.iter().any(|c| matches!(c, Content::ToolResult { is_error: true, content, .. }
                if content.contains("vehicle_not_responding")))
        });
        assert!(fed_back, "the failure was not shown to the model");
    }

    #[tokio::test]
    async fn a_malformed_report_is_handed_back_for_correction() {
        let provider = ScriptedProvider::new(vec![
            // The codes have to be read before a report is accepted at all.
            turn(vec![tool_use("t0", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            // Missing the required `summary`.
            turn(vec![tool_use("t1", report::SUBMIT_TOOL, json!({"verdict":"negotiate","headline":"h"}))], StopReason::ToolUse),
            turn(vec![tool_use("t2", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider).inspect("sys", "check it", &mut exec, &mut sink).await.unwrap();
        assert!(out.report.is_some(), "the model was not given a chance to correct itself");
    }

    #[tokio::test]
    async fn the_step_limit_stops_a_model_that_never_finishes() {
        // A model stuck in a read loop must not run all night on a slow adapter,
        // and must not return nothing either.
        let mut turns: Vec<ChatResponse> = (0..2)
            .map(|i| turn(vec![tool_use(&format!("t{i}"), "read_dtcs", json!({}))], StopReason::ToolUse))
            .collect();
        // On the final step only `submit_report` is on offer, so a cooperative
        // model concludes rather than reading again.
        turns.push(turn(vec![tool_use("t9", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse));

        let provider = ScriptedProvider::new(turns);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider)
            .with_max_steps(3)
            .inspect("sys", "check it", &mut exec, &mut sink)
            .await
            .unwrap();

        assert_eq!(out.steps, 3);
        assert!(out.report.is_some(), "a truncated run must still produce a report");
    }

    #[tokio::test]
    async fn the_last_step_withdraws_the_read_tools() {
        // Proves the mechanism rather than the model's cooperation: on the
        // final step there is nothing to call except the report.
        use std::sync::{Arc, Mutex as StdMutex};

        struct SpyProvider {
            seen: Arc<StdMutex<Vec<Vec<String>>>>,
        }

        #[async_trait::async_trait]
        impl LlmProvider for SpyProvider {
            fn id(&self) -> &str { "spy" }
            fn label(&self) -> &str { "Spy" }
            fn model(&self) -> &str { "spy" }
            async fn chat(&self, r: &ChatRequest) -> Result<ChatResponse, AgentError> {
                self.seen.lock().unwrap().push(r.tools.iter().map(|t| t.name.clone()).collect());
                Ok(turn(vec![tool_use("t", "read_dtcs", json!({}))], StopReason::ToolUse))
            }
            async fn probe(&self) -> Result<ProviderInfo, AgentError> {
                Ok(ProviderInfo { reachable: true, models: vec![], elapsed_ms: 0, detail: None })
            }
        }

        let seen = Arc::new(StdMutex::new(Vec::new()));
        let provider = SpyProvider { seen: Arc::clone(&seen) };
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        Agent::new(&provider)
            .with_max_steps(2)
            .inspect("sys", "check it", &mut exec, &mut sink)
            .await
            .unwrap();

        let offered = seen.lock().unwrap().clone();
        assert!(offered[0].contains(&"read_dtcs".to_string()));
        assert_eq!(offered[1], vec![report::SUBMIT_TOOL.to_string()]);
    }

    #[tokio::test]
    async fn an_inspection_that_answers_in_prose_is_asked_for_the_report() {
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t0", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![Content::text("Looks fine to me.")], StopReason::EndTurn),
            turn(vec![tool_use("t1", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider).inspect("sys", "check it", &mut exec, &mut sink).await.unwrap();
        assert!(out.report.is_some());
    }

    #[tokio::test]
    async fn a_report_is_refused_until_the_codes_have_been_read() {
        // qwen3:8b did exactly this: scanned modules, read the VIN, never read
        // the codes, then reported "no active diagnostic trouble codes were
        // detected during the scan" on a truck with three stored codes.
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse),
            turn(vec![tool_use("t2", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![tool_use("t3", report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider).inspect("sys", "check it", &mut exec, &mut sink).await.unwrap();

        // The first submission was refused, so the codes did get read.
        assert!(exec.calls.contains(&"read_dtcs".to_string()));
        assert!(out.report.is_some());
        let refused = out.messages.iter().any(|m| {
            m.content.iter().any(|c| matches!(c, Content::ToolResult { content, is_error: true, .. }
                if content.contains("read_dtcs")))
        });
        assert!(refused, "the premature report was not refused");
    }

    #[tokio::test]
    async fn a_model_that_never_reports_still_returns_the_evidence() {
        // What the owner hit: 14 steps, 9 minutes of real reads, and the UI
        // showed "no report" with nothing else. The evidence was gathered and
        // then thrown away.
        let dtc_envelope = json!({
            "tool": "read_dtcs", "success": true, "raw_evidence_ref": 103,
            "values": [], "warnings": [],
            "data": { "dtcs": [
                { "code": "P2463", "status": "confirmed",
                  "description": "Diesel particulate filter restriction, soot accumulation" }
            ] }
        });
        // A model that reads, then only ever talks.
        let turns: Vec<ChatResponse> = vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![Content::text("I am still thinking about it.")], StopReason::EndTurn),
            turn(vec![Content::text("Still thinking.")], StopReason::EndTurn),
        ];
        let provider = ScriptedProvider::new(turns);
        let mut exec = FakeExecutor { calls: vec![], answer: dtc_envelope };
        let mut sink = NullSink;

        let out = Agent::new(&provider)
            .with_max_steps(3)
            .inspect("sys", "check it", &mut exec, &mut sink)
            .await
            .unwrap();

        let r = out.report.expect("evidence must survive a model that never reports");
        assert!(out.truncated, "the run must still be marked unfinished");
        assert_eq!(r.verdict, report::Verdict::Inconclusive);
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].source, report::Source::Catalog);
        assert!(r.findings[0].plain_english.contains("P2463"));
        assert!(r.findings[0].plain_english.contains("soot accumulation"));
        // It must not pretend to have concluded anything.
        assert!(r.headline.contains("did not finish"));
    }

    #[tokio::test]
    async fn a_silent_vehicle_still_produces_an_honest_empty_report() {
        let provider = ScriptedProvider::new(vec![
            turn(vec![Content::text("thinking")], StopReason::EndTurn),
            turn(vec![Content::text("thinking")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider)
            .with_max_steps(2)
            .inspect("sys", "check it", &mut exec, &mut sink)
            .await
            .unwrap();

        let r = out.report.expect("a report is always produced");
        assert!(r.findings.is_empty());
        assert!(r.headline.contains("nothing was concluded"));
    }

    #[tokio::test]
    async fn the_baseline_check_cannot_deadlock_the_final_step() {
        // A vehicle whose codes genuinely cannot be read must still produce a
        // report rather than looping until the budget runs out.
        let turns: Vec<ChatResponse> = (0..10)
            .map(|i| turn(vec![tool_use(&format!("t{i}"), report::SUBMIT_TOOL, valid_report())], StopReason::ToolUse))
            .collect();
        let provider = ScriptedProvider::new(turns);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider)
            .with_max_steps(3)
            .inspect("sys", "check it", &mut exec, &mut sink)
            .await
            .unwrap();

        assert!(out.report.is_some(), "the final step must accept a report regardless");
    }

    #[tokio::test]
    async fn an_identical_call_is_served_from_the_previous_result() {
        // Every repeat is a real round trip over a slow Bluetooth adapter, and
        // a 7B model repeats itself readily.
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![tool_use("t2", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![Content::text("done")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        let out = Agent::new(&provider)
            .chat("sys", vec![Message::user("go")], &mut exec, &mut sink)
            .await
            .unwrap();

        assert_eq!(exec.calls, vec!["read_dtcs"], "the vehicle was read twice");
        let told = out.messages.iter().any(|m| {
            m.content.iter().any(|c| matches!(c, Content::ToolResult { content, .. }
                if content.contains("already_run")))
        });
        assert!(told, "the model was not told it had already run that call");
    }

    #[tokio::test]
    async fn a_different_argument_is_not_treated_as_a_repeat() {
        let provider = ScriptedProvider::new(vec![
            turn(vec![tool_use("t1", "read_dtcs", json!({"module":"ECU_7E8"}))], StopReason::ToolUse),
            turn(vec![tool_use("t2", "read_dtcs", json!({"module":"ECU_7EA"}))], StopReason::ToolUse),
            turn(vec![Content::text("done")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = NullSink;

        Agent::new(&provider).chat("sys", vec![Message::user("go")], &mut exec, &mut sink).await.unwrap();
        assert_eq!(exec.calls.len(), 2, "a second module must still be read");
    }

    #[tokio::test]
    async fn progress_events_are_emitted_for_a_watching_ui() {
        struct Recorder(Vec<AgentEvent>);
        impl EventSink for Recorder {
            fn emit(&mut self, e: AgentEvent) { self.0.push(e); }
        }

        let provider = ScriptedProvider::new(vec![
            turn(vec![Content::text("Reading codes."), tool_use("t1", "read_dtcs", json!({}))], StopReason::ToolUse),
            turn(vec![Content::text("Done.")], StopReason::EndTurn),
        ]);
        let mut exec = FakeExecutor { calls: vec![], answer: ok_envelope() };
        let mut sink = Recorder(vec![]);

        Agent::new(&provider).chat("sys", vec![Message::user("go")], &mut exec, &mut sink).await.unwrap();

        assert!(sink.0.iter().any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "read_dtcs")));
        assert!(sink.0.iter().any(|e| matches!(e, AgentEvent::ToolFinished { success: true, evidence_ref: Some(103), .. })));
        assert_eq!(sink.0.last(), Some(&AgentEvent::Finished));
    }
}
