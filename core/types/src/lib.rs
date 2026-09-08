//! `aim-types` — shared domain vocabulary for AI Mechanic.
//!
//! This crate deliberately contains no I/O. It defines the words every other
//! crate uses: identifiers, timestamps, provenance, decoded values, adapter
//! capabilities, the §12 persistence model, the §7 `ToolResult` envelope and
//! the structured error type that crosses the localhost API boundary.
//!
//! Two rules from the handoff are encoded here rather than left to convention:
//!
//! 1. **Raw bytes are never mixed with user-facing meaning.** A [`DecodedValue`]
//!    always carries the [`Provenance`] of the bytes it came from.
//! 2. **Errors are structured.** [`AimError`] carries a machine-readable
//!    [`ErrorCode`] plus optional capability state; it never carries a string
//!    written for a particular UI.

pub mod capability;
pub mod domain;
pub mod error;
pub mod event;
pub mod ids;
pub mod provenance;
pub mod tool;
pub mod value;

pub use capability::{
    AdapterCapabilities, AdapterHealth, ConnectionState, ObdProtocol, RequestBudget, TransportKind,
};
pub use domain::{
    AgentTrace, Connection, Diagnosis, DtcRecord, DtcStatus, Measurement, Module, ModuleIdentity,
    Session, TestOutcome, TestRun, Vehicle,
};
pub use error::{AimError, AimResult, ErrorCode};
pub use event::{EventKind, SessionEvent};
pub use ids::{ConnectionId, DiagnosisId, ModuleId, SessionId, TestRunId, VehicleId};
pub use provenance::{Provenance, SourceKind, VerificationStatus};
pub use tool::{PermissionLevel, RiskClass, ToolResult, Warning, WarningSeverity};
pub use value::{DecodedValue, ValidRange, Value};

/// Current wall-clock time as an RFC 3339 timestamp.
pub fn now() -> Timestamp {
    Timestamp(time::OffsetDateTime::now_utc())
}

/// An RFC 3339 UTC timestamp.
///
/// Wrapped in a newtype so that every serialized timestamp in the API and the
/// SQLite store has exactly one representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Timestamp(#[serde(with = "time::serde::rfc3339")] pub time::OffsetDateTime);

impl Timestamp {
    /// Construct from a Unix timestamp in milliseconds.
    pub fn from_unix_millis(ms: i128) -> Self {
        Timestamp(
            time::OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000)
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
        )
    }

    /// Milliseconds since the Unix epoch.
    pub fn unix_millis(&self) -> i128 {
        self.0.unix_timestamp_nanos() / 1_000_000
    }

    /// RFC 3339 rendering, the canonical string form used everywhere.
    pub fn to_rfc3339(&self) -> String {
        self.0
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
    }

    /// Parse from the canonical RFC 3339 string form.
    pub fn parse_rfc3339(s: &str) -> Option<Self> {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .ok()
            .map(Timestamp)
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

/// Format a byte slice as lowercase hex, the canonical raw-evidence encoding.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Parse a hex string (whitespace tolerated) into bytes.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.len() % 2 != 0 {
        return None;
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let bytes = vec![0x41, 0x0c, 0x1a, 0xf0, 0x00];
        assert_eq!(hex(&bytes), "410c1af000");
        assert_eq!(unhex("41 0C 1A F0 00").unwrap(), bytes);
        assert_eq!(unhex("410"), None);
        assert_eq!(unhex("41zz"), None);
    }

    #[test]
    fn timestamp_roundtrip() {
        let ts = Timestamp::from_unix_millis(1_700_000_000_000);
        let s = ts.to_rfc3339();
        assert_eq!(Timestamp::parse_rfc3339(&s).unwrap(), ts);
        assert_eq!(ts.unix_millis(), 1_700_000_000_000);
    }
}
