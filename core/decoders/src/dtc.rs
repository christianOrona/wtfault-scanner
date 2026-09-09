//! DTC decoding and description lookup.
//!
//! Structural decoding (system letter, standard vs manufacturer-specific,
//! subsystem group) is derived from the code itself and is always reliable.
//! *Descriptions* come from a data file and are only attached when the file
//! actually has the code. A missing description stays missing — the core never
//! invents one, and a code with no entry is reported as unverified.

use aim_types::{AimError, AimResult, ErrorCode, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The system a DTC belongs to, from its leading letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DtcSystem {
    /// P — powertrain.
    Powertrain,
    /// C — chassis.
    Chassis,
    /// B — body.
    Body,
    /// U — network / vehicle integration.
    Network,
}

impl DtcSystem {
    /// The code's leading letter.
    pub fn letter(&self) -> char {
        match self {
            DtcSystem::Powertrain => 'P',
            DtcSystem::Chassis => 'C',
            DtcSystem::Body => 'B',
            DtcSystem::Network => 'U',
        }
    }

    /// Standard-language group name.
    pub fn label(&self) -> &'static str {
        match self {
            DtcSystem::Powertrain => "Powertrain",
            DtcSystem::Chassis => "Chassis",
            DtcSystem::Body => "Body",
            DtcSystem::Network => "Network / vehicle integration",
        }
    }
}

/// Everything known about one DTC string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DtcInfo {
    /// The code, normalised to uppercase.
    pub code: String,
    /// System group.
    pub system: DtcSystem,
    /// False when the first digit is 1 or 3, i.e. manufacturer-specific.
    /// A manufacturer-specific code has no generic meaning and this project
    /// will not guess one.
    pub is_generic: bool,
    /// Description from the data file, when the code is present in it.
    pub description: Option<String>,
    /// `verified` only when a description came from the table.
    pub verification: VerificationStatus,
    /// Structural summary, always available.
    pub structural_summary: String,
}

impl DtcInfo {
    /// True when a description may be shown to a user as fact.
    pub fn has_trustworthy_description(&self) -> bool {
        self.description.is_some() && self.verification.is_trustworthy()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DtcEntry {
    description: String,
    #[serde(default)]
    verification: VerificationStatus,
}

#[derive(Debug, Clone, Deserialize)]
struct DtcFile {
    version: u32,
    source: String,
    codes: BTreeMap<String, DtcEntry>,
}

/// DTC description catalog loaded from data files.
#[derive(Debug, Clone, Default)]
pub struct DtcCatalog {
    codes: BTreeMap<String, DtcEntry>,
    version: u32,
    sources: Vec<String>,
}

impl DtcCatalog {
    /// An empty catalog. Every lookup still returns structural information.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the generic SAE table embedded in the binary.
    pub fn generic_sae() -> AimResult<DtcCatalog> {
        let mut c = DtcCatalog::new();
        c.load_yaml(include_str!("../../../vehicle-profiles/generic-obd/dtc/generic-sae.yaml"))?;
        Ok(c)
    }

    /// Merge a YAML DTC data file.
    pub fn load_yaml(&mut self, yaml: &str) -> AimResult<()> {
        let file: DtcFile = serde_yaml_ng::from_str(yaml).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("could not parse DTC data file: {e}"),
            )
        })?;
        for (code, entry) in file.codes {
            let normalised = code.trim().to_ascii_uppercase();
            validate_code(&normalised)?;
            self.codes.insert(normalised, entry);
        }
        self.version = self.version.max(file.version);
        self.sources.push(file.source);
        Ok(())
    }

    /// Number of described codes.
    pub fn len(&self) -> usize {
        self.codes.len()
    }

    /// True when no descriptions are loaded.
    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    /// Catalog version, recorded in provenance.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The data-file sources merged in.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Look up a code. Always succeeds for a structurally valid code; the
    /// description is `None` when the catalog does not have it.
    pub fn describe(&self, code: &str) -> AimResult<DtcInfo> {
        let code = code.trim().to_ascii_uppercase();
        validate_code(&code)?;
        let system = match code.as_bytes()[0] {
            b'P' => DtcSystem::Powertrain,
            b'C' => DtcSystem::Chassis,
            b'B' => DtcSystem::Body,
            _ => DtcSystem::Network,
        };
        let first_digit = code.as_bytes()[1] - b'0';
        // Digits 0 and 2 are SAE-defined; 1 and 3 are manufacturer-specific.
        let is_generic = first_digit == 0 || first_digit == 2;
        let entry = self.codes.get(&code);
        let structural_summary = if is_generic {
            format!("{} — SAE standard code", system.label())
        } else {
            format!("{} — manufacturer-specific code", system.label())
        };
        Ok(DtcInfo {
            code,
            system,
            is_generic,
            description: entry.map(|e| e.description.clone()),
            verification: entry.map_or(VerificationStatus::Unverified, |e| e.verification),
            structural_summary,
        })
    }
}

