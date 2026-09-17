//! Parses NHTSA vPIC's DecodeVinValues JSON reply into vehicle information.
//!
//! This module handles the parsing of VIN decode responses from the NHTSA
//! vPIC API, extracting key vehicle details like make, model, year, engine
//! specifications, and fuel type.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// Parsed results from NHTSA vPIC's DecodeVinValues endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VpicDecode {
    /// The vehicle make.
    pub make: Option<String>,
    /// The vehicle model.
    pub model: Option<String>,
    /// The vehicle model year.
    pub model_year: Option<u16>,
    /// The vehicle engine description.
    pub engine: Option<String>,
    /// The vehicle fuel type(s).
    pub fuel: Option<String>,
    /// A warning message if the decode had issues.
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VpicResponse {
    #[serde(rename = "Results")]
    results: Vec<VpicResult>,
}

#[derive(Debug, Deserialize)]
struct VpicResult {
    #[serde(rename = "Make")]
    make: Option<String>,
    #[serde(rename = "Model")]
    model: Option<String>,
    #[serde(rename = "ModelYear")]
    model_year: Option<String>,
    #[serde(rename = "DisplacementL")]
    displacement_l: Option<String>,
    #[serde(rename = "EngineCylinders")]
    engine_cylinders: Option<String>,
    #[serde(rename = "EngineConfiguration")]
    engine_configuration: Option<String>,
    #[serde(rename = "FuelTypePrimary")]
    fuel_type_primary: Option<String>,
    #[serde(rename = "FuelTypeSecondary")]
    fuel_type_secondary: Option<String>,
    #[serde(rename = "ErrorCode")]
    error_code: Option<String>,
    #[serde(rename = "ErrorText")]
    error_text: Option<String>,
}

