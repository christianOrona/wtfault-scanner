//! Wiring the agent to the diagnostic core.
//!
//! This module owns the one place the model's intent becomes a real request to
//! a vehicle. Everything the model asks for goes through `aim_tools::execute`,
//! which validates the name against the registry, checks arguments against the
//! schema, and runs the safety gate — so the boundary is enforced here, not
//! trusted to the prompt.

use crate::state::AppState;
use aim_agent::provider::ToolSpec;
use aim_agent::tools::spec_from_schema;
use aim_agent::{
    prompts, Agent, AgentError, AgentEvent, AgentOutcome, EventSink, LlmProvider, Message,
    ToolExecutor,
};
use aim_tools::ToolCall;
use serde_json::Value;

/// Executes tool calls against the connected vehicle.
pub struct CoreExecutor {
    state: AppState,
    specs: Vec<ToolSpec>,
}

/// The one tool that reaches a person instead of a vehicle.
///
/// Everything the vehicle can answer, the model asks the vehicle. This is for
/// what it cannot: whether the dash menu says the setting is on, whether the
/// noise happens cold or warm, whether the key is in the car. Those answers
/// exist only in somebody's head, and until now the model had to describe a
/// question in prose and hope for a typed reply in the right shape.
///
/// It is deliberately *not* a diagnostic capability and never goes through the
/// safety gate, because it does nothing: it posts a question and ends the
/// turn. The person's answer arrives as an ordinary next message, through the
/// same path as anything else they type — so nothing here can put words in
/// their mouth, and a button they did not press is a message that was never
/// sent.
pub const ASK_THE_PERSON: &str = "ask_the_person";

impl CoreExecutor {
    /// Build an executor over the shared state.
    ///
    /// Only enabled tools are offered. Describing a permanently refused
    /// operation to a model wastes turns on calls that can never succeed; the
    /// prompt states the read-only boundary in prose instead.
    pub fn new(state: &AppState) -> Self {
        let mut specs: Vec<ToolSpec> = state
            .tools
            .enabled()
            .into_iter()
            .map(|t| spec_from_schema(&t.name, &t.description, &t.returns, t.parameters.clone()))
            .collect();
        specs.push(ask_the_person_spec());
        CoreExecutor { state: state.clone(), specs }
    }
}

/// How the model is told to ask a person something.
fn ask_the_person_spec() -> ToolSpec {
    spec_from_schema(
        ASK_THE_PERSON,
        "Ask the person a question the vehicle cannot answer, offering them the answers to \
         choose from. Use this for anything only they can see or know: what a dash menu \
         currently shows, whether a noise happens cold or warm, whether the key is inside the \
         car, which of two symptoms came first. Do NOT use it for anything you could read from \
         the vehicle — read that instead. Offer between two and five concrete options, and \
         include an escape like \"I am not sure\" whenever being wrong would matter. Asking \
         ends your turn: you will not get an answer in this turn, you will get it as their next \
         message.",
        "Confirmation that the question was put to them. Not an answer — end your turn after \
         calling this.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question, in one sentence, in plain language."
                },
                "options": {
                    "type": "array",
                    "description": "The answers to offer, each a short phrase they would \
                                    recognise. Two to five.",
                    "items": { "type": "string" },
                    "minItems": 2,
                    "maxItems": 5
                },
                "why": {
                    "type": "string",
                    "description": "Optional. What the answer would let you work out, so they \
                                    know why it is worth answering."
                }
            },
            "required": ["question", "options"]
        }),
    )
}

/// The question the model most recently put to the person, if it put one.
///
/// Read back out of the run's own event log rather than carried on the
/// executor. The events are already the record of what the model did, and a
/// second copy on the side is a second thing that can disagree with the first.
///
/// Only the last one survives: a model that asks twice in one turn has asked
/// one question badly, and showing two sets of buttons would make the person
/// answer the wrong one.
pub fn pending_question(sink: &RecordingSink) -> Option<Value> {
    let args = sink.events.iter().rev().find_map(|e| match e {
        aim_agent::AgentEvent::ToolStarted { name, arguments } if name == ASK_THE_PERSON => {
            Some(arguments)
        }
        _ => None,
    })?;

    let question = args.get("question").and_then(Value::as_str)?.trim().to_string();
    if question.is_empty() {
        return None;
    }
    let options: Vec<String> = args
        .get("options")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    // A question with nothing to choose from is prose, and the model already
    // has prose. Dropping it leaves the question in the reply text where it
    // will read perfectly well.
    if options.len() < 2 {
        return None;
    }

    Some(serde_json::json!({
        "question": question,
        "options": options,
        "why": args.get("why").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()),
    }))
}

