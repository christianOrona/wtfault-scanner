//! Provenance: where a value came from.
//!
//! Handoff §8 requires that every knowledge-derived claim carry provenance
//! internally. Applied to the diagnostic core this means: a decoded value must
//! be able to answer "which raw bytes produced you, which decoder ran, which
//! version of that decoder, and has that decoder been validated?".

use crate::Timestamp;
use serde::{Deserialize, Serialize};

/// What kind of thing produced a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Bytes returned by an ECU over the wire.
    EcuResponse,
    /// A value produced by a data-file decoder from ECU bytes.
    Decoder,
    /// A value read out of a versioned vehicle profile / knowledge file.
    ProfileData,
    /// A value produced by the deterministic simulator, not a real vehicle.
    Simulator,
    /// A value replayed from a recorded adapter transcript.
    Replay,
    /// A value the user typed or asserted.
    UserStatement,
    /// A value derived by computation over other values.
    Inference,
}

/// Whether the definition that produced a value has been validated.
///
/// The handoff is explicit that unvalidated entries must be marked as such and
/// must not be used to make claims. This enum is how that marking travels with
/// the data at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Defined by a public standard the project has checked against.
    Verified,
    /// Present in a data file but never validated against a vehicle or standard.
    /// Consumers must surface this and must not present the value as fact.
    #[default]
    Unverified,
    /// Known to be wrong or deprecated; kept for transcript compatibility only.
    Rejected,
}

impl VerificationStatus {
    /// True when a consumer may present this value as a factual reading.
    pub fn is_trustworthy(&self) -> bool {
        matches!(self, VerificationStatus::Verified)
    }
}

/// The audit trail attached to a single decoded value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// What sort of origin this is.
    pub source: SourceKind,
    /// Lowercase hex of the exact bytes the value was decoded from.
    pub raw_hex: String,
    /// Identifier of the decoder that ran, e.g. `obd2.mode01.pid0C`.
    pub decoder_id: String,
    /// Version of the decoder definition, so replays can detect drift.
    pub decoder_version: String,
    /// Whether the decoder definition is validated.
    pub verification: VerificationStatus,
    /// When the underlying bytes were observed.
    pub observed_at: Timestamp,
    /// Optional pointer into the append-only event log for the full exchange.
    pub evidence_ref: Option<i64>,
}

impl Provenance {
    /// Provenance for a value decoded from ECU bytes by a data-file decoder.
    pub fn decoded(
        raw: &[u8],
        decoder_id: impl Into<String>,
        decoder_version: impl Into<String>,
        verification: VerificationStatus,
        observed_at: Timestamp,
    ) -> Self {
        Provenance {
            source: SourceKind::Decoder,
            raw_hex: crate::hex(raw),
            decoder_id: decoder_id.into(),
            decoder_version: decoder_version.into(),
            verification,
            observed_at,
            evidence_ref: None,
        }
    }

    /// Attach the event-log row id that holds the full request/response.
    pub fn with_evidence_ref(mut self, id: i64) -> Self {
        self.evidence_ref = Some(id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unverified_is_the_default_and_is_not_trustworthy() {
        assert_eq!(VerificationStatus::default(), VerificationStatus::Unverified);
        assert!(!VerificationStatus::default().is_trustworthy());
        assert!(VerificationStatus::Verified.is_trustworthy());
        assert!(!VerificationStatus::Rejected.is_trustworthy());
    }

    #[test]
    fn provenance_keeps_the_raw_bytes() {
        let p = Provenance::decoded(
            &[0x41, 0x0c, 0x1a, 0xf8],
            "obd2.mode01.pid0C",
            "1",
            VerificationStatus::Verified,
            crate::now(),
        );
        assert_eq!(p.raw_hex, "410c1af8");
        assert_eq!(p.source, SourceKind::Decoder);
    }
}
