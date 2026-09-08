//! SAE J1979 readiness monitors, decoded from Mode 01 PID 01.
//!
//! This is the most useful PID on the bus for a used-car buyer. Clearing the
//! trouble codes also resets every monitor to "not complete", and they only
//! finish again after the car has been driven through the right conditions —
//! usually 50-100 miles. So no stored codes plus incomplete monitors means the
//! codes were very likely cleared shortly before the viewing.
//!
//! The bit polarity is the part worth pinning down: byte C says a monitor is
//! *supported*, byte D says it is *not complete*. Reading either backwards
//! would invert the conclusion and tell a buyer a car is fine when it is hiding
//! something, which is the worst failure this project can have.

use aim_decoders::DecoderSet;
use aim_types::{Timestamp, Value};

/// Assert one flag, naming what it means. The supported/incomplete pairing is
/// the whole point of these tests, so both polarities are asserted explicitly
/// rather than only the true cases.
#[track_caller]
fn flag(m: &std::collections::BTreeMap<String, bool>, id: &str, expected: bool) {
    let actual = *m.get(id).unwrap_or_else(|| panic!("{id} was not decoded"));
    assert!(
        actual == expected,
        "{id} should be {expected}, decoded as {actual}"
    );
}

/// Decode PID 01 from its four payload bytes and return the flags by id.
fn monitors(a: u8, b: u8, c: u8, d: u8) -> std::collections::BTreeMap<String, bool> {
    let decoders = DecoderSet::generic_obd().unwrap();
    let values = decoders
        .pids
        .decode(0x01, 0x01, &[a, b, c, d], Timestamp::from_unix_millis(0))
        .expect("PID 01 should decode");
    let flags = values
        .iter()
        .find_map(|v| match &v.value {
            Value::Flags(f) => Some(f),
            _ => None,
        })
        .expect("PID 01 should produce a flag set");
    flags.iter().map(|f| (f.id.clone(), f.set)).collect()
}

#[test]
fn a_car_that_has_finished_all_its_monitors_reads_as_ready() {
    // Every monitor supported (C = 0xFF), none incomplete (D = 0x00).
    // MIL off, no codes.
    let m = monitors(0x00, 0x07, 0xFF, 0x00);

    flag(&m, "mil_on", false);
    flag(&m, "catalyst_supported", true);
    flag(&m, "o2_sensor_supported", true);
    // Nothing incomplete: this car has been driven and its self-tests passed.
    flag(&m, "catalyst_incomplete", false);
    flag(&m, "evap_incomplete", false);
    flag(&m, "o2_sensor_incomplete", false);
}

#[test]
fn a_car_whose_codes_were_just_cleared_reads_as_not_ready() {
    // The signature of a recent clear: monitors supported but not yet complete.
    let m = monitors(0x00, 0x77, 0xFF, 0xFF);

    flag(&m, "mil_on", false); //"no lamp, which is what makes this deceptive"
    flag(&m, "catalyst_supported", true);
    flag(&m, "catalyst_incomplete", true); //"not finished since the clear"
    flag(&m, "evap_incomplete", true);
    flag(&m, "misfire_incomplete", true);
    flag(&m, "fuel_incomplete", true);
}

#[test]
fn supported_and_incomplete_are_not_confused() {
    // A vehicle that supports only the catalyst monitor (C bit 0), and has
    // completed it (D = 0). Everything else is simply not fitted.
    let m = monitors(0x00, 0x00, 0x01, 0x00);

    flag(&m, "catalyst_supported", true);
    flag(&m, "catalyst_incomplete", false);
    // Not supported at all - which must never be presented as "ready".
    flag(&m, "evap_supported", false);
    flag(&m, "secondary_air_supported", false);
    flag(&m, "egr_supported", false);
}

#[test]
fn the_warning_lamp_and_code_count_come_from_byte_a() {
    // Bit 7 set means the lamp is lit; the low seven bits are the code count.
    let m = monitors(0x83, 0x00, 0x00, 0x00);
    flag(&m, "mil_on", true);

    let decoders = DecoderSet::generic_obd().unwrap();
    let values = decoders
        .pids
        .decode(0x01, 0x01, &[0x83, 0x00, 0x00, 0x00], Timestamp::from_unix_millis(0))
        .unwrap();
    let count = values
        .iter()
        .find(|v| v.signal_id == "dtc_count")
        .expect("dtc_count should be derived");
    assert_eq!(count.value, Value::Number(3.0), "0x83 is the lamp plus three codes");
}

#[test]
fn engine_type_is_reported_because_it_changes_what_bytes_c_and_d_mean() {
    // Byte B bit 3 distinguishes diesel from petrol, and the two use different
    // monitor names for the same bit positions.
    let petrol = monitors(0x00, 0x07, 0x00, 0x00);
    flag(&petrol, "compression_ignition", false);

    let diesel = monitors(0x00, 0x0F, 0x00, 0x00);
    flag(&diesel, "compression_ignition", true);
}
