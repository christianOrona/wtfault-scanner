//! A vPIC lookup, once fetched on request, is cached and ranked into identity (#55).

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::DiagnosticService;
use aim_safety::SafetyGate;
use aim_session::SessionStore;
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_types::ErrorCode;
use std::sync::Arc;

const USER: &str = "user:test";
const F250: &str = include_str!("../../decoders/tests/fixtures/vpic/ford-f250-2019.json");

fn connected() -> DiagnosticService {
    let transport = SimulatedTransport::new(ScenarioId::Healthy);
    let adapter: Box<dyn DiagnosticAdapter> =
        Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
    let store = SessionStore::open_in_memory().unwrap();
    let decoders = Arc::new(DecoderSet::generic_obd().unwrap());
    let mut service =
        DiagnosticService::start(adapter, store, decoders, SafetyGate::phase1(), None).unwrap();
    assert!(service.connect(USER).success);
    service
}

fn identified() -> DiagnosticService {
    let mut service = connected();
    assert!(service.identify_vehicle(USER).success);
    service
}

#[test]
fn the_request_is_for_the_connected_vehicles_vin() {
    let service = connected();
    let err = service.vpic_request().expect_err("no VIN yet, so nothing to ask about");
    assert_eq!(err.code, ErrorCode::PreconditionFailed);

    let service = identified();
    assert_eq!(
        service.vpic_request().unwrap(),
        "https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/1FT7W2BT6KEC00001?format=json"
    );
}

#[test]
fn a_reply_is_cached_and_establishes_the_model() {
    let mut service = identified();
    let vin = service.identity().settled("vin").unwrap().to_string();
    let before = aim_diagnostics::scorecard::scorecard(service.store(), &vin).unwrap();
    assert!(service.cached_vpic().is_none());
    assert_eq!(service.identity().settled("model"), None);

    let url = service.vpic_request().unwrap();
    let decode = service.record_vpic_reply(F250, &url).unwrap();
    assert_eq!(decode.model.as_deref(), Some("F-250"));

    let cached = service.cached_vpic().expect("kept against the VIN");
    assert_eq!(cached.source_url, url);
    assert_eq!(cached.vin, vin);

    let identity = service.identity();
    assert_eq!(identity.settled("model"), Some("F-250"));
    assert!(identity.contested().is_empty(), "{:?}", identity.contested());

    let after = aim_diagnostics::scorecard::scorecard(service.store(), &vin).unwrap();
    let diff = before.diff(&after);
    assert!(
        diff.identity.gained.iter().any(|g| g == "model: unresolved -> settled"),
        "{:?}",
        diff.identity
    );
    assert!(diff.identity.lost.is_empty(), "{:?}", diff.identity);
}

#[test]
fn a_reply_that_does_not_parse_is_not_kept() {
    let mut service = identified();
    let url = service.vpic_request().unwrap();
    let err = service.record_vpic_reply("<html>Service Unavailable</html>", &url).unwrap_err();
    assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
    assert!(service.cached_vpic().is_none());
}

#[test]
fn nothing_is_kept_without_a_settled_vin() {
    let mut service = connected();
    let err = service
        .record_vpic_reply(F250, "https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/X")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PreconditionFailed);
    assert!(service.cached_vpic().is_none());
}
