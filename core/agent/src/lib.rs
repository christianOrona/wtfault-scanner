//! `aim-agent` — the reasoning layer.
//!
//! The handoff's rule for this crate (§7) is that the language model is the
//! *interaction and reasoning* layer and nothing else. It is not the CAN bus
//! driver, not the safety boundary, and it may not invent vehicle commands.
//!
//! That is enforced structurally rather than by asking nicely in a prompt:
//!
//! * the model is only ever offered tools that already exist in the registry
//!   in `aim-tools`, so there is no name it can say that reaches the vehicle by
//!   a path the safety gate does not own;
//! * arguments are validated against each tool's JSON Schema before execution,
//!   so a hallucinated PID is rejected as a bad request rather than transmitted;
//! * every tool result the model sees is the same [`aim_types`] envelope the UI
//!   sees, warnings and provenance included, so the model cannot be told a
//!   reading is solid when the core called it unverified.
//!
//! What remains a prompt-level concern — tone, humility, not calling a single
//! trouble code a diagnosis — is in the system prompt, and is the part that
//! needs evaluating rather than trusting.
//!
//! # Layout
//!
//! * [`provider`] — the vendor-neutral seam: Anthropic, OpenAI-compatible, Ollama.
//! * [`settings`] — which providers are configured, and where the keys live.
//! * [`error`] — failures, and the [`error::Secret`] wrapper that keeps
//!   credentials out of logs.

#![warn(missing_docs)]

pub mod credentials;
pub mod error;
pub mod prompts;
pub mod provider;
pub mod report;
pub mod run;
pub mod scripted;
pub mod settings;
pub mod tools;

pub use error::{AgentError, Secret};
pub use provider::{
    ChatRequest, ChatResponse, Content, LlmProvider, Message, ProviderInfo, Role, StopReason,
    ToolSpec, Usage,
};
pub use report::{Finding, Report, Severity, Source, Verdict};
pub use run::{Agent, AgentEvent, AgentOutcome, EventSink, NullSink};
pub use settings::{ProviderConfig, ProviderKind, ProviderSettings, ProviderView, SettingsStore};
pub use tools::ToolExecutor;

/// Default ceiling on a single model turn.
///
/// Generous because a diagnostic summary that stops mid-sentence is worse than
/// useless — the user cannot tell a truncated finding from a complete one.
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;
