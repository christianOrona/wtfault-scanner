//! Whether a file is a module's calibration.
//!
//! Four answers and no percentage. Every answer is a list of comparisons
//! anyone can read, and the status is a rule applied to that list:
//!
//! - **No match** when anything both sides state disagrees.
//! - **Exact** when the module's own calibration identification equals one a
//!   source has declared for the file, and nothing disagrees.
//! - **Partial** when something agrees and the calibration identification is
//!   not among the things that do.
//! - **Unknown** when nothing could be compared at all.
//!
//! A file's name can agree, and that is recorded, but a name is what the last
//! person to rename the file typed. It can make a match partial. It cannot
//! make one exact, and no reasoning about how well everything else fits can
//! either: the rule has no input for it.

use crate::artifact::{ArtifactRecord, Basis};
use crate::cache::{Cache, Incoming};
use crate::identity::{same_identifier, CalibrationIdentity, Field};
use crate::source::{CalibrationSource, SourceInfo};
use crate::validate::{validate, Validation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The module reports it and a source declares the same for the file.
    Confirmed,
    /// Both state it and they differ.
    Conflict,
    /// The file's name agrees with what the module reports. Recorded, and
    /// worth exactly what a file name is worth.
    NameAgrees,
    /// The file's name suggests something else. Not a conflict: a name is not
    /// a statement about the file.
    NameDiffers,
    /// One side or both do not state it, or it is a description that cannot
    /// be compared as an identifier.
    Unknown,
}

/// One comparison between what the module reports and what the file claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// What was compared.
    pub field: Field,
    /// What was found.
    pub verdict: Verdict,
    /// What the module, or the vehicle's identification, states.
    pub reported: Vec<String>,
    /// What is claimed for the file, each with its basis.
    pub claimed: Vec<(String, Basis)>,
    /// The comparison in words.
    pub note: String,
}

/// The four answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchStatus {
    /// The file is declared to be the calibration the module reports.
    ExactMatch,
    /// Something agrees; the calibration itself is not established.
    PartialMatch,
    /// Nothing could be compared.
    Unknown,
    /// Something both sides state disagrees.
    NoMatch,
}

/// A file judged against a module, with everything the judgement rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchReport {
    /// The answer.
    pub status: MatchStatus,
    /// Every comparison made, decisive ones last.
    pub checks: Vec<Check>,
    /// Why the answer is what it is, in one or two sentences.
    pub reason: String,
}

/// The fields compared, in the order shown. Vehicle first, then the module,
/// then its software, ending with the one that decides.
const COMPARED: [Field; 14] = [
    Field::Make,
    Field::Model,
    Field::ModelYear,
    Field::Engine,
    Field::Transmission,
    Field::HardwareNumber,
    Field::PartNumber,
    Field::SoftwareNumber,
    Field::ProgramId,
    Field::StrategyId,
    Field::RomId,
    Field::BootSoftwareId,
    Field::CalibrationVerificationNumber,
    Field::CalibrationId,
];

/// Always shown, even when neither side states them, because their absence
/// is the reason a match is not exact.
const ALWAYS_SHOWN: [Field; 2] = [Field::HardwareNumber, Field::CalibrationId];

/// Words rather than identifiers. Two sources name the same thing
/// differently: a VIN gives the make as `Honda (US)` where a file's metadata
/// says `Honda`, and an engine is `3.5 L V6` or `3.5L V6`.
fn is_description(field: Field) -> bool {
    matches!(field, Field::Make | Field::Model | Field::Engine | Field::Transmission)
}

/// Whether wording that differs is a disagreement. A make or a model that
/// shares no word with the other side's is a different make or model. An
/// engine described two ways is only described two ways.
fn differing_words_conflict(field: Field) -> bool {
    matches!(field, Field::Make | Field::Model)
}

/// A description with its punctuation and spacing removed.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase()
}

/// A description's words.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

fn agree(field: Field, a: &str, b: &str) -> bool {
    if !is_description(field) {
        return same_identifier(a, b);
    }
    let (sa, sb) = (squash(a), squash(b));
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    // The same once spacing is ignored, or one side says everything the other
    // does and then adds to it.
    let (wa, wb) = (words(a), words(b));
    sa == sb || wa.iter().all(|w| wb.contains(w)) || wb.iter().all(|w| wa.contains(w))
}

