//! The situations the calibration subsystem has to get right, each as a
//! fixture: a module's identity, a folder of files, and the one answer that
//! is true.
//!
//! The vehicle is a 2023 Honda Odyssey, 3.5 L V6, and the module its PCM. The
//! identifiers are made up and say so (`TEST-...`), because what is tested is
//! the rule, and a rule that only worked on one real part number would not be
//! one.
//!
//! No manufacturer's file is in this repository. The packages here are built
//! by `aim_calibration::rwd::fixture` to the shape the format is publicly
//! described as having, around bytes that stand in for the software. That
//! tests this crate against the description. It does not test the description
//! against a real file, and nothing here claims it does.

use aim_calibration::resolve::Outcome;
use aim_calibration::rwd::fixture;
use aim_calibration::{
    resolve, validate, Basis, Cache, CalibrationIdentity, CalibrationSource, Compression, Content,
    DirectorySource, Evaluated, Field, Identified, IdentitySource, ManufacturerOrigin, MatchStatus,
    NotGiven, Resolution, SourceStatus, Unanswered, Understanding, ValidationStatus, Verdict,
};
use std::io::Write;
use std::path::Path;

const NOW: &str = "2026-10-04T12:00:00Z";
const CALIBRATION: &[u8] = b"\x00\x01TEST calibration image\xFF\xFE";

fn known(value: &str, source: IdentitySource) -> Identified {
    Identified { value: value.into(), source, evidence_ref: Some(52), raw_hex: None }
}

/// The Odyssey's PCM, with the vehicle known and whichever software
/// identifiers the fixture gives it.
fn pcm(
    hardware: Option<&str>,
    program: Option<&str>,
    calibration: Option<&str>,
) -> CalibrationIdentity {
    let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
    id.protocol = Some("ISO 15765-4 CAN 29/500".into());
    let looked_up = || IdentitySource::Lookup { name: "NHTSA vPIC".into() };
    id.record(Field::Make, known("Honda", IdentitySource::VinStructure));
    id.record(Field::Model, known("Odyssey", looked_up()));
    id.record(Field::ModelYear, known("2023", IdentitySource::VinStructure));
    id.record(Field::Engine, known("3.5 L V6", looked_up()));
    id.record(Field::ModuleName, known("PCM", IdentitySource::ObdInfoType { info_type: 0x0A }));
    if let Some(v) = hardware {
        id.record(Field::HardwareNumber, known(v, IdentitySource::UdsDid { did: 0xF191 }));
    }
    if let Some(v) = program {
        id.record(Field::ProgramId, known(v, IdentitySource::UdsDid { did: 0xF188 }));
    }
    if let Some(v) = calibration {
        id.record(Field::CalibrationId, known(v, IdentitySource::ObdInfoType { info_type: 0x04 }));
    }
    id
}

/// `TEST-CAL-001.bin` with the metadata a careful source would supply.
fn put_artifact(dir: &Path, name: &str, bytes: &[u8], metadata: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
    if !metadata.is_empty() {
        std::fs::write(dir.join(format!("{name}.json")), metadata).unwrap();
    }
}

fn full_metadata(hardware: &str) -> String {
    format!(
        r#"{{ "sha256": "{}",
              "make": "Honda", "model": "Odyssey", "model_year": [2023], "engine": "3.5L V6",
              "hardware_number": "{hardware}",
              "program_id": "TEST-PROG-001",
              "calibration_id": "TEST-CAL-001" }}"#,
        aim_calibration::sha256_hex(CALIBRATION)
    )
}

fn verdict_of(report: &aim_calibration::MatchReport, field: Field) -> Verdict {
    report.checks.iter().find(|c| c.field == field).map(|c| c.verdict).expect("the field is shown")
}