#[async_trait::async_trait]
impl ToolExecutor for CoreExecutor {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }

    async fn run(&mut self, name: &str, arguments: &Value, initiator: &str) -> Value {
        // Intercepted before the registry, because it is not a diagnostic
        // capability and there is nothing for the safety gate to authorise. It
        // touches no vehicle: the question is already recorded in the run's
        // events, and the interface lifts it from there.
        if name == ASK_THE_PERSON {
            return serde_json::json!({
                "tool": ASK_THE_PERSON,
                "success": true,
                "posted": true,
                "note": "The question has been put to them with those options. This is not an \
                         answer — end your turn now and their reply will arrive as the next \
                         message."
            });
        }

        let registry = std::sync::Arc::clone(&self.state.tools);
        let call = ToolCall {
            tool: name.to_string(),
            arguments: arguments.clone(),
            // Recorded in the flight recorder against every operation, so a
            // reading the agent asked for is distinguishable from one a person
            // clicked. Handoff §10 makes this mandatory.
            initiator: format!("agent:{initiator}"),
            confirmation: None,
        };

        let result =
            self.state.with_service(move |svc| aim_tools::execute(svc, &registry, &call)).await;

        match result {
            Ok(r) => serde_json::to_value(r).unwrap_or_else(|e| {
                serde_json::json!({ "success": false, "error": { "code": "internal",
                                    "message": format!("could not serialise the result: {e}") } })
            }),
            // Nothing connected, or the lock was poisoned. Reported in the same
            // envelope shape the model reads everything else in.
            Err(e) => {
                let inner = e.0;
                serde_json::json!({
                    "tool": name,
                    "success": false,
                    "error": { "code": inner.code, "message": inner.message }
                })
            }
        }
    }
}

/// Collects agent events so a completed run can report what it did.
#[derive(Default)]
pub struct RecordingSink {
    /// Everything that happened, in order.
    pub events: Vec<AgentEvent>,
}

impl EventSink for RecordingSink {
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

/// Build the context block describing what is currently connected.
pub async fn describe_context(state: &AppState) -> String {
    // One pass over the service: each `peek_service` takes the lock, and the
    // context must describe a single consistent moment rather than three.
    let snapshot = state
        .peek_service(|s| {
            let modules = s
                .store()
                .modules(s.session_id())
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.module_key)
                .collect::<Vec<_>>();
            (
                s.adapter_descriptor(),
                format!("{:?}", s.state()),
                s.vehicle().and_then(|v| v.vin.clone()),
                modules,
            )
        })
        .await
        .ok()
        .flatten();

    let (descriptor, conn_state, vin, modules) =
        snapshot.unwrap_or_else(|| ("none".into(), "disconnected".into(), None, Vec::new()));

    prompts::context_block(&descriptor, &conn_state, vin.as_deref(), None, &modules)
}

/// A live provider plus the per-provider limits the agent should run under.
pub struct ActiveProvider {
    /// The client to ask.
    pub provider: Box<dyn LlmProvider>,
    /// Step ceiling for one run, or `None` for the built-in default.
    pub max_steps: Option<usize>,
    /// Output-token ceiling per turn, or `None` for the built-in default.
    pub max_tokens: Option<u32>,
    /// Whether the user owns this vehicle or is considering buying it.
    pub purpose: aim_agent::settings::ScanPurpose,
    /// How direct the agent should be.
    pub tone: aim_agent::settings::Tone,
}

/// Resolve the configured provider and its per-provider tuning.
pub fn active_provider(state: &AppState) -> Result<ActiveProvider, AgentError> {
    let settings = state.settings.load()?;
    let config = settings.active().ok_or_else(|| {
        AgentError::NoProvider(if settings.providers.is_empty() {
            "add one in Settings: Anthropic with an API key, or an Ollama endpoint on your network"
                .into()
        } else {
            "several providers are configured but none is selected".into()
        })
    })?;
    Ok(ActiveProvider {
        provider: config.build()?,
        max_steps: config.max_steps,
        max_tokens: config.max_tokens,
        // Not per-provider: who is asking and how blunt to be is a property of
        // the person, not of which model happens to be selected.
        purpose: settings.purpose,
        tone: settings.tone,
    })
}

/// Run a pre-purchase inspection.
pub async fn run_inspection(
    state: &AppState,
    request: &str,
    sink: &mut dyn EventSink,
) -> Result<AgentOutcome, AgentError> {
    let ActiveProvider { provider, max_steps, max_tokens, purpose, tone } = active_provider(state)?;
    let system = prompts::inspection_system_prompt(&describe_context(state).await, purpose, tone);
    let mut executor = CoreExecutor::new(state);
    let mut agent = Agent::new(provider.as_ref());
    if let Some(n) = max_steps {
        agent = agent.with_max_steps(n);
    }
    if let Some(n) = max_tokens {
        agent = agent.with_max_tokens(n);
    }
    agent.inspect(&system, request, &mut executor, sink).await
}

