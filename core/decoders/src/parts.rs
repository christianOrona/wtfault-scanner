//! The kind of part a finding is about: what one looks like, and roughly
//! where to look for it.
//!
//! # What this can say, and what it cannot
//!
//! "Your catalytic converter is failing" leaves somebody who has never seen
//! one with a name and nothing to look for. What a catalytic converter looks
//! like, and that it is the first large canister in the exhaust, is true of
//! every vehicle that has one, because it follows from what the part is.
//!
//! What is not known is where it is on *this* vehicle. This project has part
//! locations and pictures for no vehicle. So a [`Part`] describes a kind of
//! part and carries a [`Region`], never a position, and the interface draws
//! a generic one and says so.
//!
//! # What points at a part
//!
//! Only what a published standard assigns to one kind of part: ranges of the
//! generic SAE J2012 trouble codes, and SAE J1979 service 06 monitor ids. A
//! misfire is not a spark plug and a lean mixture is not an air flow sensor,
//! so codes like those name no part and keep only their zone. A
//! manufacturer-specific code is never matched, whatever its number.
//!
//! The table is data, in `vehicle-profiles/generic-obd/parts.yaml`.

use crate::regions::Region;
use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// One kind of part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// Stable id, e.g. `oxygen_sensor`.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// The area of a vehicle it is in.
    pub zone: Region,
    /// What one looks like, for somebody who has never seen one.
    pub looks_like: String,
    /// Where to look, as far as that follows from what the part is for.
    pub find_it: String,
    /// The drawing the interface uses. The part's own id unless it shares
    /// another's.
    #[serde(default)]
    pub sketch: Option<String>,
    /// Generic trouble codes about this part, singly or as inclusive ranges.
    #[serde(default, skip_serializing)]
    codes: Vec<String>,
    /// Service 06 monitor ids about this part, in hex, singly or as ranges.
    #[serde(default, skip_serializing)]
    monitors: Vec<String>,
}

/// Roughly where a finding is, and what kind of part it is about.
///
/// Either may be missing. A fuel trim code is in the engine bay and about no
/// one part; a network code is about nothing that has a place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The area of the vehicle. `unknown` when nothing places it.
    pub region: Region,
    /// The kind of part, when the standard names one.
    pub part: Option<Part>,
}

impl Placement {
    /// A finding nothing places.
    pub fn unknown() -> Placement {
        Placement { region: Region::Unknown, part: None }
    }

