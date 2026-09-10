//! OpenAI-compatible Chat Completions.
//!
//! This one adapter covers most of what anyone actually runs: Ollama's `/v1`
//! shim, vLLM, llama.cpp's server, LM Studio, text-generation-webui, OpenRouter,
//! Together, Groq, and OpenAI itself. They differ in what they are good at, not
//! in the shape of the request.
//!
//! The important asymmetry with Anthropic: tool arguments arrive as a JSON
//! **string** rather than an object, and small local models are quite capable of
//! putting malformed JSON in it. That is parsed defensively here and reported as
//! a tool error the model can see and correct, rather than crashing the loop.

use super::{
    status_error, transport_error, ChatRequest, ChatResponse, Content, LlmProvider, Message,
    ProviderInfo, Role, StopReason, ToolSpec, Usage,
};
use crate::error::{AgentError, Secret};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

/// An OpenAI-compatible endpoint.
pub struct OpenAiProvider {
    id: String,
    label: String,
    model: String,
    base_url: String,
    api_key: Option<Secret>,
    http: reqwest::Client,
}

impl OpenAiProvider {
    /// Build a provider.
    ///
    /// `api_key` is optional: a self-hosted endpoint on the home network
    /// usually has none, and demanding one would make the common local case
    /// impossible to configure.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        model: impl Into<String>,
        base_url: &str,
        api_key: Option<Secret>,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            id: id.into(),
            label: label.into(),
            model: model.into(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.filter(|k| !k.is_empty()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .map_err(|e| AgentError::Settings(e.to_string()))?,
        })
    }

    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => rb.header("authorization", format!("Bearer {}", k.expose())),
            None => rb,
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
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
        let endpoint = format!("{}/chat/completions", self.base_url);

        let mut messages = vec![json!({ "role": "system", "content": request.system })];
        for m in &request.messages {
            messages.extend(to_wire_messages(m));
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(to_wire_tool).collect::<Vec<_>>());
            body["tool_choice"] = json!("auto");
        }

        let res = self
            .authed(self.http.post(&endpoint))
            .header("content-type", "application/json")
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

        let choice =
            parsed.choices.into_iter().next().ok_or_else(|| AgentError::MalformedResponse {
                provider: self.label.clone(),
                detail: "the response contained no choices".into(),
            })?;

        let mut content = Vec::new();
        if let Some(t) = choice.message.content.filter(|t| !t.trim().is_empty()) {
            content.push(Content::Text { text: t });
        }
        for call in choice.message.tool_calls.unwrap_or_default() {
            content.push(Content::ToolUse {
                id: call.id,
                name: call.function.name,
                input: parse_arguments(&call.function.arguments),
            });
        }

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("stop") | None => {
                // Some servers report "stop" even when they emitted tool calls.
                // Trust the content over the label, or the loop stalls with a
                // pending call nobody runs.
                if content.iter().any(|c| matches!(c, Content::ToolUse { .. })) {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                }
            }
            Some(other) => StopReason::Other(other.to_string()),
        };

        Ok(ChatResponse {
            model: parsed.model.unwrap_or_else(|| self.model.clone()),
            content,
            stop_reason,
            usage: Usage {
                input_tokens: parsed.usage.as_ref().map_or(0, |u| u.prompt_tokens),
                output_tokens: parsed.usage.as_ref().map_or(0, |u| u.completion_tokens),
                cache_read_tokens: 0,
            },
        })
    }

    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        let endpoint = format!("{}/models", self.base_url);
        let started = Instant::now();

        let res = self
            .authed(self.http.get(&endpoint))
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;

        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }

        let models: Vec<String> = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Naming a model the endpoint does not have is the single most common
        // misconfiguration here, and it otherwise only shows up as a 404 on the
        // first real question. Say it now.
        let detail = if !models.is_empty() && !models.iter().any(|m| m == &self.model) {
            Some(format!(
                "this endpoint does not list \"{}\"; it offers: {}",
                self.model,
                models.join(", ")
            ))
        } else {
            None
        };

        Ok(ProviderInfo {
            reachable: true,
            models,
            elapsed_ms: started.elapsed().as_millis() as u64,
            detail,
        })
    }
}

