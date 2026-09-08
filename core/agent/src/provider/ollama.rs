//! Ollama's native API (`/api/chat`, `/api/tags`).
//!
//! Ollama also speaks the OpenAI dialect at `/v1`, and [`super::openai`] handles
//! that. This exists for the two things the native API does better: `/api/tags`
//! reports what is actually pulled onto the box (the `/v1` shim is vaguer), and
//! tool arguments come back as a real JSON object instead of a string that has
//! to be re-parsed and may be malformed.
//!
//! For a GPU box on the home network this is the better client of the two.

use super::{
    status_error, transport_error, ChatRequest, ChatResponse, Content, LlmProvider, Message,
    ProviderInfo, Role, StopReason, ToolSpec, Usage,
};
use crate::error::AgentError;
use crate::settings::Speed;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

/// Where Ollama listens by default.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Context window to ask Ollama for, when the provider does not name one.
///
/// Both extremes were measured on a 6 GB card, and both are bad:
///
/// * **4096** (Ollama's own default) truncates ~92% of an inspection. The model
///   never sees its instructions and cannot finish. Fast and useless.
/// * **40960** (qwen3:8b's maximum) works, but the KV cache pushes the model to
///   11 GB, so it runs 52% on CPU. Prompt processing is proportional to context
///   length, so each step costs more than the last: measured gaps between tool
///   calls grew 66s → 103s → 123s → 169s within a single run.
///
/// 16384 is the compromise. A successful inspection converged in 7 steps and
/// stayed well inside it, while leaving enough VRAM for more of the model to sit
/// on the GPU. Override per provider when the hardware allows more.
pub const DEFAULT_CONTEXT_TOKENS: u32 = 16_384;

/// An Ollama endpoint.
pub struct OllamaProvider {
    id: String,
    label: String,
    model: String,
    base_url: String,
    speed: Speed,
    context_tokens: u32,
    http: reqwest::Client,
}

impl OllamaProvider {
    /// Build a provider. Ollama has no authentication of its own.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        speed: Speed,
        context_tokens: Option<u32>,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            id: id.into(),
            label: label.into(),
            model: model.into(),
            base_url: base_url
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            speed,
            context_tokens: context_tokens.unwrap_or(DEFAULT_CONTEXT_TOKENS),
            http: reqwest::Client::builder()
                // A cold model has to be loaded into VRAM before the first
                // token; on a big model that alone can take minutes.
                .timeout(Duration::from_secs(900))
                .build()
                .map_err(|e| AgentError::Settings(e.to_string()))?,
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
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
        let endpoint = format!("{}/api/chat", self.base_url);