/// Fixture A. Everything the module reports is what the file is declared to
/// be, and the file is what its source said it would be.
#[test]
fn a_exact_match() {
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    let folder = DirectorySource::new("folder", dir.path());
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder], None, NOW);

    assert_eq!(found.outcome, Outcome::ArtifactFound);
    let one = &found.matches[0];
    assert_eq!(one.matching.status, MatchStatus::ExactMatch, "{}", one.matching.reason);
    assert_eq!(one.validation.status, ValidationStatus::Valid, "{:#?}", one.validation);
    for field in [
        Field::Make,
        Field::Model,
        Field::ModelYear,
        Field::Engine,
        Field::HardwareNumber,
        Field::ProgramId,
        Field::CalibrationId,
    ] {
        assert_eq!(verdict_of(&one.matching, field), Verdict::Confirmed, "{field:?}");
    }
    assert_eq!(one.artifact.sha256, aim_calibration::sha256_hex(CALIBRATION));
    // Exact, and on the person's word: the two are always read together.
    assert_eq!(one.matching.rests_on, Some(Basis::UserDeclared));
    assert_eq!(found.sources[0].status, SourceStatus::Matched);
    assert_eq!(found.sources[0].matched, 1);
}

/// Fixture B. The same file, for a different board. One conflict is enough,
/// however much else agrees.
#[test]
fn b_wrong_ecu_hardware() {
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("WRONG-HW"));
    let folder = DirectorySource::new("folder", dir.path());
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder], None, NOW);

    assert_eq!(found.outcome, Outcome::NoArtifactFound);
    assert!(found.matches.is_empty());
    let ruled_out = &found.set_aside[0];
    assert_eq!(ruled_out.matching.status, MatchStatus::NoMatch);
    assert_eq!(verdict_of(&ruled_out.matching, Field::HardwareNumber), Verdict::Conflict);
    // The calibration identification does agree, and that changes nothing.
    assert_eq!(verdict_of(&ruled_out.matching, Field::CalibrationId), Verdict::Confirmed);
    assert!(ruled_out.matching.reason.contains("Hardware number"));
}

/// Fixture C. Model, year and engine are known and agree. The module did not
/// report a calibration identification. That is partial, and never more.
#[test]
fn c_partial_identity_is_never_exact() {
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    let folder = DirectorySource::new("folder", dir.path());

    let without_calibration = pcm(Some("TEST-HW-001"), None, None);
    let found = resolve(&without_calibration, &[&folder], None, NOW);
    let one = &found.matches[0];
    assert_eq!(one.matching.status, MatchStatus::PartialMatch);
    assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::Unknown);
    assert_eq!(verdict_of(&one.matching, Field::HardwareNumber), Verdict::Confirmed);
    assert!(one.matching.reason.contains("exact match cannot be established"));

    // Nothing but the vehicle known: still partial.
    let vehicle_only = pcm(None, None, None);
    let found = resolve(&vehicle_only, &[&folder], None, NOW);
    assert_eq!(found.matches[0].matching.status, MatchStatus::PartialMatch);
}

/// A file whose only claim to being this calibration is its name. The module
/// reports the very identifier the name spells, and it is still not exact.
#[test]
fn c_a_file_name_cannot_make_a_match_exact() {
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, "");
    let folder = DirectorySource::new("folder", dir.path());
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder], None, NOW);

    let one = &found.matches[0];
    assert_eq!(one.matching.status, MatchStatus::PartialMatch);
    assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::NameAgrees);
    assert_eq!(one.artifact.claimed(Field::CalibrationId)[0].basis, Basis::Filename);
    // And nothing vouches for its content either.
    assert_eq!(one.validation.status, ValidationStatus::PartiallyValidated);

    // A name that spells something else is not a conflict, and not a match.
    let other = pcm(None, None, Some("TEST-CAL-999"));
    let mut stripped = other.clone();
    stripped.fields.retain(|f, _| *f == Field::CalibrationId);
    let found = resolve(&stripped, &[&folder], None, NOW);
    assert_eq!(found.outcome, Outcome::NoArtifactFound);
    assert_eq!(found.set_aside[0].matching.status, MatchStatus::Unknown);
    assert_eq!(
        verdict_of(&found.set_aside[0].matching, Field::CalibrationId),
        Verdict::NameDiffers
    );
}

