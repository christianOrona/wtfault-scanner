//! Community signal definitions, in the OBDb signalset format.
//!
//! # What this is for
//!
//! The legislated OBD-II PIDs are a small, standard set, and this build already
//! ships decoders for them. Everything a manufacturer adds beyond that —
//! transmission temperature on a Ford, state of charge on a hybrid, the several
//! hundred parameters behind service `0x22` — is undocumented unless somebody
//! has written it down. [OBDb](https://obdb.community) is a community that has,
//! for around 740 vehicles, one repository per make and model.
//!
//! Without something like this, an agent asked "what is my transmission
//! temperature" on a vehicle outside the legislated set has two options: guess
//! at a data identifier, or say it cannot know. The first is the behaviour this
//! project exists to refuse and the second is unhelpful. A catalogue turns it
//! into a third option — *ask the vehicle a question somebody has recorded, and
//! report what it answers* — which is neither guessing nor giving up.
//!
//! # What it is not
//!
//! **These definitions are claims, not measurements.** Nobody in this process
//! has verified that a given identifier exists on the vehicle in front of you,
//! that it means what the catalogue says, or that the scaling is right for your
//! model year. A catalogue entry is [`SourceKind::ProfileData`] and
//! [`VerificationStatus::Unverified`] — somebody's recorded observation — and it
//! stays that way until this vehicle has been shown to answer it sensibly.
//!
//! Two consequences are deliberate and load-bearing:
//!
//! * A signal decoded through a catalogue entry carries the catalogue as the
//!   source of its *interpretation*, even though the bytes themselves were
//!   measured. "Your truck returned these bytes, and a community definition
//!   says they mean 96 °C" is a different claim from "your truck reported
//!   96 °C", and only the first one is true.
//! * A definition that does not apply to this model year is not offered. The
//!   year filter fails closed: a definition with no stated range applies to the
//!   whole model, and one with a range applies only inside it.
//!
//! # Licence
//!
//! OBDb data is CC BY-SA 4.0. It is *data*, kept separate from this crate's
//! code, and any bundled copy must carry its attribution and licence. Nothing
//! in this module embeds catalogue content — it reads what the caller supplies.

use aim_types::{SourceKind, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One vehicle's signal definitions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SignalSet {
    /// Every command this vehicle is said to answer.
    #[serde(default)]
    pub commands: Vec<CommandDef>,
}

/// One request, and the signals its reply is said to carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDef {
    /// Request header — the module to address, e.g. `7E0` or `18DA10F1`.
    ///
    /// A string rather than a number, which is what lets the same field carry
    /// an 11-bit and a 29-bit address. Storing this as an integer is the exact
    /// assumption that made this project a single-make scanner.
    pub hdr: String,
    /// Response address to match, when the catalogue states one.
    #[serde(default)]
    pub rax: Option<String>,
    /// Service and parameter, as hex strings: `{"22": "F40C"}`.
    ///
    /// A map because that is the shape OBDb uses. In practice it holds exactly
    /// one entry, and anything else is rejected rather than guessed at.
    pub cmd: BTreeMap<String, String>,
    /// Suggested poll rate in Hz. Advisory only.
    #[serde(default)]
    pub freq: Option<f64>,
    /// Model years this command applies to.
    #[serde(default)]
    pub filter: Option<YearFilter>,
    /// The signals carried in the reply.
    #[serde(default)]
    pub signals: Vec<SignalDef>,
}

/// Model years a definition applies to. Absent bounds are open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearFilter {
    /// First model year, inclusive.
    #[serde(default)]
    pub from: Option<u16>,
    /// Last model year, inclusive.
    #[serde(default)]
    pub to: Option<u16>,
}

impl YearFilter {
    /// Whether `year` falls inside this filter.
    pub fn contains(&self, year: u16) -> bool {
        self.from.is_none_or(|f| year >= f) && self.to.is_none_or(|t| year <= t)
    }
}

