//! Which software a module runs, read from a vehicle, and whether a file is
//! that software (the acceptance test for calibration discovery).
//!
//! The vehicle is the simulated 2023 Honda Odyssey, which reports what the
//! real one reported on 2026-10-04: an engine controller at `18DAF110` with
//! calibration `37805-5MR-C120`, a transmission controller at `18DAF11E` with
//! `28102-5MX-A200`, and every standard identification identifier refused.
//! Its VIN is the simulator's, with a zeroed serial; no real vehicle's VIN is
//! needed for any of this, and none is in it.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_calibration::resolve::Outcome;
use aim_calibration::{
    resolve, Availability, Basis, Cache, CalibrationIdentity, DirectorySource, Field, Found,
    IdentitySource, Library, ManufacturerOrigin, MatchStatus, NotGiven, SourceStatus,
    ValidationStatus,
};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{
    AdapterPersonality, ReplayMode, ReplayTransport, ScenarioId, SimulatedTransport, Transcript,
    VirtualVehicle, SIMULATED_HONDA_VIN,
};
use aim_types::EventKind;
use std::sync::Arc;

const USER: &str = "user:test";
const ENGINE: &str = "ECU_18DAF110";
const TRANSMISSION: &str = "ECU_18DAF11E";

fn odyssey() -> DiagnosticService {
    let transport = SimulatedTransport::with_vehicle(
        VirtualVehicle::honda_odyssey(ScenarioId::Healthy),
        AdapterPersonality::genuine_v1_5(),
    );
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let mut service = DiagnosticService::start(
        adapter,
        SessionStore::open_in_memory().unwrap(),
        Arc::new(DecoderSet::generic_obd().unwrap()),
        SafetyGate::phase1(),
        None,
    )
    .unwrap();
    assert!(service.connect(USER).success);
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);
    service
}

fn identity_of(service: &mut DiagnosticService, module: &str) -> CalibrationIdentity {
    let read = service.read_calibration_identity(module, USER);
    assert!(read.success, "{:?}", read.error);
    serde_json::from_value(read.data.expect("an identity")["identity"].clone()).unwrap()
}