fn compare(field: Field, identity: &CalibrationIdentity, record: &ArtifactRecord) -> Option<Check> {
    let reported: Vec<String> = identity.values(field).into_iter().map(String::from).collect();
    let claims = record.claimed(field);
    if reported.is_empty() && claims.is_empty() && !ALWAYS_SHOWN.contains(&field) {
        return None;
    }
    let claimed: Vec<(String, Basis)> = claims.iter().map(|c| (c.value.clone(), c.basis)).collect();
    let label = field.label();

    let agrees_on = |basis: Basis| {
        claims
            .iter()
            .filter(|c| c.basis == basis)
            .any(|c| reported.iter().any(|r| agree(field, r, &c.value)))
    };
    let has = |basis: Basis| claims.iter().any(|c| c.basis == basis);

    let (verdict, note) = if reported.is_empty() && claims.is_empty() {
        (
            Verdict::Unknown,
            format!("{label}: not reported by the module and not stated for the file"),
        )
    } else if reported.is_empty() {
        (
            Verdict::Unknown,
            format!("{label}: not reported by the module, so the file's cannot be checked"),
        )
    } else if claims.is_empty() {
        (Verdict::Unknown, format!("{label}: not stated for the file"))
    } else if agrees_on(Basis::Declared) {
        (Verdict::Confirmed, format!("{label}: the module reports what the file is declared to be"))
    } else if has(Basis::Declared) {
        if is_description(field) && !differing_words_conflict(field) {
            (
                Verdict::Unknown,
                format!("{label}: worded differently on each side; a description is not compared as an identifier"),
            )
        } else {
            (
                Verdict::Conflict,
                format!(
                    "{label}: the module reports one thing and the file is declared to be another"
                ),
            )
        }
    } else if agrees_on(Basis::Filename) {
        (
            Verdict::NameAgrees,
            format!(
                "{label}: the file's name agrees with the module, and nothing but its name says so"
            ),
        )
    } else {
        (Verdict::NameDiffers, format!("{label}: the file's name suggests a different one"))
    };

    Some(Check { field, verdict, reported, claimed, note })
}

/// Judge one artifact against one module's identity.
pub fn evaluate(identity: &CalibrationIdentity, record: &ArtifactRecord) -> MatchReport {
    let checks: Vec<Check> =
        COMPARED.iter().filter_map(|f| compare(*f, identity, record)).collect();

    let of = |v: Verdict| -> Vec<&Check> { checks.iter().filter(|c| c.verdict == v).collect() };
    let labels = |cs: &[&Check]| cs.iter().map(|c| c.field.label()).collect::<Vec<_>>().join(", ");

    let conflicts = of(Verdict::Conflict);
    let confirmed = of(Verdict::Confirmed);
    let by_name = of(Verdict::NameAgrees);
    let calibration_confirmed = confirmed.iter().any(|c| c.field == Field::CalibrationId);

    let (status, reason) = if !conflicts.is_empty() {
        (
            MatchStatus::NoMatch,
            format!("Conflict on {}. A file that disagrees with the module on anything both state is not its calibration.", labels(&conflicts)),
        )
    } else if calibration_confirmed {
        // Said with the match, not left for somebody to notice: what the file
        // is declared for that the module never reported.
        let unchecked: Vec<&Check> = checks
            .iter()
            .filter(|c| {
                c.verdict == Verdict::Unknown && c.reported.is_empty() && !c.claimed.is_empty()
            })
            .collect();
        let not_established = if unchecked.is_empty() {
            String::new()
        } else {
            format!(
                " Not established, because the module did not report it: {}.",
                labels(&unchecked)
            )
        };
        (
            MatchStatus::ExactMatch,
            format!(
                "The module reports the calibration identification this file is declared to be, and nothing either side states disagrees. Confirmed: {}.{not_established}",
                labels(&confirmed)
            ),
        )
    } else if !confirmed.is_empty() || !by_name.is_empty() {
        let mut parts = Vec::new();
        if !confirmed.is_empty() {
            parts.push(format!("confirmed: {}", labels(&confirmed)));
        }
        if !by_name.is_empty() {
            parts.push(format!("by file name only: {}", labels(&by_name)));
        }
        (
            MatchStatus::PartialMatch,
            format!(
                "An exact match cannot be established: the calibration identification is not confirmed by anything a source declared. What agrees, {}.",
                parts.join("; ")
            ),
        )
    } else {
        (
            MatchStatus::Unknown,
            String::from("Insufficient identifiers to establish applicability: nothing the module reports could be compared with anything stated for the file."),
        )
    };

    MatchReport { status, checks, reason }
}

