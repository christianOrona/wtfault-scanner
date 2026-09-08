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

impl CoreExecutor {
    /// Build an executor over the shared state.
    ///
    /// Only enabled tools are offered. Describing a permanently refused
    /// operation to a model wastes turns on calls that can never succeed; the
    /// prompt states the read-only boundary in prose instead.
    pub fn new(state: &AppState) -> Self {
        let specs = state
            .tools
            .enabled()
            .into_iter()
            .map(|t| spec_from_schema(&t.name, &t.description, &t.returns, t.parameters.clone()))
            .collect();
        CoreExecutor { state: state.clone(), specs }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for CoreExecutor {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }

    async fn run(&mut self, name: &str, arguments: &Value, initiator: &str) -> Value {
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

        let result = self
            .state
            .with_service(move |svc| aim_tools::execute(svc, &registry, &call))
            .await;

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

    let (descriptor, conn_state, vin, modules) = snapshot
        .unwrap_or_else(|| ("none".into(), "disconnected".into(), None, Vec::new()));

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
    let ActiveProvider { provider, max_steps, max_tokens, purpose, tone } =
        active_provider(state)?;
    let system =
        prompts::inspection_system_prompt(&describe_context(state).await, purpose, tone);
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
    let ActiveProvider { provider, max_steps, max_tokens, purpose, tone } =
        active_provider(state)?;
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