/// Every request the session sent to the vehicle, as the adapter was given it.
fn requests(service: &DiagnosticService) -> Vec<String> {
    service
        .store()
        .events_since(service.session_id(), 0, 100_000)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AdapterRequest { command } => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn the_vehicle_and_its_engine_controller_are_identified_with_their_evidence() {
    let mut service = odyssey();
    let id = identity_of(&mut service, ENGINE);

    // 1. The vehicle.
    assert_eq!(id.first(Field::Vin), Some(SIMULATED_HONDA_VIN));
    assert_eq!(id.first(Field::ModelYear), Some("2023"));
    assert!(id.first(Field::Make).is_some_and(|m| m.contains("Honda")));
    // 2. The module.
    assert_eq!(id.module_key, ENGINE);
    assert_eq!(id.address, "18DAF110");
    assert_eq!(id.first(Field::ModuleName), Some("ECM-EngineControl"));
    assert!(id.protocol.as_deref().is_some_and(|p| p.contains("29")));
    // 3. Its software.
    assert_eq!(id.values(Field::CalibrationId), vec!["37805-5MR-C120"]);
    assert_eq!(id.values(Field::CalibrationVerificationNumber).len(), 1);
    assert!(id.names_its_software());

    // 4. Each identifier says how it was read, and keeps the reply it was
    //    read from: the bytes, and the row of the flight recorder they are in.
    let calibration = &id.fields[&Field::CalibrationId][0];
    assert_eq!(calibration.source, IdentitySource::ObdInfoType { info_type: 0x04 });
    let raw = calibration.raw_hex.as_deref().expect("the bytes are kept");
    // "37805-5MR-C120" as the module sent it.
    assert!(raw.contains("33373830352d354d522d43313230"), "{raw}");
    let row = calibration.evidence_ref.expect("an evidence reference");
    let recorded = service.store().event_by_id(row).unwrap();
    assert!(
        matches!(&recorded.kind, EventKind::AdapterResponse { command, .. } if command == "0904"),
        "{:?}",
        recorded.kind
    );
}

/// The Odyssey's modules refuse every standard identification identifier.
/// That is what is recorded: asked, refused, with the refusal's own bytes.
#[test]
fn an_identifier_the_module_refuses_is_kept_as_a_refusal() {
    let mut service = odyssey();
    let id = identity_of(&mut service, ENGINE);

    assert!(id.first(Field::HardwareNumber).is_none());
    assert!(id.first(Field::PartNumber).is_none());

    let hardware = id
        .unanswered
        .iter()
        .find(|u| u.source == IdentitySource::UdsDid { did: 0xF191 })
        .expect("F191 was asked for");
    assert_eq!(hardware.field, Some(Field::HardwareNumber));
    assert!(hardware.reason.contains("refused"), "{}", hardware.reason);
    // "Request out of range" is the module saying it has no such identifier,
    // and it is kept as that, not as a failure to read.
    assert_eq!(hardware.state, NotGiven::NotSupported);
    assert!(hardware.raw_hex.as_deref().is_some_and(|r| r.starts_with("7f22")), "{hardware:?}");
    assert!(hardware.evidence_ref.is_some());
    // All thirteen were asked or accounted for, and none was made up.
    let asked = id.unanswered.iter().filter(|u| matches!(u.source, IdentitySource::UdsDid { .. }));
    assert_eq!(asked.count(), 13);
}

/// What the first real scan lost: the calibration identification was read and
/// then stored nowhere a module's record could show it.
#[test]
fn what_was_read_is_kept_on_the_module_and_against_the_vehicle() {
    let mut service = odyssey();
    for (module, calibration) in [(ENGINE, "37805-5MR-C120"), (TRANSMISSION, "28102-5MX-A200")] {
        let id = identity_of(&mut service, module);
        assert_eq!(id.values(Field::CalibrationId), vec![calibration]);
    }

    let modules = service.store().modules(service.session_id()).unwrap();
    let engine = modules.iter().find(|m| m.module_key == ENGINE).unwrap();
    assert_eq!(engine.identity.calibration_ids, vec!["37805-5MR-C120"]);
    assert_eq!(engine.identity.calibration_verification_numbers.len(), 1);
    let gearbox = modules.iter().find(|m| m.module_key == TRANSMISSION).unwrap();
    assert_eq!(gearbox.identity.calibration_ids, vec!["28102-5MX-A200"]);

    let known = service.store().knowledge(SIMULATED_HONDA_VIN).unwrap();
    let finding = known
        .iter()
        .find(|f| f.subject == format!("module.{ENGINE}.calibration"))
        .expect("a finding about the engine controller's calibration");
    assert!(finding.claim.contains("37805-5MR-C120"), "{}", finding.claim);
}

/// The whole path: read the identity, look in the sources, judge what is
/// there, keep what matches.
#[test]
fn a_file_is_matched_to_the_module_only_as_far_as_the_evidence_goes() {
    let mut service = odyssey();
    let id = identity_of(&mut service, ENGINE);
    let now = "2026-10-04T12:00:00Z";

    let folder = tempfile::tempdir().unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(cache_dir.path()).unwrap();
    let source = DirectorySource::new("folder", folder.path());

    // Nothing supplied: nothing found, and that is the answer.
    let nothing = resolve(&id, &[&source], Some(&cache), now);
    assert_eq!(nothing.outcome, Outcome::NoArtifactFound);
    assert!(nothing.sources.iter().all(|s| !s.source.uses_network));

    // Three files a person might drop in the folder.
    let image = b"stand-in bytes; not a real calibration";
    let put = |name: &str, bytes: &[u8], metadata: Option<String>| {
        std::fs::write(folder.path().join(name), bytes).unwrap();
        if let Some(m) = metadata {
            std::fs::write(folder.path().join(format!("{name}.json")), m).unwrap();
        }
    };
    // Declared to be this calibration, with the hash it should have.
    put(
        "pcm-update.bin",
        image,
        Some(format!(
            r#"{{ "sha256": "{}", "calibration_id": "37805-5MR-C120",
                  "make": "Honda", "model_year": [2023] }}"#,
            aim_calibration::sha256_hex(image)
        )),
    );
    // Only named like it: a package by its shape, whose header names
    // something else. Made up, and nobody's real file.
    let package = aim_calibration::rwd::fixture::z(&[&[b"TEST-OTHER-0001"]], &[0x9C, 0x41, 0x07]);
    put("37805-5MR-C120.rwd", &package, None);
    // Declared to be the transmission's.
    put(
        "gearbox.bin",
        b"yet other bytes",
        Some(r#"{ "calibration_id": "28102-5MX-A200" }"#.to_string()),
    );

    let found = resolve(&id, &[&source], Some(&cache), now);
    assert_eq!(found.outcome, Outcome::ArtifactFound);

    let by_name = |name: &str| {
        found
            .matches
            .iter()
            .chain(&found.set_aside)
            .find(|e| e.artifact.filename == name)
            .unwrap_or_else(|| panic!("{name} was not evaluated"))
    };
    let declared = by_name("pcm-update.bin");
    assert_eq!(declared.matching.status, MatchStatus::ExactMatch, "{}", declared.matching.reason);
    assert_eq!(declared.validation.status, ValidationStatus::Valid);
    assert_eq!(declared.artifact.sha256, aim_calibration::sha256_hex(image));
    assert!(declared.cached);

    let named = by_name("37805-5MR-C120.rwd");
    assert_eq!(named.matching.status, MatchStatus::PartialMatch, "a name is not evidence");
    assert_eq!(named.validation.status, ValidationStatus::PartiallyValidated);

    let other = by_name("gearbox.bin");
    assert_eq!(other.matching.status, MatchStatus::NoMatch);
    assert!(!other.cached);

    // Exact first.
    assert_eq!(found.matches[0].artifact.filename, "pcm-update.bin");
    assert_eq!(cache.list().unwrap().len(), 2);
}

