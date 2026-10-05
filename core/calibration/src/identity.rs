//! What a module said about the software it runs.
//!
//! Every identifier is kept with where it came from and the bytes it was read
//! out of, and every identifier asked for and not given is kept too, with the
//! refusal. A manufacturer exposes whichever of these it chooses, so nothing
//! here is required: an identity with one field is still an identity.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One thing that can be known about a module, its software or its vehicle.
///
/// Named for what the thing is, not for one manufacturer's word for it. A
/// manufacturer's own identifier that fits none of these is not forced into
/// one: it stays in the raw evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The vehicle identification number.
    Vin,
    /// The company that built the vehicle, as the VIN's first three characters say.
    Manufacturer,
    /// The brand.
    Make,
    /// The model.
    Model,
    /// The model year.
    ModelYear,
    /// The engine.
    Engine,
    /// The transmission.
    Transmission,
    /// What the module is: its own name for itself.
    ModuleName,
    /// The module's hardware number.
    HardwareNumber,
    /// The hardware's version.
    HardwareVersion,
    /// The part number the module is sold under.
    PartNumber,
    /// The software's number.
    SoftwareNumber,
    /// The software's version.
    SoftwareVersion,
    /// The calibration identification: which calibration is loaded.
    CalibrationId,
    /// The calibration verification number: a checksum the module computes
    /// over the calibration it is running.
    CalibrationVerificationNumber,
    /// A program identifier, where a manufacturer keeps one apart from the
    /// calibration identification.
    ProgramId,
    /// A strategy identifier.
    StrategyId,
    /// A ROM identifier.
    RomId,
    /// The boot software's identification.
    BootSoftwareId,
    /// The application data's identification.
    ApplicationDataId,
    /// Who supplied the module.
    Supplier,
}

impl Field {
    /// The name a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Field::Vin => "VIN",
            Field::Manufacturer => "Manufacturer",
            Field::Make => "Make",
            Field::Model => "Model",
            Field::ModelYear => "Model year",
            Field::Engine => "Engine",
            Field::Transmission => "Transmission",
            Field::ModuleName => "Module",
            Field::HardwareNumber => "Hardware number",
            Field::HardwareVersion => "Hardware version",
            Field::PartNumber => "Part number",
            Field::SoftwareNumber => "Software number",
            Field::SoftwareVersion => "Software version",
            Field::CalibrationId => "Calibration ID",
            Field::CalibrationVerificationNumber => "Calibration verification number",
            Field::ProgramId => "Program ID",
            Field::StrategyId => "Strategy ID",
            Field::RomId => "ROM ID",
            Field::BootSoftwareId => "Boot software ID",
            Field::ApplicationDataId => "Application data ID",
            Field::Supplier => "Supplier",
        }
    }

    /// True for the fields that describe the vehicle and not the module.
    pub fn is_about_the_vehicle(self) -> bool {
        matches!(
            self,
            Field::Vin
                | Field::Manufacturer
                | Field::Make
                | Field::Model
                | Field::ModelYear
                | Field::Engine
                | Field::Transmission
        )
    }
}

/// Where an identifier came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentitySource {
    /// An OBD-II service 09 information type, the legislated way a module
    /// reports its VIN (02), calibration identification (04), calibration
    /// verification number (06) and name (0A).
    ObdInfoType {
        /// The information type asked for.
        info_type: u8,
    },
    /// A UDS ReadDataByIdentifier, service 0x22.
    UdsDid {
        /// The data identifier asked for.
        did: u16,
    },
    /// Read out of the VIN's own structure.
    VinStructure,
    /// An outside lookup the person asked for.
    Lookup {
        /// Which one, for instance `NHTSA vPIC`.
        name: String,
    },
}

impl IdentitySource {
    /// How it was asked, in words.
    pub fn describe(&self) -> String {
        match self {
            IdentitySource::ObdInfoType { info_type } => {
                format!("OBD-II service 09, information type {info_type:02X}")
            }
            IdentitySource::UdsDid { did } => {
                format!("UDS ReadDataByIdentifier, identifier {did:04X}")
            }
            IdentitySource::VinStructure => String::from("the VIN's own structure"),
            IdentitySource::Lookup { name } => format!("looked up: {name}"),
        }
    }
}

/// An identifier, with what stands behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identified {
    /// The value, as text.
    pub value: String,
    /// How it was obtained.
    pub source: IdentitySource,
    /// The flight-recorder row holding the reply it was read from, when it
    /// was read from a vehicle.
    pub evidence_ref: Option<i64>,
    /// The bytes it was decoded from, as hex.
    pub raw_hex: Option<String>,
}