/// Fixture D. The file is not the file its source described.
#[test]
fn d_corrupt_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let mut damaged = CALIBRATION.to_vec();
    damaged[5] ^= 0x01;
    put_artifact(dir.path(), "TEST-CAL-001.bin", &damaged, &full_metadata("TEST-HW-001"));
    let folder = DirectorySource::new("folder", dir.path());
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder], None, NOW);

    // What is declared about it still matches the module. The file itself is
    // not what was declared, and the two are reported separately.
    let one = &found.matches[0];
    assert_eq!(one.matching.status, MatchStatus::ExactMatch);
    assert_eq!(one.validation.status, ValidationStatus::Invalid);
    assert!(one
        .validation
        .checks
        .iter()
        .any(|c| c.what == "declared hash" && c.passed == Some(false)));

    // A kept file that is changed afterwards is caught the same way.
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    let kept = resolve(&identity, &[&folder], Some(&cache), NOW).matches[0].artifact.clone();
    assert!(cache.intact(&kept).unwrap());
    std::fs::write(cache.file_path(&kept), &damaged).unwrap();
    assert!(!cache.intact(&kept).unwrap());
    let reread = cache.read(&kept).unwrap();
    assert_eq!(validate(&kept, Some(&reread)).status, ValidationStatus::Invalid);
}

/// Fixture E. The same bytes from two places are one artifact.
#[test]
fn e_duplicate_artifact() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    put_artifact(first.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    // Renamed, and with nothing declared about it.
    put_artifact(second.path(), "downloaded (2).bin", CALIBRATION, "");
    let a = DirectorySource::new("folder", first.path());
    let b = DirectorySource::new("usb-stick", second.path());
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&a, &b], Some(&cache), NOW);

    assert_eq!(found.matches.len(), 1, "one logical artifact");
    let one = &found.matches[0];
    assert_eq!(one.artifact.sha256, aim_calibration::sha256_hex(CALIBRATION));
    let sources: Vec<&str> = one.artifact.sources.iter().map(|s| s.source.as_str()).collect();
    assert_eq!(sources, vec!["folder", "usb-stick"]);
    assert!(one.cached);
    assert_eq!(cache.list().unwrap().len(), 1);
    let kept: Vec<_> = std::fs::read_dir(cache_dir.path().join("artifacts")).unwrap().collect();
    assert_eq!(kept.len(), 1);

    // Found again from the cache alone, after both folders are gone.
    drop(first);
    drop(second);
    let again = resolve(&identity, &[&cache as &dyn CalibrationSource], Some(&cache), NOW);
    assert_eq!(again.matches.len(), 1);
    assert_eq!(again.matches[0].matching.status, MatchStatus::ExactMatch);
    assert_eq!(cache.list().unwrap().len(), 1);
}

/// Fixture F. A perfectly good identity and nothing to match it with. This is
/// the usual case, and it is a result.
#[test]
fn f_unknown_calibration_is_a_result_not_an_error() {
    let empty = tempfile::tempdir().unwrap();
    let folder = DirectorySource::new("folder", empty.path());
    let switched_off = DirectorySource::new("archive", empty.path()).enabled(false);
    let missing = DirectorySource::new("never-made", empty.path().join("not-here"));
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder, &switched_off, &missing], None, NOW);

    assert_eq!(found.outcome, Outcome::NoArtifactFound);
    assert!(found.matches.is_empty() && found.set_aside.is_empty());
    // And it says where it looked, and where it did not.
    let searched: Vec<(&str, bool)> =
        found.sources.iter().map(|s| (s.source.id.as_str(), s.searched)).collect();
    assert_eq!(searched, vec![("folder", true), ("archive", false), ("never-made", true)]);
    assert!(found.sources.iter().all(|s| s.error.is_none() && s.offered == 0));
    assert!(found.sources.iter().all(|s| !s.source.uses_network));
    // Searched and empty is one thing; switched off is another.
    let statuses: Vec<SourceStatus> = found.sources.iter().map(|s| s.status).collect();
    assert_eq!(
        statuses,
        vec![SourceStatus::NoMatch, SourceStatus::SwitchedOff, SourceStatus::NoMatch]
    );
    assert!(!found.incomplete);
}