fn parse_engine(
    displacement_l: &Option<String>,
    engine_cylinders: &Option<String>,
    engine_configuration: &Option<String>,
) -> Option<String> {
    let mut parts = Vec::new();

    // Parse displacement part
    if let Some(displ) = displacement_l {
        if !displ.is_empty() && displ != "Not Applicable" {
            if let Ok(val) = displ.parse::<f64>() {
                parts.push(format!("{:.1} L", val));
            }
        }
    }

    // Parse cylinder part
    let mut cylinders = None;
    if let Some(cyl) = engine_cylinders {
        if !cyl.is_empty() && cyl != "Not Applicable" {
            if let Ok(val) = cyl.parse::<u8>() {
                cylinders = Some(val);
            }
        }
    }

    // Determine layout letter
    let layout_letter = if let Some(config) = engine_configuration {
        if config == "V-Shaped" {
            Some('V')
        } else if config == "In-Line" {
            Some('I')
        } else if config.to_lowercase().contains("opposed") {
            Some('H')
        } else {
            None
        }
    } else {
        None
    };

    // Build cylinder part
    if let Some(cyl_count) = cylinders {
        if let Some(letter) = layout_letter {
            parts.push(format!("{}{}", letter, cyl_count));
        } else {
            parts.push(format!("{}-cylinder", cyl_count));
        }
    } else if let Some(letter) = layout_letter {
        // If we have a layout but no cylinder count, just return the layout
        // This case is not expected based on the rules, but included for completeness
        parts.push(letter.to_string());
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn parse_fuel(
    fuel_type_primary: &Option<String>,
    fuel_type_secondary: &Option<String>,
) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(primary) = fuel_type_primary {
        if !primary.is_empty() && primary != "Not Applicable" {
            parts.push(primary.trim().to_string());
        }
    }

    if let Some(secondary) = fuel_type_secondary {
        if !secondary.is_empty() && secondary != "Not Applicable" {
            parts.push(secondary.trim().to_string());
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

fn parse_warning(error_code: &Option<String>, error_text: &Option<String>) -> Option<String> {
    if let Some(code) = error_code {
        if code != "0" && !code.is_empty() {
            if let Some(text) = error_text {
                if !text.is_empty() {
                    Some(text.trim().to_string())
                } else {
                    Some(code.trim().to_string())
                }
            } else {
                Some(code.trim().to_string())
            }
        } else {
            None
        }
    } else {
        None
    }
}

/// Parse a NHTSA vPIC DecodeVinValues JSON response into structured data.
///
/// This function takes the full JSON response from the NHTSA vPIC API's
/// DecodeVinValues endpoint and extracts key vehicle information.
///
/// # Arguments
///
/// * `json` - A string slice containing the complete JSON response
///
/// # Returns
///
/// * `AimResult<VpicDecode>` - The parsed vehicle information or an error
///
/// # Errors
///
/// Returns `ErrorCode::DecoderInputInvalid` if:
/// - The input is not valid JSON
/// - There is no Results array
/// - The Results array is empty
pub fn parse_decode_vin_values(json: &str) -> AimResult<VpicDecode> {
    let response: VpicResponse = serde_json::from_str(json)
        .map_err(|_| AimError::new(ErrorCode::DecoderInputInvalid, "Invalid JSON"))?;

    if response.results.is_empty() {
        return Err(AimError::new(ErrorCode::DecoderInputInvalid, "No results in response"));
    }

    let result = &response.results[0];

    // Parse model year
    let model_year = if let Some(year_str) = &result.model_year {
        if !year_str.is_empty() && year_str != "Not Applicable" {
            year_str.parse::<u16>().ok()
        } else {
            None
        }
    } else {
        None
    };

    // Parse engine
    let engine = parse_engine(
        &result.displacement_l,
        &result.engine_cylinders,
        &result.engine_configuration,
    );

    // Parse fuel
    let fuel = parse_fuel(&result.fuel_type_primary, &result.fuel_type_secondary);

    // Parse warning
    let warning = parse_warning(&result.error_code, &result.error_text);

    Ok(VpicDecode {
        make: result
            .make
            .as_ref()
            .filter(|s| !s.trim().is_empty() && s.trim() != "Not Applicable")
            .map(|s| s.trim().to_string()),
        model: result
            .model
            .as_ref()
            .filter(|s| !s.trim().is_empty() && s.trim() != "Not Applicable")
            .map(|s| s.trim().to_string()),
        model_year,
        engine,
        fuel,
        warning,
    })
}

/// The vPIC request that decodes one VIN, e.g.
/// `https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/1FT7W2BT7KEF78036?format=json`.
///
/// Building it validates the VIN first, so nothing malformed is ever sent.
pub fn decode_vin_values_url(vin: &str) -> AimResult<String> {
    let vin = crate::vin::validate(vin)?;

    Ok(format!("https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/{vin}?format=json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ford_f250_2019() {
        let json = include_str!("../tests/fixtures/vpic/ford-f250-2019.json");
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.make, Some("FORD".to_string()));
        assert_eq!(result.model, Some("F-250".to_string()));
        assert_eq!(result.model_year, Some(2019));
        assert_eq!(result.engine, Some("6.7 L V8".to_string()));
        assert_eq!(result.fuel, Some("Diesel".to_string()));
        assert_eq!(result.warning, None);
    }

    #[test]
    fn test_mazda3_2019() {
        let json = include_str!("../tests/fixtures/vpic/mazda3-2019.json");
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.make, Some("MAZDA".to_string()));
        assert_eq!(result.model, Some("Mazda3".to_string()));
        assert_eq!(result.model_year, Some(2019));
        assert_eq!(result.engine, Some("2.5 L I4".to_string()));
        assert_eq!(result.fuel, Some("Gasoline".to_string()));
        assert_eq!(result.warning, None);
    }

    #[test]
    fn test_invalid_vin() {
        let json = include_str!("../tests/fixtures/vpic/invalid-vin.json");
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.make, None);
        assert_eq!(result.model, None);
        assert_eq!(result.model_year, None);
        assert_eq!(result.engine, None);
        assert_eq!(result.fuel, None);
        assert!(result.warning.is_some());
        assert!(result.warning.as_ref().unwrap().starts_with("1 - Check Digit"));
    }

    #[test]
    fn test_invalid_json() {
        let result = parse_decode_vin_values("not json");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::DecoderInputInvalid);
    }

    #[test]
    fn test_empty_results() {
        let result = parse_decode_vin_values(r#"{"Results": []}"#);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::DecoderInputInvalid);
    }

    #[test]
    fn test_missing_results() {
        let result = parse_decode_vin_values(r#"{"Count": 0}"#);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::DecoderInputInvalid);
    }

    #[test]
    fn test_engine_combinations() {
        // Test displacement + cylinders + configuration
        let json = r#"{
            "Results": [{
                "DisplacementL": "3.0",
                "EngineCylinders": "6",
                "EngineConfiguration": "Horizontally opposed (boxer)"
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.engine, Some("3.0 L H6".to_string()));

        // Test cylinders only
        let json = r#"{
            "Results": [{
                "DisplacementL": "",
                "EngineCylinders": "4",
                "EngineConfiguration": ""
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.engine, Some("4-cylinder".to_string()));

        // Test displacement only
        let json = r#"{
            "Results": [{
                "DisplacementL": "1.5",
                "EngineCylinders": "Not Applicable",
                "EngineConfiguration": ""
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.engine, Some("1.5 L".to_string()));

        // Test displacement only (integer)
        let json = r#"{
            "Results": [{
                "DisplacementL": "2",
                "EngineCylinders": "",
                "EngineConfiguration": ""
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.engine, Some("2.0 L".to_string()));
    }

    #[test]
    fn test_empty_result() {
        let json = r#"{"Results": [{}]}"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.make, None);
        assert_eq!(result.model, None);
        assert_eq!(result.model_year, None);
        assert_eq!(result.engine, None);
        assert_eq!(result.fuel, None);
        assert_eq!(result.warning, None);
    }

    #[test]
    fn test_fuel_combinations() {
        // Test primary and secondary fuel
        let json = r#"{
            "Results": [{
                "FuelTypePrimary": "Gasoline",
                "FuelTypeSecondary": "Electric"
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.fuel, Some("Gasoline / Electric".to_string()));

        // Test only primary
        let json = r#"{
            "Results": [{
                "FuelTypePrimary": "Diesel",
                "FuelTypeSecondary": ""
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.fuel, Some("Diesel".to_string()));

        // Test only secondary
        let json = r#"{
            "Results": [{
                "FuelTypePrimary": "",
                "FuelTypeSecondary": "Hybrid"
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.fuel, Some("Hybrid".to_string()));

        // Test none
        let json = r#"{
            "Results": [{
                "FuelTypePrimary": "",
                "FuelTypeSecondary": ""
            }]
        }"#;
        let result = parse_decode_vin_values(json).unwrap();
        assert_eq!(result.fuel, None);
    }
}
