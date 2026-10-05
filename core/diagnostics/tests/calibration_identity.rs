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
    resolve, Cache, CalibrationIdentity, DirectorySource, Field, IdentitySource, MatchStatus,
    ValidationStatus,
};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{
    AdapterPersonality, ScenarioId, SimulatedTransport, VirtualVehicle, SIMULATED_HONDA_VIN,
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
    // Only named like it.
    put("37805-5MR-C120.rwd", b"some other bytes", None);
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
        // OBD-II services 01 to 0A, TesterPresent, ReadDTCInformation and
        // ReadDataByIdentifier: every one of them a read. Session control,
        // reset, security access, writes, routines and transfers are not
        // among them.
        assert!(matches!(service_id, 0x01..=0x0A | 0x3E | 0x19 | 0x22), "{request} is not a read");
    }
    // And the UDS reads the identity itself made are the thirteen
    // identifiers, for each of two modules.
    let identity_reads = to_vehicle.iter().filter(|c| c.starts_with("22")).count();
    assert_eq!(identity_reads, 26);
}
