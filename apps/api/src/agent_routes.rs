//! HTTP surface for the agent and its settings.
//!
//! Same contract as the rest of `/api/v1`: an operation that *ran* returns 200
//! with an envelope to check, and a malformed request gets a real status code.

use crate::agent::{self, RecordingSink};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use aim_agent::provider::Role;
use aim_agent::settings::Speed;
use aim_agent::{AgentError, AgentEvent, Content, Message, ProviderConfig, ProviderKind, Secret};
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Map an agent failure onto the API's error envelope.
///
/// The distinction that matters to a user is between "you have not set a model
/// up yet" and "the model is broken", so those get different status codes.
fn agent_error(e: AgentError) -> ApiError {
    use aim_types::{AimError, ErrorCode};
    let (code, status_hint) = match &e {
        AgentError::NoProvider(_) | AgentError::MissingCredential { .. } => {
            (ErrorCode::PreconditionFailed, "configure a model provider in Settings")
        }
        AgentError::Unauthorized { .. } => (ErrorCode::OperationNotAllowed, "check the API key"),
        AgentError::Unreachable { .. } => (ErrorCode::TransportOpenFailed, "check the endpoint"),
        AgentError::Refused { .. } => (ErrorCode::OperationNotAllowed, "the model declined"),
        AgentError::Settings(_) => (ErrorCode::BadRequest, "check the settings file"),
        _ => (ErrorCode::Internal, "provider failure"),
    };
    ApiError::new(
        AimError::new(code, e.to_string())
            .with_details(json!({ "agent_code": e.code(), "hint": status_hint })),
    )
}

// ---------------------------------------------------------------- settings

/// `GET /api/v1/settings/providers`
pub async fn list_providers(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let settings = state.settings.load().map_err(agent_error)?;
    Ok(Json(json!({
        "providers": settings.views(),
        "purpose": settings.purpose,
        "tone": settings.tone,
        "settings_path": state.settings.path().display().to_string(),
        // Said plainly rather than buried: the user is about to paste a key.
        "storage_note": "Keys are stored in this file in plain text, readable only by your \
                         Windows account. This build does not use the OS credential store.",
        "kinds": [
            { "id": "anthropic", "label": "Anthropic (Claude)", "requires_key": true,
              "default_base_url": "https://api.anthropic.com",
              "help": "Strongest reasoning. Needs an API key; your data leaves this machine." },
            { "id": "ollama", "label": "Ollama", "requires_key": false,
              "default_base_url": "http://127.0.0.1:11434",
              "help": "Runs on your own hardware. Point it at a GPU box on your network." },
            { "id": "xai", "label": "xAI (Grok)", "requires_key": true,
              "default_base_url": aim_agent::settings::XAI_BASE_URL,
              "help": "Grok, via xAI's OpenAI-compatible API. Leave the model blank and press Test - it will list what your key can use." },
            { "id": "openai_compatible", "label": "OpenAI-compatible", "requires_key": false,
              "default_base_url": null,
              "help": "vLLM, LM Studio, OpenRouter, or anything speaking /chat/completions." }
        ]
    })))
}

/// Body for creating or updating a provider.
#[derive(Debug, Deserialize)]
pub struct ProviderBody {
    kind: ProviderKind,
    label: String,
    #[serde(default)]
    base_url: Option<String>,
    model: String,
    /// Omit to keep the stored key; send `""` to clear it.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    speed: Speed,
    /// Step ceiling for one inspection; omit for the built-in default.
    #[serde(default)]
    max_steps: Option<usize>,
    /// Output-token ceiling per turn; omit for the built-in default.
    #[serde(default)]
    max_tokens: Option<u32>,
    /// Ollama context window; omit for the built-in default.
    #[serde(default)]
    context_tokens: Option<u32>,
    #[serde(default)]
    select: bool,
}

/// `POST /api/v1/settings/providers`
pub async fn add_provider(
    State(state): State<AppState>,
    Json(body): Json<ProviderBody>,
) -> ApiResult<Json<Value>> {
    if body.label.trim().is_empty() || body.model.trim().is_empty() {
        return Err(ApiError::bad_request("a provider needs a label and a model"));
    }
    let mut settings = state.settings.load().map_err(agent_error)?;

    let id = format!("prov_{:016x}", fastrand_id());
    let config = ProviderConfig {
        id: id.clone(),
        kind: body.kind,
        label: body.label.trim().to_string(),
        base_url: body.base_url.filter(|u| !u.trim().is_empty()),
        model: body.model.trim().to_string(),
        api_key: body.api_key.filter(|k| !k.is_empty()).map(Secret::new),
        speed: body.speed,
        max_steps: body.max_steps.map(|n| n.clamp(2, 40)),
        max_tokens: body.max_tokens.map(|n| n.clamp(512, 64_000)),
        // Below 2k cannot hold a system prompt plus one tool result; above 128k
        // no consumer card has the VRAM for the KV cache.
        context_tokens: body.context_tokens.map(|n| n.clamp(2_048, 131_072)),
    };

    // First provider is selected automatically: making someone add one and then
    // separately choose it is a step with no decision in it.
    let select = body.select || settings.providers.is_empty();
    settings.providers.push(config);
    if select {
        settings.selected = Some(id.clone());
    }
    state.settings.save(&settings).map_err(agent_error)?;

    Ok(Json(json!({ "providers": settings.views(), "added": id })))
}

