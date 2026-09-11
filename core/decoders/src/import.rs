//! Bringing somebody else's profile in, and refusing to take its word for
//! anything.
//!
//! # The rule everything here exists to enforce
//!
//! **An imported mapping arrives unverified, whatever the file says about
//! itself.** `verification: verified` in a downloaded file is a claim by its
//! author about work this installation did not witness. Honouring it would
//! mean a stranger's YAML could mark itself trustworthy and then be written to
//! a body module on that basis — which is the entire attack, and it needs no
//! cleverness at all.
//!
//! So verification is stripped on the way in, always, and re-earned on *this*
//! vehicle by the same predict-and-check loop everything else uses. The file's
//! own claim is preserved as a `claimed_verified` finding — not to be acted
//! on, but because "this author says they measured it" is worth showing a
//! person deciding whether to test it.
//!
//! # A profile is data
//!
//! No expressions are evaluated at import, nothing is loaded as code, and
//! nothing in a profile can name a file path or a URL. The formula parser that
//! PIDs use is deliberately not reachable from here: it is a real evaluator,
//! and reaching it from imported text would turn data into behaviour.
//!
//! # Preview before accept
//!
//! Importing shows what would be added and what would be overridden, by id,
//! before anything changes. Overriding a mapping that was measured on this
//! vehicle with one from a stranger is exactly the mistake worth catching, and
//! it is invisible unless somebody is shown it.

use crate::features::FeatureCatalog;
use aim_types::VerificationStatus;
use serde::{Deserialize, Serialize};

/// How serious a validation finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing; the import proceeds.
    Note,
    /// Worth a person's attention before accepting.
    Caution,
    /// The import cannot proceed.
    Blocking,
}

/// One thing noticed about a profile being imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable identifier for the kind of finding.
    pub code: String,
    /// Which feature it concerns, when it concerns one.
    pub feature_id: Option<String>,
    /// What was noticed, in language meant for a person.
    pub detail: String,
    /// How serious it is.
    pub severity: Severity,
}

impl Finding {
    fn new(code: &str, severity: Severity, detail: impl Into<String>) -> Finding {
        Finding { code: code.to_string(), feature_id: None, detail: detail.into(), severity }
    }

    fn about(mut self, feature_id: impl Into<String>) -> Finding {
        self.feature_id = Some(feature_id.into());
        self
    }
}

/// What one feature in an imported profile would do to what is already here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The feature id.
    pub id: String,
    /// Its human name, as the file gives it.
    pub name: String,
    /// Whether something with this id already exists.
    pub overrides_existing: bool,
    /// Whether what it would replace was measured on this vehicle.
    ///
    /// The finding that matters most in the whole preview. Replacing a mapping
    /// somebody measured on their own truck with one from a stranger's file is
    /// a real loss, and it happens silently unless it is shown.
    pub overrides_measured: bool,
    /// What the file claims about its own verification.
    pub claimed_verification: VerificationStatus,
    /// What it will actually be given. Always unverified.
    pub verification_on_import: VerificationStatus,
}

/// Everything an import would do, before it does any of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    /// Where it came from, for the record.
    pub source: String,
    /// What each feature would do.
    pub changes: Vec<Change>,
    /// Everything noticed while checking it.
    pub findings: Vec<Finding>,
    /// Whether it can be accepted at all.
    pub acceptable: bool,
}

impl ImportPreview {
    /// Findings at or above a severity.
    pub fn at_least(&self, severity: Severity) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.severity >= severity)
    }

    /// How many features would replace one measured on this vehicle.
    pub fn overriding_measured(&self) -> usize {
        self.changes.iter().filter(|c| c.overrides_measured).count()
    }
}

/// The largest profile this will look at.
///
/// A megabyte is far more than any real profile and small enough that a
/// hostile or accidental multi-gigabyte file is refused before it is parsed
/// rather than after it has been held in memory.
pub const MAX_PROFILE_BYTES: usize = 1024 * 1024;

