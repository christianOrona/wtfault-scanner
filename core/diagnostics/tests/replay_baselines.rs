//! Regression tests for issue #59: recorded vehicle sessions replay through the current core in CI.
//!
//! The simulator's fixtures are regenerated with
//! `cargo test -p aim-diagnostics --test replay_baselines -- --ignored record_the_simulator_sessions`.
//! A real session is exported by `cargo run -p aim-diagnostics --example export_replay`, and its
//! baseline recorded by `record_missing_baselines`. A baseline is only ever raised deliberately.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ReplayMode, ReplayTransport, ScenarioId, SimulatedTransport, Transcript};
use aim_transport::Transport;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const USER: &str = "user:test";
const REPLAYS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/replays");

fn service_on(transport: Box<dyn Transport>) -> DiagnosticService {
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(transport, Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap()
}

fn first_visit(service: &mut DiagnosticService) {
    let _connect = service.connect(USER);
    let _identify = service.identify_vehicle(USER);
    let _scan_modules = service.scan_modules(USER);

    let modules = service.store().modules(service.session_id()).unwrap();
    for module in modules {
        let _supported_pids = service.read_supported_pids(&module.module_key, USER);
    }

    let _scan_all_modules = service.scan_all_modules(USER);
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Discovery {
    identity_settled: usize,
    modules_found: u32,
    modules_named: u32,
    findings: u32,
}

fn discover(service: &DiagnosticService) -> Discovery {
    let identity = service.identity();
    let Some(vin) = identity.settled("vin") else {
        return Discovery { identity_settled: 0, modules_found: 0, modules_named: 0, findings: 0 };
    };

    let card = aim_diagnostics::scorecard::scorecard(service.store(), vin).unwrap();
    Discovery {
        identity_settled: card.identity.settled.len(),
        modules_found: card.modules.found,
        modules_named: card.modules.named,
        findings: card.findings.established + card.findings.ruled_out + card.findings.observed,
    }
}

fn replay(text: &str) -> Discovery {
    replay_with_vin(text).0
}

/// What a replay discovers, and the VIN it settled on.
fn replay_with_vin(text: &str) -> (Discovery, Option<String>) {
    let transcript = Transcript::parse(text).unwrap();
    let transport = ReplayTransport::new(transcript, ReplayMode::Lookup, "replay");
    let mut service = service_on(Box::new(transport));
    first_visit(&mut service);
    let vin = service.identity().settled("vin").map(str::to_string);
    (discover(&service), vin)
}

fn transcripts() -> Vec<PathBuf> {
    let mut transcripts = fs::read_dir(REPLAYS)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("transcript")
        })
        .collect::<Vec<_>>();
    transcripts.sort();
    transcripts
}

/// Recorded from a real vehicle rather than the simulator, and so only
/// committed with its VIN anonymised.
fn is_real(transcript: &Path) -> bool {
    !transcript.file_name().and_then(|name| name.to_str()).unwrap_or("").starts_with("simulator-")
}

#[test]
#[ignore]
fn record_the_simulator_sessions() {
    fs::create_dir_all(REPLAYS).unwrap();

    for (name, scenario) in
        [("simulator-healthy", ScenarioId::Healthy), ("simulator-dpf-regen", ScenarioId::DpfRegen)]
    {
        let mut service = service_on(Box::new(SimulatedTransport::new(scenario)));
        first_visit(&mut service);

        let transcript = aim_diagnostics::transcript::export_transcript(
            service.store(),
            service.session_id(),
            &format!("{name}: recorded from the simulator by record_the_simulator_sessions"),
        )
        .unwrap();

        fs::write(format!("{REPLAYS}/{name}.transcript"), transcript.as_bytes()).unwrap();

        let discovery = replay(&transcript);
        fs::write(
            format!("{REPLAYS}/{name}.baseline.json"),
            serde_json::to_string_pretty(&discovery).unwrap(),
        )
        .unwrap();
    }
}

/// Write a baseline for every transcript that has none, from what the current
/// core discovers replaying it. Existing baselines are left alone.
#[test]
#[ignore]
fn record_missing_baselines() {
    for transcript in transcripts() {
        let baseline_path = transcript.with_extension("baseline.json");
        if baseline_path.is_file() {
            continue;
        }
        let discovery = replay(&fs::read_to_string(&transcript).unwrap());
        fs::write(&baseline_path, serde_json::to_string_pretty(&discovery).unwrap()).unwrap();
        println!("{}: {discovery:?}", baseline_path.display());
    }
}

#[test]
fn no_recorded_vehicle_discovers_less_than_its_baseline() {
    let transcripts = transcripts();

    assert!(!transcripts.is_empty(), "no replay transcripts found in {REPLAYS}");

    let mut failures = Vec::new();
    for transcript in transcripts {
        let baseline_path = transcript.with_extension("baseline.json");
        if !baseline_path.is_file() {
            failures.push(format!(
                "{}: missing baseline {}",
                transcript.display(),
                baseline_path.display()
            ));
            continue;
        }

        let transcript_text = fs::read_to_string(&transcript).unwrap();
        let baseline: Discovery =
            serde_json::from_slice(&fs::read(&baseline_path).unwrap()).unwrap();
        let (actual, vin) = replay_with_vin(&transcript_text);

        // `export_replay` refuses to write a real VIN; this catches a
        // transcript that reached the repository some other way.
        if is_real(&transcript) {
            if let Some(vin) = vin.filter(|vin| !vin.ends_with("000000")) {
                failures.push(format!(
                    "{}: settles on VIN {vin}, which is not anonymised; export it with export_replay",
                    transcript.display()
                ));
            }
        }

        if actual.identity_settled < baseline.identity_settled {
            failures.push(format!(
                "{}: identity_settled {} < baseline {}",
                transcript.display(),
                actual.identity_settled,
                baseline.identity_settled
            ));
        }
        if actual.modules_found < baseline.modules_found {
            failures.push(format!(
                "{}: modules_found {} < baseline {}",
                transcript.display(),
                actual.modules_found,
                baseline.modules_found
            ));
        }
        if actual.modules_named < baseline.modules_named {
            failures.push(format!(
                "{}: modules_named {} < baseline {}",
                transcript.display(),
                actual.modules_named,
                baseline.modules_named
            ));
        }
        if actual.findings < baseline.findings {
            failures.push(format!(
                "{}: findings {} < baseline {}",
                transcript.display(),
                actual.findings,
                baseline.findings
            ));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A replay is only a stand-in for the vehicle if it discovers what the vehicle
/// did. Replies are recorded per addressed module (`ATSH`), so a replay that
/// answered by command alone gave every module the first module's answers.
#[test]
fn a_replayed_session_discovers_what_the_live_one_did() {
    for scenario in [ScenarioId::Healthy, ScenarioId::DpfRegen] {
        let mut live = service_on(Box::new(SimulatedTransport::new(scenario)));
        first_visit(&mut live);
        let text =
            aim_diagnostics::transcript::export_transcript(live.store(), live.session_id(), "live")
                .unwrap();
        assert_eq!(replay(&text), discover(&live), "{scenario:?}");
    }
}