/// A source that could not be searched is not a source with nothing in it,
/// and a search that could not look everywhere says so.
#[test]
fn f_a_source_that_failed_is_not_a_source_that_found_nothing() {
    struct Broken;
    impl CalibrationSource for Broken {
        fn info(&self) -> aim_calibration::SourceInfo {
            aim_calibration::SourceInfo {
                id: "broken".into(),
                name: "A source that cannot be read".into(),
                kind: aim_calibration::SourceKind::Other,
                location: "nowhere".into(),
                uses_network: false,
                enabled: true,
            }
        }
        fn search(
            &self,
            _identity: &CalibrationIdentity,
            _now: &str,
        ) -> Result<aim_calibration::Offered, aim_calibration::SourceError> {
            Err(aim_calibration::SourceError::Failed("the disk said no".into()))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    let folder = DirectorySource::new("folder", dir.path());
    let empty = tempfile::tempdir().unwrap();
    let nothing_there = DirectorySource::new("empty", empty.path());
    let identity = pcm(Some("TEST-HW-001"), Some("TEST-PROG-001"), Some("TEST-CAL-001"));

    // Failed beside an empty one: nothing found, and it is not the same nothing.
    let found = resolve(&identity, &[&Broken, &nothing_there], None, NOW);
    assert_eq!(found.outcome, Outcome::NoArtifactFound);
    assert!(found.incomplete, "a search that could not look everywhere says so");
    assert_eq!(found.sources[0].status, SourceStatus::Failed);
    assert!(!found.sources[0].searched);
    assert_eq!(found.sources[0].error.as_deref(), Some("the disk said no"));
    assert_eq!(found.sources[1].status, SourceStatus::NoMatch);
    assert!(found.sources[1].searched && found.sources[1].error.is_none());

    // A failure in one source takes nothing from what another found.
    let found = resolve(&identity, &[&Broken, &folder], None, NOW);
    assert_eq!(found.outcome, Outcome::ArtifactFound);
    assert_eq!(found.matches[0].matching.status, MatchStatus::ExactMatch);
    assert_eq!(found.sources[0].status, SourceStatus::Failed);
    assert_eq!(found.sources[1].status, SourceStatus::Matched);
    assert!(found.incomplete);
}

/// Fixture G. The module was asked for an identifier and refused. The refusal
/// is kept, with its bytes, and no value is made of it.
#[test]
fn g_missing_identifier_is_kept_as_a_refusal() {
    let mut identity = pcm(None, None, Some("TEST-CAL-001"));
    identity.unanswered.push(Unanswered {
        field: Some(Field::HardwareNumber),
        source: IdentitySource::UdsDid { did: 0xF191 },
        state: NotGiven::NotSupported,
        reason: "the module refused: request out of range".into(),
        evidence_ref: Some(1034),
        raw_hex: Some("7f2231".into()),
    });

    assert!(identity.first(Field::HardwareNumber).is_none());
    let refusal = &identity.unanswered[0];
    assert_eq!(refusal.raw_hex.as_deref(), Some("7f2231"));
    assert_eq!(refusal.evidence_ref, Some(1034));
    assert_eq!(refusal.source.describe(), "UDS ReadDataByIdentifier, identifier F191");

    // It survives being stored and read back, as it will be.
    let json = serde_json::to_string(&identity).unwrap();
    let back: CalibrationIdentity = serde_json::from_str(&json).unwrap();
    assert_eq!(back, identity);

    // And an unreported hardware number leaves the match short of nothing it
    // had: the calibration identification still decides.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-CAL-001.bin", CALIBRATION, &full_metadata("TEST-HW-001"));
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&identity, &[&folder], None, NOW);
    let one = &found.matches[0];
    assert_eq!(verdict_of(&one.matching, Field::HardwareNumber), Verdict::Unknown);
    assert_eq!(one.matching.status, MatchStatus::ExactMatch);
}

/// A metadata file with a mistake in it declares nothing, and says why. Seen
/// while trying the feature: one stray character, and a file declared to be
/// the module's calibration became an unexplained "cannot tell".
#[test]
fn a_metadata_file_that_cannot_be_read_is_reported_not_swallowed() {
    let dir = tempfile::tempdir().unwrap();
    put_artifact(
        dir.path(),
        "pcm-update.bin",
        CALIBRATION,
        r#"{ "sha256": "\e398", "calibration_id": "TEST-CAL-001" }"#,
    );
    let folder = DirectorySource::new("folder", dir.path());
    let identity = pcm(None, None, Some("TEST-CAL-001"));

    let found = resolve(&identity, &[&folder], None, NOW);

    let one = found.matches.iter().chain(&found.set_aside).next().expect("the file is evaluated");
    // Nothing from the broken file was taken as declared.
    assert!(one.artifact.claimed(Field::CalibrationId).iter().all(|c| c.basis == Basis::Filename));
    assert_ne!(one.matching.status, MatchStatus::ExactMatch);
    let problem = one.artifact.metadata_problem.as_deref().expect("the problem is kept");
    assert!(problem.contains("pcm-update.bin.json"), "{problem}");
    assert!(one
        .validation
        .checks
        .iter()
        .any(|c| c.what == "metadata file" && c.passed == Some(false)));
}

/// A make is words, not an identifier: the VIN's "Honda (US)" is the file's
/// "Honda". A different make altogether is a conflict.
#[test]
fn makes_and_models_are_compared_as_words() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = |make: &str| {
        format!(r#"{{ "make": "{make}", "model": "Odyssey", "calibration_id": "TEST-CAL-001" }}"#)
    };
    let mut identity = pcm(None, None, Some("TEST-CAL-001"));
    identity.fields.remove(&Field::Make);
    identity.record(Field::Make, known("Honda (US)", IdentitySource::VinStructure));

    put_artifact(dir.path(), "a.bin", CALIBRATION, &metadata("Honda"));
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&identity, &[&folder], None, NOW);
    assert_eq!(found.matches[0].matching.status, MatchStatus::ExactMatch);
    assert_eq!(verdict_of(&found.matches[0].matching, Field::Make), Verdict::Confirmed);

    put_artifact(dir.path(), "a.bin", CALIBRATION, &metadata("Toyota"));
    let found = resolve(&identity, &[&folder], None, NOW);
    assert_eq!(found.set_aside[0].matching.status, MatchStatus::NoMatch);
    assert_eq!(verdict_of(&found.set_aside[0].matching, Field::Make), Verdict::Conflict);
}