/// `PUT /api/v1/settings/providers/{id}`
pub async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProviderBody>,
) -> ApiResult<Json<Value>> {
    let mut settings = state.settings.load().map_err(agent_error)?;
    let existing = settings
        .providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::new(aim_types::AimError::new(aim_types::ErrorCode::NotFound, format!("no provider {id:?}"))))?;

    existing.kind = body.kind;
    existing.label = body.label.trim().to_string();
    existing.base_url = body.base_url.filter(|u| !u.trim().is_empty());
    existing.model = body.model.trim().to_string();
    existing.speed = body.speed;
    // Clamped: one step cannot finish an inspection, and forty is already far
    // past the point where a small model loses the thread.
    existing.max_steps = body.max_steps.map(|n| n.clamp(2, 40));
    existing.max_tokens = body.max_tokens.map(|n| n.clamp(512, 64_000));
    existing.context_tokens = body.context_tokens.map(|n| n.clamp(2_048, 131_072));
    // A missing key means "leave it alone"; an empty string means "remove it".
    // Without that distinction, editing the model would silently wipe the key.
    match body.api_key {
        None => {}
        Some(k) if k.is_empty() => existing.api_key = None,
        Some(k) => existing.api_key = Some(Secret::new(k)),
    }
    if body.select {
        settings.selected = Some(id.clone());
    }
    state.settings.save(&settings).map_err(agent_error)?;
    Ok(Json(json!({ "providers": settings.views() })))
}

/// `DELETE /api/v1/settings/providers/{id}`
pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut settings = state.settings.load().map_err(agent_error)?;
    let before = settings.providers.len();
    settings.providers.retain(|p| p.id != id);
    if settings.providers.len() == before {
        return Err(ApiError::new(aim_types::AimError::new(aim_types::ErrorCode::NotFound, format!("no provider {id:?}"))));
    }
    if settings.selected.as_deref() == Some(id.as_str()) {
        settings.selected = settings.providers.first().map(|p| p.id.clone());
    }
    state.settings.save(&settings).map_err(agent_error)?;
    Ok(Json(json!({ "providers": settings.views() })))
}

/// `POST /api/v1/settings/providers/{id}/select`
pub async fn select_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut settings = state.settings.load().map_err(agent_error)?;
    if !settings.providers.iter().any(|p| p.id == id) {
        return Err(ApiError::new(aim_types::AimError::new(aim_types::ErrorCode::NotFound, format!("no provider {id:?}"))));
    }
    settings.selected = Some(id);
    state.settings.save(&settings).map_err(agent_error)?;
    Ok(Json(json!({ "providers": settings.views() })))
}

/// `POST /api/v1/settings/providers/{id}/test`
///
/// Returns 200 with `reachable: false` when the endpoint is down. Being unable
/// to reach a model is a result of the test, not a failure of the request.
pub async fn test_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let settings = state.settings.load().map_err(agent_error)?;
    let config = settings
        .providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::new(aim_types::AimError::new(aim_types::ErrorCode::NotFound, format!("no provider {id:?}"))))?;

    let provider = match config.build() {
        Ok(p) => p,
        Err(e) => {
            return Ok(Json(json!({ "reachable": false, "error":
                { "code": e.code(), "message": e.to_string() } })))
        }
    };

    match provider.probe().await {
        Ok(info) => Ok(Json(json!({
            "reachable": true,
            "models": info.models,
            "elapsed_ms": info.elapsed_ms,
            "detail": info.detail,
        }))),
        Err(e) => Ok(Json(json!({
            "reachable": false,
            "error": { "code": e.code(), "message": e.to_string() },
        }))),
    }
}

// ------------------------------------------------------------------- agent

/// `GET /api/v1/agent` — whether the agent is usable, and why not if it isn't.
pub async fn agent_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let settings = state.settings.load().map_err(agent_error)?;
    let active = settings.active();
    Ok(Json(json!({
        "ready": active.is_some(),
        "provider": active.map(|p| json!({ "id": p.id, "label": p.label, "model": p.model,
                                          "speed": p.speed, "max_steps": p.max_steps })),
        "reason": if active.is_some() { Value::Null } else {
            json!("no model provider is configured")
        },
        // The UI needs this to know whether to frame a verdict as a purchase
        // decision or as a maintenance one.
        "purpose": settings.purpose,
        "tone": settings.tone,
    })))
}