/// Reading an identity only ever reads. Every request the whole session sent
/// is looked at: nothing that writes, resets, opens a session, asks for
/// security access, or starts a routine or a download.
#[test]
fn nothing_but_reads_is_sent_to_the_vehicle() {
    let mut service = odyssey();
    let before = requests(&service).len();
    identity_of(&mut service, ENGINE);
    identity_of(&mut service, TRANSMISSION);
    let sent = requests(&service);
    assert!(sent.len() > before + 20, "the identity read should have asked something");

    // UDS and OBD requests are hex; everything else is an adapter command.
    let to_vehicle: Vec<&String> = sent
        .iter()
        .filter(|c| !c.is_empty() && c.len() % 2 == 0 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    for request in &to_vehicle {
        let service_id = u8::from_str_radix(&request[..2], 16).unwrap();
        // The OBD-II services that read, TesterPresent, ReadDTCInformation
        // and ReadDataByIdentifier. Not OBD-II 04, which clears codes, nor
        // 08, which commands a component: they sit between the reads and are
        // not reads. Session control, reset, security access, writes,
        // routines and transfers are not among them either.
        assert!(
            matches!(service_id, 0x01..=0x03 | 0x05..=0x07 | 0x09 | 0x0A | 0x3E | 0x19 | 0x22),
            "{request} is not a read"
        );
    }
    // And the UDS reads the identity itself made are the thirteen
    // identifiers, for each of two modules.
    let identity_reads = to_vehicle.iter().filter(|c| c.starts_with("22")).count();
    assert_eq!(identity_reads, 26);
}

/// Where every field stands is said in one word each, and the words differ:
/// what the module gave, what it said it does not have, and what nothing in
/// this build asks for at all.
#[test]
fn each_field_is_available_not_supported_or_not_read_and_they_are_not_confused() {
    let mut service = odyssey();
    let read = service.read_calibration_identity(ENGINE, USER);
    let data = read.data.expect("an identity");
    let stands: std::collections::BTreeMap<Field, Availability> =
        serde_json::from_value(data["availability"].clone()).unwrap();

    assert_eq!(stands[&Field::CalibrationId], Availability::Available);
    assert_eq!(stands[&Field::CalibrationVerificationNumber], Availability::Available);
    assert_eq!(stands[&Field::ModuleName], Availability::Available);
    // Asked for by every request that could read it, and refused as not there.
    for field in [Field::HardwareNumber, Field::PartNumber, Field::SoftwareNumber] {
        assert_eq!(stands[&field], Availability::NotSupported, "{field:?}");
    }
    // No standard request reads these, so nothing was asked and nothing is known.
    for field in [Field::ProgramId, Field::StrategyId, Field::RomId] {
        assert_eq!(stands[&field], Availability::NotRead, "{field:?}");
    }
    assert_eq!(data["availability"]["hardware_number"], "NOT_SUPPORTED");
    assert_eq!(data["availability"]["program_id"], "NOT_READ");
}

fn found_by(service: &mut DiagnosticService, module: &str) -> (serde_json::Value, Found) {
    let result = service.find_calibration(module, "agent:test");
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.tool, "find_calibration");
    let data = result.data.expect("a search");
    let found: Found = serde_json::from_value(data["search"].clone()).unwrap();
    (data, found)
}

