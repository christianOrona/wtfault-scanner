//! Whether a file is intact and consistent with itself.
//!
//! This is about the artifact, not about any vehicle. A file can be perfectly
//! valid and be for a different car: that is [`crate::resolve`]'s question.
//! And a hash existing proves nothing by itself. A file is called valid only
//! when its content was checked against a hash somebody stated for it
//! beforehand.

use crate::artifact::{ArtifactRecord, Basis};
use crate::format::{content_agrees, sniff, Content};
use crate::identity::{same_identifier, Field};
use serde::{Deserialize, Serialize};

/// The largest file treated as a calibration. A module's whole flash is a few
/// megabytes; a file a hundred times that is something else.
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// What validation concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationStatus {
    /// The content matches a hash stated for it, is a kind of file its name
    /// can be, and nothing claimed about it contradicts anything else.
    Valid,
    /// Something is wrong with it: it is missing, empty, not what its hash
    /// says, not what its name says, or claimed to be two different things.
    Invalid,
    /// Nothing is wrong and nothing vouches for it: it hashes to what it
    /// hashes to, and no source stated what that should be.
    PartiallyValidated,
    /// It is not a kind of file this build treats as a calibration.
    Unknown,
}

/// One thing validation looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCheck {
    /// What was looked at.
    pub what: String,
    /// `Some(true)` passed, `Some(false)` failed, `None` could not be checked.
    pub passed: Option<bool>,
    /// What was found, in words.
    pub detail: String,
}

/// The result, with everything it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    /// The conclusion.
    pub status: ValidationStatus,
    /// The SHA-256 the content actually has, when there was content to hash.
    pub sha256: Option<String>,
    /// Every check, in the order made.
    pub checks: Vec<ValidationCheck>,
}

impl Validation {
    fn check(&mut self, what: &str, passed: Option<bool>, detail: impl Into<String>) {
        self.checks.push(ValidationCheck { what: what.into(), passed, detail: detail.into() });
    }
}

