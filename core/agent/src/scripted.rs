//! A provider that replays a script instead of calling a model.
//!
//! # Why this exists
//!
//! Two reasons, and the second is the important one.
//!
//! The cheap reason: running the agent against a real model costs money every
//! time, and a test suite that costs money per run is a test suite nobody runs.
//! This one is free and deterministic, so it can sit in CI.
//!
//! The real reason: **the guarantees this project makes must hold regardless of
//! what the model says.** Prompt instructions are not a security boundary, and
//! a model that follows them is not evidence that a model which does not would
//! be caught. The only way to know is to script the badly-behaved model and
//! watch what the rest of the system does with its output.
//!
//! So the interesting scripts here are not the well-behaved ones. They are a
//! model that claims a measurement it never took, invents a fault code, or
//! reaches for a tool it is not allowed to have.

use crate::provider::{
    ChatRequest, ChatResponse, Content, LlmProvider, ProviderInfo, StopReason, Usage,
};
use crate::AgentError;
use std::sync::Mutex;

/// Replays a fixed sequence of responses, one per call.
///
/// Records the requests it was given, so a test can also assert on what the
/// system asked for - which tools were offered, what the system prompt said -
/// rather than only on what came back.
pub struct ScriptedProvider {
    responses: Mutex<std::collections::VecDeque<ChatResponse>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl ScriptedProvider {
    /// Build one from the replies it should give, in order.
    pub fn new(responses: Vec<ChatResponse>) -> Self {
        ScriptedProvider { responses: Mutex::new(responses.into()), seen: Mutex::new(Vec::new()) }
    }

    /// Every request the agent made, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().expect("not poisoned").clone()
    }

    /// How many replies are still unused.
    ///
    /// A script with leftovers means the loop stopped earlier than the test
    /// expected, which is usually the interesting half of a failure.
    pub fn remaining(&self) -> usize {
        self.responses.lock().expect("not poisoned").len()
    }
}

/// A reply that is a single tool call.
pub fn tool_call(name: &str, input: serde_json::Value) -> ChatResponse {
    ChatResponse {
        model: "scripted".into(),
        content: vec![Content::ToolUse { id: format!("call_{name}"), name: name.into(), input }],
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

/// A reply that is plain prose.
pub fn say(text: &str) -> ChatResponse {
    ChatResponse {
        model: "scripted".into(),
        content: vec![Content::Text { text: text.into() }],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

#[async_trait::async_trait]
impl LlmProvider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    fn label(&self) -> &str {
        "scripted replay"
    }

    fn model(&self) -> &str {
        "scripted"
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, AgentError> {
        self.seen.lock().expect("not poisoned").push(request.clone());
        self.responses.lock().expect("not poisoned").pop_front().ok_or_else(|| {
            // A script that runs out is a test that did not predict the
            // loop, not a provider failure. Say so plainly rather than
            // returning something plausible.
            AgentError::MalformedResponse {
                provider: "scripted".into(),
                detail: "the script ran out of replies; the agent asked more times than the \
                             test expected"
                    .into(),
            }
        })
    }

    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        Ok(ProviderInfo {
            reachable: true,
            models: vec!["scripted".into()],
            elapsed_ms: 0,
            detail: Some("replaying a script; no model was called".into()),
        })
    }
}