// ---- what is inside a file, and who says what about it ----------------------

/// Bytes standing in for a package's encoded software.
const SOFTWARE: &[u8] = &[0x9C, 0x41, 0x07, 0xE2, 0x55, 0x00, 0x13, 0x37, 0xC0, 0xDE];

/// A package whose header names two versions, in the counted layout.
fn package() -> Vec<u8> {
    fixture::z(&[&[b"TEST-MOD-A010\x00\x00", b"TEST-MOD-A020\x00\x00"], &[&[1, 2, 3]]], SOFTWARE)
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// A module that reports one calibration identification and nothing else.
fn module_running(calibration: &str) -> CalibrationIdentity {
    let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
    id.record(
        Field::CalibrationId,
        known(calibration, IdentitySource::ObdInfoType { info_type: 4 }),
    );
    id
}

fn the_one(found: &Resolution) -> &Evaluated {
    let all: Vec<&Evaluated> = found.matches.iter().chain(&found.set_aside).collect();
    assert_eq!(all.len(), 1, "one file was put there");
    all[0]
}

/// Fixture H. A package that is what its name says, with nothing declared.
/// Its header names the module's calibration, and that is as far as it goes.
#[test]
fn h_a_plain_package_is_read_as_far_as_its_header() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = package();
    put_artifact(dir.path(), "update.rwd", &bytes, "");
    let folder = DirectorySource::new("folder", dir.path());

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], None, NOW);
    let one = the_one(&found);

    // The file: a package by name and by content.
    assert_eq!(
        one.validation.status,
        ValidationStatus::PartiallyValidated,
        "{:#?}",
        one.validation
    );
    assert_eq!(one.inspection.compression, None);
    assert_eq!(one.inspection.content, Content::Rwd);
    let header = one.inspection.rwd.as_ref().expect("its header was read");
    assert!(header.headers_read);
    assert_eq!(header.texts(), vec!["TEST-MOD-A010", "TEST-MOD-A020"]);
    // The software after the header is bytes and is called that.
    assert_eq!(one.inspection.software, Understanding::Opaque);

    // The match: the module's identifier is in the header, so partial. Not
    // exact, because nobody knows what the header means by naming it.
    assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::InFileHeader);
    assert_eq!(one.matching.status, MatchStatus::PartialMatch);
    assert_eq!(one.matching.rests_on, Some(Basis::FileHeader));
    assert!(one.matching.warnings.iter().any(|w| w.contains("different version")));
    let check = one.matching.checks.iter().find(|c| c.field == Field::CalibrationId).unwrap();
    assert!(check.claimed.contains(&("TEST-MOD-A010".to_string(), Basis::FileHeader)));
    assert_eq!(one.origin.manufacturer, ManufacturerOrigin::NotEstablished);
}

