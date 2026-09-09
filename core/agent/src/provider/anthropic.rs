//! Anthropic Messages API.
//!
//! Raw HTTP: there is no official Anthropic SDK for Rust, and a community
//! wrapper would be a dependency between this code and the wire format it
//! already has to understand. The shapes here follow the documented
//! `POST /v1/messages` contract.

use super::{
    status_error, transport_error, ChatRequest, ChatResponse, Content, LlmProvider, Message,
    ProviderInfo, Role, StopReason, ToolSpec, Usage,
};
use crate::error::{AgentError, Secret};
use crate::settings::Speed;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, Instant};

/// The API version header. Pinned: Anthropic treats this as the contract, and
/// tracking it silently would mean the wire format could change underneath us.
const API_VERSION: &str = "2023-06-01";

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The default model.
///
/// Diagnostic reasoning over tool results is exactly the kind of work the
/// strongest model earns its cost on, so this does not default to a cheaper
/// tier. The user can pick another in settings.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// An Anthropic-backed provider.
pub struct AnthropicProvider {
    id: String,
    label: String,
    model: String,
    base_url: String,
    api_key: Secret,
    speed: Speed,
    http: reqwest::Client,
}

impl AnthropicProvider {
    /// Build a provider from a configured endpoint.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        api_key: Secret,
        speed: Speed,
    ) -> Result<Self, AgentError> {
        let label = label.into();
        if api_key.is_empty() {
            return Err(AgentError::MissingCredential { provider: label });
        }
        Ok(Self {
            id: id.into(),
            label,
            model: model.into(),
            base_url: base_url
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key,
            speed,
            http: reqwest::Client::builder()
                // Diagnostic turns think for a while. The default would cut
                // off a long reasoning turn and look like a network fault.
                .timeout(Duration::from_secs(600))
                .build()
                .map_err(|e| AgentError::Settings(e.to_string()))?,
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, AgentError> {
        let endpoint = format!("{}/v1/messages", self.base_url);

        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": request.messages.iter().map(to_wire_message).collect::<Vec<_>>(),
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(to_wire_tool).collect::<Vec<_>>());
        }
        // Lower the effort rather than disabling thinking. Anthropic documents
        // that thinking-off on Opus 5 can make the model write a tool call into
        // its visible text instead of emitting one - the turn succeeds, the
        // call never runs, and nothing errors. In an agent loop that is a silent
        // wrong answer, which is the one failure mode this project cannot have.
        if self.speed.suppress_thinking() {
            body["output_config"] = json!({ "effort": "low" });
        }

        let res = self
            .http
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;

        let status = res.status().as_u16();
        let retry_after = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let text = res.text().await.unwrap_or_default();

        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, retry_after));
        }

        let parsed: WireResponse = serde_json::from_str(&text).map_err(|e| {
            AgentError::MalformedResponse { provider: self.label.clone(), detail: e.to_string() }
        })?;

        // A refusal is a successful call in which the model declined. Surface
        // it as its own thing so the UI can say so rather than showing an
        // empty answer or a spurious network error.
        if parsed.stop_reason.as_deref() == Some("refusal") {
            return Err(AgentError::Refused {
                category: parsed.stop_details.as_ref().and_then(|d| d.category.clone()),
                explanation: parsed.stop_details.as_ref().and_then(|d| d.explanation.clone()),
            });
        }

        Ok(ChatResponse {
            model: parsed.model.unwrap_or_else(|| self.model.clone()),
            content: parsed.content.into_iter().filter_map(from_wire_content).collect(),
            stop_reason: match parsed.stop_reason.as_deref() {
                Some("end_turn") => StopReason::EndTurn,
                Some("tool_use") => StopReason::ToolUse,
                Some("max_tokens") => StopReason::MaxTokens,
                Some(other) => StopReason::Other(other.to_string()),
                None => StopReason::EndTurn,
            },
            usage: Usage {
                input_tokens: parsed.usage.input_tokens,
                output_tokens: parsed.usage.output_tokens,
                cache_read_tokens: parsed.usage.cache_read_input_tokens,
            },
        })
    }

    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        // `GET /v1/models` verifies the key and the endpoint without spending
        // output tokens, which matters when someone is testing a paste.
        let endpoint = format!("{}/v1/models", self.base_url);
        let started = Instant::now();

        let res = self
            .http
            .get(&endpoint)
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", API_VERSION)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;

        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }

        let models = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("data").cloned())
            .and_then(|d| d.as_array().cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(ProviderInfo {
            reachable: true,
            models,
            elapsed_ms: started.elapsed().as_millis() as u64,
            detail: None,
        })
    }
}

