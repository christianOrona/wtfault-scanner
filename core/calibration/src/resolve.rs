//! Whether a file is a module's calibration.
//!
//! Five answers and no percentage. Every answer is a list of comparisons
//! anyone can read, and the status is a rule applied to that list:
//!
//! - **No match** when a declaration about the file disagrees with the
//!   module and nothing in the file itself says otherwise.
//! - **Conflicting evidence** when what is said about the file disagrees with
//!   itself: two declarations name two calibrations, or a declaration says one
//!   thing while the file's own header names what the module reports.
//! - **Exact** when the module's own calibration identification equals one
//!   declared for the file, and nothing disagrees.
//! - **Partial** when something agrees and the calibration identification is
//!   not confirmed by a declaration.
//! - **Unknown** when nothing could be compared at all.
//!
//! # What outranks what
//!
//! Stated once, here, and applied the same way to every file: a declaration,
//! by the person or by the file's source, outranks the file's name. The
//! declaration decides and the name is still shown. Two declarations that
//! disagree are not ranked against each other, whoever made them: that is
//! conflicting evidence.
//!
//! The file's own header stands apart. It can *agree* with the module: the
//! identifier the module reports is written in it. It cannot be made to
//! disagree, because the public description of the format does not say what a
//! header value means, and a header that lists other identifiers is not
//! thereby a header that says "not this one". So a header can corroborate, it
//! can put a declaration in doubt, and it can never by itself make a match
//! exact.
//!
//! # What a match is not
//!
//! A match says a file is *described as*, or *names*, the calibration a module
//! reports. It does not say who made the file. That is a separate answer,
//! [`ManufacturerOrigin`], and this build has one value for it.

use crate::artifact::{ArtifactRecord, Basis};
use crate::cache::{Cache, Incoming};
use crate::format::{inspect, Inspection};
use crate::identity::{same_identifier, CalibrationIdentity, Field};
use crate::source::{CalibrationSource, SourceError, SourceInfo};
use crate::validate::{validate_inspected, Validation, ValidationStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The module reports it and it is declared, by the person or by the
    /// file's source, to be the same for the file.
    Confirmed,
    /// It is declared for the file and the module reports something else.
    Conflict,
    /// What is said about the file disagrees with itself, so the comparison
    /// with the module is not made.
    ConflictingEvidence,
    /// The module's identifier is written in the file's own header. What the
    /// header means by naming it is not established.
    InFileHeader,
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
    /// What is said of the file, each with its basis: declared, read from its
    /// name, or found in its own header.
    pub claimed: Vec<(String, Basis)>,
    /// The comparison in words, including any weaker evidence that points the
    /// other way.
    pub note: String,
}

/// The five answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchStatus {
    /// The file is declared to be the calibration the module reports.
    ExactMatch,
    /// Something agrees; the calibration itself is not established.
    PartialMatch,
    /// What is said about the file disagrees with itself. Not judged.
    ConflictingEvidence,
    /// Nothing could be compared.
    Unknown,
    /// A declaration about the file disagrees with the module.
    NoMatch,
}

/// A file judged against a module, with everything the judgement rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchReport {
    /// The answer.
    pub status: MatchStatus,
    /// The strongest thing that ties the file to the module's calibration
    /// identification, when anything does. An exact match always has one, and
    /// it is always a declaration: read the status with it, never without.
    pub rests_on: Option<Basis>,
    /// Every comparison made, decisive ones last.
    pub checks: Vec<Check>,
    /// Why the answer is what it is, in one or two sentences.
    pub reason: String,
    /// What the answer leaves open, said rather than left to be noticed.
    pub warnings: Vec<String>,
}

/// Whether a file is established to have come from the vehicle's manufacturer.
///
/// One value, because this build can establish nothing more. A matching
/// calibration identification, a matching verification number, a hash, a
/// folder, a file's name and anybody's say-so are all evidence about *which
/// calibration* a file is. None of them is evidence about *who made it*: that
/// would take a signature this crate can check, and it has none. A second
/// value is added the day something can earn it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManufacturerOrigin {
    /// Nothing here establishes who made the file.
    #[serde(rename = "NOT_ESTABLISHED")]
    NotEstablished,
}