/// One value within a reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalDef {
    /// Catalogue identifier, e.g. `F150_RPM`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Grouping the catalogue files it under, e.g. `Engine`.
    #[serde(default)]
    pub path: Option<String>,
    /// How to read it out of the reply.
    pub fmt: Fmt,
    /// Standard metric name, when the catalogue maps it to one.
    #[serde(rename = "suggestedMetric", default)]
    pub suggested_metric: Option<String>,
    /// Free text from the catalogue.
    #[serde(default)]
    pub description: Option<String>,
}

/// Where a value sits in the reply and how to scale it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fmt {
    /// Width in bits.
    #[serde(default)]
    pub len: u32,
    /// Bit index of a single-bit field, counted from the start of the payload.
    #[serde(default)]
    pub bix: Option<u32>,
    /// Two's-complement when true.
    #[serde(default)]
    pub sign: bool,
    /// Multiply by this before dividing.
    #[serde(default)]
    pub mul: Option<f64>,
    /// Divide by this.
    #[serde(default)]
    pub div: Option<f64>,
    /// Add this last.
    #[serde(default)]
    pub add: Option<f64>,
    /// Unit string as the catalogue gives it.
    #[serde(default)]
    pub unit: Option<String>,
    /// Lowest plausible value.
    #[serde(default)]
    pub min: Option<f64>,
    /// Highest plausible value.
    #[serde(default)]
    pub max: Option<f64>,
    /// Enumerated meanings, keyed by raw value.
    #[serde(default)]
    pub map: Option<BTreeMap<String, MapEntry>>,
}

/// One enumerated value's meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapEntry {
    /// Short value label.
    #[serde(default)]
    pub value: Option<String>,
    /// Longer description.
    #[serde(default)]
    pub description: Option<String>,
}

/// What a catalogue definition produced when applied to real bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogReading {
    /// Catalogue signal id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Scaled numeric value.
    pub value: f64,
    /// Unit, when stated.
    pub unit: Option<String>,
    /// The enumerated label, when this signal is enumerated.
    pub label: Option<String>,
    /// True when the value fell outside the catalogue's own stated bounds.
    ///
    /// A strong hint that this definition does not fit this vehicle, and the
    /// reason a catalogue reading is never promoted to measured on its own.
    pub out_of_stated_range: bool,
}

impl CommandDef {
    /// Whether this definition is offered for `year`.
    ///
    /// Fails closed: a definition scoped to years outside this vehicle's is not
    /// offered at all, rather than offered with a warning. An unscoped
    /// definition applies to the whole model, which is what the catalogue means
    /// by omitting the filter.
    pub fn applies_to_year(&self, year: Option<u16>) -> bool {
        match (self.filter, year) {
            (None, _) => true,
            // A scoped definition and no known model year is not a match. We
            // cannot tell whether it applies, and offering it anyway is how a
            // definition for a different generation gets sent to somebody's
            // vehicle.
            (Some(_), None) => false,
            (Some(f), Some(y)) => f.contains(y),
        }
    }

    /// The service and parameter as bytes, ready to send.
    ///
    /// Returns `None` when the entry is not the single service/parameter pair
    /// the format specifies — malformed catalogue data is skipped rather than
    /// guessed at.
    pub fn request_bytes(&self) -> Option<Vec<u8>> {
        if self.cmd.len() != 1 {
            return None;
        }
        let (service, parameter) = self.cmd.iter().next()?;
        let mut out = hex_bytes(service)?;
        out.extend(hex_bytes(parameter)?);
        Some(out)
    }

    /// A short description of what this asks for, for a person or a log.
    pub fn describe(&self) -> String {
        let (service, parameter) = match self.cmd.iter().next() {
            Some((s, p)) => (s.as_str(), p.as_str()),
            None => ("?", "?"),
        };
        format!("service {service} parameter {parameter} at header {}", self.hdr)
    }
}

