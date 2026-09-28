//! OpenRouter, and its free models.
//!
//! OpenRouter speaks the OpenAI dialect, so the transport is [`OpenAiProvider`].
//! What this adds is the part that makes the free models usable without
//! babysitting them:
//!
//! - **`auto:free`** picks the newest free model that can call tools, from
//!   OpenRouter's public catalogue, instead of making the person choose one
//!   and notice when it is withdrawn. See [`super::free_models`].
//! - **Rotation.** Free models are shared and often busy. When the one
//!   answering is rate limited, overloaded or no longer served, the turn is
//!   retried on the next one, and the one that worked is tried first next time.
//! - **The daily allowance is not a busy model.** OpenRouter caps free
//!   requests per account per day. Every model answers 429 once that is used
//!   up, so trying the others would only burn time and print five failures.
//!   It stops at once and says what ran out and when it comes back.

use super::free_models::free_tool_models;
use super::openai::OpenAiProvider;
use super::{status_error, transport_error, ChatRequest, ChatResponse, LlmProvider, ProviderInfo};
use crate::error::{AgentError, Secret};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// OpenRouter's API.
pub const BASE_URL: &str = "https://openrouter.ai/api/v1";

/// The model setting that means "the best free model available right now".
pub const AUTO_FREE: &str = "auto:free";

/// How long a fetched catalogue is trusted. Free models come and go over days,
/// not minutes, and the fetch is one more request before the first answer.
const CATALOGUE_TTL: Duration = Duration::from_secs(60 * 60);

/// Models tried for one turn before giving up. Five busy models in a row means
/// OpenRouter's free pool is having a bad moment, not that a sixth will help.
const MAX_ATTEMPTS: usize = 5;

/// Sent so OpenRouter can attribute the traffic to this app, as its API asks.
/// Names the project; carries nothing about the person or the vehicle.
const APP_URL: &str = "https://github.com/christianOrona/wtfault-scanner";
const APP_TITLE: &str = "WTFault Scanner";

/// An OpenRouter account, asking either for one model or for `auto:free`.
pub struct OpenRouterProvider {
    id: String,
    label: String,
    model: String,
    base_url: String,
    api_key: Secret,
    inner: OpenAiProvider,
    http: reqwest::Client,
    rotation: Mutex<Rotation>,
}

#[derive(Default)]
struct Rotation {
    /// Free model ids, best first.
    catalogue: Vec<String>,
    fetched: Option<Instant>,
    /// The model that answered last, tried first next time.
    preferred: Option<String>,
}

impl OpenRouterProvider {
    /// Build a provider. An empty `model` means [`AUTO_FREE`].
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        api_key: Secret,
    ) -> Result<Self, AgentError> {
        let id = id.into();
        let label = label.into();
        let model = model.into();
        let model = if model.trim().is_empty() { AUTO_FREE.to_string() } else { model };
        let base_url = base_url.unwrap_or(BASE_URL).trim_end_matches('/').to_string();
        let inner = OpenAiProvider::new(&id, &label, &model, &base_url, Some(api_key.clone()))?
            .with_header("HTTP-Referer", APP_URL)
            .with_header("X-Title", APP_TITLE);
        Ok(Self {
            id,
            label,
            model,
            base_url,
            api_key,
            inner,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| AgentError::Settings(e.to_string()))?,
            rotation: Mutex::new(Rotation::default()),
        })
    }

    fn is_auto(&self) -> bool {
        self.model == AUTO_FREE
    }

    /// The free models, best first, fetched when the cached list is stale.
    async fn catalogue(&self) -> Result<Vec<String>, AgentError> {
        if let Ok(r) = self.rotation.lock() {
            if r.fetched.is_some_and(|t| t.elapsed() < CATALOGUE_TTL) && !r.catalogue.is_empty() {
                return Ok(r.catalogue.clone());
            }
        }

        let endpoint = format!("{}/models", self.base_url);
        let res = self
            .http
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }
        let ids: Vec<String> = free_tool_models(&text)
            .map_err(|e| AgentError::MalformedResponse {
                provider: self.label.clone(),
                detail: format!("the model list could not be read: {e}"),
            })?
            .into_iter()
            .map(|m| m.id)
            .collect();

        if let Ok(mut r) = self.rotation.lock() {
            r.catalogue = ids.clone();
            r.fetched = Some(Instant::now());
        }
        Ok(ids)
    }

    /// The order to try models in for the next turn.
    async fn candidates(&self) -> Result<Vec<String>, AgentError> {
        let catalogue = self.catalogue().await?;
        if catalogue.is_empty() {
            return Err(AgentError::Api {
                provider: self.label.clone(),
                status: 404,
                message: "OpenRouter lists no free model that can call tools right now. Pick a \
                          specific model instead, or try again later."
                    .into(),
            });
        }
        let preferred = self.rotation.lock().ok().and_then(|r| r.preferred.clone());
        Ok(order(catalogue, preferred.as_deref()))
    }

    fn remember(&self, model: &str) {
        if let Ok(mut r) = self.rotation.lock() {
            r.preferred = Some(model.to_string());
        }
    }

    /// Turn a 429 that means "today's free allowance is gone" into an error
    /// that says so; leave every other error as it is.
    fn explain(&self, e: AgentError) -> AgentError {
        match daily_limit_detail(&e) {
            Some(detail) => AgentError::DailyLimit { provider: self.label.clone(), detail },
            None => e,
        }
    }
}

