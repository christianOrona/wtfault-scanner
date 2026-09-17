//! A recorded session exported as a transcript replays through the core, and a
//! redacted one tells the next reader nothing about the original vehicle (#59).

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::transcript::{export_transcript, redact_vin};
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ReplayMode, ReplayTransport, ScenarioId, SimulatedTransport, Transcript};
use aim_transport::Transport;
use std::sync::Arc;

const USER: &str = "user:test";
const REPLACEMENT: &str = "1FTEX1EP5JFA00000";

fn service_on(transport: Box<dyn Transport>) -> DiagnosticService {
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(transport, Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap()
}

/// Connect, read the VIN and scan, the way a first visit starts.
fn first_visit(service: &mut DiagnosticService) -> String {
    assert!(service.connect(USER).success, "connect");
    assert!(service.identify_vehicle(USER).success, "identify");
    assert!(service.scan_modules(USER).success, "scan");
    service.identity().settled("vin").expect("a VIN").to_string()
}

fn recorded() -> (String, String) {
    let mut service = service_on(Box::new(SimulatedTransport::new(ScenarioId::Healthy)));
    let vin = first_visit(&mut service);
    let text =
        export_transcript(service.store(), service.session_id(), "simulator, healthy").unwrap();
    (vin, text)
}

#[test]
fn an_export_is_a_transcript_of_every_exchange() {
    let (_, text) = recorded();
    assert!(
        text.starts_with(
            "# simulator, healthy
"
        ),
        "{}",
        &text[..text.len().min(200)]
    );
    let parsed = Transcript::parse(&text).unwrap();
    assert!(parsed.exchanges.len() > 5, "{} exchanges", parsed.exchanges.len());
    assert!(
        parsed.exchanges.iter().any(|e| e.command.eq_ignore_ascii_case("0902")),
        "the VIN read is in it"
    );
}

#[test]
fn an_exported_session_replays_strictly_through_the_core() {
    let (vin, text) = recorded();
    let transcript = Transcript::parse(&text).unwrap();
    let replay = ReplayTransport::new(transcript, ReplayMode::Strict, "replay:test");
    let mut service = service_on(Box::new(replay));
    assert_eq!(first_visit(&mut service), vin);
}

#[test]
fn a_redacted_transcript_carries_no_trace_of_the_vin() {
    let (vin, text) = recorded();
    let redacted = redact_vin(&text, &vin, REPLACEMENT);

    assert!(!redacted.contains(&vin), "the VIN as text");
    // No line of any reply holds five consecutive bytes of the VIN's ASCII.
    let hex: Vec<String> = vin.bytes().map(|b| format!("{b:02X}")).collect();
    for window in hex.windows(5) {
        let run = window.join(" ");
        assert!(
            !redacted.lines().any(|l| l.starts_with("< ") && l.contains(&run)),
            "VIN bytes {run}"
        );
    }

    // And it still replays, now identifying as the replacement.
    let replay = ReplayTransport::new(
        Transcript::parse(&redacted).unwrap(),
        ReplayMode::Strict,
        "replay:test",
    );
    let mut service = service_on(Box::new(replay));
    assert_eq!(first_visit(&mut service), REPLACEMENT);
}

#[test]
fn a_transcript_without_the_vin_is_unchanged() {
    let text = "# note
> ATZ
< ELM327 v1.5
";
    assert_eq!(redact_vin(text, "1FT7W2BT6KEC00001", REPLACEMENT), text);
}
