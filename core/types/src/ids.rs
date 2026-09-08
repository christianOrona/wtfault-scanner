//! Strongly typed identifiers.
//!
//! Every id is a UUID rendered as a string at the API boundary. They are
//! distinct types so that a `ModuleId` can never be passed where a `SessionId`
//! is expected.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Generate a fresh random identifier.
            pub fn new() -> Self {
                $name(format!("{}_{}", $prefix, uuid::Uuid::new_v4().simple()))
            }

            /// Wrap an existing string (e.g. one read back out of SQLite).
            pub fn from_string(s: impl Into<String>) -> Self {
                $name(s.into())
            }

            /// Borrow the underlying string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_string())
            }
        }
    };
}

id_type!(
    /// A diagnostic session: one connection lifecycle plus everything read during it.
    SessionId, "ses");
id_type!(
    /// A physical/logical vehicle, keyed on VIN when one could be read.
    VehicleId, "veh");
id_type!(
    /// One adapter connection attempt and its lifetime.
    ConnectionId, "con");
id_type!(
    /// An ECU/module discovered during a scan.
    ModuleId, "mod");
id_type!(
    /// A single execution of a diagnostic test.
    TestRunId, "tst");
id_type!(
    /// An agent-produced diagnosis record.
    DiagnosisId, "dia");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("ses_"));
        assert!(ModuleId::new().as_str().starts_with("mod_"));
    }

    #[test]
    fn ids_serialize_as_bare_strings() {
        let id = SessionId::from_string("ses_abc");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"ses_abc\"");
    }
}