/// `catalogue` with `preferred` moved to the front, when it is still listed.
fn order(mut catalogue: Vec<String>, preferred: Option<&str>) -> Vec<String> {
    if let Some(p) = preferred {
        if let Some(i) = catalogue.iter().position(|m| m == p) {
            let m = catalogue.remove(i);
            catalogue.insert(0, m);
        }
    }
    catalogue
}

/// Whether the next free model could succeed where this one failed.
///
/// Busy, overloaded, withdrawn or broken: all about the model. A rejected key,
/// an unreachable network or a refusal would fail the same way everywhere.
fn worth_another_model(e: &AgentError) -> bool {
    match e {
        AgentError::RateLimited { .. } | AgentError::MalformedResponse { .. } => true,
        AgentError::Api { status, .. } => matches!(status, 404 | 408 | 409 | 429) || *status >= 500,
        _ => false,
    }
}

/// When `e` is OpenRouter saying the account's free requests for the day are
/// used up, what to tell the person.
fn daily_limit_detail(e: &AgentError) -> Option<String> {
    let AgentError::RateLimited { message: Some(message), .. } = e else { return None };
    let lower = message.to_ascii_lowercase();
    if !(lower.contains("per-day") || lower.contains("per day")) {
        return None;
    }
    Some(format!(
        "the free allowance for today is used up (OpenRouter said: \"{message}\"). Free models \
         allow 50 requests a day per account, or 1,000 once the account has bought $10 of \
         credit, and the count resets at midnight UTC. One inspection takes roughly 10 to 30 \
         requests."
    ))
}

#[async_trait::async_trait]
impl LlmProvider for OpenRouterProvider {
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
        if !self.is_auto() {
            return self.inner.chat_with(&self.model, request).await.map_err(|e| self.explain(e));
        }

        let mut last = None;
        for model in self.candidates().await?.into_iter().take(MAX_ATTEMPTS) {
            match self.inner.chat_with(&model, request).await {
                Ok(response) => {
                    self.remember(&model);
                    return Ok(response);
                }
                Err(e) => {
                    let e = self.explain(e);
                    if !worth_another_model(&e) {
                        return Err(e);
                    }
                    tracing::warn!(model = %model, error = %e, "free model failed; trying the next");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| AgentError::Api {
            provider: self.label.clone(),
            status: 503,
            message: "no free model answered".into(),
        }))
    }