/// Validate an artifact's content against its record.
///
/// `content` is the file's bytes, or `None` when the file could not be read,
/// which is itself a result and not an error.
pub fn validate(record: &ArtifactRecord, content: Option<&[u8]>) -> Validation {
    let mut v = Validation { status: ValidationStatus::Unknown, sha256: None, checks: Vec::new() };

    let Some(bytes) = content else {
        v.check("file", Some(false), "the file is missing or could not be read");
        v.status = ValidationStatus::Invalid;
        return v;
    };
    v.check("file", Some(true), "the file exists and was read");

    let size = bytes.len() as u64;
    let size_ok = size > 0 && size <= MAX_ARTIFACT_BYTES && size == record.size;
    v.check(
        "size",
        Some(size_ok),
        if size == 0 {
            String::from("the file is empty")
        } else if size > MAX_ARTIFACT_BYTES {
            format!("{size} bytes is larger than any calibration this build expects")
        } else if size != record.size {
            format!("{size} bytes, where its record says {}", record.size)
        } else {
            format!("{size} bytes")
        },
    );

    let actual = crate::sha256_hex(bytes);
    v.sha256 = Some(actual.clone());
    // The record's own hash is how the file is filed. A file that no longer
    // hashes to it has been changed since it was kept.
    let filed_ok = record.sha256.eq_ignore_ascii_case(&actual);
    v.check(
        "content hash",
        Some(filed_ok),
        if filed_ok {
            format!("SHA-256 {actual}")
        } else {
            format!("SHA-256 is {actual}, where its record says {}", record.sha256)
        },
    );

    // What a source said it should be, before anybody here hashed it.
    let declared_ok = record.declared_sha256.as_deref().map(|d| d.eq_ignore_ascii_case(&actual));
    v.check(
        "declared hash",
        declared_ok,
        match (&record.declared_sha256, declared_ok) {
            (Some(d), Some(true)) => format!("matches the SHA-256 its source stated, {d}"),
            (Some(d), _) => format!("its source stated SHA-256 {d}, and the file is not that"),
            (None, _) => String::from(
                "no source stated what this file's SHA-256 should be, so its content is \
                 unvouched for",
            ),
        },
    );

    let supported = record.format.is_supported();
    v.check(
        "format",
        Some(supported),
        if supported {
            format!("named as {:?}", record.format)
        } else {
            String::from("not a kind of file this build treats as a calibration")
        },
    );

    let content_kind = sniff(bytes);
    let agrees = content_agrees(record.format, content_kind);
    v.check(
        "content against name",
        agrees,
        match agrees {
            Some(true) => format!("the content is {content_kind:?}, as the name says"),
            Some(false) if content_kind == Content::Empty => String::from("there is no content"),
            Some(false) => {
                format!("the name says {:?} and the content is {content_kind:?}", record.format)
            }
            None => String::from(
                "this kind of file has nothing its content can be checked against here",
            ),
        },
    );

    if let Some(problem) = &record.metadata_problem {
        v.check(
            "metadata file",
            Some(false),
            format!("{problem}. Nothing in it was used, so nothing is declared about this file"),
        );
    }

    // Two declared values for the one thing a file can only be one of.
    let mut contradictions = Vec::new();
    for field in [Field::CalibrationId, Field::HardwareNumber, Field::ProgramId, Field::RomId] {
        let declared: Vec<&str> = record
            .claimed(field)
            .iter()
            .filter(|c| c.basis == Basis::Declared)
            .map(|c| c.value.as_str())
            .collect();
        // Hardware may legitimately be several: one calibration, several
        // boards. A file is one calibration.
        if field != Field::HardwareNumber
            && declared.iter().any(|a| declared.iter().any(|b| !same_identifier(a, b)))
        {
            contradictions.push(format!(
                "{} is declared as {}",
                field.label(),
                declared.join(" and ")
            ));
        }
    }
    v.check(
        "claims",
        Some(contradictions.is_empty()),
        if contradictions.is_empty() {
            String::from("nothing declared about it contradicts anything else declared")
        } else {
            contradictions.join("; ")
        },
    );

    v.status = if !supported {
        ValidationStatus::Unknown
    } else if !size_ok
        || !filed_ok
        || declared_ok == Some(false)
        || agrees == Some(false)
        || !contradictions.is_empty()
    {
        ValidationStatus::Invalid
    } else if declared_ok == Some(true) {
        ValidationStatus::Valid
    } else {
        ValidationStatus::PartiallyValidated
    };
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{Claim, Claims, SourceRef};
    use crate::format::ArtifactFormat;

    fn record_for(name: &str, bytes: &[u8], declared: Option<&str>) -> ArtifactRecord {
        ArtifactRecord {
            sha256: crate::sha256_hex(bytes),
            size: bytes.len() as u64,
            format: ArtifactFormat::from_filename(name),
            filename: name.into(),
            sources: vec![SourceRef {
                source: "folder".into(),
                reference: name.into(),
                found_at: "2026-10-04T00:00:00Z".into(),
            }],
            claims: Claims::new(),
            declared_sha256: declared.map(String::from),
            added_at: "2026-10-04T00:00:00Z".into(),
            metadata_problem: None,
        }
    }

    #[test]
    fn a_file_matching_its_stated_hash_is_valid() {
        let bytes = b"calibration bytes";
        let record = record_for("TEST-CAL-001.bin", bytes, Some(&crate::sha256_hex(bytes)));
        let v = validate(&record, Some(bytes));
        assert_eq!(v.status, ValidationStatus::Valid, "{v:#?}");
        assert_eq!(v.sha256.as_deref(), Some(record.sha256.as_str()));
    }

    /// A hash existing is not a file being vouched for.
    #[test]
    fn a_file_nobody_stated_a_hash_for_is_only_partly_validated() {
        let bytes = b"calibration bytes";
        let v = validate(&record_for("TEST-CAL-001.bin", bytes, None), Some(bytes));
        assert_eq!(v.status, ValidationStatus::PartiallyValidated);
        assert!(v.checks.iter().any(|c| c.what == "declared hash" && c.passed.is_none()));
    }

    #[test]
    fn content_that_is_not_what_was_stated_or_filed_is_invalid() {
        let good = b"calibration bytes";
        let record = record_for("TEST-CAL-001.bin", good, Some(&crate::sha256_hex(good)));

        // Changed since it was kept.
        let v = validate(&record, Some(b"calibration bytez"));
        assert_eq!(v.status, ValidationStatus::Invalid);

        // Never was what its source said.
        let wrong = record_for("TEST-CAL-001.bin", good, Some(&"0".repeat(64)));
        assert_eq!(validate(&wrong, Some(good)).status, ValidationStatus::Invalid);

        // Gone.
        assert_eq!(validate(&record, None).status, ValidationStatus::Invalid);
        // Empty.
        assert_eq!(
            validate(&record_for("x.bin", b"", None), Some(b"")).status,
            ValidationStatus::Invalid
        );
    }

    #[test]
    fn a_name_the_content_cannot_be_is_invalid() {
        let bytes = b"not gzip at all";
        let v = validate(&record_for("37805-5MR-C120.rwd.gz", bytes, None), Some(bytes));
        assert_eq!(v.status, ValidationStatus::Invalid);
    }

    #[test]
    fn a_kind_of_file_this_build_does_not_know_is_unknown_not_invalid() {
        let bytes = b"some notes";
        let v = validate(&record_for("notes.txt", bytes, None), Some(bytes));
        assert_eq!(v.status, ValidationStatus::Unknown);
    }

    #[test]
    fn a_file_declared_to_be_two_calibrations_is_invalid() {
        let bytes = b"calibration bytes";
        let mut record = record_for("TEST-CAL-001.bin", bytes, Some(&crate::sha256_hex(bytes)));
        for value in ["TEST-CAL-001", "TEST-CAL-002"] {
            record.claim(
                Field::CalibrationId,
                Claim { value: value.into(), basis: Basis::Declared, stated_in: None },
            );
        }
        let v = validate(&record, Some(bytes));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(v.checks.iter().any(|c| c.what == "claims" && c.passed == Some(false)));
    }
}