/// The search a model asks for is the search a person asks for: the same
/// library, the same rules, the same answer.
#[test]
fn the_tool_a_model_calls_runs_the_same_search_as_the_screen() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::at(dir.path().join("calibrations")).with_program_files(None);
    let mut service = odyssey();

    // With nowhere to look, it says so and sends nothing.
    let before = requests(&service).len();
    let nowhere = service.find_calibration(ENGINE, "agent:test");
    assert!(!nowhere.success);
    assert_eq!(requests(&service).len(), before);

    service.set_calibration_library(Some(library.clone()));

    // Nothing supplied: an answer, with every source and how it ended.
    let (data, found) = found_by(&mut service, ENGINE);
    assert_eq!(found.resolution.outcome, Outcome::NoArtifactFound);
    assert!(!found.resolution.incomplete);
    let status = |found: &Found, id: &str| {
        found.resolution.sources.iter().find(|s| s.source.id == id).map(|s| s.status)
    };
    assert_eq!(status(&found, "folder"), Some(SourceStatus::NoMatch));
    assert_eq!(status(&found, "honda-j2534-rewrite"), Some(SourceStatus::NotConfigured));
    assert_eq!(status(&found, "cache"), Some(SourceStatus::NoMatch));
    assert_eq!(data["limits"]["manufacturer_origin"], "NOT_ESTABLISHED");
    assert_eq!(data["limits"]["downloads"], false);
    assert_eq!(data["availability"]["calibration_id"], "AVAILABLE");
    assert_eq!(data["identity"]["fields"]["calibration_id"][0]["value"], "37805-5MR-C120");

    // A file the person declares to be this calibration.
    let image = b"stand-in bytes; not a real calibration";
    library.add_file("pcm-update.bin", image).unwrap();
    std::fs::write(
        library.folder().join("pcm-update.bin.json"),
        r#"{ "calibration_id": "37805-5MR-C120" }"#,
    )
    .unwrap();

    let (_, by_tool) = found_by(&mut service, ENGINE);
    assert_eq!(by_tool.resolution.outcome, Outcome::ArtifactFound);
    let one = &by_tool.resolution.matches[0];
    assert_eq!(one.matching.status, MatchStatus::ExactMatch);
    assert_eq!(one.matching.rests_on, Some(Basis::UserDeclared));
    assert_eq!(one.origin.manufacturer, ManufacturerOrigin::NotEstablished);
    assert_eq!(status(&by_tool, "folder"), Some(SourceStatus::Matched));

    // The same question asked the way the screen asks it: the identity read
    // from the module, handed to the library. One implementation, one answer.
    let identity = identity_of(&mut service, ENGINE);
    let by_screen = library.find(&identity, "2026-10-04T12:00:00Z");
    let judged = |found: &Found| {
        found
            .resolution
            .matches
            .iter()
            .map(|e| (e.artifact.sha256.clone(), e.matching.status, e.matching.rests_on))
            .collect::<Vec<_>>()
    };
    assert_eq!(judged(&by_tool), judged(&by_screen));
    assert_eq!(by_tool.resolution.outcome, by_screen.resolution.outcome);
}