/// An identifier that was asked for and not given.
///
/// Kept because "the module would not say" and "nobody asked" are different,
/// and only the first is a fact about the vehicle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unanswered {
    /// What was asked for, when the request maps to one thing.
    pub field: Option<Field>,
    /// How it was asked.
    pub source: IdentitySource,
    /// Why there is no value: the module's refusal in words, that nothing
    /// answered, or that the reply could not be read as text.
    pub reason: String,
    /// The flight-recorder row holding the reply, when there was one.
    pub evidence_ref: Option<i64>,
    /// The reply's bytes as hex, when there was one. A reply that could not
    /// be parsed is kept here whole and no value is made up from it.
    pub raw_hex: Option<String>,
}

/// Everything one module said about itself and its software, normalised.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationIdentity {
    /// The module's key in this app, for instance `ECU_18DAF110`.
    pub module_key: String,
    /// The address it answers on.
    pub address: String,
    /// The diagnostic protocol it was reached on.
    pub protocol: Option<String>,
    /// What is known, by field. A module may report several of one thing: an
    /// engine controller commonly holds more than one calibration.
    pub fields: BTreeMap<Field, Vec<Identified>>,
    /// What was asked for and not given.
    pub unanswered: Vec<Unanswered>,
}

impl CalibrationIdentity {
    /// An identity for a module, with nothing known yet.
    pub fn for_module(module_key: impl Into<String>, address: impl Into<String>) -> Self {
        CalibrationIdentity {
            module_key: module_key.into(),
            address: address.into(),
            ..Default::default()
        }
    }

    /// Record an identifier. An empty value is not recorded: a module that
    /// answers with blanks has not said anything.
    pub fn record(&mut self, field: Field, identified: Identified) {
        if identified.value.trim().is_empty() {
            return;
        }
        let known = self.fields.entry(field).or_default();
        if !known.iter().any(|k| k.value == identified.value && k.source == identified.source) {
            known.push(identified);
        }
    }

    /// Every value known for a field.
    pub fn values(&self, field: Field) -> Vec<&str> {
        self.fields
            .get(&field)
            .map(|v| v.iter().map(|i| i.value.as_str()).collect())
            .unwrap_or_default()
    }

    /// The first value known for a field.
    pub fn first(&self, field: Field) -> Option<&str> {
        self.fields.get(&field).and_then(|v| v.first()).map(|i| i.value.as_str())
    }

    /// True when the module reported anything that identifies its software.
    pub fn names_its_software(&self) -> bool {
        [
            Field::CalibrationId,
            Field::ProgramId,
            Field::SoftwareNumber,
            Field::RomId,
            Field::StrategyId,
        ]
        .iter()
        .any(|f| self.fields.get(f).is_some_and(|v| !v.is_empty()))
    }
}

/// How two identifiers are compared: as written, ignoring case and the spaces
/// and padding a module adds around them. Nothing else is forgiven. `C120` and
/// `C130` are different calibrations.
pub fn same_identifier(a: &str, b: &str) -> bool {
    let tidy =
        |s: &str| s.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_ascii_uppercase();
    let (a, b) = (tidy(a), tidy(b));
    !a.is_empty() && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_service_09(value: &str) -> Identified {
        Identified {
            value: value.into(),
            source: IdentitySource::ObdInfoType { info_type: 0x04 },
            evidence_ref: Some(52),
            raw_hex: Some("33373830352d354d522d43313230".into()),
        }
    }

    #[test]
    fn an_identity_keeps_several_of_one_thing_and_no_blanks() {
        let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        id.record(Field::CalibrationId, from_service_09("37805-5MR-C120"));
        id.record(Field::CalibrationId, from_service_09("37805-5MR-C120"));
        id.record(Field::CalibrationId, from_service_09("SECOND-CAL"));
        id.record(Field::CalibrationId, from_service_09("   "));

        assert_eq!(id.values(Field::CalibrationId), vec!["37805-5MR-C120", "SECOND-CAL"]);
        assert!(id.names_its_software());
        assert!(id.first(Field::HardwareNumber).is_none());
    }

    #[test]
    fn a_name_alone_does_not_identify_software() {
        let mut id = CalibrationIdentity::for_module("ECU_18DAF10B", "18DAF10B");
        id.record(
            Field::ModuleName,
            Identified {
                value: "ECM-EngineControl".into(),
                source: IdentitySource::ObdInfoType { info_type: 0x0A },
                evidence_ref: None,
                raw_hex: None,
            },
        );
        assert!(!id.names_its_software());
    }

    #[test]
    fn identifiers_are_compared_as_written() {
        assert!(same_identifier("37805-5MR-C120", " 37805-5mr-c120\0\0"));
        assert!(!same_identifier("37805-5MR-C120", "37805-5MR-C130"));
        assert!(!same_identifier("37805-5MR-C120", "378055MRC120"));
        assert!(!same_identifier("", ""));
    }
}