        let mut messages = vec![json!({ "role": "system", "content": request.system })];
        for m in &request.messages {
            messages.extend(to_wire_messages(m));
        }

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            // Non-streaming: the agent loop needs a whole turn before it can
            // do anything with it. Streaming belongs in the chat UI, not here.
            "stream": false,
            "options": {
                "num_predict": request.max_tokens,
                // THE context setting, and the single worst default in this
                // stack. Ollama uses 4096 tokens unless told otherwise, no
                // matter what the model supports — qwen3:8b handles 40960.
                //
                // An inspection conversation reaches ~50k tokens, so Ollama was
                // silently discarding ~92% of it: the system prompt, the tool
                // results, and the instruction to submit the report were all
                // truncated away before the model saw them. Both qwen3:8b and
                // gpt-oss:20b then failed identically — 14 steps, no report —
                // which looked like two models being too small and was in fact
                // one bad default.
                //
                // Nothing warns you. `ollama ps` reports CONTEXT 4096 and the
                // request succeeds.
                "num_ctx": self.context_tokens,
            },
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(to_wire_tool).collect::<Vec<_>>());
        }
        // Reasoning models (Qwen3 and friends) think before every turn, and on
        // modest hardware that is where nearly all the wall clock goes. Older
        // Ollama builds and non-reasoning models ignore this field.
        if self.speed.suppress_thinking() {
            body["think"] = json!(false);
        }

        let res = self
            .http
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;

        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }

        let parsed: WireResponse = serde_json::from_str(&text).map_err(|e| {
            AgentError::MalformedResponse { provider: self.label.clone(), detail: e.to_string() }
        })?;

        let mut content = Vec::new();
        if !parsed.message.content.trim().is_empty() {
            content.push(Content::Text { text: parsed.message.content });
        }
        // Ollama 0.16+ issues call ids; older builds do not. The loop needs one
        // to pair a result with its call, so fall back to a positional id
        // rather than assuming either behaviour.
        for (i, call) in parsed.message.tool_calls.unwrap_or_default().into_iter().enumerate() {
            content.push(Content::ToolUse {
                id: call.id.unwrap_or_else(|| format!("call_{i}")),
                name: call.function.name,
                input: call.function.arguments,
            });
        }

        let stop_reason = if content.iter().any(|c| matches!(c, Content::ToolUse { .. })) {
            StopReason::ToolUse
        } else if parsed.done_reason.as_deref() == Some("length") {
            StopReason::MaxTokens
        } else {
            StopReason::EndTurn
        };

        Ok(ChatResponse {
            model: parsed.model.unwrap_or_else(|| self.model.clone()),
            content,
            stop_reason,
            usage: Usage {
                input_tokens: parsed.prompt_eval_count,
                output_tokens: parsed.eval_count,
                cache_read_tokens: 0,
            },
        })
    }

    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        let endpoint = format!("{}/api/tags", self.base_url);
        let started = Instant::now();

        let res = self
            .http
            .get(&endpoint)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;

        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }

        let models: Vec<String> = serde_json::from_str::<WireTags>(&text)
            .map(|t| t.models.into_iter().map(|m| m.name).collect())
            .unwrap_or_default();

        // Ollama tags carry a `:tag` suffix; "llama3.1" and "llama3.1:latest"
        // are the same model, so match on the stem before complaining.
        let stem = |s: &str| s.split(':').next().unwrap_or(s).to_string();
        let have = models.iter().any(|m| m == &self.model || stem(m) == stem(&self.model));
        let detail = if !models.is_empty() && !have {
            Some(format!(
                "\"{}\" is not pulled on this host. Available: {}. Pull it with: ollama pull {}",
                self.model,
                models.join(", "),
                self.model
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

    match m.role {
        Role::Assistant => {
            let calls: Vec<serde_json::Value> = m
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::ToolUse { name, input, .. } => {
                        Some(json!({ "function": { "name": name, "arguments": input } }))
                    }
                    _ => None,
                })
                .collect();
            let mut msg = json!({ "role": "assistant", "content": text });
            if !calls.is_empty() {
                msg["tool_calls"] = json!(calls);
            }
            out.push(msg);
        }
        Role::User => {
            let mut any_result = false;
            for c in &m.content {
                if let Content::ToolResult { content, .. } = c {
                    any_result = true;
                    out.push(json!({ "role": "tool", "content": content }));
                }
            }
            if !any_result || !text.is_empty() {
                out.push(json!({ "role": "user", "content": text }));
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    model: Option<String>,
    message: WireMessage,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: u64,
    #[serde(default)]
    eval_count: u64,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
    /// Present from Ollama 0.16 onward; absent on older builds.
    #[serde(default)]
    id: Option<String>,
    function: WireFunction,
}

#[derive(Debug, Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WireTags {
    #[serde(default)]
    models: Vec<WireTag>,
}

#[derive(Debug, Deserialize)]
struct WireTag {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_native_tool_call() {
        // Captured verbatim from Ollama 0.16.3 answering qwen2.5:7b.
        let body = r#"{"model":"qwen2.5:7b","message":{"role":"assistant","content":"",
            "tool_calls":[{"id":"call_3ivyqca8","function":{"index":0,"name":"read_dtcs",
            "arguments":{"module":"ECU_7E8"}}}]},
            "done":true,"done_reason":"stop","prompt_eval_count":185,"eval_count":26}"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        let calls = parsed.message.tool_calls.unwrap();
        assert_eq!(calls[0].function.name, "read_dtcs");
        assert_eq!(calls[0].id.as_deref(), Some("call_3ivyqca8"));
        // Native arguments are already an object - no string reparse needed.
        assert_eq!(calls[0].function.arguments["module"], "ECU_7E8");
    }

    #[test]
    fn fast_mode_sends_think_false_and_quality_omits_it() {
        // Measured against Ollama 0.16.3 / qwen3:8b: `think: false` cut one
        // turn from 6.6s to 1.5s (116 reasoning tokens down to 26) and still
        // produced a correct tool call. Omitting the field entirely, rather
        // than sending `true`, keeps non-reasoning models on their own default.
        let fast = OllamaProvider::new("p", "P", "qwen3:8b", None, Speed::Fast, None).unwrap();
        assert!(fast.speed.suppress_thinking());

        let quality = OllamaProvider::new("p", "P", "qwen3:8b", None, Speed::Quality, None).unwrap();
        assert!(!quality.speed.suppress_thinking());
    }

    #[test]
    fn a_context_window_is_always_sent() {
        // The bug this guards against cost most of a session: Ollama defaults to
        // 4096 tokens whatever the model supports, silently truncated ~92% of
        // every inspection, and two different models then failed identically in
        // a way that looked like both being too small.
        let dflt = OllamaProvider::new("p", "P", "qwen3:8b", None, Speed::Quality, None).unwrap();
        assert_eq!(dflt.context_tokens, DEFAULT_CONTEXT_TOKENS);
        // A compile-time fact, so it fails the build rather than a test run.
        const { assert!(DEFAULT_CONTEXT_TOKENS > 4096) };

        let tuned =
            OllamaProvider::new("p", "P", "qwen3:8b", None, Speed::Quality, Some(40_960)).unwrap();
        assert_eq!(tuned.context_tokens, 40_960);
    }

    #[test]
    fn older_builds_without_call_ids_still_parse() {
        let body = r#"{"message":{"content":"",
            "tool_calls":[{"function":{"name":"scan_modules","arguments":{}}}]},"done_reason":"stop"}"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.message.tool_calls.unwrap()[0].id.is_none());
    }

    #[test]
    fn empty_content_does_not_become_an_empty_text_block() {
        let body = r#"{"message":{"content":"   "},"done_reason":"stop"}"#;
        let parsed: WireResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.message.content.trim().is_empty());
    }

    #[test]
    fn tag_stems_match_across_the_latest_suffix() {
        let stem = |s: &str| s.split(':').next().unwrap_or(s).to_string();
        assert_eq!(stem("llama3.1:latest"), stem("llama3.1"));
        assert_ne!(stem("llama3.1"), stem("qwen2.5"));
    }
}