/// Fixture I. The same package, packed. Two layers, two hashes, one file.
#[test]
fn i_a_packed_package_is_gzip_holding_a_package_not_just_gzip() {
    let dir = tempfile::tempdir().unwrap();
    let inner = package();
    let packed = gz(&inner);
    put_artifact(dir.path(), "TEST-MOD-A010.rwd.gz", &packed, "");
    let folder = DirectorySource::new("folder", dir.path());
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], Some(&cache), NOW);
    let one = the_one(&found);

    // What the name says, as two facts.
    assert_eq!(one.artifact.compression, Some(Compression::Gzip));
    assert_eq!(one.artifact.format, aim_calibration::ArtifactFormat::Rwd);
    // What the content is, as two facts.
    assert_eq!(one.inspection.compression, Some(Compression::Gzip));
    assert_eq!(one.inspection.content, Content::Rwd);
    assert!(one.inspection.rwd.as_ref().is_some_and(|h| h.headers_read));
    // The artifact is the file as it arrived. What is inside has its own hash.
    assert_eq!(one.artifact.sha256, aim_calibration::sha256_hex(&packed));
    assert_eq!(
        one.inspection.payload_sha256.as_deref(),
        Some(aim_calibration::sha256_hex(&inner).as_str())
    );
    assert_eq!(one.inspection.payload_size, Some(inner.len() as u64));
    assert_eq!(
        one.validation.status,
        ValidationStatus::PartiallyValidated,
        "{:#?}",
        one.validation
    );
    assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::InFileHeader);

    // Kept packed, exactly as it arrived.
    assert!(one.cached);
    let kept = cache.file_path(&one.artifact);
    assert!(kept.to_string_lossy().ends_with(".rwd.gz"));
    assert_eq!(std::fs::read(kept).unwrap(), packed);
}

/// Fixture J. A gzip that does not open. Nothing is said about what is in it.
#[test]
fn j_a_gzip_that_does_not_open_is_invalid_and_nothing_inside_is_claimed() {
    let dir = tempfile::tempdir().unwrap();
    let mut packed = gz(&package());
    packed.truncate(packed.len() - 9);
    put_artifact(dir.path(), "TEST-MOD-A010.rwd.gz", &packed, "");
    let folder = DirectorySource::new("folder", dir.path());
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], Some(&cache), NOW);
    let one = the_one(&found);

    assert_eq!(one.validation.status, ValidationStatus::Invalid);
    assert!(one
        .validation
        .checks
        .iter()
        .any(|c| c.what == "packing" && c.passed == Some(false) && c.detail.contains("unpack")));
    assert_eq!(one.inspection.software, Understanding::NotReached);
    assert!(one.inspection.rwd.is_none() && one.inspection.payload_sha256.is_none());
    // All that is left is its name, and a name is a partial match at most.
    assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::NameAgrees);
    assert_eq!(one.matching.status, MatchStatus::PartialMatch);
    // And a file that is not what it says is not kept.
    assert!(!one.cached);
    assert!(cache.list().unwrap().is_empty());
}

/// Fixture K. A gzip that opens, named as a package, holding something else.
#[test]
fn k_a_gzip_holding_something_that_is_not_a_package_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let packed = gz(b"meeting notes, mostly about lunch");
    put_artifact(dir.path(), "TEST-MOD-A010.rwd.gz", &packed, "");
    let folder = DirectorySource::new("folder", dir.path());

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], None, NOW);
    let one = the_one(&found);

    assert_eq!(one.inspection.compression, Some(Compression::Gzip));
    assert_eq!(one.inspection.content, Content::Opaque);
    assert!(one.inspection.rwd.is_none());
    assert_eq!(one.validation.status, ValidationStatus::Invalid);
    // The packing is fine. It is what is inside that is not what the name says.
    assert!(one.validation.checks.iter().any(|c| c.what == "packing" && c.passed == Some(true)));
    assert!(one
        .validation
        .checks
        .iter()
        .any(|c| c.what == "content against name" && c.passed == Some(false)));
}

