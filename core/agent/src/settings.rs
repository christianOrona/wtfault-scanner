//! Configured model providers, and where their credentials live.
//!
//! Deliberately **not** in the session database. That file holds VINs and
//! vehicle history and is the thing you would hand to a mechanic or attach to a
//! report; an API key has no business travelling with it.
//!
//! Keys are stored in plaintext in the user's own profile directory, readable
//! only by that user. That is an honest trade rather than a hidden one: the OS
//! keychain would be better, and it is three different platform integrations
//! that this build does not have. The UI says so where the key is entered.

use crate::error::{AgentError, Secret};
use crate::provider::{
    anthropic::AnthropicProvider, ollama::OllamaProvider, openai::OpenAiProvider, LlmProvider,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// xAI's OpenAI-compatible endpoint.
///
/// Verified against the live service: `GET /v1/models` answers `401` with
/// `unauthenticated:no-credentials`, and `POST /v1/chat/completions` answers
/// `400 Bad data: Messages cannot be empty` — the standard OpenAI shapes, so
/// [`OpenAiProvider`] drives it unchanged.
pub const XAI_BASE_URL: &str = "https://api.x.ai/v1";

/// Which dialect a configured endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anthropic Messages API.
    Anthropic,
    /// Anything speaking OpenAI Chat Completions.
    ///
    /// Renamed explicitly: `rename_all = "snake_case"` turns `OpenAiCompatible`
    /// into `open_ai_compatible`, with a word break inside "OpenAI" that no
    /// caller would guess. The wire name is part of the HTTP contract, so it is
    /// pinned here rather than left to depend on how the variant is spelled.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// Ollama's native API.
    Ollama,
    /// xAI (Grok). Speaks the OpenAI dialect at a fixed endpoint, so it reuses
    /// that transport; it exists as its own kind only so the UI can prefill the
    /// URL and know that a key is required.
    Xai,
}

impl ProviderKind {
    /// Whether this kind cannot work without a credential.
    pub fn requires_key(self) -> bool {
        matches!(self, ProviderKind::Anthropic | ProviderKind::Xai)
    }

    /// A sensible base URL to prefill in the UI.
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::Ollama => Some(crate::provider::ollama::DEFAULT_BASE_URL),
            ProviderKind::Xai => Some(XAI_BASE_URL),
            ProviderKind::OpenAiCompatible => None,
        }
    }
}

/// One configured provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable id, generated when the provider is added.
    pub id: String,
    /// What dialect it speaks.
    pub kind: ProviderKind,
    /// A name the user chose, e.g. "Jarvis (GPU box)".
    pub label: String,
    /// Endpoint. `None` uses the kind's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The model to ask for.
    pub model: String,
    /// The credential, when the endpoint needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<Secret>,
    /// How much the model should deliberate. See [`Speed`].
    #[serde(default)]
    pub speed: Speed,
    /// Step ceiling for one inspection. `None` uses the built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    /// Output-token ceiling for a single turn. `None` uses the built-in default.
    ///
    /// Matters far more on local hardware than on a hosted model. The default
    /// of 16k is harmless at hosted speeds, but a CPU-bound 20B generates at a
    /// few tokens a second, so the same ceiling lets one turn run for over an
    /// hour before anything stops it. Measured: gpt-oss:20b spent 21 minutes on
    /// a single turn and had not finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Context window to ask an Ollama endpoint for. `None` uses the default.
    ///
    /// The right value is a property of the hardware, not the model: it decides
    /// how much of the KV cache fits in VRAM, and therefore how much of the run
    /// happens on the GPU rather than the CPU. Ignored by every other provider,
    /// which manage their own context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
}

