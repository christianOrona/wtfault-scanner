//! VIN validation and the parts of a VIN that are standardised.
//!
//! ISO 3779 fixes the *structure* of a VIN and North American regulation fixes
//! the check digit and the model-year code. Those are decoded here. Model,
//! trim, engine and transmission are **not** derivable from a VIN without a
//! manufacturer table this project does not have, so they are never guessed —
//! [`aim_types::Vehicle`] keeps them `None` until a validated profile fills
//! them in.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// What can be said about a VIN from the standard alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VinInfo {
    /// The VIN, uppercased.
    pub vin: String,
    /// World manufacturer identifier, characters 1-3.
    pub wmi: String,
    /// Vehicle descriptor section, characters 4-9.
    pub vds: String,
    /// Vehicle identifier section, characters 10-17.
    pub vis: String,
    /// Manufacturer name, when the WMI is in the small verified table below.
    pub manufacturer: Option<String>,
    /// Region implied by the first character.
    pub region: Option<String>,
    /// Model year from character 10, for the 1980-2039 cycle.
    pub model_year: Option<u16>,
    /// Whether the character-9 check digit validates. North American VINs
    /// must; VINs from other regions frequently do not, so a false value is
    /// reported rather than treated as an error.
    pub check_digit_valid: bool,
}

/// Validate a VIN's shape: 17 characters, no I, O or Q.
pub fn validate(vin: &str) -> AimResult<String> {
    let vin = vin.trim().to_ascii_uppercase();
    if vin.len() != 17 {
        return Err(AimError::new(
            ErrorCode::DecoderInputInvalid,
            format!("VIN must be 17 characters, got {}", vin.len()),
        ));
    }
    if let Some(bad) = vin.chars().find(|c| !c.is_ascii_alphanumeric()) {
        return Err(AimError::new(
            ErrorCode::DecoderInputInvalid,
            format!("VIN contains non-alphanumeric character {bad:?}"),
        ));
    }
    if let Some(bad) = vin.chars().find(|c| matches!(c, 'I' | 'O' | 'Q')) {
        return Err(AimError::new(
            ErrorCode::DecoderInputInvalid,
            format!("VIN contains character {bad:?}, which ISO 3779 excludes"),
        ));
    }
    Ok(vin)
}

/// Transliteration weight of a VIN character, per the North American check-digit rules.
fn transliterate(c: char) -> Option<u32> {
    Some(match c {
        '0'..='9' => c as u32 - '0' as u32,
        'A' | 'J' => 1,
        'B' | 'K' | 'S' => 2,
        'C' | 'L' | 'T' => 3,
        'D' | 'M' | 'U' => 4,
        'E' | 'N' | 'V' => 5,
        'F' | 'W' => 6,
        'G' | 'P' | 'X' => 7,
        'H' | 'Y' => 8,
        'R' | 'Z' => 9,
        _ => return None,
    })
}

/// Compute the character-9 check digit for a VIN.
pub fn check_digit(vin: &str) -> AimResult<char> {
    let vin = validate(vin)?;
    const WEIGHTS: [u32; 17] = [8, 7, 6, 5, 4, 3, 2, 10, 0, 9, 8, 7, 6, 5, 4, 3, 2];
    let mut sum = 0u32;
    for (i, c) in vin.chars().enumerate() {
        let v = transliterate(c).ok_or_else(|| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("VIN character {c:?} has no transliteration value"),
            )
        })?;
        sum += v * WEIGHTS[i];
    }
    let r = sum % 11;
    Ok(if r == 10 { 'X' } else { char::from_digit(r, 10).expect("r < 10") })
}

/// Model year from character 10, for the 1980-2039 code cycle.
///
/// The cycle repeats every 30 years, so the answer is ambiguous in principle.
/// `reference_year` disambiguates: the returned year is the most recent
/// matching year at or before `reference_year + 1`.
pub fn model_year(code: char, reference_year: u16) -> Option<u16> {
    const CYCLE: [char; 30] = [
        'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'J', 'K', 'L', 'M', 'N', 'P', 'R', 'S', 'T', 'V',
        'W', 'X', 'Y', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    ];
    let idx = CYCLE.iter().position(|c| *c == code.to_ascii_uppercase())? as u16;
    // 1980 is 'A'; the code repeats every 30 years.
    let mut year = 1980 + idx;
    while year + 30 <= reference_year + 1 {
        year += 30;
    }
    Some(year)
}

