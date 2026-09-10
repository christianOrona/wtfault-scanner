//! Failures the agent layer can produce.
//!
//! These stay distinct from `aim-types`' diagnostic errors on purpose. "The
//! language model is unreachable" and "the truck did not answer" are different
//! problems with different fixes, and collapsing them would leave the user
//! looking at the wrong one.

use std::fmt;

/// Something went wrong talking to a model provider, or configuring one.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No provider is configured, or the selected one no longer exists.
    #[error("no model provider is configured: {0}")]
    NoProvider(String),

    /// The provider needs a credential it does not have.
    #[error("{provider} needs an API key")]
    MissingCredential {
        /// Which provider.
        provider: String,
    },

    /// The provider rejected the credential.
    #[error("{provider} rejected the credential (HTTP {status})")]
    Unauthorized {
        /// Which provider.
        provider: String,
        /// The status it answered with.
        status: u16,
    },

    /// The provider could not be reached at all.
    #[error("cannot reach {provider} at {endpoint}: {detail}")]
    Unreachable {
        /// Which provider.
        provider: String,
        /// The URL that was tried.
        endpoint: String,
        /// What the transport reported.
        detail: String,
    },

    /// The provider answered, unhappily.
    #[error("{provider} answered HTTP {status}: {message}")]
    Api {
        /// Which provider.
        provider: String,
        /// HTTP status.
        status: u16,
        /// The provider's own message, verbatim.
        message: String,
    },

    /// The provider is rate limiting us.
    #[error("{provider} is rate limiting; retry after {retry_after_secs:?}s")]
    RateLimited {
        /// Which provider.
        provider: String,
        /// Seconds the provider asked us to wait, when it said.
        retry_after_secs: Option<u64>,
    },

    /// The answer did not have the shape the provider's API documents.
    #[error("{provider} returned a response this build cannot read: {detail}")]
    MalformedResponse {
        /// Which provider.
        provider: String,
        /// What was wrong with it.
        detail: String,
    },

    /// The model declined the request. Distinct from an error: the call worked.
    #[error("the model declined this request{}", .category.as_ref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Refused {
        /// The provider's category for the refusal, when it gave one.
        category: Option<String>,
        /// The provider's explanation, when it gave one.
        explanation: Option<String>,
    },

    /// The agent hit its own ceiling on tool calls without reaching a conclusion.
    #[error("the agent stopped after {steps} steps without finishing")]
    StepLimit {
        /// How many steps it took.
        steps: usize,
    },

    /// Reading or writing the provider settings failed.
    #[error("provider settings: {0}")]
    Settings(String),
}

impl AgentError {
    /// Whether retrying the same request unchanged could plausibly work.
    ///
    /// Used to decide between "try again" and "tell the user"; a 400 is not
    /// going to fix itself, and retrying a refusal just spends money.
    pub fn is_retryable(&self) -> bool {
        match self {
            AgentError::Unreachable { .. } | AgentError::RateLimited { .. } => true,
            AgentError::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// A stable snake_case discriminant, matching how the HTTP API reports errors.
    pub fn code(&self) -> &'static str {
        match self {
            AgentError::NoProvider(_) => "no_provider",
            AgentError::MissingCredential { .. } => "missing_credential",
            AgentError::Unauthorized { .. } => "provider_unauthorized",
            AgentError::Unreachable { .. } => "provider_unreachable",
            AgentError::Api { .. } => "provider_error",
            AgentError::RateLimited { .. } => "provider_rate_limited",
            AgentError::MalformedResponse { .. } => "provider_malformed_response",
            AgentError::Refused { .. } => "model_refused",
            AgentError::StepLimit { .. } => "agent_step_limit",
            AgentError::Settings(_) => "settings_error",
        }
    }
}

/// A credential that does not leak into logs, `Debug` output or error messages.
///
/// The only way to read it is [`Secret::expose`], which is deliberately
/// awkward to type so that every use is easy to find in review.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wrap a credential.
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// Read the credential. Every call site is a place a key could escape.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether there is anything here at all.
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// A form safe to show a user: enough to recognise the key, not to use it.
    pub fn hint(&self) -> String {
        let s = self.0.trim();
        let n = s.chars().count();
        if n <= 8 {
            return "*".repeat(n.max(1));
        }
        let tail: String = s.chars().skip(n - 4).collect();
        let head: String = s.chars().take(3).collect();
        format!("{head}\u{2026}{tail}")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_never_debugs_its_value() {
        let s = Secret::new("sk-ant-super-secret-value");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert!(!format!("{s:?}").contains("secret"));
    }

    #[test]
    fn hint_shows_enough_to_recognise_and_no_more() {
        let s = Secret::new("sk-ant-api03-abcdefghijklmnop");
        let h = s.hint();
        assert!(h.starts_with("sk-"));
        assert!(h.ends_with("mnop"));
        assert!(!h.contains("abcdefghij"));
    }

    #[test]
    fn short_secrets_are_fully_masked() {
        assert_eq!(Secret::new("abc").hint(), "***");
        // An empty key must still not render as an empty string, or the UI
        // would show nothing where a credential is supposed to be.
        assert_eq!(Secret::new("").hint(), "*");
    }

    #[test]
    fn retryable_classification() {
        assert!(
            AgentError::RateLimited { provider: "p".into(), retry_after_secs: None }.is_retryable()
        );
        assert!(AgentError::Api { provider: "p".into(), status: 503, message: String::new() }
            .is_retryable());
        assert!(!AgentError::Api { provider: "p".into(), status: 400, message: String::new() }
            .is_retryable());
        assert!(!AgentError::Refused { category: None, explanation: None }.is_retryable());
    }
}