impl SignalDef {
    /// Apply this definition to a reply payload.
    ///
    /// `payload` is the reply with its service echo and parameter id already
    /// removed — the data bytes only.
    ///
    /// Returns `None` when the payload is too short for the definition, which
    /// is itself informative: it means this definition does not describe what
    /// this vehicle actually returned.
    pub fn decode(&self, payload: &[u8], bit_offset: u32) -> Option<CatalogReading> {
        let raw = if let Some(bix) = self.fmt.bix {
            u64::from(bit_at(payload, bix)?)
        } else {
            bits_at(payload, bit_offset, self.fmt.len.max(1))?
        };

        let signed = if self.fmt.sign { sign_extend(raw, self.fmt.len.max(1)) } else { raw as i64 };
        let mut value = signed as f64;
        if let Some(mul) = self.fmt.mul {
            value *= mul;
        }
        if let Some(div) = self.fmt.div {
            if div == 0.0 {
                return None;
            }
            value /= div;
        }
        if let Some(add) = self.fmt.add {
            value += add;
        }

        let label = self.fmt.map.as_ref().and_then(|m| {
            let key = raw.to_string();
            m.get(&key).map(|e| {
                e.description.clone().or_else(|| e.value.clone()).unwrap_or_else(|| key.clone())
            })
        });

        // Checked against the catalogue's own bounds, not against anything this
        // project believes about the vehicle. A value outside them means the
        // definition and the vehicle disagree, and that has to be visible.
        let out_of_stated_range = self.fmt.min.is_some_and(|min| value < min)
            || self.fmt.max.is_some_and(|max| value > max);

        Some(CatalogReading {
            id: self.id.clone(),
            name: self.name.clone(),
            value,
            unit: self.fmt.unit.clone(),
            label,
            out_of_stated_range,
        })
    }

    /// How this reading must be labelled.
    ///
    /// Always unverified. The bytes were measured; the claim that they mean
    /// this was not, and will not be until this vehicle has been shown to
    /// answer this definition sensibly. Promoting a catalogue entry to verified
    /// because it decoded without error would be verifying arithmetic, not the
    /// vehicle.
    pub fn verification(&self) -> VerificationStatus {
        VerificationStatus::Unverified
    }

    /// The origin kind a reading from this definition carries.
    ///
    /// [`SourceKind::ProfileData`] rather than [`SourceKind::Decoder`]: the
    /// arithmetic is ours but the claim that these bits mean this is somebody
    /// else`s recorded observation, and that is the part a reader needs to be
    /// able to weigh.
    pub fn source_kind(&self) -> SourceKind {
        SourceKind::ProfileData
    }
}

impl SignalSet {
    /// Parse an OBDb v3 signalset document.
    pub fn from_json(text: &str) -> Result<SignalSet, String> {
        serde_json::from_str(text).map_err(|e| format!("not a signalset document: {e}"))
    }

    /// Commands offered for a model year, in catalogue order.
    pub fn for_year(&self, year: Option<u16>) -> Vec<&CommandDef> {
        self.commands.iter().filter(|c| c.applies_to_year(year)).collect()
    }

    /// Every signal offered for a model year, with the command that fetches it.
    pub fn signals_for_year(&self, year: Option<u16>) -> Vec<(&CommandDef, &SignalDef)> {
        self.for_year(year)
            .into_iter()
            .flat_map(|c| c.signals.iter().map(move |s| (c, s)))
            .collect()
    }
}

/// Parse an even-length hex string into bytes.
fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.is_empty() || s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// One bit, counted from the most significant bit of byte zero.
fn bit_at(payload: &[u8], index: u32) -> Option<u8> {
    let byte = payload.get((index / 8) as usize)?;
    Some((byte >> (7 - (index % 8))) & 1)
}