// ---------------------------------------------------------------- wire types

fn to_wire_tool(t: &ToolSpec) -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": t.name,
            "description": t.description,
            "parameters": t.input_schema,
        }
    })
}

/// One neutral message can become several wire messages: this dialect wants one
/// `role: "tool"` entry per tool result, where Anthropic takes them as a batch.
fn to_wire_messages(m: &Message) -> Vec<serde_json::Value> {
    let mut out = Vec::new();

    let text: String = m
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let tool_calls: Vec<serde_json::Value> = m
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": input.to_string() },
            })),
            _ => None,
        })
        .collect();

    match m.role {
        Role::Assistant => {
            let mut msg = json!({ "role": "assistant" });
            msg["content"] = if text.is_empty() { json!(null) } else { json!(text) };
            if !tool_calls.is_empty() {
                msg["tool_calls"] = json!(tool_calls);
            }
            out.push(msg);
        }
        Role::User => {
            let results: Vec<&Content> =
                m.content.iter().filter(|c| matches!(c, Content::ToolResult { .. })).collect();

            if results.is_empty() {
                out.push(json!({ "role": "user", "content": text }));
            } else {
                for c in results {
                    if let Content::ToolResult { tool_use_id, content, .. } = c {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": content,
                        }));
                    }
                }
                if !text.is_empty() {
                    out.push(json!({ "role": "user", "content": text }));
                }
            }
        }
    }
    out
}

/// Tool arguments arrive as a JSON string. Small models get this wrong often
/// enough that a parse failure must be data, not a panic: an object carrying the
/// raw text lets schema validation reject it and tell the model what it sent.
fn parse_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return json!({});
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) if v.is_object() => v,
        _ => json!({ "__unparsed_arguments": raw }),
    }
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    model: Option<String>,
    #[serde(default)]
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
    message: WireMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFunction,
}

#[derive(Debug, Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_tool_arguments_become_data_not_a_crash() {
        // A local model emitting broken JSON is routine; it must survive it.
        let v = parse_arguments("{module: ECU_7E8");
        assert_eq!(v["__unparsed_arguments"], "{module: ECU_7E8");
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments(r#"{"module":"ECU_7E8"}"#)["module"], "ECU_7E8");
    }

    #[test]
    fn a_bare_array_is_not_accepted_as_arguments() {
        // Arguments must be an object; anything else fails schema validation
        // with the raw text visible rather than being silently reshaped.
        let v = parse_arguments("[1,2,3]");
        assert!(v.get("__unparsed_arguments").is_some());
    }

    #[test]
    fn tool_results_become_one_wire_message_each() {
        let m = Message::tool_results(vec![
            Content::ToolResult { tool_use_id: "a".into(), content: "1".into(), is_error: false },
            Content::ToolResult { tool_use_id: "b".into(), content: "2".into(), is_error: true },
        ]);
        let wire = to_wire_messages(&m);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0]["role"], "tool");
        assert_eq!(wire[0]["tool_call_id"], "a");
        assert_eq!(wire[1]["tool_call_id"], "b");
    }

    #[test]
    fn assistant_tool_calls_serialise_arguments_as_a_string() {
        let m = Message {
            role: Role::Assistant,
            content: vec![Content::ToolUse {
                id: "call_1".into(),
                name: "read_dtcs".into(),
                input: json!({"module": "ECU_7E8"}),
            }],
        };
        let wire = to_wire_messages(&m);
        assert_eq!(wire[0]["tool_calls"][0]["function"]["arguments"], r#"{"module":"ECU_7E8"}"#);
    }

    #[test]
    fn tool_calls_win_over_a_stop_finish_reason() {
        // Several local servers label a tool-call turn "stop". Believing the
        // label would leave a pending call nobody runs.
        let body = r#"{"model":"m","choices":[{"message":{"content":null,
            "tool_calls":[{"id":"c1","type":"function",
            "function":{"name":"scan_modules","arguments":"{}"}}]},
            "finish_reason":"stop"}]}"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        let choice = parsed.choices.into_iter().next().unwrap();
        assert!(choice.message.tool_calls.is_some());
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
    }
}