/// Fixture L. A header whose identifiers are known. Each is found when the
/// module reports it, in either described layout, and one the header does not
/// hold is not found: the header is searched, never interpreted.
#[test]
fn l_a_header_is_searched_for_what_the_module_reports() {
    let delimited =
        fixture::one(&[(b'$', &["TEST-MOD-A010", "TEST-MOD-A020"]), (b'&', &["0a1b2c"])], SOFTWARE);
    for bytes in [package(), delimited] {
        let dir = tempfile::tempdir().unwrap();
        put_artifact(dir.path(), "update.rwd", &bytes, "");
        let folder = DirectorySource::new("folder", dir.path());

        for reported in ["TEST-MOD-A010", "test-mod-a020 "] {
            let found = resolve(&module_running(reported), &[&folder], None, NOW);
            assert_eq!(
                verdict_of(&the_one(&found).matching, Field::CalibrationId),
                Verdict::InFileHeader,
                "{reported}"
            );
        }

        // Not in the header, and the name says nothing: cannot tell. Not a
        // conflict, because a header that names other versions has not said
        // "not this one".
        let found = resolve(&module_running("TEST-MOD-A999"), &[&folder], None, NOW);
        let one = the_one(&found);
        assert_eq!(found.outcome, Outcome::NoArtifactFound);
        assert_eq!(one.matching.status, MatchStatus::Unknown);
        assert_eq!(verdict_of(&one.matching, Field::CalibrationId), Verdict::NameDiffers);
        // The group described as holding the encoding key is never read out.
        let shown = serde_json::to_string(&one.inspection).unwrap();
        assert!(!shown.contains("0a1b2c"), "{shown}");
    }
}

/// Fixture M. A name that says one thing about a file that is another.
#[test]
fn m_a_misleading_name_gets_a_file_nowhere() {
    let identity = module_running("TEST-MOD-A010");

    // Named as this module's package, and not a package at all.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "TEST-MOD-A010.rwd", b"holiday photos", "");
    let folder = DirectorySource::new("folder", dir.path());
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();
    let found = resolve(&identity, &[&folder], Some(&cache), NOW);
    let one = the_one(&found);
    assert_eq!(one.validation.status, ValidationStatus::Invalid);
    assert_eq!(
        one.matching.status,
        MatchStatus::PartialMatch,
        "its name agrees, and only its name"
    );
    assert_eq!(one.matching.rests_on, Some(Basis::Filename));
    assert!(!one.cached && cache.list().unwrap().is_empty());

    // Named as this module's calibration, and declared to be another one. The
    // declaration decides, and the name's disagreement is said, not dropped.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(
        dir.path(),
        "TEST-MOD-A010.bin",
        CALIBRATION,
        r#"{ "calibration_id": "TEST-MOD-A999" }"#,
    );
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&identity, &[&folder], None, NOW);
    let one = the_one(&found);
    assert_eq!(one.matching.status, MatchStatus::NoMatch);
    let check = one.matching.checks.iter().find(|c| c.field == Field::CalibrationId).unwrap();
    assert_eq!(check.verdict, Verdict::Conflict);
    assert!(check.note.contains("name agrees"), "{}", check.note);
    assert!(check.claimed.contains(&("TEST-MOD-A999".to_string(), Basis::UserDeclared)));
    assert!(check.claimed.contains(&("TEST-MOD-A010".to_string(), Basis::Filename)));

    // The other way round: named as another calibration, declared to be this
    // one. Exact on the declaration, and the name is still mentioned.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(
        dir.path(),
        "TEST-MOD-A999.bin",
        CALIBRATION,
        r#"{ "calibration_id": "TEST-MOD-A010" }"#,
    );
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&identity, &[&folder], None, NOW);
    let one = the_one(&found);
    assert_eq!(one.matching.status, MatchStatus::ExactMatch);
    let check = one.matching.checks.iter().find(|c| c.field == Field::CalibrationId).unwrap();
    assert!(check.note.contains("name suggests TEST-MOD-A999"), "{}", check.note);
}