/// Looking for a calibration only ever reads the vehicle, and reads it
/// exactly as much as reading the identity does: the search itself sends
/// nothing at all.
#[test]
fn looking_for_a_calibration_sends_the_vehicle_nothing_but_the_identity_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut service = odyssey();
    service.set_calibration_library(Some(
        Library::at(dir.path().join("calibrations")).with_program_files(None),
    ));

    // What reaches the vehicle, as opposed to what only sets the adapter up.
    let to_vehicle = |service: &DiagnosticService| {
        requests(service)
            .into_iter()
            .filter(|c| {
                !c.is_empty() && c.len() % 2 == 0 && c.bytes().all(|b| b.is_ascii_hexdigit())
            })
            .count()
    };
    let start = to_vehicle(&service);
    identity_of(&mut service, ENGINE);
    let after_identity = to_vehicle(&service);
    found_by(&mut service, ENGINE);
    let sent = requests(&service);

    assert!(after_identity > start);
    assert_eq!(
        to_vehicle(&service) - after_identity,
        after_identity - start,
        "the search sent the vehicle something of its own"
    );
    for request in sent
        .iter()
        .filter(|c| !c.is_empty() && c.len() % 2 == 0 && c.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        let service_id = u8::from_str_radix(&request[..2], 16).unwrap();
        // Nothing that writes (2E), runs a routine (31), opens a session
        // (10), resets (11), asks for security access (27), or starts,
        // carries or ends a transfer (34, 36, 37).
        assert!(
            !matches!(
                service_id,
                0x04 | 0x08 | 0x10 | 0x11 | 0x27 | 0x2E | 0x31 | 0x34 | 0x36 | 0x37
            ),
            "{request} was sent while looking for a calibration"
        );
        assert!(
            matches!(service_id, 0x01..=0x03 | 0x05..=0x07 | 0x09 | 0x0A | 0x3E | 0x19 | 0x22),
            "{request} is not a read"
        );
    }
}

// ---- the real vehicle -------------------------------------------------------

/// The 2023 Odyssey's first real scan, as it was recorded on 2026-10-04 (its
/// VIN's serial zeroed). The simulator above was written to imitate this
/// recording; these bytes are the vehicle's own.
const REAL_SCAN: &str = include_str!("replays/2023-honda-odyssey-first-scan.transcript");

fn the_real_odyssey() -> DiagnosticService {
    let transport = ReplayTransport::new(
        Transcript::parse(REAL_SCAN).unwrap(),
        ReplayMode::Lookup,
        "replay of the real Odyssey",
    );
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let mut service = DiagnosticService::start(
        adapter,
        SessionStore::open_in_memory().unwrap(),
        Arc::new(DecoderSet::generic_obd().unwrap()),
        SafetyGate::phase1(),
        None,
    )
    .unwrap();
    assert!(service.connect(USER).success);
    assert!(service.identify_vehicle(USER).success);
    assert!(service.scan_modules(USER).success);
    service
}

