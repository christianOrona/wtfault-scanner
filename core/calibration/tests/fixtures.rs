//! The seven situations the calibration subsystem has to get right, each as
//! a fixture: a module's identity, a folder of files, and the one answer that
//! is true.
//!
//! The vehicle is a 2023 Honda Odyssey, 3.5 L V6, and the module its PCM. The
//! identifiers are made up and say so (`TEST-...`), because what is tested is
//! the rule, and a rule that only worked on one real part number would not be
//! one.

use aim_calibration::resolve::Outcome;
use aim_calibration::{
    resolve, validate, Basis, Cache, CalibrationIdentity, CalibrationSource, DirectorySource,
    Field, Identified, IdentitySource, MatchStatus, Unanswered, ValidationStatus, Verdict,
};
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
}

/// Fixture G. The module was asked for an identifier and refused. The refusal
/// is kept, with its bytes, and no value is made of it.
#[test]
fn g_missing_identifier_is_kept_as_a_refusal() {
    let mut identity = pcm(None, None, Some("TEST-CAL-001"));
    identity.unanswered.push(Unanswered {
        field: Some(Field::HardwareNumber),
        source: IdentitySource::UdsDid { did: 0xF191 },
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