/// Who made a file, as far as this build can say, and why no further.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Whether the manufacturer is established as its maker.
    pub manufacturer: ManufacturerOrigin,
    /// Why not, for this file.
    pub reason: String,
}

fn origin_of(record: &ArtifactRecord) -> Origin {
    use crate::artifact::SourceKind;
    let from_a_tool = record.sources.iter().any(|s| s.kind == SourceKind::ToolInstallation);
    Origin {
        manufacturer: ManufacturerOrigin::NotEstablished,
        reason: if from_a_tool {
            String::from(
                "It was found in a manufacturer's service tool folder on this computer. That is \
                 where it was, not proof of who made it: nothing checks a signature, and a file \
                 can be put in any folder.",
            )
        } else {
            String::from(
                "Nothing checks a manufacturer's signature on it. A matching identifier, a \
                 matching hash and a declaration all say which calibration a file is described \
                 as, not who made it.",
            )
        },
    }
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

/// The identifiers a file's header is searched for. Not the verification
/// number: it is a checksum the module computes, not something a package is
/// described as carrying.
const SOUGHT_IN_HEADER: [Field; 8] = [
    Field::HardwareNumber,
    Field::PartNumber,
    Field::SoftwareNumber,
    Field::ProgramId,
    Field::StrategyId,
    Field::RomId,
    Field::BootSoftwareId,
    Field::CalibrationId,
];

/// What a file can only be one of. Hardware may legitimately be several: one
/// calibration, several boards. A file is one calibration.
pub(crate) const ONE_PER_FILE: [Field; 3] = [Field::CalibrationId, Field::ProgramId, Field::RomId];

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

fn compare(
    field: Field,
    identity: &CalibrationIdentity,
    record: &ArtifactRecord,
    header_texts: &[&str],
) -> Option<Check> {
    let reported: Vec<String> = identity.values(field).into_iter().map(String::from).collect();
    let claims = record.claimed(field);
    // The module's own identifier, found written in the file's header.
    let in_header: Vec<&str> = if SOUGHT_IN_HEADER.contains(&field) {
        header_texts
            .iter()
            .copied()
            .filter(|t| reported.iter().any(|r| same_identifier(r, t)))
            .collect()
    } else {
        Vec::new()
    };
    if reported.is_empty() && claims.is_empty() && !ALWAYS_SHOWN.contains(&field) {
        return None;
    }
    let mut claimed: Vec<(String, Basis)> =
        claims.iter().map(|c| (c.value.clone(), c.basis)).collect();
    claimed.extend(in_header.iter().map(|t| (t.to_string(), Basis::FileHeader)));
    let label = field.label();

    let declared: Vec<&crate::artifact::Claim> =
        claims.iter().filter(|c| c.basis.is_declared()).collect();
    let declared_agrees =
        declared.iter().any(|c| reported.iter().any(|r| agree(field, r, &c.value)));
    let name_claims: Vec<&crate::artifact::Claim> =
        claims.iter().filter(|c| c.basis == Basis::Filename).collect();
    let name_agrees =
        name_claims.iter().any(|c| reported.iter().any(|r| agree(field, r, &c.value)));
    // Two declarations of the one thing a file can only be one of.
    let contradictory = ONE_PER_FILE.contains(&field)
        && declared.iter().any(|a| declared.iter().any(|b| !same_identifier(&a.value, &b.value)));
    let each = |cs: &[&crate::artifact::Claim]| {
        cs.iter()
            .map(|c| format!("{} ({})", c.value, c.basis.in_words()))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let (verdict, note) = if contradictory {
        (
            Verdict::ConflictingEvidence,
            format!(
                "{label}: the file is declared to be more than one, {}. A file is one, so \
                 neither is taken",
                each(&declared)
            ),
        )
    } else if reported.is_empty() && claims.is_empty() {
        (
            Verdict::Unknown,
            format!("{label}: not reported by the module and not stated for the file"),
        )
    } else if reported.is_empty() {
        (
            Verdict::Unknown,
            format!("{label}: not reported by the module, so the file's cannot be checked"),
        )
    } else if declared_agrees {
        let mut note = format!("{label}: the module reports what the file is declared to be");
        if !name_claims.is_empty() && !name_agrees {
            note.push_str(&format!(
                ". The file's name suggests {}; a declaration outranks a name",
                each(&name_claims)
            ));
        }
        (Verdict::Confirmed, note)
    } else if !declared.is_empty() {
        if is_description(field) && !differing_words_conflict(field) {
            (
                Verdict::Unknown,
                format!("{label}: worded differently on each side; a description is not compared as an identifier"),
            )
        } else if !in_header.is_empty() {
            (
                Verdict::ConflictingEvidence,
                format!(
                    "{label}: the file is declared to be {}, and its own header names {}, which \
                     is what the module reports. Both cannot be read as agreeing, and which is \
                     right is not established",
                    each(&declared),
                    in_header.join(", ")
                ),
            )
        } else {
            let mut note = format!(
                "{label}: the module reports one thing and the file is declared to be another"
            );
            if name_agrees {
                note.push_str(
                    ". The file's name agrees with the module; a declaration outranks a name",
                );
            }
            (Verdict::Conflict, note)
        }
    } else if !in_header.is_empty() {
        (
            Verdict::InFileHeader,
            format!(
                "{label}: the module's is written in the file's own header. Whether the header \
                 names what the file contains or what it replaces is not described anywhere \
                 public"
            ),
        )
    } else if name_agrees {
        (
            Verdict::NameAgrees,
            format!(
                "{label}: the file's name agrees with the module, and nothing but its name says so"
            ),
        )
    } else if !name_claims.is_empty() {
        (Verdict::NameDiffers, format!("{label}: the file's name suggests a different one"))
    } else {
        (Verdict::Unknown, format!("{label}: not stated for the file"))
    };

    Some(Check { field, verdict, reported, claimed, note })
}

/// Judge one artifact against one module's identity.
///
/// `inspection` is what looking inside the file found, when it was looked
/// into. Without it only what is declared and the file's name are compared.
pub fn evaluate(
    identity: &CalibrationIdentity,
    record: &ArtifactRecord,
    inspection: Option<&Inspection>,
) -> MatchReport {
    let header_texts: Vec<&str> = inspection.map(Inspection::header_texts).unwrap_or_default();
    let checks: Vec<Check> =
        COMPARED.iter().filter_map(|f| compare(*f, identity, record, &header_texts)).collect();

    let of = |v: Verdict| -> Vec<&Check> { checks.iter().filter(|c| c.verdict == v).collect() };
    let labels = |cs: &[&Check]| cs.iter().map(|c| c.field.label()).collect::<Vec<_>>().join(", ");

    let conflicts = of(Verdict::Conflict);
    let in_doubt = of(Verdict::ConflictingEvidence);
    let confirmed = of(Verdict::Confirmed);
    let in_header = of(Verdict::InFileHeader);
    let by_name = of(Verdict::NameAgrees);
    let calibration = checks.iter().find(|c| c.field == Field::CalibrationId);
    let calibration_confirmed = calibration.is_some_and(|c| c.verdict == Verdict::Confirmed);

    // What ties the file to the module's calibration identification, by the
    // strongest thing that does.
    let rests_on = calibration.and_then(|c| match c.verdict {
        Verdict::Confirmed => record
            .claimed(Field::CalibrationId)
            .iter()
            .filter(|claim| {
                claim.basis.is_declared()
                    && c.reported.iter().any(|r| same_identifier(r, &claim.value))
            })
            .map(|claim| claim.basis)
            .max(),
        Verdict::InFileHeader => Some(Basis::FileHeader),
        Verdict::NameAgrees => Some(Basis::Filename),
        _ => None,
    });

    let mut warnings = Vec::new();
    let (status, reason) = if !conflicts.is_empty() {
        (
            MatchStatus::NoMatch,
            format!("Conflict on {}. A file declared to be something the module is not, is not its calibration.", labels(&conflicts)),
        )
    } else if !in_doubt.is_empty() {
        (
            MatchStatus::ConflictingEvidence,
            format!(
                "What is said about this file disagrees with itself on {}, so it is not judged \
                 either way. {}",
                labels(&in_doubt),
                in_doubt.iter().map(|c| c.note.as_str()).collect::<Vec<_>>().join(". ")
            ),
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
        let whose = rests_on.map(Basis::in_words).unwrap_or("a declaration");
        warnings.push(format!(
            "This match rests on {whose}. It says what the file is described as. The software \
             inside the file was not read, and who made the file is not established."
        ));
        (
            MatchStatus::ExactMatch,
            format!(
                "The module reports the calibration identification this file is declared to be, and nothing either side states disagrees. Confirmed: {}.{not_established}",
                labels(&confirmed)
            ),
        )
    } else if !confirmed.is_empty() || !in_header.is_empty() || !by_name.is_empty() {
        let mut parts = Vec::new();
        if !confirmed.is_empty() {
            parts.push(format!("confirmed: {}", labels(&confirmed)));
        }
        if !in_header.is_empty() {
            parts.push(format!("written in the file's own header: {}", labels(&in_header)));
            warnings.push(String::from(
                "The file's header names an identifier this module reports. A package may name \
                 the software it contains or the software it is meant to replace, and no public \
                 description says which, so this file may be a different version for the same \
                 module.",
            ));
        }
        if !by_name.is_empty() {
            parts.push(format!("by file name only: {}", labels(&by_name)));
        }
        (
            MatchStatus::PartialMatch,
            format!(
                "An exact match cannot be established: the calibration identification is not confirmed by anything declared for the file. What agrees, {}.",
                parts.join("; ")
            ),
        )
    } else {
        (
            MatchStatus::Unknown,
            String::from("Insufficient identifiers to establish applicability: nothing the module reports could be compared with anything stated for the file."),
        )
    };

    MatchReport { status, rests_on, checks, reason, warnings }
}

/// An artifact with every judgement made of it, each kept apart: against the
/// module, of the file itself, what is inside it, and who made it. None
/// stands in for another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluated {
    /// The artifact.
    pub artifact: ArtifactRecord,
    /// Whether it is this module's calibration.
    pub matching: MatchReport,
    /// Whether the file is intact and consistent with itself.
    pub validation: Validation,
    /// What looking inside it found: its packing, and its header.
    pub inspection: Inspection,
    /// Who made it, as far as this build can say.
    pub origin: Origin,
    /// Whether it is now in the local cache.
    pub cached: bool,
}

/// How searching one source ended. Each is a different thing to be told:
/// "it holds nothing for this module" is not "it could not be searched".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// Searched, and at least one file it offered is a match.
    Matched,
    /// Searched, and nothing it holds is a match. It may hold nothing at all.
    NoMatch,
    /// It was there and looking through it failed.
    Failed,
    /// It has somewhere to look and that place is not there.
    Unavailable,
    /// Nobody has said where it should look.
    NotConfigured,
    /// It is switched off, and was not searched.
    SwitchedOff,
}

/// What searching one source came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReport {
    /// The source.
    pub source: SourceInfo,
    /// How it ended.
    pub status: SourceStatus,
    /// Whether it was looked through. False when it is switched off, has
    /// nowhere to look, or is not there.
    pub searched: bool,
    /// How many files it offered for judging.
    pub offered: usize,
    /// How many of those are a match, exact or partial.
    pub matched: usize,
    /// How many files it holds and did not offer.
    pub passed_over: usize,
    /// What it did, in words, when that needs saying.
    pub note: Option<String>,
    /// Why it could not be searched, when it could not.
    pub error: Option<String>,
}

