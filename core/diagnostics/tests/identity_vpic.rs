//! An NHTSA vPIC decode is identity evidence with its source, ranked against
//! what the vehicle itself reported rather than replacing it (#55).

use aim_decoders::{parse_decode_vin_values, VpicDecode};
use aim_diagnostics::{EvidenceSource, VehicleIdentity};
use aim_types::{now, Vehicle, VehicleId};

fn truck() -> Vehicle {
    Vehicle {
        id: VehicleId::new(),
        vin: Some(String::from("1FT7W2BT7KEF78036")),
        make: Some(String::from("Ford Motor Company (US, truck)")),
        model: None,
        year: Some(2019),
        trim: None,
        engine: None,
        transmission: None,
        discovered_at: now(),
    }
}

fn f250() -> VpicDecode {
    parse_decode_vin_values(include_str!("../../decoders/tests/fixtures/vpic/ford-f250-2019.json"))
        .unwrap()
}

/// The model is what the VIN's own structure never gives, and the reason to ask.
#[test]
fn a_vpic_decode_establishes_the_model_with_its_source() {
    let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
    assert!(id.unresolved.contains(&String::from("model")));

    id.record_vpic(&f250());

    let model = id.field("model").expect("vPIC gives the model");
    assert_eq!(model.settled(), Some("F-250"));
    assert_eq!(model.candidates[0].evidence[0].source, EvidenceSource::Vpic);
    assert!(!id.unresolved.contains(&String::from("model")));
    assert!(id.contested().is_empty(), "{:?}", id.contested());
}

/// "FORD" from vPIC and "Ford Motor Company (US, truck)" from the VIN structure
/// are the same make said at two levels of detail, not a disagreement.
#[test]
fn a_make_that_agrees_strengthens_the_existing_answer() {
    let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
    id.record_vpic(&f250());

    let make = id.field("make").unwrap();
    assert_eq!(make.settled(), Some("Ford Motor Company (US, truck)"));
    assert_eq!(make.candidates[0].evidence.len(), 2);
    let vpic =
        make.candidates[0].evidence.iter().find(|e| e.source == EvidenceSource::Vpic).unwrap();
    assert_eq!(vpic.value, "FORD", "the evidence keeps what vPIC said verbatim");

    let year = id.field("model_year").unwrap();
    assert_eq!(year.settled(), Some("2019"));
    assert_eq!(year.candidates[0].evidence.len(), 2);
}

/// A lookup that names a different make is exactly the case worth stopping for.
#[test]
fn a_make_that_disagrees_stays_visible() {
    let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
    id.record_vpic(&VpicDecode { make: Some(String::from("MAZDA")), ..f250() });

    let make = id.field("make").unwrap();
    assert_eq!(make.candidates.len(), 2);
    assert_eq!(make.settled(), None);
}

/// With nothing read off the vehicle, vPIC alone fills the fields.
#[test]
fn a_decode_on_its_own_fills_make_model_and_year() {
    let mazda = parse_decode_vin_values(include_str!(
        "../../decoders/tests/fixtures/vpic/mazda3-2019.json"
    ))
    .unwrap();
    let mut id = VehicleIdentity::assemble(None, &[]);
    id.record_vpic(&mazda);

    assert_eq!(id.settled("make"), Some("MAZDA"));
    assert_eq!(id.settled("model"), Some("Mazda3"));
    assert_eq!(id.settled("model_year"), Some("2019"));
}

/// Engine and fuel have no field of their own; they are kept as observations.
#[test]
fn engine_and_fuel_are_kept_as_observations() {
    let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
    id.record_vpic(&f250());

    let engine = id.observations_about("engine");
    assert_eq!(engine.len(), 1);
    assert_eq!(engine[0].value, "6.7 L V8");
    assert_eq!(engine[0].source, EvidenceSource::Vpic);
    assert_eq!(id.observations_about("fuel")[0].value, "Diesel");
}

/// A US government lookup is not a module answering.
#[test]
fn a_lookup_is_not_measured_off_the_vehicle() {
    assert!(!EvidenceSource::Vpic.measured());
    assert_eq!(EvidenceSource::Vpic.as_str(), "nhtsa_vpic");
}