    /// Check the key, and list what `auto:free` would choose from.
    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        let started = Instant::now();
        let endpoint = format!("{}/key", self.base_url);
        let res = self
            .http
            .get(&endpoint)
            .header("authorization", format!("Bearer {}", self.api_key.expose()))
            .send()
            .await
            .map_err(|e| transport_error(&self.label, &endpoint, e))?;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(status_error(&self.label, status, &text, None));
        }
        let key: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let free_tier = key.pointer("/data/is_free_tier").and_then(|v| v.as_bool());

        let catalogue = self.catalogue().await?;
        let mut models = vec![AUTO_FREE.to_string()];
        models.extend(catalogue.iter().cloned());

        let account = match free_tier {
            Some(true) => {
                "The key works. The account has no credit, so free models allow 50 \
                           requests a day."
            }
            Some(false) => {
                "The key works. The account has credit, so free models allow 1,000 \
                            requests a day."
            }
            None => "The key works.",
        };
        let choice = if self.is_auto() {
            match catalogue.first() {
                Some(first) => format!(
                    " {} free models can call tools; Automatic starts with {first} and moves \
                     on when one is busy.",
                    catalogue.len()
                ),
                None => " OpenRouter lists no free model that can call tools right now.".into(),
            }
        } else if catalogue.iter().any(|m| m == &self.model) {
            format!(" {} is free.", self.model)
        } else {
            format!(
                " {} is not one of the free models, so requests to it are billed to the \
                 OpenRouter account's credit.",
                self.model
            )
        };

        Ok(ProviderInfo {
            reachable: true,
            models,
            elapsed_ms: started.elapsed().as_millis() as u64,
            detail: Some(format!("{account}{choice}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_limited(message: &str) -> AgentError {
        AgentError::RateLimited {
            provider: "OpenRouter".into(),
            retry_after_secs: None,
            message: Some(message.into()),
        }
    }

    #[test]
    fn the_model_that_worked_is_tried_first() {
        let list = vec!["a".to_string(), "b".into(), "c".into()];
        assert_eq!(order(list.clone(), Some("c")), ["c", "a", "b"]);
        // One that has since left the catalogue changes nothing.
        assert_eq!(order(list.clone(), Some("gone")), ["a", "b", "c"]);
        assert_eq!(order(list, None), ["a", "b", "c"]);
    }

    #[test]
    fn a_used_up_day_is_recognised_and_explained() {
        let e = rate_limited(
            "Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model \
             requests per day",
        );
        let detail = daily_limit_detail(&e).expect("a daily cap");
        assert!(detail.contains("midnight UTC"));
        assert!(detail.contains("free-models-per-day"));
    }

    #[test]
    fn a_busy_model_is_not_a_used_up_day() {
        let e = rate_limited("Rate limit exceeded: free-models-per-min.");
        assert!(daily_limit_detail(&e).is_none());
        assert!(worth_another_model(&e));
    }

    #[test]
    fn only_model_specific_failures_move_to_the_next_model() {
        let api = |status| AgentError::Api { provider: "p".into(), status, message: "m".into() };
        assert!(worth_another_model(&api(502)));
        assert!(worth_another_model(&api(404)));
        assert!(!worth_another_model(&api(400)));
        assert!(!worth_another_model(&AgentError::Unauthorized {
            provider: "p".into(),
            status: 401
        }));
        assert!(!worth_another_model(&AgentError::DailyLimit {
            provider: "p".into(),
            detail: "d".into()
        }));
    }

    /// Reaches openrouter.ai. `cargo test -p aim-agent -- --ignored openrouter`
    #[tokio::test]
    #[ignore = "reaches openrouter.ai"]
    async fn live_the_catalogue_offers_free_models_and_a_bad_key_is_named() {
        let p = OpenRouterProvider::new(
            "id",
            "OpenRouter",
            AUTO_FREE,
            None,
            Secret::new("sk-or-not-a-key"),
        )
        .unwrap();
        let catalogue = p.catalogue().await.expect("the public model list");
        assert!(!catalogue.is_empty(), "no free tool-capable models listed");

        let request = ChatRequest {
            system: "Say hi.".into(),
            messages: vec![super::super::Message::user("hi")],
            tools: vec![],
            max_tokens: 16,
        };
        match p.chat(&request).await {
            Err(AgentError::Unauthorized { status: 401, .. }) => {}
            other => panic!("expected the key to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_model_means_automatic() {
        let p = OpenRouterProvider::new("id", "OpenRouter", "  ", None, Secret::new("k")).unwrap();
        assert_eq!(p.model(), AUTO_FREE);
    }
}