/// Fixture N. What a person declared about a file against what is in the
/// file. The declaration is not simply believed and the header is not simply
/// believed: the two are shown, and the file is not judged.
#[test]
fn n_a_declaration_that_the_file_itself_puts_in_doubt_is_conflicting_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = package();
    // The file's name and its header both say A010. The note beside it says A999.
    put_artifact(
        dir.path(),
        "TEST-MOD-A010.rwd",
        &bytes,
        r#"{ "calibration_id": "TEST-MOD-A999" }"#,
    );
    let folder = DirectorySource::new("folder", dir.path());
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], Some(&cache), NOW);

    assert_eq!(found.outcome, Outcome::ConflictingEvidence);
    assert!(found.matches.is_empty());
    let one = &found.set_aside[0];
    assert_eq!(one.matching.status, MatchStatus::ConflictingEvidence);
    assert_eq!(one.matching.rests_on, None);
    let check = one.matching.checks.iter().find(|c| c.field == Field::CalibrationId).unwrap();
    assert_eq!(check.verdict, Verdict::ConflictingEvidence);
    // Every piece of evidence is there, each with whose it is.
    for expected in [
        ("TEST-MOD-A999", Basis::UserDeclared),
        ("TEST-MOD-A010", Basis::Filename),
        ("TEST-MOD-A010", Basis::FileHeader),
    ] {
        assert!(check.claimed.contains(&(expected.0.to_string(), expected.1)), "{expected:?}");
    }
    assert!(one.matching.reason.contains("TEST-MOD-A999"), "{}", one.matching.reason);
    assert!(one.matching.reason.contains("its own header"), "{}", one.matching.reason);
    // A file nobody can vouch for either way is not kept.
    assert!(!one.cached && cache.list().unwrap().is_empty());

    // Two declarations of two calibrations. One of them is the module's, and
    // that used to be enough for "exact". It is not: the file is one thing.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(
        dir.path(),
        "pcm.bin",
        CALIBRATION,
        r#"{ "calibration_id": ["TEST-MOD-A010", "TEST-MOD-A999"] }"#,
    );
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], None, NOW);
    assert_eq!(found.outcome, Outcome::ConflictingEvidence);
    let one = &found.set_aside[0];
    assert_eq!(one.matching.status, MatchStatus::ConflictingEvidence);
    assert_eq!(one.validation.status, ValidationStatus::Invalid);

    // A conflict with the module that nothing in the file contradicts is
    // still a plain no.
    let dir = tempfile::tempdir().unwrap();
    put_artifact(dir.path(), "pcm.bin", CALIBRATION, r#"{ "calibration_id": "TEST-MOD-A999" }"#);
    let folder = DirectorySource::new("folder", dir.path());
    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], None, NOW);
    assert_eq!(found.outcome, Outcome::NoArtifactFound);
    assert_eq!(found.set_aside[0].matching.status, MatchStatus::NoMatch);
}

/// Fixture O. The calibration identification matches, and who made the file
/// is not established. Both are true at once and neither hides the other.
#[test]
fn o_an_exact_match_says_nothing_about_who_made_the_file() {
    let dir = tempfile::tempdir().unwrap();
    // Everything a person could type to make a file look official.
    let metadata = format!(
        r#"{{ "sha256": "{}", "calibration_id": "TEST-MOD-A010",
              "manufacturer": "Honda", "supplier": "Honda",
              "manufacturer_origin": "ESTABLISHED", "oem": true, "verified": true,
              "authentic": "yes", "source": "Honda official" }}"#,
        aim_calibration::sha256_hex(CALIBRATION)
    );
    put_artifact(dir.path(), "pcm.bin", CALIBRATION, &metadata);
    let folder = DirectorySource::new("folder", dir.path());

    let found = resolve(&module_running("TEST-MOD-A010"), &[&folder], None, NOW);
    let one = the_one(&found);

    // The match is as strong as a match gets here.
    assert_eq!(one.matching.status, MatchStatus::ExactMatch);
    assert_eq!(one.validation.status, ValidationStatus::Valid);
    // And it is the person's declaration, said in the data and in words.
    assert_eq!(one.matching.rests_on, Some(Basis::UserDeclared));
    assert!(one
        .matching
        .warnings
        .iter()
        .any(|w| w.contains("who made the file is not established")));
    // Who made it: not established, whatever the note beside it says.
    assert_eq!(one.origin.manufacturer, ManufacturerOrigin::NotEstablished);
    let shown = serde_json::to_value(one).unwrap();
    assert_eq!(shown["origin"]["manufacturer"], "NOT_ESTABLISHED");
    assert_eq!(shown["matching"]["rests_on"], "user_declared");
    assert_eq!(shown["inspection"]["software"], "PAYLOAD_OPAQUE");
    // There is no value of that answer that says otherwise for a note to reach.
    assert!(serde_json::from_value::<ManufacturerOrigin>(serde_json::json!("ESTABLISHED")).is_err());
    assert!(
        serde_json::from_value::<ManufacturerOrigin>(serde_json::json!("OEM_VERIFIED")).is_err()
    );
}