/// How much deliberation to buy.
///
/// Measured on this project: qwen3:8b spends 35–68 seconds *per step* thinking,
/// against adapter reads that take 86–169 **milliseconds**. Over 99% of a local
/// inspection is the model deliberating, so this is the single biggest lever on
/// how long a scan takes.
///
/// How it is applied is deliberately per-provider rather than one flag jammed
/// through every API, because "think less" means different things and the naive
/// version is harmful on some models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    /// Let the model deliberate as it normally would.
    #[default]
    Quality,
    /// Trade judgement for speed.
    ///
    /// * **Ollama** — sends `think: false`, which turns Qwen3-style reasoning
    ///   off entirely. This is where the minutes go, so the win is large.
    /// * **Anthropic** — lowers `output_config.effort` rather than disabling
    ///   thinking. Anthropic documents that disabling thinking on Opus 5 can
    ///   make the model write a tool call into its visible text instead of
    ///   emitting a real one: the turn silently succeeds, the call never runs,
    ///   and in an agent loop that text poisons later turns. Lower effort gets
    ///   most of the speed without that failure mode.
    /// * **OpenAI-compatible** — no effect. The dialect has no portable field
    ///   for it, and guessing a vendor extension would fail differently on
    ///   every endpoint.
    Fast,
}

impl Speed {
    /// Whether a reasoning model should skip its thinking phase.
    pub fn suppress_thinking(self) -> bool {
        self == Speed::Fast
    }
}

/// A provider as shown to the user: same fields, credential reduced to a hint.
///
/// The HTTP layer returns this and never [`ProviderConfig`], so there is no
/// route out of the process that carries a key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderView {
    /// Stable id.
    pub id: String,
    /// What dialect it speaks.
    pub kind: ProviderKind,
    /// The user's name for it.
    pub label: String,
    /// Endpoint, resolved to the default when unset.
    pub base_url: Option<String>,
    /// The model it will ask for.
    pub model: String,
    /// Whether a credential is stored.
    pub has_key: bool,
    /// Enough of the key to recognise it. Never enough to use it.
    pub key_hint: Option<String>,
    /// How much the model should deliberate.
    pub speed: Speed,
    /// Step ceiling, or null for the default.
    pub max_steps: Option<usize>,
    /// Output-token ceiling per turn, or null for the default.
    pub max_tokens: Option<u32>,
    /// Ollama context window, or null for the default.
    pub context_tokens: Option<u32>,
    /// Whether a context window applies to this kind of endpoint at all.
    pub context_supported: bool,
    /// Whether the speed setting does anything for this kind of endpoint.
    pub speed_supported: bool,
    /// Whether this is the provider the agent will use.
    pub selected: bool,
}

impl ProviderConfig {
    /// Redact for display.
    pub fn view(&self, selected: bool) -> ProviderView {
        ProviderView {
            id: self.id.clone(),
            kind: self.kind,
            label: self.label.clone(),
            base_url: self
                .base_url
                .clone()
                .or_else(|| self.kind.default_base_url().map(String::from)),
            model: self.model.clone(),
            has_key: self.api_key.as_ref().is_some_and(|k| !k.is_empty()),
            key_hint: self.api_key.as_ref().filter(|k| !k.is_empty()).map(|k| k.hint()),
            speed: self.speed,
            max_steps: self.max_steps,
            max_tokens: self.max_tokens,
            context_tokens: self.context_tokens,
            context_supported: matches!(self.kind, ProviderKind::Ollama),
            // Said plainly so the UI can grey the control rather than offering
            // a switch that quietly does nothing.
            speed_supported: !matches!(
                self.kind,
                ProviderKind::OpenAiCompatible | ProviderKind::Xai
            ),
            selected,
        }
    }

    /// Build a live client for this configuration.
    pub fn build(&self) -> Result<Box<dyn LlmProvider>, AgentError> {
        match self.kind {
            ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::new(
                &self.id,
                &self.label,
                &self.model,
                self.base_url.as_deref(),
                self.api_key
                    .clone()
                    .ok_or_else(|| AgentError::MissingCredential { provider: self.label.clone() })?,
                self.speed,
            )?)),
            ProviderKind::Xai => Ok(Box::new(OpenAiProvider::new(
                &self.id,
                &self.label,
                &self.model,
                self.base_url.as_deref().unwrap_or(XAI_BASE_URL),
                Some(self.api_key.clone().ok_or_else(|| AgentError::MissingCredential {
                    provider: self.label.clone(),
                })?),
            )?)),
            ProviderKind::OpenAiCompatible => {
                let base = self.base_url.as_deref().ok_or_else(|| {
                    AgentError::Settings(format!(
                        "{} is OpenAI-compatible and needs a base URL",
                        self.label
                    ))
                })?;
                Ok(Box::new(OpenAiProvider::new(
                    &self.id,
                    &self.label,
                    &self.model,
                    base,
                    self.api_key.clone(),
                )?))
            }
            ProviderKind::Ollama => Ok(Box::new(OllamaProvider::new(
                &self.id,
                &self.label,
                &self.model,
                self.base_url.as_deref(),
                self.speed,
                self.context_tokens,
            )?)),
        }
    }
}