/// How a search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    /// At least one artifact is an exact or a partial match.
    ArtifactFound,
    /// No artifact is a match, and at least one could not be judged because
    /// what is said about it disagrees with itself.
    ConflictingEvidence,
    /// No source holds an artifact that matches. A result, not a failure:
    /// most calibrations are not available to anyone outside a dealership.
    /// Whether every source could be searched is said per source.
    NoArtifactFound,
}

/// The result of looking for a module's calibration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// How it ended.
    pub outcome: Outcome,
    /// Exact matches first, then partial ones.
    pub matches: Vec<Evaluated>,
    /// Artifacts that were looked at and are not established as this
    /// module's: a conflict, evidence that disagrees with itself, or nothing
    /// to compare. Listed so that "not found" can be told from "found and
    /// ruled out". Those in doubt come first.
    pub set_aside: Vec<Evaluated>,
    /// Every source, searched or not.
    pub sources: Vec<SourceReport>,
    /// True when a source that should have been searched could not be. A
    /// search that found nothing while this is true has not shown there is
    /// nothing to find.
    pub incomplete: bool,
}

/// Look for a module's calibration in `sources`.
///
/// Every file offered is hashed, looked into, validated and judged the same
/// way wherever it came from. The same content from two sources is one
/// artifact. A match, exact or partial, is copied into `cache` when one is
/// given. Never fails: a source that cannot be searched is reported, and
/// finding nothing is an outcome.
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
        let mut report = SourceReport {
            source: info.clone(),
            status: SourceStatus::NoMatch,
            searched: false,
            offered: 0,
            matched: 0,
            passed_over: 0,
            note: None,
            error: None,
        };
        if !info.enabled {
            report.status = SourceStatus::SwitchedOff;
            reports.push(report);
            continue;
        }
        match source.search(identity, now) {
            Ok(offered) => {
                report.searched = true;
                report.offered = offered.candidates.len();
                report.passed_over = offered.passed_over;
                report.note = offered.note;
                for c in offered.candidates {
                    match seen.get_mut(&c.record.sha256) {
                        Some((known, _)) => known.absorb(&c.record),
                        None => {
                            seen.insert(c.record.sha256.clone(), (c.record, c.bytes));
                        }
                    }
                }
            }
            Err(SourceError::NotConfigured(why)) => {
                report.status = SourceStatus::NotConfigured;
                report.error = Some(why);
            }
            Err(SourceError::Unavailable(why)) => {
                report.status = SourceStatus::Unavailable;
                report.error = Some(why);
            }
            Err(SourceError::Failed(why)) => {
                report.status = SourceStatus::Failed;
                report.error = Some(why);
            }
        }
        reports.push(report);
    }

    let mut matches = Vec::new();
    let mut set_aside = Vec::new();
    for (_, (mut record, bytes)) in seen {
        let inspection = inspect(&bytes);
        let matching = evaluate(identity, &record, Some(&inspection));
        let validation = validate_inspected(&record, Some(&bytes), &inspection);
        let is_match =
            matches!(matching.status, MatchStatus::ExactMatch | MatchStatus::PartialMatch);

        let mut cached = cache.is_some_and(|c| matches!(c.get(&record.sha256), Ok(Some(_))));
        // A file that is not what its source described is not kept, however
        // well the description fits the module.
        let keepable = validation.status != ValidationStatus::Invalid;
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

        if is_match {
            for report in &mut reports {
                if record.sources.iter().any(|s| s.source == report.source.id) {
                    report.matched += 1;
                    report.status = SourceStatus::Matched;
                }
            }
        }
        let origin = origin_of(&record);
        let evaluated =
            Evaluated { artifact: record, matching, validation, inspection, origin, cached };
        if is_match {
            matches.push(evaluated);
        } else {
            set_aside.push(evaluated);
        }
    }
    matches.sort_by(|a, b| {
        (a.matching.status, &a.artifact.filename).cmp(&(b.matching.status, &b.artifact.filename))
    });
    set_aside.sort_by(|a, b| {
        (a.matching.status, &a.artifact.filename).cmp(&(b.matching.status, &b.artifact.filename))
    });

    let in_doubt = set_aside.iter().any(|e| e.matching.status == MatchStatus::ConflictingEvidence);
    Resolution {
        outcome: if !matches.is_empty() {
            Outcome::ArtifactFound
        } else if in_doubt {
            Outcome::ConflictingEvidence
        } else {
            Outcome::NoArtifactFound
        },
        matches,
        set_aside,
        incomplete: reports.iter().any(|r| r.status == SourceStatus::Failed),
        sources: reports,
    }
}
