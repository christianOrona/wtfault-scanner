//! A fault list that stops partway keeps the codes that arrived and says it
//! stopped.
//!
//! Measured on a 2012 F-250: one module's `59 02` reply announced 407 bytes
//! and ended partway. The read discarded every code in it and reported the
//! module as not answering, which reads as a module with nothing to say.
//!
//! Replayed from the simulator's own session, with the last frame of the
//! module at 768's fault list taken out. Its three records are a failing
//! wheel speed fault, a stored lost-communication fault, and a test that has
//! not run; the first two arrive whole.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ReplayMode, ReplayTransport, Transcript};
use std::sync::Arc;

const USER: &str = "user:test";
const RECORDED: &str = include_str!("replays/simulator-healthy.transcript");
const LAST_FRAME: &str = "< 768 22 00 50 00 00 00 00 00\n";

fn cut_short() -> DiagnosticService {
    assert!(RECORDED.contains(LAST_FRAME), "the recording no longer has the frame this cuts");
    let text = RECORDED.replace(LAST_FRAME, "");
    let transport =
        ReplayTransport::new(Transcript::parse(&text).unwrap(), ReplayMode::Lookup, "cut short");
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    assert!(service.connect(USER).success);
    assert!(service.scan_modules(USER).success);
    service
}

#[test]
fn the_full_scan_keeps_the_codes_that_arrived_and_says_the_list_stopped() {
    let mut service = cut_short();
    let result = service.scan_all_modules(USER);
    assert!(result.success, "{:?}", result.error);

    let modules = result.data.as_ref().unwrap()["modules"].as_array().unwrap().clone();
    let module = modules.iter().find(|m| m["address"] == "768").expect("768 was scanned");
    let codes: Vec<&str> =
        module["faults"].as_array().unwrap().iter().map(|f| f["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["C0035-00", "U0121-87"], "both whole records, nothing invented");
    assert!(module["note"].is_null(), "it answered; it did not refuse: {module}");

    let cut = module["cut_off"].as_str().expect("the module says its list stopped");
    assert!(cut.contains("15 bytes") && cut.contains("after 13"), "{cut}");
    assert!(cut.contains("not its whole fault memory"), "{cut}");

    let warned: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(warned.contains(&"fault_list_cut_off"), "{warned:?}");
}

#[test]
fn reading_the_module_on_its_own_keeps_the_codes_that_arrived_too() {
    let mut service = cut_short();
    assert!(service.scan_all_modules(USER).success);

    let result = service.read_dtcs(Some("ECU_768"), USER);
    assert!(result.success, "{:?}", result.error);
    let dtcs: Vec<aim_diagnostics::DtcReport> =
        serde_json::from_value(result.data.as_ref().unwrap()["dtcs"].clone()).unwrap();
    let codes: Vec<&str> = dtcs.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, ["C0035-00", "U0121-87"]);

    let warned: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(warned.contains(&"fault_list_cut_off"), "{warned:?}");
    assert!(!warned.contains(&"module_did_not_report_codes"), "it did answer: {warned:?}");
}

/// Two visits in one database: the first reads 768's whole list, the second
/// loses a frame of it. The faults the second never received are not called
/// gone when the visits are compared.
#[test]
fn a_comparison_does_not_call_codes_that_never_arrived_gone() {
    use aim_simulator::{ScenarioId, SimulatedTransport};

    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let start = |transport: Box<dyn aim_transport::Transport>| {
        let adapter: Box<dyn DiagnosticAdapter> =
            Box::new(Elm327Adapter::new(transport, Elm327Config::fast()));
        let mut service = DiagnosticService::start(
            adapter,
            store.clone(),
            decoders.clone(),
            SafetyGate::phase1(),
            None,
        )
        .unwrap();
        assert!(service.connect(USER).success);
        assert!(service.scan_modules(USER).success);
        assert!(service.scan_all_modules(USER).success);
        service.session_id().clone()
    };

    let whole = start(Box::new(SimulatedTransport::new(ScenarioId::Healthy)));

    // The middle frame lost: what arrived is the first frame, no whole record.
    let lost = "< 768 21 09 C1 21 87 08 9C 09\n";
    assert!(RECORDED.contains(lost));
    let text = RECORDED.replace(lost, "");
    let replay =
        ReplayTransport::new(Transcript::parse(&text).unwrap(), ReplayMode::Lookup, "lost");
    let cut = start(Box::new(replay));

    let comparison = aim_session::compare_sessions(&store, &whole, &cut).unwrap();
    let change = |code: &str| {
        comparison
            .faults
            .iter()
            .find(|f| f.code == code)
            .unwrap_or_else(|| panic!("{code} in {:?}", comparison.faults))
            .change
    };
    assert_eq!(change("C0035-00"), aim_session::FaultChange::NotReceived);
    assert_eq!(change("U0121-87"), aim_session::FaultChange::NotReceived);
}