/// Why someone is scanning this vehicle.
///
/// This turned out to matter more than any other setting. The whole product was
/// written for a person standing next to a stranger's car deciding whether to
/// buy it — "before you pay full price", "get a pre-purchase inspection", "treat
/// whatever it costs as money off". Read by the person who has owned the truck
/// for years, every one of those sentences is noise at best and faintly
/// insulting at worst, and it made the app sound like it was talking past them.
///
/// Same evidence, same honesty about uncertainty. Different question being
/// answered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPurpose {
    /// The user owns this vehicle and wants to keep it running.
    #[default]
    Owner,
    /// The user is deciding whether to buy it.
    Buyer,
}

/// How direct the agent should be.
///
/// Not a personality dial for its own sake. The default came across as
/// relentlessly discouraging — every answer led with what could not be done —
/// and a person holding a working truck does not need to be talked out of it
/// four times in one conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Practical and encouraging. Leads with what was found and what to do.
    #[default]
    Practical,
    /// Neutral and factual. States findings without framing.
    Neutral,
    /// Terse. Findings only, minimal explanation.
    Blunt,
}

/// Everything the agent needs to know about which models are available.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Configured providers, in the order the user added them.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Id of the one the agent uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// Whether the user owns this vehicle or is considering buying it.
    #[serde(default)]
    pub purpose: ScanPurpose,
    /// How direct the agent should be.
    #[serde(default)]
    pub tone: Tone,
}

impl ProviderSettings {
    /// The configuration the agent should use, if any.
    pub fn active(&self) -> Option<&ProviderConfig> {
        match &self.selected {
            Some(id) => self.providers.iter().find(|p| &p.id == id),
            // One provider and no explicit choice is not ambiguous.
            None if self.providers.len() == 1 => self.providers.first(),
            None => None,
        }
    }

    /// Redacted view of everything, for the settings UI.
    pub fn views(&self) -> Vec<ProviderView> {
        let active = self.active().map(|p| p.id.clone());
        self.providers
            .iter()
            .map(|p| p.view(active.as_ref() == Some(&p.id)))
            .collect()
    }
}

/// Reads and writes [`ProviderSettings`] on disk.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// Point at a settings file. The file need not exist yet.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        SettingsStore { path: path.into() }
    }

    /// Where the settings live.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load, treating "no file yet" as "nothing configured".
    ///
    /// A corrupt file is an error rather than a silent reset: quietly
    /// discarding someone's configuration, keys included, is worse than
    /// refusing to start until they look at it.
    pub fn load(&self) -> Result<ProviderSettings, AgentError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                AgentError::Settings(format!(
                    "{} is not valid settings JSON: {e}. Move it aside to start fresh.",
                    self.path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProviderSettings::default()),
            Err(e) => Err(AgentError::Settings(format!(
                "cannot read {}: {e}",
                self.path.display()
            ))),
        }
    }

    /// Save, replacing the file atomically so a crash mid-write cannot leave
    /// a half-written file where the credentials used to be.
    pub fn save(&self, settings: &ProviderSettings) -> Result<(), AgentError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AgentError::Settings(format!("cannot create {}: {e}", dir.display())))?;
        }
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| AgentError::Settings(e.to_string()))?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())
            .map_err(|e| AgentError::Settings(format!("cannot write {}: {e}", tmp.display())))?;
        restrict_permissions(&tmp);
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            AgentError::Settings(format!("cannot replace {}: {e}", self.path.display()))
        })?;
        Ok(())
    }
}

