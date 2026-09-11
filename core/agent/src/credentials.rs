//! Where a provider's API key actually lives.
//!
//! # The problem
//!
//! Keys were kept in a JSON file in the user's profile, in plain text. The
//! About screen said so plainly rather than glossing it, which was the right
//! interim behaviour and not a solution: anything running as that user could
//! read the file, and a key pasted into a support thread by somebody
//! screenshotting their settings directory is a key on the internet.
//!
//! # Three places, and the order matters
//!
//! 1. **An environment variable**, for development and CI. Checked first so a
//!    machine can be given a key without touching either the store or the disk
//!    — and so a test run cannot accidentally read a developer's real key out
//!    of their own keychain.
//! 2. **The operating system's credential store** — Credential Manager on
//!    Windows, Keychain on macOS, the Secret Service on Linux. The normal case.
//! 3. **The settings file**, in plain text, where there is no store. Some Linux
//!    systems have no Secret Service running, and refusing to work at all there
//!    would be choosing a principle over the person using it. What this must
//!    never do is fall back *silently*: [`CredentialStore::describe`] says
//!    which of the three is in use, and the interface repeats it.
//!
//! # Migration
//!
//! An existing plaintext key is moved into the store on the next save and then
//! **removed from the file**. Leaving a copy behind would mean the migration
//! improved nothing while looking like it had, which is worse than not
//! migrating: a person told their key is now in the credential store would
//! reasonably stop worrying about the file.

use crate::error::Secret;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// What the credential store is identified as to the operating system.
///
/// Shown to the person by Credential Manager and Keychain, so it is the
/// product name rather than a crate name.
const SERVICE: &str = "WTFault Scanner";

/// The environment variable prefix, for development and CI.
///
/// `AIM_PROVIDER_KEY_<ID>`, with the provider id upper-cased and any character
/// that is not a letter or digit replaced by an underscore — environment
/// variable names cannot hold the hyphens an id can.
const ENV_PREFIX: &str = "AIM_PROVIDER_KEY_";

/// Where a particular key came from, for saying so on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// An environment variable set on this machine.
    Environment,
    /// The operating system's credential store.
    OperatingSystem,
    /// The settings file, in plain text.
    PlainFile,
    /// Nothing has been stored.
    None,
}

impl KeySource {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            KeySource::Environment => "environment",
            KeySource::OperatingSystem => "operating_system",
            KeySource::PlainFile => "plain_file",
            KeySource::None => "none",
        }
    }

    /// What it means, in language meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            KeySource::Environment => {
                "From an environment variable on this machine. It is not stored by this app at \
                 all, and it disappears when the variable does."
            }
            KeySource::OperatingSystem => {
                "In your operating system's credential store, the same place your browser keeps \
                 saved passwords. This app asks for it when it needs it and never writes it to \
                 disk itself."
            }
            KeySource::PlainFile => {
                "In a plain text file in your user profile. No credential store was available on \
                 this machine, so anything running as you can read it. That is worth knowing \
                 before you paste a key with a spending limit attached."
            }
            KeySource::None => "No key is stored for this provider.",
        }
    }

    /// Whether the key is readable by anything running as this user.
    pub fn is_plaintext(&self) -> bool {
        matches!(self, KeySource::PlainFile)
    }
}

/// Reads and writes API keys somewhere other than the settings file.
///
/// A trait so the tests can run against a store that is not the developer's
/// own keychain. A test that wrote to the real Credential Manager would leave
/// entries behind on whatever machine ran it, and one that read from it would
/// pass or fail depending on whose laptop it was.
pub trait CredentialStore: Send + Sync {
    /// The key for a provider, wherever it lives.
    fn get(&self, provider_id: &str) -> Option<Secret>;

    /// Store a key, replacing any previous one.
    ///
    /// Returns whether it was stored somewhere other than the settings file.
    /// `false` means the caller must keep writing it to the file, and must say
    /// so on screen.
    fn set(&self, provider_id: &str, key: &Secret) -> bool;

    /// Forget a key. Absence is success: the goal is that it is gone.
    fn delete(&self, provider_id: &str);

    /// Where a particular provider's key is being read from.
    fn source(&self, provider_id: &str) -> KeySource;

    /// One line naming where keys go on this machine.
    fn describe(&self) -> &'static str;
}

/// The operating system's own credential store, with an environment override.
pub struct OsCredentialStore;

impl OsCredentialStore {
    /// The environment variable a provider's key would be read from.
    pub fn env_var(provider_id: &str) -> String {
        let sanitised: String = provider_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
            .collect();
        format!("{ENV_PREFIX}{sanitised}")
    }