/// A deliberately tiny WMI table.
///
/// Only entries this project is confident about are listed. An unlisted WMI
/// yields `None`, which is honest, rather than a plausible-sounding guess.
fn manufacturer_for(wmi: &str) -> Option<&'static str> {
    Some(match wmi {
        "1FT" | "1FD" => "Ford Motor Company (US, truck)",
        "1FA" | "1FB" | "1FC" | "1FM" => "Ford Motor Company (US)",
        "2FM" | "2FT" => "Ford Motor Company (Canada)",
        "3FA" | "3FE" => "Ford Motor Company (Mexico)",
        "1G1" | "1GC" | "1GK" => "General Motors (US)",
        "1C3" | "1C4" | "1C6" | "2C3" | "3C4" => "Stellantis / FCA US",
        "5YJ" => "Tesla",
        "JHM" | "JHL" => "Honda (Japan)",
        "JTD" | "JTE" | "JTM" => "Toyota (Japan)",
        "WBA" | "WBS" => "BMW (Germany)",
        "WDB" | "WDD" => "Mercedes-Benz (Germany)",
        "WVW" | "WV1" | "WV2" => "Volkswagen (Germany)",
        _ => return None,
    })
}

fn region_for(first: char) -> Option<&'static str> {
    Some(match first {
        '1' | '4' | '5' => "United States",
        '2' => "Canada",
        '3' => "Mexico",
        '6' | '7' => "Oceania",
        '8' | '9' => "South America",
        'A'..='H' => "Africa",
        'J'..='R' => "Asia",
        'S'..='Z' => "Europe",
        _ => return None,
    })
}

/// Decode the standardised parts of a VIN.
pub fn decode(vin: &str, reference_year: u16) -> AimResult<VinInfo> {
    let vin = validate(vin)?;
    let chars: Vec<char> = vin.chars().collect();
    let wmi: String = chars[0..3].iter().collect();
    let vds: String = chars[3..9].iter().collect();
    let vis: String = chars[9..17].iter().collect();
    let expected = check_digit(&vin).ok();
    Ok(VinInfo {
        manufacturer: manufacturer_for(&wmi).map(String::from),
        region: region_for(chars[0]).map(String::from),
        model_year: model_year(chars[9], reference_year),
        check_digit_valid: expected == Some(chars[8]),
        wmi,
        vds,
        vis,
        vin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The simulator's synthetic VIN. Not a real vehicle's VIN: the serial is
    /// a sequential placeholder.
    const SIM_VIN: &str = "1FT7W2BT6KEC00001";

    #[test]
    fn the_simulator_vin_is_structurally_valid() {
        let info = decode(SIM_VIN, 2026).unwrap();
        assert_eq!(info.wmi, "1FT");
        assert_eq!(info.manufacturer.as_deref(), Some("Ford Motor Company (US, truck)"));
        assert_eq!(info.region.as_deref(), Some("United States"));
        assert_eq!(info.model_year, Some(2019));
        assert!(
            info.check_digit_valid,
            "check digit should be {:?}",
            check_digit(SIM_VIN).unwrap()
        );
    }

    #[test]
    fn check_digit_algorithm_matches_the_published_example() {
        // The NHTSA worked example: 1M8GDM9AXKP042788 has check digit X.
        assert_eq!(check_digit("1M8GDM9AXKP042788").unwrap(), 'X');
    }

    #[test]
    fn a_wrong_check_digit_is_reported_not_thrown() {
        let mut bad: Vec<char> = SIM_VIN.chars().collect();
        bad[8] = if bad[8] == '0' { '1' } else { '0' };
        let vin: String = bad.into_iter().collect();
        let info = decode(&vin, 2026).unwrap();
        assert!(!info.check_digit_valid);
    }

    #[test]
    fn model_year_codes_resolve_within_the_current_cycle() {
        assert_eq!(model_year('K', 2026), Some(2019));
        assert_eq!(model_year('A', 2026), Some(2010));
        assert_eq!(model_year('R', 2026), Some(2024));
        assert_eq!(model_year('S', 2026), Some(2025));
        assert_eq!(model_year('T', 2026), Some(2026));
        assert_eq!(
            model_year('V', 2026),
            Some(2027),
            "next model year is already in use during the preceding calendar year"
        );
        assert_eq!(model_year('I', 2026), None, "not a valid code");
    }

    #[test]
    fn shape_violations_are_rejected() {
        assert!(validate("TOOSHORT").is_err());
        assert!(validate("1FT7W2BT9KEC0000!").is_err());
        assert!(validate("1FT7W2BT9KEC0000I").is_err(), "I is excluded by ISO 3779");
        assert!(validate("1FT7W2BT9KEC0000O").is_err());
        assert!(validate("1FT7W2BT9KEC0000Q").is_err());
    }

    #[test]
    fn unknown_wmis_yield_none_rather_than_a_guess() {
        let info = decode("ZZZ7W2BT9KEC00001", 2026).unwrap();
        assert_eq!(info.manufacturer, None);
        assert_eq!(info.region.as_deref(), Some("Europe"));
    }

    #[test]
    fn sections_are_split_at_the_standard_boundaries() {
        let info = decode(SIM_VIN, 2026).unwrap();
        assert_eq!(info.wmi.len(), 3);
        assert_eq!(info.vds.len(), 6);
        assert_eq!(info.vis.len(), 8);
        assert_eq!(format!("{}{}{}", info.wmi, info.vds, info.vis), SIM_VIN);
    }
}