/// Body for an inspection.
#[derive(Debug, Deserialize)]
pub struct InspectBody {
    /// What the user wants to know. Defaults to the buyer's question.
    #[serde(default)]
    pub request: Option<String>,
}

/// The buyer's question, when none was given.
const DEFAULT_INSPECTION: &str = "I am thinking about buying this vehicle. Inspect it and tell me \
                                  what is wrong with it, how serious each thing is, what it is \
                                  likely to cost, and whether I should buy it.";

/// `POST /api/v1/agent/inspect`
pub async fn inspect(
    State(state): State<AppState>,
    body: Option<Json<InspectBody>>,
) -> ApiResult<Json<Value>> {
    let request = body
        .and_then(|Json(b)| b.request)
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_INSPECTION.to_string());

    let mut sink = RecordingSink::default();
    let outcome = agent::run_inspection(&state, &request, &mut sink)
        .await
        .map_err(agent_error)?;

    Ok(Json(json!({
        "report": outcome.report,
        "text": outcome.text,
        "steps": outcome.steps,
        "truncated": outcome.truncated,
        "usage": outcome.usage,
        "trace": trace_of(&sink),
    })))
}

/// One turn of a conversation, as the UI sends it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTurn {
    /// `user` or `assistant`.
    pub role: String,
    /// What was said.
    pub content: String,
}

/// Body for a chat turn.
#[derive(Debug, Deserialize)]
pub struct ChatBody {
    /// The conversation so far, oldest first, ending with the new user message.
    pub messages: Vec<WireTurn>,
}

/// `POST /api/v1/agent/messages`
pub async fn messages(
    State(state): State<AppState>,
    Json(body): Json<ChatBody>,
) -> ApiResult<Json<Value>> {
    if body.messages.is_empty() {
        return Err(ApiError::bad_request("send at least one message"));
    }

    // Only prose crosses this boundary. Tool traffic from a previous turn is
    // not replayed from the client: the client is not the authority on what the
    // vehicle said, and accepting fabricated tool results from it would be a
    // way to put words in the core's mouth.
    let history: Vec<Message> = body
        .messages
        .iter()
        .map(|t| Message {
            role: if t.role == "assistant" { Role::Assistant } else { Role::User },
            content: vec![Content::text(&t.content)],
        })
        .collect();

    let mut sink = RecordingSink::default();
    let outcome = agent::run_chat(&state, history, &mut sink)
        .await
        .map_err(agent_error)?;

    Ok(Json(json!({
        "text": outcome.text,
        "steps": outcome.steps,
        "truncated": outcome.truncated,
        "usage": outcome.usage,
        "trace": trace_of(&sink),
    })))
}

/// Render the run's events so the UI can show what the agent actually did.
fn trace_of(sink: &RecordingSink) -> Vec<Value> {
    sink.events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Thinking(t) => Some(json!({ "type": "thinking", "text": t })),
            AgentEvent::ToolStarted { name, arguments } => {
                Some(json!({ "type": "tool", "name": name, "arguments": arguments }))
            }
            AgentEvent::ToolFinished { name, success, evidence_ref } => Some(json!({
                "type": "tool_done", "name": name,
                "success": success, "evidence_ref": evidence_ref
            })),
            AgentEvent::Finished => None,
        })
        .collect()
}

/// A short random id without pulling in a dependency for it.
fn fastrand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // Mix so consecutive calls do not produce near-identical ids.
    let mut x = nanos ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// How the agent should address this user.
#[derive(Debug, serde::Deserialize)]
pub struct VoiceBody {
    /// Owner or buyer.
    #[serde(default)]
    pub purpose: Option<aim_agent::settings::ScanPurpose>,
    /// How direct to be.
    #[serde(default)]
    pub tone: Option<aim_agent::settings::Tone>,
}

/// Set who the agent thinks it is talking to, and how bluntly.
///
/// Separate from provider configuration on purpose: this is a property of the
/// person, not of which model happens to be selected, and it should survive
/// switching from a local model to a hosted one.
pub async fn set_voice(
    State(state): State<AppState>,
    Json(body): Json<VoiceBody>,
) -> ApiResult<Json<Value>> {
    let mut settings = state.settings.load().map_err(agent_error)?;
    if let Some(p) = body.purpose {
        settings.purpose = p;
    }
    if let Some(t) = body.tone {
        settings.tone = t;
    }
    state.settings.save(&settings).map_err(agent_error)?;
    Ok(Json(json!({ "purpose": settings.purpose, "tone": settings.tone })))
}