fn validate_code(code: &str) -> AimResult<()> {
    let bytes = code.as_bytes();
    let ok = bytes.len() == 5
        && matches!(bytes[0], b'P' | b'C' | b'B' | b'U')
        && bytes[1].is_ascii_digit()
        && (bytes[1] - b'0') <= 3
        && bytes[2..].iter().all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(AimError::new(
            ErrorCode::BadRequest,
            format!(
                "{code:?} is not a valid DTC (expected P/C/B/U, a digit 0-3, then 3 hex digits)"
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> DtcCatalog {
        DtcCatalog::generic_sae().expect("embedded DTC table must load")
    }

    #[test]
    fn the_shipped_table_loads() {
        let c = catalog();
        assert!(c.len() > 50, "loaded {} codes", c.len());
        assert!(!c.is_empty());
        assert!(c.version() >= 1);
        assert_eq!(c.sources().len(), 1);
    }

    #[test]
    fn known_codes_get_a_verified_description() {
        let c = catalog();
        let info = c.describe("P0401").unwrap();
        assert_eq!(
            info.description.as_deref(),
            Some("Exhaust gas recirculation flow insufficient detected")
        );
        assert!(info.is_generic);
        assert_eq!(info.system, DtcSystem::Powertrain);
        assert!(info.has_trustworthy_description());
    }

    #[test]
    fn the_dpf_scenario_codes_are_described() {
        let c = catalog();
        for code in ["P2463", "P242F", "P2002"] {
            let info = c.describe(code).unwrap();
            assert!(info.has_trustworthy_description(), "{code} should have a description");
            assert!(info.is_generic, "{code} is in the SAE P2xxx range");
        }
    }

    #[test]
    fn unknown_codes_get_structure_but_never_an_invented_description() {
        let c = catalog();
        let info = c.describe("P0999").unwrap();
        assert_eq!(info.description, None);
        assert_eq!(info.verification, VerificationStatus::Unverified);
        assert!(!info.has_trustworthy_description());
        assert_eq!(info.structural_summary, "Powertrain — SAE standard code");
    }

    #[test]
    fn manufacturer_specific_codes_are_labelled_as_such() {
        let c = catalog();
        let info = c.describe("P1234").unwrap();
        assert!(!info.is_generic);
        assert_eq!(info.structural_summary, "Powertrain — manufacturer-specific code");
        assert_eq!(info.description, None);
        let info = c.describe("P3000").unwrap();
        assert!(!info.is_generic);
    }

    #[test]
    fn every_system_letter_is_classified() {
        let c = catalog();
        assert_eq!(c.describe("C0321").unwrap().system, DtcSystem::Chassis);
        assert_eq!(c.describe("B0123").unwrap().system, DtcSystem::Body);
        assert_eq!(c.describe("U0100").unwrap().system, DtcSystem::Network);
        assert_eq!(
            c.describe("U0100").unwrap().description.as_deref(),
            Some("Lost communication with ECM/PCM A")
        );
    }

    #[test]
    fn hex_digits_after_the_first_are_accepted() {
        let c = catalog();
        let info = c.describe("P242F").unwrap();
        assert!(info.has_trustworthy_description());
        assert!(c.describe("U11FF").is_ok());
    }

    #[test]
    fn lookups_are_case_insensitive_and_trimmed() {
        let c = catalog();
        assert_eq!(c.describe(" p0401 ").unwrap().code, "P0401");
    }

    #[test]
    fn malformed_codes_are_rejected() {
        let c = catalog();
        for bad in ["", "P040", "P04011", "X0401", "P9401", "P04G1"] {
            assert!(c.describe(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn an_empty_catalog_still_decodes_structure() {
        let c = DtcCatalog::new();
        let info = c.describe("P0401").unwrap();
        assert_eq!(info.description, None);
        assert_eq!(info.system, DtcSystem::Powertrain);
        assert!(info.is_generic);
    }
}
