//! A calibration file, and what is claimed about it.
//!
//! The file is identified by its SHA-256 and by nothing else. Its name is a
//! label somebody gave it. What it is *for* is a set of claims, and a claim is
//! only as good as whoever made it, so each one says who that was and on what
//! basis.
//!
//! Nothing here can say a file came from its manufacturer. A claim is a
//! statement by a named somebody; it is never authentication, and no basis
//! below is one.

use crate::format::{ArtifactFormat, Compression, Named};
use crate::identity::Field;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// On what basis something is said about an artifact.
///
/// Ordered weakest first. When two disagree, the stronger one decides and the
/// disagreement is still shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Read off the file's name. A name is whatever the last person to rename
    /// the file typed.
    Filename,
    /// Stated by the person using this app, in a metadata file they put beside
    /// the artifact. It is evidence of what they say the file is. Records
    /// written before bases were told apart call this `declared`: every one of
    /// them was a metadata file in the person's own folder.
    #[serde(alias = "declared")]
    UserDeclared,
    /// Stated by a source in its own right, about a file it supplied. No
    /// source that ships with this crate states anything of its own.
    SourceDeclared,
    /// Read out of the file's own header. The file is speaking for itself,
    /// which is more than anyone's description of it. What a header value
    /// *means* is a separate question, and for an RWD package it is not
    /// settled: see [`crate::rwd`].
    FileHeader,
}

impl Basis {
    /// True for a statement somebody made about the file: by the person, or
    /// by a source.
    pub fn is_declared(self) -> bool {
        matches!(self, Basis::UserDeclared | Basis::SourceDeclared)
    }

    /// Who is behind it, in words a result can show.
    pub fn in_words(self) -> &'static str {
        match self {
            Basis::Filename => "the file's name",
            Basis::UserDeclared => "what you declared about the file",
            Basis::SourceDeclared => "what the file's source declared about it",
            Basis::FileHeader => "the file's own header",
        }
    }
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
    /// The id of the source it reached this app through. `None` in a record
    /// written before this was kept.
    #[serde(default)]
    pub source: Option<String>,
}

/// Everything claimed about an artifact, by field. Several values for one
/// field mean any of them: a calibration released for three model years
/// carries three.
pub type Claims = BTreeMap<Field, Vec<Claim>>;

/// What kind of place a source is.
///
/// Only kinds that something in this crate actually is. A source that fetches
/// from a network, an index of metadata, a plug-in: none exists here, so none
/// is named here, and adding one adds its kind with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A folder the person puts files in themselves.
    UserFolder,
    /// The folder a manufacturer's own service tool installed its files in,
    /// on this computer. It is somewhere files are, not a statement by the
    /// manufacturer about any one of them.
    ToolInstallation,
    /// The files this app has already kept.
    Kept,
    /// Any other place, for a source this crate does not ship.
    Other,
}

impl SourceKind {
    /// What every sighting recorded before kinds were kept was: the person's
    /// own folder, the only place that wrote one.
    fn of_an_old_record() -> SourceKind {
        SourceKind::UserFolder
    }
}

/// One place an artifact was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// The source's id, for instance `folder`.
    pub source: String,
    /// What kind of place that source is.
    #[serde(default = "SourceKind::of_an_old_record")]
    pub kind: SourceKind,
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
    /// What its name says it is, once any packing is taken off.
    pub format: ArtifactFormat,
    /// The packing its name states, when it states one.
    #[serde(default)]
    pub compression: Option<Compression>,
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
    /// A record for content found in one place, with nothing claimed yet.
    pub fn found(filename: &str, bytes: &[u8], source: SourceRef, now: &str) -> ArtifactRecord {
        let named = Named::from_filename(filename);
        ArtifactRecord {
            sha256: crate::sha256_hex(bytes),
            size: bytes.len() as u64,
            format: named.payload,
            compression: named.compression,
            filename: filename.to_string(),
            sources: vec![source],
            claims: Claims::new(),
            declared_sha256: None,
            added_at: now.to_string(),
            metadata_problem: None,
        }
    }

    /// What its name says it is, layer by layer. Read from the name itself,
    /// so a record written before packing was kept apart gives the same
    /// answer as one written now.
    pub fn named(&self) -> Named {
        Named::from_filename(&self.filename)
    }

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
        ArtifactRecord::found(
            "TEST-CAL-001.bin",
            b"test",
            SourceRef {
                source: source.into(),
                kind: SourceKind::UserFolder,
                reference: format!("{source}/TEST-CAL-001.bin"),
                found_at: "2026-10-04T00:00:00Z".into(),
            },
            "2026-10-04T00:00:00Z",
        )
    }

    #[test]
    fn the_same_content_from_two_places_is_one_record_with_two_sources() {
        let mut first = record("folder");
        let mut second = record("another-folder");
        second.claim(
            Field::CalibrationId,
            Claim {
                value: "TEST-CAL-001".into(),
                basis: Basis::UserDeclared,
                stated_in: None,
                source: Some("another-folder".into()),
            },
        );

        first.absorb(&second);
        first.absorb(&second);

        assert_eq!(first.sources.len(), 2);
        assert_eq!(first.claimed(Field::CalibrationId).len(), 1);
        assert_eq!(first.filename, "TEST-CAL-001.bin");
    }

    #[test]
    fn bases_are_ordered_and_none_of_them_is_the_manufacturer() {
        assert!(Basis::UserDeclared > Basis::Filename);
        assert!(Basis::SourceDeclared > Basis::UserDeclared);
        assert!(Basis::FileHeader > Basis::SourceDeclared);
        assert!(Basis::UserDeclared.is_declared() && Basis::SourceDeclared.is_declared());
        assert!(!Basis::Filename.is_declared() && !Basis::FileHeader.is_declared());
    }

    /// A record kept by the first build of this crate still reads, and what
    /// it called `declared` is what the person declared.
    #[test]
    fn a_record_written_before_provenance_was_kept_still_reads() {
        let old = r#"{
            "sha256": "abababababababababababababababababababababababababababababababab",
            "size": 4, "format": "gz", "filename": "TEST-CAL-001.rwd.gz",
            "sources": [{ "source": "folder", "reference": "x", "found_at": "t" }],
            "claims": { "calibration_id": [
                { "value": "TEST-CAL-001", "basis": "declared", "stated_in": "x.json" } ] },
            "added_at": "t" }"#;
        let record: ArtifactRecord = serde_json::from_str(old).unwrap();

        let claim = &record.claimed(Field::CalibrationId)[0];
        assert_eq!(claim.basis, Basis::UserDeclared);
        assert_eq!(claim.source, None);
        assert_eq!(record.sources[0].kind, SourceKind::UserFolder);
        // The layers come from the name, whatever the old record said.
        let named = record.named();
        assert_eq!(named.compression, Some(Compression::Gzip));
        assert_eq!(named.payload, ArtifactFormat::Rwd);
    }

    #[test]
    fn a_record_is_named_by_both_layers() {
        let packed = ArtifactRecord::found(
            "TEST-MOD-A010.rwd.gz",
            b"x",
            SourceRef {
                source: "folder".into(),
                kind: SourceKind::UserFolder,
                reference: "x".into(),
                found_at: "t".into(),
            },
            "t",
        );
        assert_eq!(packed.format, ArtifactFormat::Rwd);
        assert_eq!(packed.compression, Some(Compression::Gzip));
    }
}