/// Make the file owner-only where the platform lets us say so.
///
/// Best effort by design: failing to tighten permissions is not a reason to
/// refuse to save, but it is a reason not to claim the file is protected.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {
    // On Windows the file inherits the profile directory's ACL, which is
    // already owner-only for a normal account.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            id: id.into(),
            kind,
            label: format!("provider {id}"),
            base_url: None,
            model: "m".into(),
            api_key: Some(Secret::new("sk-ant-api03-abcdefghijklmnop")),
            speed: Speed::Quality,
            max_steps: None,
            max_tokens: None,
            context_tokens: None,
        }
    }

    #[test]
    fn a_view_never_carries_the_key() {
        let v = cfg("p1", ProviderKind::Anthropic).view(true);
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("abcdefghijklmnop"));
        assert!(v.has_key);
        assert!(v.key_hint.unwrap().ends_with("mnop"));
    }

    #[test]
    fn one_provider_needs_no_explicit_selection() {
        let s = ProviderSettings {
            providers: vec![cfg("p1", ProviderKind::Ollama)],
            selected: None,
            ..Default::default()
        };
        assert_eq!(s.active().map(|p| p.id.as_str()), Some("p1"));
    }

    #[test]
    fn two_providers_with_no_choice_is_ambiguous_and_stays_unset() {
        let s = ProviderSettings {
            providers: vec![cfg("p1", ProviderKind::Ollama), cfg("p2", ProviderKind::Anthropic)],
            selected: None,
            ..Default::default()
        };
        assert!(s.active().is_none());
    }

    #[test]
    fn round_trips_through_disk_with_the_key_intact() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("providers.json"));
        assert!(store.load().unwrap().providers.is_empty());

        let s = ProviderSettings {
            providers: vec![cfg("p1", ProviderKind::Anthropic)],
            selected: Some("p1".into()),
            ..Default::default()
        };
        store.save(&s).unwrap();

        let back = store.load().unwrap();
        assert_eq!(back.providers.len(), 1);
        assert_eq!(back.active().unwrap().api_key.as_ref().unwrap().expose(), "sk-ant-api03-abcdefghijklmnop");
    }

    #[test]
    fn a_corrupt_file_is_reported_not_silently_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = SettingsStore::new(&path).load().unwrap_err();
        assert!(matches!(err, AgentError::Settings(_)));
    }

    #[test]
    fn wire_names_match_what_the_ui_sends() {
        // These strings are the HTTP contract. `rename_all = "snake_case"`
        // alone produces `open_ai_compatible`, which no caller would guess and
        // which silently rejected every attempt to add such a provider.
        let name = |k: ProviderKind| serde_json::to_string(&k).unwrap();
        assert_eq!(name(ProviderKind::Anthropic), "\"anthropic\"");
        assert_eq!(name(ProviderKind::Ollama), "\"ollama\"");
        assert_eq!(name(ProviderKind::OpenAiCompatible), "\"openai_compatible\"");
        assert_eq!(name(ProviderKind::Xai), "\"xai\"");

        // And they round-trip, which is what the POST body actually does.
        for s in ["anthropic", "ollama", "openai_compatible", "xai"] {
            serde_json::from_str::<ProviderKind>(&format!("\"{s}\""))
                .unwrap_or_else(|e| panic!("{s} should deserialize: {e}"));
        }
    }

    #[test]
    fn xai_prefills_its_endpoint_and_demands_a_key() {
        assert!(ProviderKind::Xai.requires_key());
        assert_eq!(ProviderKind::Xai.default_base_url(), Some(XAI_BASE_URL));

        // Without a key it must fail before any request is made.
        let mut c = cfg("p1", ProviderKind::Xai);
        c.api_key = None;
        let Err(e) = c.build() else { panic!("a keyless xAI provider should not build") };
        assert!(matches!(e, AgentError::MissingCredential { .. }));
    }

    #[test]
    fn xai_builds_on_the_openai_transport_without_a_stored_url() {
        // The endpoint is fixed, so a user who never typed a URL still works.
        let mut c = cfg("p1", ProviderKind::Xai);
        c.base_url = None;
        assert!(c.build().is_ok());
        // And the view shows the endpoint it will actually use.
        assert_eq!(c.view(true).base_url.as_deref(), Some(XAI_BASE_URL));
    }

    #[test]
    fn openai_compatible_without_a_base_url_fails_clearly() {
        let mut c = cfg("p1", ProviderKind::OpenAiCompatible);
        c.base_url = None;
        // `Box<dyn LlmProvider>` is not Debug, so match rather than unwrap_err.
        let Err(e) = c.build() else { panic!("expected a configuration error") };
        assert!(e.to_string().contains("needs a base URL"));
    }
}