/// Check a profile against what is already loaded, without changing anything.
///
/// `existing` is the catalogue it would be merged into, so the preview can say
/// what would be overridden rather than only what the file contains.
pub fn preview(text: &str, source: &str, existing: &FeatureCatalog) -> ImportPreview {
    let mut findings = Vec::new();
    let mut changes = Vec::new();

    if text.len() > MAX_PROFILE_BYTES {
        findings.push(Finding::new(
            "too_large",
            Severity::Blocking,
            format!(
                "This file is {} bytes. A profile is a small text file; anything this large is \
                 not one.",
                text.len()
            ),
        ));
        return ImportPreview { source: source.to_string(), changes, findings, acceptable: false };
    }

    // Parsed into a scratch catalogue so a malformed file cannot half-apply
    // itself into the live one. Nothing the caller holds is touched until the
    // preview has been accepted.
    let mut scratch = FeatureCatalog::default();
    match scratch.load_yaml(text, source) {
        Ok(0) => findings.push(Finding::new(
            "nothing_to_import",
            Severity::Blocking,
            "The file parsed but defines no features.",
        )),
        Ok(_) => {}
        Err(e) => {
            findings.push(Finding::new(
                "will_not_parse",
                Severity::Blocking,
                format!("This is not a profile this build can read: {}", e.message),
            ));
            return ImportPreview {
                source: source.to_string(),
                changes,
                findings,
                acceptable: false,
            };
        }
    }

    let mut seen: Vec<&str> = Vec::new();
    for f in scratch.all() {
        // Duplicate ids inside one file. The catalogue itself deduplicates by
        // id, so the second silently wins and the author never finds out which.
        if seen.contains(&f.id.as_str()) {
            findings.push(
                Finding::new(
                    "duplicate_id",
                    Severity::Caution,
                    "This id appears more than once in the file. Only one of them will be used.",
                )
                .about(&f.id),
            );
        }
        seen.push(&f.id);

        // A mapping this build cannot execute is not a failure, but somebody
        // importing a file expecting to change a setting should be told the
        // file does not contain the means.
        if f.mapping.is_none() {
            findings.push(
                Finding::new(
                    "described_only",
                    Severity::Note,
                    "Describes a feature but says nothing about where it lives, so it can be \
                     explained and never read or changed.",
                )
                .about(&f.id),
            );
        }

        // Provenance. A mapping with no stated source cannot be traced back to
        // whoever measured it, which is most of what makes a wrong one fixable.
        //
        // Compared against the import source rather than checked for emptiness:
        // `FeatureCatalog::load_yaml` substitutes the file's own name when an
        // author states nothing, so by the time a definition exists it always
        // has *a* source. Attribution to "the file it arrived in" is not the
        // same as the author saying where they measured it, and the first
        // draft of this check tested a field that could never be empty.
        if f.mapping.is_some() && f.source.as_deref() == Some(source) {
            findings.push(
                Finding::new(
                    "no_provenance",
                    Severity::Caution,
                    "This mapping does not say where it came from. A wrong one cannot be traced \
                     back to whoever measured it.",
                )
                .about(&f.id),
            );
        }

        // Scope. A mapping with no applicability at all claims to fit every
        // vehicle ever built, which no measured mapping does.
        if f.mapping.is_some()
            && f.applies_to.vins.is_empty()
            && f.applies_to.makes.is_empty()
            && f.applies_to.wmi_prefixes.is_empty()
            && f.applies_to.model_years.is_none()
        {
            findings.push(
                Finding::new(
                    "unscoped_mapping",
                    Severity::Caution,
                    "This mapping says which bits hold a setting but not which vehicles it was \
                     measured on, so it will be offered for every vehicle. No measured mapping \
                     is true of every vehicle.",
                )
                .about(&f.id),
            );
        }

        // The claim this module exists to disarm.
        if f.verification == VerificationStatus::Verified {
            findings.push(
                Finding::new(
                    "claimed_verified",
                    Severity::Note,
                    "The file says this mapping is verified. That is a claim about work this \
                     computer did not witness, so it is imported unverified and has to be \
                     confirmed on your vehicle before it can be written.",
                )
                .about(&f.id),
            );
        }
        if f.write_verification.as_ref().is_some_and(|w| w.is_verified()) {
            findings.push(
                Finding::new(
                    "claimed_write_verified",
                    Severity::Caution,
                    "The file claims writing this has been demonstrated. Write evidence is never \
                     imported: it is the claim that would let a stranger's file change your \
                     vehicle, and it has to be earned here.",
                )
                .about(&f.id),
            );
        }

        // Risk. A file cannot grant itself permission to touch a class this
        // build refuses, but importing one that will be permanently refused is
        // worth saying rather than leaving to be discovered.
        if !f.risk.is_ever_implementable() {
            findings.push(
                Finding::new(
                    "risk_never_permitted",
                    Severity::Note,
                    format!(
                        "Classed {}, which this app never changes whatever evidence exists. It \
                         will be imported and can be read about, not written.",
                        f.risk.label()
                    ),
                )
                .about(&f.id),
            );
        }

        let previous = existing.get(&f.id);
        changes.push(Change {
            id: f.id.clone(),
            name: f.name.clone(),
            overrides_existing: previous.is_some(),
            overrides_measured: previous.is_some_and(|p| {
                p.verification == VerificationStatus::Verified && !p.applies_to.vins.is_empty()
            }),
            claimed_verification: f.verification,
            verification_on_import: VerificationStatus::Unverified,
        });
    }

    for c in changes.iter().filter(|c| c.overrides_measured) {
        findings.push(
            Finding::new(
                "overrides_a_measured_mapping",
                Severity::Caution,
                "Something already here with this id was measured on a specific vehicle. \
                 Importing this replaces it with a mapping from somewhere else.",
            )
            .about(&c.id),
        );
    }

    let acceptable = !findings.iter().any(|f| f.severity == Severity::Blocking);
    ImportPreview { source: source.to_string(), changes, findings, acceptable }
}

