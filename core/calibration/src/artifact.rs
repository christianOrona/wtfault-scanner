//! A calibration file, and what is claimed about it.
//!
//! The file is identified by its SHA-256 and by nothing else. Its name is a
//! label somebody gave it. What it is *for* is a set of claims, and a claim is
//! only as good as whoever made it, so each one says on what basis it is made.

use crate::format::ArtifactFormat;
use crate::identity::Field;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// On what basis something is claimed about an artifact.
///
/// The order matters and is the whole point: only a [`Basis::Declared`] claim
/// can make a match exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Read off the file's name. A name is whatever the last person to rename
    /// the file typed.
    Filename,
    /// Stated by the source in metadata it supplied alongside the file, which
    /// is a statement somebody can be held to.
    Declared,
}

/// One claim about an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// What is claimed, as text.
    pub value: String,
    /// On what basis.
    pub basis: Basis,
    /// Where exactly it was read, for instance the metadata file's name.
    pub stated_in: Option<String>,
}

/// Everything claimed about an artifact, by field. Several values for one
/// field mean any of them: a calibration released for three model years
/// carries three.
pub type Claims = BTreeMap<Field, Vec<Claim>>;

/// One place an artifact was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// The source's id, for instance `folder`.
    pub source: String,
    /// Where in that source: a path, or an address.
    pub reference: String,
    /// When it was found there, RFC 3339.
    pub found_at: String,
}

/// What is kept about an artifact beside the file itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    /// SHA-256 of the file's content, lowercase hex. This is the artifact.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// What kind of file its name says it is.
    pub format: ArtifactFormat,
    /// The name it had where it was first found.
    pub filename: String,
    /// Everywhere it has been found. The same content from two places is one
    /// artifact with two of these.
    pub sources: Vec<SourceRef>,
    /// What is claimed about it.
    #[serde(default)]
    pub claims: Claims,
    /// The SHA-256 a source said the file should have, when one did. Kept
    /// apart from `sha256`, which is what the file actually hashes to.
    #[serde(default)]
    pub declared_sha256: Option<String>,
    /// When it was first kept, RFC 3339.
    pub added_at: String,
    /// Why the metadata file found beside it could not be used, when one was
    /// there and could not. Nothing from such a file is taken, and saying so
    /// is the difference between "nothing was declared" and "what was
    /// declared could not be read".
    #[serde(default)]
    pub metadata_problem: Option<String>,
}

impl ArtifactRecord {
    /// Every value claimed for a field, with its basis.
    pub fn claimed(&self, field: Field) -> &[Claim] {
        self.claims.get(&field).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Add a claim, unless the same one is already there.
    pub fn claim(&mut self, field: Field, claim: Claim) {
        if claim.value.trim().is_empty() {
            return;
        }
        let known = self.claims.entry(field).or_default();
        if !known.iter().any(|k| k.value == claim.value && k.basis == claim.basis) {
            known.push(claim);
        }
    }

    /// Fold another sighting of the same content into this record: its
    /// sources and claims are added, and nothing already here is replaced.
    pub fn absorb(&mut self, other: &ArtifactRecord) {
        for s in &other.sources {
            if !self.sources.iter().any(|k| k.source == s.source && k.reference == s.reference) {
                self.sources.push(s.clone());
            }
        }
        for (field, claims) in &other.claims {
            for c in claims {
                self.claim(*field, c.clone());
            }
        }
        if self.declared_sha256.is_none() {
            self.declared_sha256 = other.declared_sha256.clone();
        }
        if self.metadata_problem.is_none() {
            self.metadata_problem = other.metadata_problem.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(source: &str) -> ArtifactRecord {
        ArtifactRecord {
            sha256: "ab".repeat(32),
            size: 4,
            format: ArtifactFormat::Bin,
            filename: "TEST-CAL-001.bin".into(),
            sources: vec![SourceRef {
                source: source.into(),
                reference: format!("{source}/TEST-CAL-001.bin"),
                found_at: "2026-10-04T00:00:00Z".into(),
            }],
            claims: Claims::new(),
            declared_sha256: None,
            added_at: "2026-10-04T00:00:00Z".into(),
            metadata_problem: None,
        }
    }

    #[test]
    fn the_same_content_from_two_places_is_one_record_with_two_sources() {
        let mut first = record("folder");
        let mut second = record("another-folder");
        second.claim(
            Field::CalibrationId,
            Claim { value: "TEST-CAL-001".into(), basis: Basis::Declared, stated_in: None },
        );

        first.absorb(&second);
        first.absorb(&second);

        assert_eq!(first.sources.len(), 2);
        assert_eq!(first.claimed(Field::CalibrationId).len(), 1);
        assert_eq!(first.filename, "TEST-CAL-001.bin");
    }

    #[test]
    fn a_declared_claim_outranks_a_name() {
        assert!(Basis::Declared > Basis::Filename);
    }
}