/// An artifact with both judgements made of it: against the module, and of
/// the file itself. Neither stands in for the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluated {
    /// The artifact.
    pub artifact: ArtifactRecord,
    /// Whether it is this module's calibration.
    pub matching: MatchReport,
    /// Whether the file is intact and consistent with itself.
    pub validation: Validation,
    /// Whether it is now in the local cache.
    pub cached: bool,
}

/// What searching one source came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReport {
    /// The source.
    pub source: SourceInfo,
    /// Whether it was searched. A source that is switched off is not.
    pub searched: bool,
    /// How many files it offered.
    pub offered: usize,
    /// Why it could not be searched, when it could not.
    pub error: Option<String>,
}

/// How a search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    /// At least one artifact is an exact or a partial match.
    ArtifactFound,
    /// No source holds an artifact that matches. A result, not a failure:
    /// most calibrations are not available to anyone outside a dealership.
    NoArtifactFound,
}

/// The result of looking for a module's calibration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// How it ended.
    pub outcome: Outcome,
    /// Exact matches first, then partial ones.
    pub matches: Vec<Evaluated>,
    /// Artifacts that were looked at and are not this module's: a conflict,
    /// or nothing to compare. Listed so that "not found" can be told from
    /// "found and ruled out".
    pub set_aside: Vec<Evaluated>,
    /// Every source, searched or not.
    pub sources: Vec<SourceReport>,
}

/// Look for a module's calibration in `sources`.
///
/// Every file offered is hashed, validated and judged the same way wherever
/// it came from. The same content from two sources is one artifact. A match,
/// exact or partial, is copied into `cache` when one is given. Never fails:
/// a source that cannot be searched is reported, and finding nothing is an
/// outcome.
pub fn resolve(
    identity: &CalibrationIdentity,
    sources: &[&dyn CalibrationSource],
    cache: Option<&Cache>,
    now: &str,
) -> Resolution {
    let mut reports = Vec::new();
    // By content, so two sightings of one file are judged once, with
    // everything both sources claim.
    let mut seen: BTreeMap<String, (ArtifactRecord, Vec<u8>)> = BTreeMap::new();

    for source in sources {
        let info = source.info();
        if !info.enabled {
            reports.push(SourceReport { source: info, searched: false, offered: 0, error: None });
            continue;
        }
        match source.search(identity, now) {
            Ok(candidates) => {
                reports.push(SourceReport {
                    source: info,
                    searched: true,
                    offered: candidates.len(),
                    error: None,
                });
                for c in candidates {
                    match seen.get_mut(&c.record.sha256) {
                        Some((known, _)) => known.absorb(&c.record),
                        None => {
                            seen.insert(c.record.sha256.clone(), (c.record, c.bytes));
                        }
                    }
                }
            }
            Err(e) => reports.push(SourceReport {
                source: info,
                searched: true,
                offered: 0,
                error: Some(e.to_string()),
            }),
        }
    }

    let mut matches = Vec::new();
    let mut set_aside = Vec::new();
    for (_, (mut record, bytes)) in seen {
        let matching = evaluate(identity, &record);
        let validation = validate(&record, Some(&bytes));
        let is_match =
            matches!(matching.status, MatchStatus::ExactMatch | MatchStatus::PartialMatch);

        let mut cached = cache.is_some_and(|c| matches!(c.get(&record.sha256), Ok(Some(_))));
        // A file that is not what its source described is not kept, however
        // well the description fits the module.
        let keepable = validation.status != crate::validate::ValidationStatus::Invalid;
        if is_match && keepable {
            if let Some(cache) = cache {
                // Kept under the hash of what was actually read, with every
                // sighting of it.
                for source in record.sources.clone() {
                    let kept = cache.add(
                        &bytes,
                        Incoming {
                            filename: record.filename.clone(),
                            source,
                            claims: record.claims.clone(),
                            declared_sha256: record.declared_sha256.clone(),
                            now: now.to_string(),
                        },
                    );
                    match kept {
                        Ok(kept) => {
                            record.absorb(&kept);
                            cached = true;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "could not keep a calibration artifact")
                        }
                    }
                }
            }
        }

        let evaluated = Evaluated { artifact: record, matching, validation, cached };
        if is_match {
            matches.push(evaluated);
        } else {
            set_aside.push(evaluated);
        }
    }
    matches.sort_by(|a, b| {
        (a.matching.status, &a.artifact.filename).cmp(&(b.matching.status, &b.artifact.filename))
    });
    set_aside.sort_by(|a, b| a.artifact.filename.cmp(&b.artifact.filename));

    Resolution {
        outcome: if matches.is_empty() { Outcome::NoArtifactFound } else { Outcome::ArtifactFound },
        matches,
        set_aside,
        sources: reports,
    }
}