/// Strip every claim a file makes about having been verified.
///
/// Applied to the text before it reaches the live catalogue, so there is no
/// window in which a trusted-looking definition exists. Reading is re-earned by
/// checking the mapping against the vehicle; writing is re-earned by writing
/// and reading back. Neither is granted by a file saying so.
pub fn strip_verification_claims(catalog: &mut FeatureCatalog) {
    for f in catalog.all_mut() {
        f.verification = VerificationStatus::Unverified;
        f.write_verification = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAIMS_VERIFIED: &str = r#"
features:
  - id: imported.autolock
    name: "Auto lock"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: ["726"]
    verification: verified
    source: "measured by someone else"
    applies_to:
      vins: ["1FT7W2BT7KEF00000"]
    write_verification:
      verification: verified
      verified_on_vehicles: 9
      source: "trust me"
    mapping:
      kind: data_identifier_bits
      module: "726"
      did: 0xDE0E
      byte: 4
      mask: 0x01
      on: 0x01
      off: 0x00
"#;

    fn catalog_from(yaml: &str) -> FeatureCatalog {
        let mut c = FeatureCatalog::default();
        c.load_yaml(yaml, "test").unwrap();
        c
    }

    /// The rule the whole module exists for. A file cannot vouch for itself.
    #[test]
    fn an_imported_mapping_arrives_unverified_whatever_it_claims() {
        let mut imported = catalog_from(CLAIMS_VERIFIED);
        assert_eq!(
            imported.get("imported.autolock").unwrap().verification,
            VerificationStatus::Verified,
            "the file does claim it"
        );

        strip_verification_claims(&mut imported);

        let f = imported.get("imported.autolock").unwrap();
        assert_eq!(f.verification, VerificationStatus::Unverified);
        assert!(f.write_verification.is_none(), "write evidence is never imported");
        // And the mapping itself survives: this disarms the claim, not the data.
        assert!(f.mapping.is_some());
    }

    /// The claim is still shown, because "this author says they measured it" is
    /// worth knowing when deciding whether to test it.
    #[test]
    fn the_claim_is_reported_even_though_it_is_not_honoured() {
        let p = preview(CLAIMS_VERIFIED, "file:test.yaml", &FeatureCatalog::default());
        assert!(p.acceptable);

        let codes: Vec<&str> = p.findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"claimed_verified"), "{codes:?}");
        assert!(codes.contains(&"claimed_write_verified"), "{codes:?}");

        let change = &p.changes[0];
        assert_eq!(change.claimed_verification, VerificationStatus::Verified);
        assert_eq!(change.verification_on_import, VerificationStatus::Unverified);
    }

    /// The preview's most important job: saying that accepting this would
    /// throw away something measured on the owner's own vehicle.
    #[test]
    fn overriding_a_mapping_measured_here_is_called_out() {
        let mine = catalog_from(
            r#"
features:
  - id: imported.autolock
    name: "Auto lock"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: ["726"]
    verification: verified
    applies_to:
      vins: ["1FT7W2BT7KEF78036"]
    mapping:
      kind: data_identifier_bits
      module: "726"
      did: 0xDE0E
      byte: 4
      mask: 0x01
      on: 0x01
      off: 0x00
"#,
        );

        let p = preview(CLAIMS_VERIFIED, "url:example.com/x.yaml", &mine);
        assert_eq!(p.overriding_measured(), 1);
        assert!(p
            .findings
            .iter()
            .any(|f| f.code == "overrides_a_measured_mapping" && f.severity == Severity::Caution));
    }

    /// A mapping with no scope claims to fit every vehicle ever built.
    #[test]
    fn an_unscoped_mapping_is_flagged() {
        let unscoped =
            CLAIMS_VERIFIED.replace("    applies_to:\n      vins: [\"1FT7W2BT7KEF00000\"]\n", "");
        let p = preview(&unscoped, "file:x.yaml", &FeatureCatalog::default());
        assert!(p.findings.iter().any(|f| f.code == "unscoped_mapping"), "{:?}", p.findings);
    }

    /// Nothing half-applies. A file that will not parse changes nothing and
    /// says why.
    #[test]
    fn a_file_that_will_not_parse_is_refused_whole() {
        let p =
            preview("features: [ this is not\n  valid: yaml", "file:x", &FeatureCatalog::default());
        assert!(!p.acceptable);
        assert!(p.findings.iter().any(|f| f.severity == Severity::Blocking));
        assert!(p.changes.is_empty());
    }

    /// And a file that parses into nothing is not an import.
    #[test]
    fn an_empty_profile_is_refused() {
        let p = preview("features: []", "file:x", &FeatureCatalog::default());
        assert!(!p.acceptable);
        assert!(p.findings.iter().any(|f| f.code == "nothing_to_import"));
    }

    /// Refused before it is parsed rather than after it has been held in
    /// memory.
    #[test]
    fn an_absurdly_large_file_is_refused_before_parsing() {
        let huge = "x".repeat(MAX_PROFILE_BYTES + 1);
        let p = preview(&huge, "url:somewhere", &FeatureCatalog::default());
        assert!(!p.acceptable);
        assert!(p.findings.iter().any(|f| f.code == "too_large"));
    }

    /// A mapping nobody can trace back to whoever measured it.
    #[test]
    fn a_mapping_with_no_stated_source_is_flagged() {
        let anonymous = CLAIMS_VERIFIED.replace("    source: \"measured by someone else\"\n", "");
        let p = preview(&anonymous, "file:x", &FeatureCatalog::default());
        assert!(p.findings.iter().any(|f| f.code == "no_provenance"), "{:?}", p.findings);
    }
}