/// Run one conversational turn.
pub async fn run_chat(
    state: &AppState,
    history: Vec<Message>,
    sink: &mut dyn EventSink,
) -> Result<AgentOutcome, AgentError> {
    let ActiveProvider { provider, max_steps, max_tokens, purpose, tone } = active_provider(state)?;
    let system = prompts::chat_system_prompt(&describe_context(state).await, purpose, tone);
    let mut executor = CoreExecutor::new(state);
    let mut agent = Agent::new(provider.as_ref());
    if let Some(n) = max_steps {
        agent = agent.with_max_steps(n);
    }
    if let Some(n) = max_tokens {
        agent = agent.with_max_tokens(n);
    }
    agent.chat(&system, history, &mut executor, sink).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_agent::{AgentEvent, EventSink};

    fn asked(arguments: serde_json::Value) -> RecordingSink {
        let mut sink = RecordingSink::default();
        sink.emit(AgentEvent::ToolStarted { name: ASK_THE_PERSON.to_string(), arguments });
        sink
    }

    /// The question is read back out of the run's own event log, which is
    /// already the record of what the model did. A second copy kept on the
    /// side is a second thing that can disagree with the first.
    #[test]
    fn a_question_is_lifted_from_the_events_the_run_already_recorded() {
        let sink = asked(serde_json::json!({
            "question": "Does your dash menu show AutoLock as on?",
            "options": ["On", "Off", "I cannot find the menu"],
            "why": "It tells me whether this mapping describes your truck."
        }));

        let q = pending_question(&sink).expect("a well-formed question survives");
        assert_eq!(q["question"], "Does your dash menu show AutoLock as on?");
        assert_eq!(q["options"].as_array().unwrap().len(), 3);
        assert!(q["why"].is_string());
    }

    /// Nothing was asked, so nothing is offered. The overwhelmingly common
    /// case, and the one where a stray empty question box would be worst.
    #[test]
    fn an_ordinary_turn_asks_nothing() {
        let mut sink = RecordingSink::default();
        sink.emit(AgentEvent::ToolStarted {
            name: "read_dtcs".into(),
            arguments: serde_json::json!({}),
        });
        assert!(pending_question(&sink).is_none());
    }

    /// A question with one answer is not a question, it is a prompt to agree.
    /// Dropped, which leaves it in the reply text where it reads perfectly
    /// well as prose.
    #[test]
    fn a_question_with_nothing_to_choose_between_is_dropped() {
        let sink = asked(serde_json::json!({ "question": "Is it on?", "options": ["Yes"] }));
        assert!(pending_question(&sink).is_none());

        let sink = asked(serde_json::json!({ "question": "Is it on?", "options": [] }));
        assert!(pending_question(&sink).is_none());
    }

    /// Empty and whitespace options are discarded rather than rendered as
    /// buttons with no label on them.
    #[test]
    fn blank_options_do_not_become_blank_buttons() {
        let sink = asked(serde_json::json!({
            "question": "Cold or warm?",
            "options": ["Cold", "   ", "", "Warm"]
        }));
        let q = pending_question(&sink).unwrap();
        assert_eq!(q["options"], serde_json::json!(["Cold", "Warm"]));
    }

    /// A model that asks twice in one turn has asked one question badly. Two
    /// sets of buttons would have the person answering the wrong one.
    #[test]
    fn only_the_last_question_of_a_turn_is_offered() {
        let mut sink = asked(serde_json::json!({
            "question": "First?", "options": ["a", "b"]
        }));
        sink.emit(AgentEvent::ToolStarted {
            name: ASK_THE_PERSON.to_string(),
            arguments: serde_json::json!({ "question": "Second?", "options": ["c", "d"] }),
        });
        assert_eq!(pending_question(&sink).unwrap()["question"], "Second?");
    }

    /// It is offered to the model alongside the diagnostic tools, and it is
    /// the only one of them that does not reach the vehicle.
    #[test]
    fn the_tool_describes_itself_as_ending_the_turn() {
        let spec = ask_the_person_spec();
        assert_eq!(spec.name, ASK_THE_PERSON);
        assert!(spec.description.contains("ends your turn"), "{}", spec.description);
        // The instruction that keeps it from replacing reads it should be
        // doing against the vehicle.
        assert!(spec.description.contains("could read from the vehicle"), "{}", spec.description);
    }
}