    fn entry(provider_id: &str) -> Option<keyring::Entry> {
        match keyring::Entry::new(SERVICE, provider_id) {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::warn!(provider = provider_id, error = %e, "no credential store available");
                None
            }
        }
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, provider_id: &str) -> Option<Secret> {
        // The environment first, so a machine can be given a key without
        // touching the store — and so a CI run cannot pick up a developer's
        // real key from their own keychain.
        if let Ok(v) = std::env::var(Self::env_var(provider_id)) {
            if !v.trim().is_empty() {
                return Some(Secret::new(v));
            }
        }
        let entry = Self::entry(provider_id)?;
        match entry.get_password() {
            Ok(v) if !v.trim().is_empty() => Some(Secret::new(v)),
            Ok(_) => None,
            // Not found is the ordinary case for a provider whose key still
            // lives in the file, or one that has none.
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                tracing::warn!(provider = provider_id, error = %e, "could not read a stored key");
                None
            }
        }
    }

    fn set(&self, provider_id: &str, key: &Secret) -> bool {
        let Some(entry) = Self::entry(provider_id) else { return false };
        match entry.set_password(key.expose()) {
            Ok(()) => true,
            Err(e) => {
                // Reported rather than swallowed: the caller has to fall back
                // to the file, and the person has to be told that is what
                // happened.
                tracing::warn!(provider = provider_id, error = %e, "could not store a key");
                false
            }
        }
    }

    fn delete(&self, provider_id: &str) {
        if let Some(entry) = Self::entry(provider_id) {
            // A key that was never there is already in the state we want.
            let _ = entry.delete_credential();
        }
    }

    fn source(&self, provider_id: &str) -> KeySource {
        if std::env::var(Self::env_var(provider_id)).is_ok_and(|v| !v.trim().is_empty()) {
            return KeySource::Environment;
        }
        match Self::entry(provider_id).map(|e| e.get_password()) {
            Some(Ok(v)) if !v.trim().is_empty() => KeySource::OperatingSystem,
            _ => KeySource::None,
        }
    }

    fn describe(&self) -> &'static str {
        "your operating system's credential store"
    }
}

/// A store that holds nothing, so keys stay in the settings file.
///
/// Not only a test double. It is the honest representation of a machine with
/// no credential store — some Linux systems have no Secret Service running —
/// and naming that state is what lets the interface say "plain text file"
/// instead of quietly implying otherwise.
#[derive(Default)]
pub struct NoCredentialStore;

impl CredentialStore for NoCredentialStore {
    fn get(&self, provider_id: &str) -> Option<Secret> {
        // The environment override still works: it is the mechanism for
        // development and CI, and it does not depend on a store existing.
        std::env::var(OsCredentialStore::env_var(provider_id))
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(Secret::new)
    }

    fn set(&self, _provider_id: &str, _key: &Secret) -> bool {
        false
    }

    fn delete(&self, _provider_id: &str) {}

    fn source(&self, provider_id: &str) -> KeySource {
        if self.get(provider_id).is_some() {
            KeySource::Environment
        } else {
            KeySource::None
        }
    }

    fn describe(&self) -> &'static str {
        "a plain text file, because no credential store was available"
    }
}

/// An in-memory store, for tests.
///
/// Exists so the test suite never touches the credential store of whatever
/// machine runs it. A test that wrote to the real one would leave entries on
/// somebody's laptop; one that read from it would pass or fail depending on
/// whose laptop it was.
#[derive(Default)]
pub struct MemoryCredentialStore {
    keys: Mutex<BTreeMap<String, String>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, provider_id: &str) -> Option<Secret> {
        self.keys.lock().ok()?.get(provider_id).cloned().map(Secret::new)
    }

    fn set(&self, provider_id: &str, key: &Secret) -> bool {
        match self.keys.lock() {
            Ok(mut k) => {
                k.insert(provider_id.to_string(), key.expose().to_string());
                true
            }
            Err(_) => false,
        }
    }

    fn delete(&self, provider_id: &str) {
        if let Ok(mut k) = self.keys.lock() {
            k.remove(provider_id);
        }
    }

    fn source(&self, provider_id: &str) -> KeySource {
        match self.get(provider_id) {
            Some(_) => KeySource::OperatingSystem,
            None => KeySource::None,
        }
    }

    fn describe(&self) -> &'static str {
        "an in-memory store (tests only)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Environment variable names cannot hold the hyphens a provider id can,
    /// so the id is folded rather than used verbatim.
    #[test]
    fn an_id_becomes_a_usable_variable_name() {
        assert_eq!(OsCredentialStore::env_var("anthropic-1"), "AIM_PROVIDER_KEY_ANTHROPIC_1");
        assert_eq!(OsCredentialStore::env_var("ollama.local"), "AIM_PROVIDER_KEY_OLLAMA_LOCAL");
    }

    #[test]
    fn a_store_round_trips_a_key() {
        let store = MemoryCredentialStore::default();
        assert!(store.get("p1").is_none());
        assert_eq!(store.source("p1"), KeySource::None);

        assert!(store.set("p1", &Secret::new("sk-test-123")));
        assert_eq!(store.get("p1").unwrap().expose(), "sk-test-123");
        assert_eq!(store.source("p1"), KeySource::OperatingSystem);

        store.delete("p1");
        assert!(store.get("p1").is_none());
    }

    /// A machine with nowhere to put a key says so rather than appearing to
    /// have stored one. The caller then keeps it in the file and the interface
    /// says which of the two happened.
    #[test]
    fn a_machine_with_no_store_refuses_rather_than_pretending() {
        let store = NoCredentialStore;
        assert!(!store.set("p1", &Secret::new("sk-test-123")));
        assert!(store.get("p1").is_none());
        assert!(store.describe().contains("plain text"));
    }

    /// The distinction the interface exists to draw. Everything else can be
    /// described as "stored"; this one has to be described as readable.
    #[test]
    fn only_the_file_is_described_as_plaintext() {
        assert!(KeySource::PlainFile.is_plaintext());
        for s in [KeySource::OperatingSystem, KeySource::Environment, KeySource::None] {
            assert!(!s.is_plaintext(), "{s:?}");
        }
    }
}
