//! The vPIC request for a VIN (#55): validated before anything is sent.

use aim_decoders::vpic::decode_vin_values_url;
use aim_types::ErrorCode;

#[test]
fn a_valid_vin_becomes_the_decode_request() {
    assert_eq!(
        decode_vin_values_url("1FT7W2BT7KEF78036").unwrap(),
        "https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/1FT7W2BT7KEF78036?format=json"
    );
}

/// Whitespace and lower case are what a person pastes; the request is canonical.
#[test]
fn a_pasted_vin_is_normalised() {
    assert_eq!(
        decode_vin_values_url("  1ft7w2bt7kef78036 ").unwrap(),
        "https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/1FT7W2BT7KEF78036?format=json"
    );
}

/// Nothing malformed leaves the computer, and nothing can be smuggled into the URL.
#[test]
fn an_invalid_vin_is_never_sent() {
    for bad in
        ["", "1FT7W2BT7KEF7803", "1FT7W2BT7KEF78036X", "1FT7W2BT7KEF7803/", "1FT7W2BT7KEF780?6"]
    {
        let err = decode_vin_values_url(bad).expect_err(bad);
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid, "{bad}");
    }
}
