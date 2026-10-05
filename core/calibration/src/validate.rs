//! Whether a file is intact and consistent with itself.
//!
//! This is about the artifact, not about any vehicle. A file can be perfectly
//! valid and be for a different car: that is [`crate::resolve`]'s question.
//! And a hash existing proves nothing by itself. A file is called valid only
//! when its content was checked against a hash somebody stated for it
//! beforehand.

use crate::artifact::ArtifactRecord;
use crate::format::{content_agrees, inspect, Compression, Content, Inspection};
use crate::identity::same_identifier;
use crate::resolve::ONE_PER_FILE;
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
    let inspection = inspect(content.unwrap_or_default());
    validate_inspected(record, content, &inspection)
}

/// [`validate`], for a caller that has already looked inside the file and
/// should not unpack it twice. `inspection` must be of the same `content`.
pub fn validate_inspected(
    record: &ArtifactRecord,
    content: Option<&[u8]>,
    inspection: &Inspection,
) -> Validation {
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

    // What the name says, layer by layer, against what is actually there.
    let named = record.named();
    let supported = named.is_supported();
    v.check(
        "format",
        Some(supported),
        if supported {
            format!("named as {}", named.describe())
        } else {
            String::from("not a kind of file this build treats as a calibration")
        },
    );

    // The packing: a name that says gzip must be a gzip that opens, and a
    // file that is gzip must say so.
    let packing = match (named.compression, inspection.compression) {
        (Some(Compression::Gzip), Some(Compression::Gzip)) => match &inspection.unpack_problem {
            None => (Some(true), String::from("gzip, as the name says, and it unpacks")),
            Some(problem) => (Some(false), format!("named as gzip, and {problem}")),
        },
        (Some(Compression::Gzip), None) => {
            (Some(false), String::from("the name says gzip and the content is not gzip"))
        }
        (None, Some(Compression::Gzip)) if supported => {
            (Some(false), format!("the name says {} and the content is gzip", named.describe()))
        }
        (None, _) => (None, String::from("not packed")),
    };
    let packing_ok = packing.0 != Some(false);
    v.check("packing", packing.0, packing.1);

    // What is inside, against what the name says is inside. Only asked once
    // the packing has opened: a gzip that does not unpack has shown nothing.
    let content_kind = inspection.content;
    let agrees = if packing_ok { content_agrees(named.payload, content_kind) } else { None };
    v.check(
        "content against name",
        agrees,
        match agrees {
            Some(true) => format!("what is inside is {content_kind:?}, as the name says"),
            Some(false) if content_kind == Content::Empty => String::from("there is no content"),
            Some(false) => {
                format!("the name says {} and what is inside is {content_kind:?}", named.describe())
            }
            None if !packing_ok => String::from("not checked: the packing did not open"),
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
    for field in ONE_PER_FILE {
        let declared: Vec<&str> = record
            .claimed(field)
            .iter()
            .filter(|c| c.basis.is_declared())
            .map(|c| c.value.as_str())
            .collect();
        if declared.iter().any(|a| declared.iter().any(|b| !same_identifier(a, b))) {
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
        || !packing_ok
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
    use crate::artifact::{Basis, Claim, SourceKind, SourceRef};
    use crate::identity::Field;
    use crate::rwd::fixture;
    use std::io::Write;

    fn record_for(name: &str, bytes: &[u8], declared: Option<&str>) -> ArtifactRecord {
        let mut record = ArtifactRecord::found(
            name,
            bytes,
            SourceRef {
                source: "folder".into(),
                kind: SourceKind::UserFolder,
                reference: name.into(),
                found_at: "2026-10-04T00:00:00Z".into(),
            },
            "2026-10-04T00:00:00Z",
        );
        record.declared_sha256 = declared.map(String::from);
        record
    }

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    fn failed<'a>(v: &'a Validation, what: &str) -> Option<&'a ValidationCheck> {
        v.checks.iter().find(|c| c.what == what && c.passed == Some(false))
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

    /// The four things a file named `.rwd.gz` can turn out to be.
    #[test]
    fn a_packed_package_is_checked_one_layer_at_a_time() {
        let package = fixture::z(&[&[b"TEST-MOD-A010"]], &[0x9C, 0x41, 0x07]);
        let name = "TEST-MOD-A010.rwd.gz";

        // gzip holding an RWD package: both layers are what the name says.
        let good = gz(&package);
        let v = validate(&record_for(name, &good, None), Some(&good));
        assert_eq!(v.status, ValidationStatus::PartiallyValidated, "{v:#?}");
        assert!(v.checks.iter().any(|c| c.what == "packing" && c.passed == Some(true)));
        assert!(v
            .checks
            .iter()
            .any(|c| c.what == "content against name" && c.passed == Some(true)));

        // Not gzip at all.
        let bytes = b"not gzip at all";
        let v = validate(&record_for(name, bytes, None), Some(bytes));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(failed(&v, "packing").is_some_and(|c| c.detail.contains("not gzip")));

        // gzip that does not open.
        let mut cut = good.clone();
        cut.truncate(cut.len() - 10);
        let v = validate(&record_for(name, &cut, None), Some(&cut));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(failed(&v, "packing").is_some_and(|c| c.detail.contains("does not unpack")));
        // And nothing is said about what is inside something that did not open.
        assert!(v.checks.iter().any(|c| c.what == "content against name" && c.passed.is_none()));

        // gzip that opens, holding something that is not an RWD package.
        let other = gz(b"these are notes, not a package");
        let v = validate(&record_for(name, &other, None), Some(&other));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(failed(&v, "packing").is_none());
        assert!(failed(&v, "content against name")
            .is_some_and(|c| c.detail.contains("gzip holding a Honda RWD package")));
    }

    #[test]
    fn a_plain_package_is_checked_against_its_name_too() {
        let package = fixture::one(&[(b'$', &["TEST-MOD-A010"])], &[0x9C, 0x41]);
        let v = validate(&record_for("TEST-MOD-A010.rwd", &package, None), Some(&package));
        assert_eq!(v.status, ValidationStatus::PartiallyValidated, "{v:#?}");

        // Named as a package, and not one.
        let bytes = b"some other bytes";
        let v = validate(&record_for("TEST-MOD-A010.rwd", bytes, None), Some(bytes));
        assert_eq!(v.status, ValidationStatus::Invalid);

        // Named as a package, and really a packed one: the name left a layer out.
        let packed = gz(&package);
        let v = validate(&record_for("TEST-MOD-A010.rwd", &packed, None), Some(&packed));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(failed(&v, "packing").is_some_and(|c| c.detail.contains("is gzip")));
    }

    /// A gzip whose name does not say what it holds is opened and looked at,
    /// and nothing is required of what is inside.
    #[test]
    fn a_gzip_that_does_not_name_its_content_only_has_to_open() {
        let packed = gz(b"anything at all");
        let v = validate(&record_for("download.gz", &packed, None), Some(&packed));
        assert_eq!(v.status, ValidationStatus::PartiallyValidated, "{v:#?}");
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
                Claim {
                    value: value.into(),
                    basis: Basis::UserDeclared,
                    stated_in: None,
                    source: None,
                },
            );
        }
        let v = validate(&record, Some(bytes));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(v.checks.iter().any(|c| c.what == "claims" && c.passed == Some(false)));
    }
}