    /// Whether there is anything to show.
    pub fn is_known(&self) -> bool {
        self.region.is_known() || self.part.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct PartsFile {
    parts: Vec<Part>,
}

/// Every kind of part this build can describe.
#[derive(Debug, Clone, Default)]
pub struct PartCatalog {
    parts: Vec<Part>,
    /// Inclusive code ranges, as (system letter, first, last, part index).
    codes: Vec<(char, u16, u16, usize)>,
    /// Inclusive monitor id ranges, as (first, last, part index).
    monitors: Vec<(u8, u8, usize)>,
}

impl PartCatalog {
    /// The table embedded in the binary.
    pub fn generic_obd() -> AimResult<PartCatalog> {
        PartCatalog::from_yaml(include_str!("../../../vehicle-profiles/generic-obd/parts.yaml"))
    }

    /// Load a table, refusing one that would point two parts at one code.
    ///
    /// An overlap would make which part is shown depend on the order of a
    /// file, and showing somebody the wrong part is worse than showing none.
    pub fn from_yaml(yaml: &str) -> AimResult<PartCatalog> {
        let invalid = |message: String| AimError::new(ErrorCode::DecoderInputInvalid, message);
        let file: PartsFile = serde_yaml_ng::from_str(yaml)
            .map_err(|e| invalid(format!("could not parse parts file: {e}")))?;

        let mut catalog = PartCatalog::default();
        for (index, part) in file.parts.iter().enumerate() {
            if file.parts[..index].iter().any(|p| p.id == part.id) {
                return Err(invalid(format!("part {:?} is defined twice", part.id)));
            }
            if !part.zone.is_known() {
                return Err(invalid(format!("part {:?} has no zone", part.id)));
            }
            for spec in &part.codes {
                let (letter, first, last) = code_range(spec).ok_or_else(|| {
                    invalid(format!("part {:?}: bad code range {spec:?}", part.id))
                })?;
                if let Some((_, _, _, other)) = catalog
                    .codes
                    .iter()
                    .find(|(l, a, b, _)| *l == letter && first <= *b && *a <= last)
                {
                    return Err(invalid(format!(
                        "codes {spec} are given to both {:?} and {:?}",
                        file.parts[*other].id, part.id
                    )));
                }
                catalog.codes.push((letter, first, last, index));
            }
            for spec in &part.monitors {
                let (first, last) = monitor_range(spec).ok_or_else(|| {
                    invalid(format!("part {:?}: bad monitor range {spec:?}", part.id))
                })?;
                if let Some((_, _, other)) =
                    catalog.monitors.iter().find(|(a, b, _)| first <= *b && *a <= last)
                {
                    return Err(invalid(format!(
                        "monitors {spec} are given to both {:?} and {:?}",
                        file.parts[*other].id, part.id
                    )));
                }
                catalog.monitors.push((first, last, index));
            }
        }
        catalog.parts = file.parts;
        Ok(catalog)
    }

    /// How many kinds of part are described.
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// True when nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Every part, in file order.
    pub fn iter(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter()
    }

    /// The kind of part a trouble code is about, when the standard names one.
    ///
    /// `None` for a manufacturer-specific code: its number means whatever one
    /// manufacturer decided, and matching it against the generic ranges
    /// would be guessing at another company's scheme.
    pub fn for_dtc(&self, code: &str, is_generic: bool) -> Option<&Part> {
        if !is_generic {
            return None;
        }
        let (letter, number) = code_number(code)?;
        self.codes
            .iter()
            .find(|(l, first, last, _)| *l == letter && (*first..=*last).contains(&number))
            .map(|(_, _, _, index)| &self.parts[*index])
    }

    /// The kind of part a service 06 monitor tests, when it tests one.
    pub fn for_monitor(&self, mid: u8) -> Option<&Part> {
        self.monitors
            .iter()
            .find(|(first, last, _)| (*first..=*last).contains(&mid))
            .map(|(_, _, index)| &self.parts[*index])
    }

    /// Where a trouble code's subject is and what kind of part it is.
    ///
    /// The part's own zone when a part is named; otherwise whatever
    /// [`crate::region_for_dtc`] reads from the code's range.
    pub fn place_dtc(&self, code: &str, system: crate::DtcSystem, is_generic: bool) -> Placement {
        match self.for_dtc(code, is_generic) {
            Some(part) => Placement { region: part.zone, part: Some(part.clone()) },
            None => {
                Placement { region: crate::region_for_dtc(code, system, is_generic), part: None }
            }
        }
    }

    /// Where a service 06 monitor's subject is and what kind of part it is.
    pub fn place_monitor(&self, mid: u8) -> Placement {
        match self.for_monitor(mid) {
            Some(part) => Placement { region: part.zone, part: Some(part.clone()) },
            None => Placement { region: region_for_monitor(mid), part: None },
        }
    }
}

/// The zone a service 06 monitor's subject is in, for monitors that test a
/// system rather than one part.
///
/// Valve timing, fuelling and misfire are all at the engine; secondary air
/// and NOx aftertreatment are in the exhaust. An id the standard does not
/// define is not placed.
fn region_for_monitor(mid: u8) -> Region {
    match mid {
        // Variable valve timing, fuel system, misfire.
        0x35..=0x38 | 0x81..=0x84 | 0xA1..=0xAD => Region::EngineBay,
        // Secondary air injection, NOx adsorber and catalyst.
        0x71..=0x74 | 0x90..=0x91 | 0x98..=0x99 => Region::Exhaust,
        _ => Region::Unknown,
    }
}

/// A code as its system letter and its four hex digits.
fn code_number(code: &str) -> Option<(char, u16)> {
    let code = code.trim();
    // A UDS code carries a failure type after a dash: `P0301-00`.
    let base = code.split('-').next().unwrap_or(code);
    let mut chars = base.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let digits = chars.as_str();
    if !matches!(letter, 'P' | 'B' | 'C' | 'U') || digits.len() != 4 {
        return None;
    }
    u16::from_str_radix(digits, 16).ok().map(|n| (letter, n))
}

/// `"P0130-P0167"` or `"P0457"` as an inclusive range within one system.
fn code_range(spec: &str) -> Option<(char, u16, u16)> {
    let (first, last) = match spec.split_once('-') {
        Some((a, b)) => (code_number(a)?, code_number(b)?),
        None => (code_number(spec)?, code_number(spec)?),
    };
    (first.0 == last.0 && first.1 <= last.1).then_some((first.0, first.1, last.1))
}

/// `"39-3D"` or `"85"` as an inclusive range of monitor ids.
fn monitor_range(spec: &str) -> Option<(u8, u8)> {
    let id = |s: &str| u8::from_str_radix(s.trim(), 16).ok();
    let (first, last) = match spec.split_once('-') {
        Some((a, b)) => (id(a)?, id(b)?),
        None => (id(spec)?, id(spec)?),
    };
    (first <= last).then_some((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DtcSystem;

    fn catalog() -> PartCatalog {
        PartCatalog::generic_obd().expect("the shipped parts table must load")
    }

    fn part_for(code: &str) -> Option<String> {
        catalog().for_dtc(code, true).map(|p| p.id.clone())
    }

    /// Codes whose subject is one kind of part, by what the standard calls
    /// each of them.
    #[test]
    fn a_code_about_one_kind_of_part_names_it() {
        for (code, part) in [
            ("P0420", "catalytic_converter"), // catalyst efficiency, bank 1
            ("P0430", "catalytic_converter"), // and bank 2
            ("P0135", "oxygen_sensor"),       // sensor heater circuit
            ("P0141", "oxygen_sensor"),
            ("P0031", "oxygen_sensor"), // heater control circuit
            ("P0401", "egr_valve"),
            ("P0101", "mass_air_flow_sensor"),
            ("P0118", "coolant_temp_sensor"),
            ("P0128", "thermostat"),
            ("P0122", "throttle_body"),
            ("P2135", "throttle_body"),
            ("P0351", "ignition_coil"),
            ("P0201", "fuel_injector"),
            ("P0269", "fuel_injector"), // cylinder 3 contribution, read from a real truck
            ("P0335", "position_sensor"),
            ("P0340", "position_sensor"),
            ("P0299", "turbocharger"),
            ("P0442", "evap_system"),
            ("P0455", "evap_system"),
            ("P0457", "fuel_cap"),
            ("P0463", "fuel_level_sender"),
            ("P0562", "battery"),
            ("P2463", "particulate_filter"),
            ("P242F", "particulate_filter"),
            ("P2002", "particulate_filter"),
            ("P2201", "nox_sensor"),
        ] {
            assert_eq!(part_for(code).as_deref(), Some(part), "{code}");
        }
    }

    /// A misfire is not a spark plug, and a lean mixture is not an air flow
    /// sensor. A code whose cause could be several parts names none.
    #[test]
    fn a_code_that_could_be_several_parts_names_none() {
        for code in ["P0300", "P0303", "P0171", "P0174", "P0601", "P0700", "U0100", "P2457"] {
            assert_eq!(part_for(code), None, "{code}");
        }
        // It keeps its zone, where it has one.
        let misfire = catalog().place_dtc("P0303", DtcSystem::Powertrain, true);
        assert_eq!(misfire.region, Region::EngineBay);
        assert!(misfire.part.is_none() && misfire.is_known());
        assert!(!catalog().place_dtc("U0100", DtcSystem::Network, true).is_known());
    }

    /// A manufacturer's P1450 is not the standard's P0450 with a digit
    /// changed. Nothing is read into its number.
    #[test]
    fn a_manufacturer_code_names_no_part() {
        assert!(catalog().for_dtc("P0420", true).is_some());
        assert!(catalog().for_dtc("P0420", false).is_none());
        assert!(!catalog().place_dtc("P1420", DtcSystem::Powertrain, false).is_known());
    }

    /// The full scan reports three-byte codes, and their first two bytes are
    /// the same code.
    #[test]
    fn a_three_byte_code_is_matched_on_its_base() {
        assert_eq!(part_for("P0420-00").as_deref(), Some("catalytic_converter"));
        assert_eq!(part_for("p0420").as_deref(), Some("catalytic_converter"));
        assert_eq!(part_for("P042"), None);
        assert_eq!(part_for("nonsense"), None);
    }

    /// A part is in its own zone, and where the code ranges place a code at
    /// all they place it in the same one.
    #[test]
    fn a_part_and_its_codes_are_in_the_same_zone() {
        let catalog = catalog();
        for (letter, first, last, index) in &catalog.codes {
            let part = &catalog.parts[*index];
            for number in [*first, *last] {
                let code = format!("{letter}{number:04X}");
                let by_range = crate::region_for_dtc(&code, DtcSystem::Powertrain, true);
                assert!(
                    by_range == part.zone || !by_range.is_known(),
                    "{code} is {} by its range and {} by its part, {}",
                    by_range.as_str(),
                    part.zone.as_str(),
                    part.id
                );
                assert_eq!(catalog.place_dtc(&code, DtcSystem::Powertrain, true).region, part.zone);
            }
        }
    }

    /// The monitor ids a 2023 Honda Odyssey reported, and a diesel's.
    #[test]
    fn a_self_test_names_the_part_it_tests() {
        let catalog = catalog();
        let part = |mid: u8| catalog.for_monitor(mid).map(|p| p.id.as_str());
        assert_eq!(part(0x01), Some("oxygen_sensor"));
        assert_eq!(part(0x06), Some("oxygen_sensor"));
        assert_eq!(part(0x41), Some("oxygen_sensor"), "its heater");
        assert_eq!(part(0x22), Some("catalytic_converter"));
        assert_eq!(part(0x31), Some("egr_valve"));
        assert_eq!(part(0x3A), Some("evap_system"));
        assert_eq!(part(0x3D), Some("evap_system"));
        assert_eq!(part(0x85), Some("turbocharger"));
        assert_eq!(part(0xB0), Some("particulate_filter"));

        // Tests of a system rather than a part have a zone and no part.
        for (mid, region) in [
            (0x35, Region::EngineBay), // valve timing
            (0x81, Region::EngineBay), // fuel system
            (0xA3, Region::EngineBay), // misfire, cylinder 2
            (0x98, Region::Exhaust),   // NOx catalyst
        ] {
            let placed = catalog.place_monitor(mid);
            assert_eq!(placed.region, region, "{mid:02X}");
            assert!(placed.part.is_none(), "{mid:02X}");
        }
        // A manufacturer's own monitor is not placed.
        assert!(!catalog.place_monitor(0xE4).is_known());
    }

    /// Every part says what it looks like and where to look, and the lists of
    /// what points at it stay out of what the interface is sent.
    #[test]
    fn every_part_can_be_described_to_somebody_who_has_never_seen_one() {
        let catalog = catalog();
        assert!(catalog.len() >= 15, "only {} parts", catalog.len());
        for part in catalog.iter() {
            assert!(part.looks_like.len() > 60, "{}", part.id);
            assert!(part.find_it.len() > 15, "{}", part.id);
            assert!(!part.codes.is_empty() || !part.monitors.is_empty(), "{}", part.id);
            let sent = serde_json::to_value(part).unwrap();
            assert!(sent.get("codes").is_none() && sent.get("monitors").is_none(), "{sent}");
            assert_eq!(sent["zone"], part.zone.as_str());
        }
    }

    /// Two parts claiming one code would make the answer depend on the order
    /// of the file.
    #[test]
    fn a_table_that_gives_one_code_to_two_parts_is_refused() {
        let overlapping = r#"
parts:
  - { id: a, name: A, zone: exhaust, looks_like: x, find_it: y, codes: ["P0420-P0439"] }
  - { id: b, name: B, zone: exhaust, looks_like: x, find_it: y, codes: ["P0430"] }
"#;
        let err = PartCatalog::from_yaml(overlapping).unwrap_err();
        assert!(err.message.contains("both"), "{}", err.message);

        let backwards = r#"
parts:
  - { id: a, name: A, zone: exhaust, looks_like: x, find_it: y, codes: ["P0439-P0420"] }
"#;
        assert!(PartCatalog::from_yaml(backwards).is_err());
    }
}
