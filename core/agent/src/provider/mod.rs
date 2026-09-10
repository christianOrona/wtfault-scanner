//! The model-provider seam.
//!
//! One vocabulary — messages, tool specs, tool calls, stop reasons — that every
//! backend maps onto. The agent loop is written against this and never against
//! a vendor's wire format, which is what makes "Anthropic today, the box in the
//! garage tomorrow" a configuration change rather than a rewrite.

pub mod anthropic;
pub mod ollama;
pub mod openai;

use crate::error::AgentError;
use serde::{Deserialize, Serialize};

/// One block of content in a conversation.
///
/// This is the intersection of what the providers can express, not the union.
/// Anything a single vendor supports and the others cannot is deliberately
/// absent: the agent must behave the same whichever model is answering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// The model asking for a tool to be run.
    ToolUse {
        /// Correlates the call with its result.
        id: String,
        /// Which tool.
        name: String,
        /// Arguments, still to be validated against the tool's schema.
        input: serde_json::Value,
    },
    /// What the tool returned.
    ToolResult {
        /// The `id` of the `ToolUse` this answers.
        tool_use_id: String,
        /// The result, serialised for the model.
        content: String,
        /// Whether the tool failed. Failures are reported, never hidden — a
        /// model that is not told a read failed will reason as though it worked.
        is_error: bool,
    },
}

impl Content {
    /// Convenience for the common case.
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text { text: s.into() }
    }
}

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The user, or a tool result being handed back.
    User,
    /// The model.
    Assistant,
}

/// One turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who is speaking.
    pub role: Role,
    /// What they said.
    pub content: Vec<Content>,
}

impl Message {
    /// A user turn carrying plain text.
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: Role::User, content: vec![Content::text(text)] }
    }

    /// An assistant turn carrying plain text.
    pub fn assistant(text: impl Into<String>) -> Self {
        Message { role: Role::Assistant, content: vec![Content::text(text)] }
    }

    /// A user turn carrying tool results.
    ///
    /// All results for one assistant turn belong in a single message. Splitting
    /// them across several teaches the model to stop asking for tools in
    /// parallel, which costs a round trip per reading on a slow adapter.
    pub fn tool_results(results: Vec<Content>) -> Self {
        Message { role: Role::User, content: results }
    }

    /// Concatenated text of this message, ignoring tool traffic.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A tool offered to the model, as JSON Schema.
///
/// These are generated from the registry in `aim-tools` rather than written
/// here, so the model can only ever be offered operations the safety gate
/// already knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name, matching the registry.
    pub name: String,
    /// What it does, and what it will not do.
    pub description: String,
    /// JSON Schema for the arguments.
    pub input_schema: serde_json::Value,
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// It finished its turn.
    EndTurn,
    /// It wants tools run before it continues.
    ToolUse,
    /// It hit the output ceiling. The answer is truncated.
    MaxTokens,
    /// A safety classifier declined. The call succeeded; the model said no.
    Refusal,
    /// Something else the provider reported.
    Other(String),
}

/// Token accounting, for showing the user what a scan cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens sent.
    pub input_tokens: u64,
    /// Tokens generated.
    pub output_tokens: u64,
    /// Tokens read from a prompt cache, where the provider reports it.
    pub cache_read_tokens: u64,
}

/// What to ask a model for.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// The system prompt.
    pub system: String,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Tools the model may call. Empty means it must answer in prose.
    pub tools: Vec<ToolSpec>,
    /// Output ceiling.
    pub max_tokens: u32,
}

/// What came back.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    /// The model that actually answered — not necessarily the one asked for,
    /// if the provider fell back.
    pub model: String,
    /// The reply.
    pub content: Vec<Content>,
    /// Why it stopped.
    pub stop_reason: StopReason,
    /// What it cost.
    pub usage: Usage,
}