/// `len` bits starting at `offset`, counted from the most significant bit of
/// byte zero. Returns `None` when the payload is too short to contain them.
fn bits_at(payload: &[u8], offset: u32, len: u32) -> Option<u64> {
    if len == 0 || len > 64 {
        return None;
    }
    let end = offset.checked_add(len)?;
    if end > (payload.len() as u32).checked_mul(8)? {
        return None;
    }
    let mut out: u64 = 0;
    for i in 0..len {
        out = (out << 1) | u64::from(bit_at(payload, offset + i)?);
    }
    Some(out)
}

/// Reinterpret the low `len` bits of `raw` as two's-complement.
fn sign_extend(raw: u64, len: u32) -> i64 {
    if len == 0 || len >= 64 {
        return raw as i64;
    }
    let sign_bit = 1u64 << (len - 1);
    if raw & sign_bit != 0 {
        (raw as i64) - (1i64 << len)
    } else {
        raw as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries quoted from the OBDb Ford-F-150 signalset, which is the shape
    /// this has to read. Kept verbatim so a format change is a test failure
    /// rather than a surprise on somebody's truck.
    const F150: &str = r#"{
      "commands": [
        {
          "hdr": "7E0",
          "rax": "7E8",
          "cmd": {"22": "F40C"},
          "freq": 2,
          "signals": [
            {"id": "F150_RPM", "path": "Engine",
             "fmt": {"len": 16, "max": 16383.75, "div": 4, "unit": "rpm"},
             "name": "Engine speed"}
          ]
        },
        {
          "hdr": "7E0",
          "cmd": {"22": "F42F"},
          "filter": {"from": 2015, "to": 2020},
          "signals": [
            {"id": "F150_FLI", "path": "Fuel",
             "fmt": {"len": 8, "max": 100, "mul": 100, "div": 255, "unit": "percent"},
             "name": "Fuel level", "suggestedMetric": "fuelTankLevel"}
          ]
        }
      ]
    }"#;

    #[test]
    fn the_real_catalogue_shape_parses() {
        let set = SignalSet::from_json(F150).unwrap();
        assert_eq!(set.commands.len(), 2);
        let rpm = &set.commands[0];
        assert_eq!(rpm.hdr, "7E0");
        assert_eq!(rpm.rax.as_deref(), Some("7E8"));
        // Service 0x22 is UDS ReadDataByIdentifier: the catalogue's value is
        // that it covers manufacturer identifiers, not just legislated PIDs.
        assert_eq!(rpm.request_bytes().unwrap(), vec![0x22, 0xF4, 0x0C]);
        assert_eq!(rpm.signals[0].id, "F150_RPM");
    }

    /// A header is a string so that a 29-bit module is expressible. Storing it
    /// as a number is the assumption that made this a single-make scanner.
    #[test]
    fn a_29_bit_header_survives_the_round_trip() {
        let json = r#"{"commands":[{"hdr":"18DA10F1","cmd":{"22":"F190"},"signals":[]}]}"#;
        let set = SignalSet::from_json(json).unwrap();
        assert_eq!(set.commands[0].hdr, "18DA10F1");
        assert_eq!(set.commands[0].request_bytes().unwrap(), vec![0x22, 0xF1, 0x90]);
    }

    #[test]
    fn scaling_follows_the_catalogue_arithmetic() {
        let set = SignalSet::from_json(F150).unwrap();
        // 16 bits, divided by four: 0x0BB8 = 3000, so 750 rpm.
        let rpm = set.commands[0].signals[0].decode(&[0x0B, 0xB8], 0).unwrap();
        assert!((rpm.value - 750.0).abs() < f64::EPSILON, "{}", rpm.value);
        assert_eq!(rpm.unit.as_deref(), Some("rpm"));
        assert!(!rpm.out_of_stated_range);

        // 8 bits, x100/255: 0xFF is a full tank.
        let fuel = set.commands[1].signals[0].decode(&[0xFF], 0).unwrap();
        assert!((fuel.value - 100.0).abs() < 1e-9, "{}", fuel.value);
    }

    /// A value outside the catalogue's own bounds means the definition and the
    /// vehicle disagree. That has to be visible, not silently returned.
    #[test]
    fn a_reading_outside_the_stated_range_says_so() {
        let def = SignalDef {
            id: String::from("X"),
            name: String::from("X"),
            path: None,
            fmt: Fmt { len: 8, max: Some(100.0), ..Default::default() },
            suggested_metric: None,
            description: None,
        };
        assert!(def.decode(&[200], 0).unwrap().out_of_stated_range);
        assert!(!def.decode(&[50], 0).unwrap().out_of_stated_range);
    }

    /// The year filter fails closed, including when the year is unknown.
    #[test]
    fn a_definition_for_another_generation_is_not_offered() {
        let set = SignalSet::from_json(F150).unwrap();
        // Unscoped commands always apply; the scoped one only inside its range.
        assert_eq!(set.for_year(Some(2017)).len(), 2);
        assert_eq!(set.for_year(Some(2023)).len(), 1);
        assert_eq!(set.for_year(Some(2010)).len(), 1);
        // No model year: a scoped definition cannot be shown to apply, so it is
        // not offered. Offering it anyway is how a definition for a different
        // generation reaches somebody's vehicle.
        assert_eq!(set.for_year(None).len(), 1);
    }

    #[test]
    fn a_signed_signal_reads_negative() {
        let def = SignalDef {
            id: String::from("T"),
            name: String::from("Ambient"),
            path: None,
            fmt: Fmt {
                len: 8,
                sign: true,
                unit: Some(String::from("celsius")),
                ..Default::default()
            },
            suggested_metric: None,
            description: None,
        };
        assert_eq!(def.decode(&[0xFF], 0).unwrap().value, -1.0);
        assert_eq!(def.decode(&[0x0A], 0).unwrap().value, 10.0);
    }

    #[test]
    fn a_single_bit_signal_reads_its_bit() {
        let def = SignalDef {
            id: String::from("B"),
            name: String::from("Brake"),
            path: None,
            fmt: Fmt { len: 1, bix: Some(3), ..Default::default() },
            suggested_metric: None,
            description: None,
        };
        // 0b0001_0000: bit 3 counted from the top is set.
        assert_eq!(def.decode(&[0b0001_0000], 0).unwrap().value, 1.0);
        assert_eq!(def.decode(&[0b0000_0000], 0).unwrap().value, 0.0);
    }

    /// A definition longer than the reply describes a different vehicle. Saying
    /// so beats returning a number built from bytes that were not there.
    #[test]
    fn a_definition_too_long_for_the_reply_decodes_to_nothing() {
        let set = SignalSet::from_json(F150).unwrap();
        assert!(set.commands[0].signals[0].decode(&[0x0B], 0).is_none());
        assert!(set.commands[0].signals[0].decode(&[], 0).is_none());
    }

    #[test]
    fn malformed_catalogue_entries_are_skipped_rather_than_guessed_at() {
        let json = r#"{"commands":[
            {"hdr":"7E0","cmd":{"22":"F4","01":"0C"},"signals":[]},
            {"hdr":"7E0","cmd":{"2":"F40C"},"signals":[]},
            {"hdr":"7E0","cmd":{"22":"ZZ"},"signals":[]}
        ]}"#;
        let set = SignalSet::from_json(json).unwrap();
        for c in &set.commands {
            assert!(c.request_bytes().is_none(), "{:?} should not produce a request", c.cmd);
        }
    }

    /// A catalogue entry is somebody's claim about a vehicle, and decoding it
    /// without error verifies arithmetic rather than the vehicle.
    #[test]
    fn a_catalogue_reading_is_never_verified_by_decoding_cleanly() {
        let set = SignalSet::from_json(F150).unwrap();
        assert_eq!(set.commands[0].signals[0].verification(), VerificationStatus::Unverified);
    }
}