// ---------------------------------------------------------------- wire types

fn to_wire_tool(t: &ToolSpec) -> serde_json::Value {
    json!({
        "name": t.name,
        "description": t.description,
        "input_schema": t.input_schema,
    })
}

fn to_wire_message(m: &Message) -> serde_json::Value {
    json!({
        "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
        "content": m.content.iter().map(to_wire_content).collect::<Vec<_>>(),
    })
}

fn to_wire_content(c: &Content) -> serde_json::Value {
    match c {
        Content::Text { text } => json!({ "type": "text", "text": text }),
        Content::ToolUse { id, name, input } => {
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        Content::ToolResult { tool_use_id, content, is_error } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
    }
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    model: Option<String>,
    #[serde(default)]
    content: Vec<WireContent>,
    stop_reason: Option<String>,
    #[serde(default)]
    stop_details: Option<WireStopDetails>,
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Debug, Deserialize)]
struct WireStopDetails {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    explanation: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Thinking blocks arrive when adaptive thinking is on. They are not part
    /// of the answer and are dropped rather than shown as content.
    #[serde(other)]
    Ignored,
}

fn from_wire_content(c: WireContent) -> Option<Content> {
    match c {
        WireContent::Text { text } => Some(Content::Text { text }),
        WireContent::ToolUse { id, name, input } => Some(Content::ToolUse { id, name, input }),
        WireContent::Ignored => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_is_caught_before_any_request() {
        let e = AnthropicProvider::new(
            "p1",
            "Anthropic",
            DEFAULT_MODEL,
            None,
            Secret::new("  "),
            Speed::Quality,
        );
        assert!(matches!(e.err(), Some(AgentError::MissingCredential { .. })));
    }

    #[test]
    fn tool_results_round_trip_to_the_wire_shape() {
        let m = Message::tool_results(vec![Content::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "{\"dtcs\":[]}".into(),
            is_error: false,
        }]);
        let w = to_wire_message(&m);
        assert_eq!(w["role"], "user");
        assert_eq!(w["content"][0]["type"], "tool_result");
        assert_eq!(w["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(w["content"][0]["is_error"], false);
    }

    #[test]
    fn parses_a_tool_use_reply() {
        let body = r#"{
          "model": "claude-opus-5",
          "content": [
            {"type":"thinking","thinking":"..."},
            {"type":"text","text":"Let me read the codes."},
            {"type":"tool_use","id":"toolu_9","name":"read_dtcs","input":{"module":"ECU_7E8"}}
          ],
          "stop_reason": "tool_use",
          "usage": {"input_tokens": 10, "output_tokens": 20}
        }"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        let content: Vec<Content> =
            parsed.content.into_iter().filter_map(from_wire_content).collect();
        // The thinking block is dropped, not rendered as an empty answer.
        assert_eq!(content.len(), 2);
        assert!(matches!(&content[0], Content::Text { text } if text.contains("read the codes")));
        assert!(matches!(&content[1], Content::ToolUse { name, .. } if name == "read_dtcs"));
    }

    #[test]
    fn unknown_content_blocks_do_not_break_parsing() {
        // A future block type must not turn a good answer into a hard error.
        let body = r#"{"content":[{"type":"something_new","x":1},{"type":"text","text":"ok"}],
                       "stop_reason":"end_turn","usage":{}}"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        let content: Vec<Content> =
            parsed.content.into_iter().filter_map(from_wire_content).collect();
        assert_eq!(content, vec![Content::text("ok")]);
    }
}