impl ChatResponse {
    /// The tool calls in this reply, in order.
    pub fn tool_calls(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|c| match c {
                Content::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }

    /// The prose in this reply.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

/// What a provider reports about itself when probed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Whether it answered at all.
    pub reachable: bool,
    /// Models it says it has, when it will say.
    pub models: Vec<String>,
    /// How long the probe took.
    pub elapsed_ms: u64,
    /// Anything worth showing the user — a version banner, a caveat.
    pub detail: Option<String>,
}

/// A backend that can answer a [`ChatRequest`].
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Stable id of the configured provider this instance came from.
    fn id(&self) -> &str;

    /// Human-facing name, for error messages.
    fn label(&self) -> &str;

    /// The model this instance will ask for.
    fn model(&self) -> &str;

    /// Ask the model.
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, AgentError>;

    /// Check the endpoint is there and see what it offers.
    ///
    /// Separate from [`LlmProvider::chat`] so the settings UI can verify a
    /// configuration without spending tokens.
    async fn probe(&self) -> Result<ProviderInfo, AgentError>;
}

/// Shared helper: turn a `reqwest` failure into the right [`AgentError`].
pub(crate) fn transport_error(provider: &str, endpoint: &str, e: reqwest::Error) -> AgentError {
    AgentError::Unreachable {
        provider: provider.to_string(),
        endpoint: endpoint.to_string(),
        detail: e.to_string(),
    }
}

/// Shared helper: classify a non-2xx response.
pub(crate) fn status_error(
    provider: &str,
    status: u16,
    body: &str,
    retry_after: Option<u64>,
) -> AgentError {
    match status {
        401 | 403 => AgentError::Unauthorized { provider: provider.to_string(), status },
        429 => AgentError::RateLimited {
            provider: provider.to_string(),
            retry_after_secs: retry_after,
        },
        _ => AgentError::Api {
            provider: provider.to_string(),
            status,
            message: extract_message(body),
        },
    }
}

/// Pull a human-readable message out of an error body, whatever its shape.
///
/// Providers disagree about where the message lives; falling back to the raw
/// body is better than reporting an empty string, because the raw body is at
/// least something the user can paste into a search.
fn extract_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        for path in [["error", "message"], ["error", "detail"]] {
            if let Some(s) = v.get(path[0]).and_then(|e| e.get(path[1])).and_then(|m| m.as_str()) {
                return s.to_string();
            }
        }
        for key in ["message", "error", "detail"] {
            if let Some(s) = v.get(key).and_then(|m| m.as_str()) {
                return s.to_string();
            }
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "no message".to_string()
    } else {
        trimmed.chars().take(400).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_provider_messages() {
        assert_eq!(
            extract_message(
                r#"{"error":{"type":"invalid_request_error","message":"max_tokens too large"}}"#
            ),
            "max_tokens too large"
        );
        assert_eq!(extract_message(r#"{"error":"model not found"}"#), "model not found");
    }

    #[test]
    fn falls_back_to_the_raw_body() {
        assert_eq!(extract_message("upstream exploded"), "upstream exploded");
        assert_eq!(extract_message("   "), "no message");
    }

    #[test]
    fn status_maps_to_the_right_variant() {
        assert!(matches!(status_error("p", 401, "", None), AgentError::Unauthorized { .. }));
        assert!(matches!(
            status_error("p", 429, "", Some(30)),
            AgentError::RateLimited { retry_after_secs: Some(30), .. }
        ));
        assert!(matches!(status_error("p", 500, "", None), AgentError::Api { status: 500, .. }));
    }

    #[test]
    fn tool_results_stay_in_one_message() {
        let m = Message::tool_results(vec![
            Content::ToolResult { tool_use_id: "a".into(), content: "1".into(), is_error: false },
            Content::ToolResult { tool_use_id: "b".into(), content: "2".into(), is_error: false },
        ]);
        assert_eq!(m.role, Role::User);
        assert_eq!(m.content.len(), 2);
    }
}