/// The identity read, run over what the real vehicle actually sent.
///
/// This is as far as a recording can take it. The scan that was recorded
/// asked for the calibration identification, the verification number, the
/// name, and six of the thirteen standard identifiers, so those are checked
/// against the vehicle's own bytes. The other seven identifiers were never
/// put to the real vehicle: the recording has no answer for them, and what
/// this asserts about them is exactly that, not what the vehicle would say.
///
/// One more limit, and it is why the read asks twice. The recorded scan asked
/// for the calibration identification and verification number of every
/// module at once, never of one module alone. So a replay cannot say whether
/// this vehicle answers them when asked alone; here that goes unanswered and
/// the answer comes from asking everyone, which is the vehicle's real reply.
#[test]
fn the_real_odysseys_recorded_answers_read_as_what_they_are() {
    let mut service = the_real_odyssey();

    for (module, name, calibration, cvn) in [
        (ENGINE, "ECM-EngineControl", "37805-5MR-C120", "16B6A354"),
        (TRANSMISSION, "TCM-TransmisCtrl", "28102-5MX-A200", "550C681C"),
    ] {
        let read = service.read_calibration_identity(module, USER);
        assert!(read.success, "{:?}", read.error);
        let data = read.data.expect("an identity");
        let id: CalibrationIdentity = serde_json::from_value(data["identity"].clone()).unwrap();

        // What the vehicle gave, from its own bytes.
        assert_eq!(id.values(Field::CalibrationId), vec![calibration], "{module}");
        let cvns = id.values(Field::CalibrationVerificationNumber);
        assert!(cvns.len() == 1 && cvns[0].eq_ignore_ascii_case(cvn), "{module}: {cvns:?}");
        assert_eq!(id.first(Field::ModuleName), Some(name), "{module}");
        let given = &id.fields[&Field::CalibrationId][0];
        assert!(given.evidence_ref.is_some() && given.raw_hex.is_some());

        // What the vehicle refused: `7F 22 31` to each of the six it was
        // really asked, kept as "has no such identifier" with the bytes.
        for did in [0xF187u16, 0xF188, 0xF191, 0xF18A, 0xF195, 0xF197] {
            let refused = id
                .unanswered
                .iter()
                .find(|u| u.source == IdentitySource::UdsDid { did })
                .unwrap_or_else(|| panic!("{did:04X} was asked of {module}"));
            assert_eq!(refused.state, NotGiven::NotSupported, "{did:04X} of {module}");
            assert_eq!(refused.raw_hex.as_deref(), Some("7f2231"), "{did:04X} of {module}");
            assert!(refused.evidence_ref.is_some());
        }

        // What the vehicle was never asked. The recording is silent on the
        // first of them, and the rest are then not asked. None is called
        // "not supported": nobody knows that.
        let never_asked: Vec<NotGiven> =
            [0xF180u16, 0xF181, 0xF182, 0xF189, 0xF192, 0xF193, 0xF194]
                .iter()
                .map(|did| {
                    id.unanswered
                        .iter()
                        .find(|u| u.source == IdentitySource::UdsDid { did: *did })
                        .unwrap_or_else(|| panic!("{did:04X} is accounted for"))
                        .state
                })
                .collect();
        assert_eq!(never_asked[0], NotGiven::NoAnswer);
        assert!(never_asked[1..].iter().all(|s| *s == NotGiven::NotAsked), "{never_asked:?}");

        // And so, in a word each: the part number was asked and is not there;
        // the hardware number was asked one way of two, so it is not settled.
        assert_eq!(data["availability"]["calibration_id"], "AVAILABLE");
        assert_eq!(data["availability"]["part_number"], "NOT_SUPPORTED");
        assert_eq!(data["availability"]["hardware_number"], "NOT_READ");
        assert_eq!(data["availability"]["program_id"], "NOT_READ");
        // Nothing was made up to fill a gap.
        for field in [Field::HardwareNumber, Field::PartNumber, Field::ProgramId, Field::StrategyId]
        {
            assert!(id.first(field).is_none(), "{field:?} of {module}");
        }
    }
}

/// The whole path on the real vehicle's own identity: nothing is on this
/// computer for it, and the answer is that, with where it looked.
#[test]
fn the_real_odysseys_calibration_is_looked_for_and_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut service = the_real_odyssey();
    service.set_calibration_library(Some(
        Library::at(dir.path().join("calibrations")).with_program_files(None),
    ));

    let (data, found) = found_by(&mut service, ENGINE);

    assert_eq!(data["identity"]["fields"]["calibration_id"][0]["value"], "37805-5MR-C120");
    assert_eq!(found.resolution.outcome, Outcome::NoArtifactFound);
    assert!(found.resolution.matches.is_empty() && found.resolution.set_aside.is_empty());
    assert!(found.resolution.sources.iter().all(|s| !s.source.uses_network));
    assert_eq!(found.resolution.sources.len(), 3);
}
