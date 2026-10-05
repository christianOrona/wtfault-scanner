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

    /// Everything that can be known about a module and its software, as
    /// opposed to the vehicle it is in.
    pub const ABOUT_THE_MODULE: [Field; 14] = [
        Field::ModuleName,
        Field::HardwareNumber,
        Field::HardwareVersion,
        Field::PartNumber,
        Field::SoftwareNumber,
        Field::SoftwareVersion,
        Field::CalibrationId,
        Field::CalibrationVerificationNumber,
        Field::ProgramId,
        Field::StrategyId,
        Field::RomId,
        Field::BootSoftwareId,
        Field::ApplicationDataId,
        Field::Supplier,
    ];

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

/// Why an identifier was not given. Each is a different fact, and they are
/// kept apart: a module that has no such identifier, one that would not say,
/// one that never answered and one that was never asked are four different
/// vehicles to somebody deciding what to try next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotGiven {
    /// The module answered that it has no such identifier, or does not
    /// support asking for one.
    NotSupported,
    /// The module refused for another reason: it may have the identifier and
    /// would not give it as it was asked.
    Refused,
    /// Nothing came back.
    NoAnswer,
    /// Something came back and could not be read as the identifier. The
    /// reply is kept whole.
    Unreadable,
    /// The module answered with nothing in it.
    Empty,
    /// It was not asked, for the reason given.
    NotAsked,
    /// A record written before the reason was kept as more than words. Its
    /// words still stand.
    #[default]
    Unspecified,
}

/// Where one thing about a module stands, in a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Availability {
    /// The module reported it.
    Available,
    /// The module said it has none, to every request that could have read it.
    NotSupported,
    /// The module refused to give it.
    Refused,
    /// It was asked for and the reading failed: no answer, an empty answer,
    /// or one that could not be read.
    ReadFailed,
    /// It was not read: this build has no request that reads it, or the
    /// requests that could were not all made. Nothing is known either way.
    NotRead,
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
    /// Why there is no value, as one of a fixed set.
    #[serde(default)]
    pub state: NotGiven,
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

    /// Where each thing about the module stands: reported, not supported,
    /// refused, failed, or never read. Every field is listed, so that what
    /// was never asked for is said and not merely absent.
    pub fn availability(&self) -> BTreeMap<Field, Availability> {
        Field::ABOUT_THE_MODULE
            .into_iter()
            .map(|field| {
                let asked: Vec<NotGiven> = self
                    .unanswered
                    .iter()
                    .filter(|u| u.field == Some(field))
                    .map(|u| u.state)
                    .collect();
                let has = |states: &[NotGiven]| asked.iter().any(|s| states.contains(s));
                let state = if self.fields.get(&field).is_some_and(|v| !v.is_empty()) {
                    Availability::Available
                } else if has(&[NotGiven::Refused]) {
                    Availability::Refused
                } else if has(&[
                    NotGiven::NoAnswer,
                    NotGiven::Unreadable,
                    NotGiven::Empty,
                    NotGiven::Unspecified,
                ]) {
                    Availability::ReadFailed
                } else if asked.is_empty() || has(&[NotGiven::NotAsked]) {
                    // One way of asking coming back "not supported" does not
                    // settle it while another was never tried.
                    Availability::NotRead
                } else {
                    Availability::NotSupported
                };
                (field, state)
            })
            .collect()
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
    fn what_is_not_known_is_not_known_in_five_different_ways() {
        let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        id.record(Field::CalibrationId, from_service_09("37805-5MR-C120"));
        let mut not_given = |field: Field, did: u16, state: NotGiven| {
            id.unanswered.push(Unanswered {
                field: Some(field),
                source: IdentitySource::UdsDid { did },
                state,
                reason: String::new(),
                evidence_ref: None,
                raw_hex: None,
            });
        };
        not_given(Field::HardwareNumber, 0xF191, NotGiven::NotSupported);
        not_given(Field::HardwareNumber, 0xF192, NotGiven::NotSupported);
        not_given(Field::PartNumber, 0xF187, NotGiven::Refused);
        not_given(Field::Supplier, 0xF18A, NotGiven::NoAnswer);
        // Asked one way and told no; the other way never tried.
        not_given(Field::SoftwareVersion, 0xF189, NotGiven::NotSupported);
        not_given(Field::SoftwareVersion, 0xF195, NotGiven::NotAsked);

        let stands = id.availability();
        assert_eq!(stands[&Field::CalibrationId], Availability::Available);
        assert_eq!(stands[&Field::HardwareNumber], Availability::NotSupported);
        assert_eq!(stands[&Field::PartNumber], Availability::Refused);
        assert_eq!(stands[&Field::Supplier], Availability::ReadFailed);
        assert_eq!(stands[&Field::SoftwareVersion], Availability::NotRead);
        // Nothing in this build asks for a program identifier at all.
        assert_eq!(stands[&Field::ProgramId], Availability::NotRead);
        // Every module field is listed, and nothing about the vehicle.
        assert_eq!(stands.len(), Field::ABOUT_THE_MODULE.len());
        assert!(stands.keys().all(|f| !f.is_about_the_vehicle()));
        assert_eq!(
            serde_json::to_value(Availability::NotSupported).unwrap(),
            serde_json::json!("NOT_SUPPORTED")
        );
    }

    /// An identity stored before the reason was typed still reads.
    #[test]
    fn an_unanswered_identifier_from_an_earlier_record_still_reads() {
        let old = r#"{ "field": "hardware_number", "source": { "kind": "uds_did", "did": 61841 },
                       "reason": "the module refused: request out of range",
                       "evidence_ref": 1034, "raw_hex": "7f2231" }"#;
        let u: Unanswered = serde_json::from_str(old).unwrap();
        assert_eq!(u.state, NotGiven::Unspecified);
        assert!(u.reason.contains("refused"));
    }

    #[test]
    fn identifiers_are_compared_as_written() {
        assert!(same_identifier("37805-5MR-C120", " 37805-5mr-c120\0\0"));
        assert!(!same_identifier("37805-5MR-C120", "37805-5MR-C130"));
        assert!(!same_identifier("37805-5MR-C120", "378055MRC120"));
        assert!(!same_identifier("", ""));
    }
}
