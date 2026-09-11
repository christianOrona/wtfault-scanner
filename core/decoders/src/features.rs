//! Configurable vehicle features, as data.
//!
//! This is the layer that turns "make my mirrors fold when I lock the truck"
//! into something a machine can act on without a language model ever naming a
//! module address or a byte offset.
//!
//! # The rule that makes this safe
//!
//! A model may only ever say **which feature id** it wants and **what value**.
//! It may not supply a module, an address, a block number, a byte index, or a
//! bit position. Those come exclusively from a [`FeatureDef`] in a data file. A
//! feature id that is not in the catalogue is not a write that goes wrong — it
//! is a write that cannot be expressed.
//!
//! That is the whole security argument, and it is structural rather than
//! procedural: there is no field on the request type to smuggle an address in.
//!
//! # Knowing a feature exists is not knowing how to change it
//!
//! These two are deliberately separate:
//!
//! * A [`FeatureDef`] always describes *what the feature is* — what it does,
//!   which modules own it, what it is worth, how dangerous changing it would
//!   be. This part can be written from public knowledge.
//! * Its [`Mapping`] describes *where the bits live*. This is manufacturer
//!   specific, is not published, and is `None` until somebody verifies it
//!   against a real vehicle.
//!
//! A feature with no mapping is still worth having: the product can say "your
//! truck has the hardware for this and it is switched off in the door module",
//! which is most of the value and none of the risk. A feature with an
//! unverified mapping can be read but never written.

use aim_types::{AimError, AimResult, ErrorCode, RiskClass, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a feature's current setting lives inside a module's configuration.
///
/// Deliberately expressed as a block/byte/bit triple rather than as a raw
/// command: the write path builds the command, and there is no variant here
/// that can carry arbitrary bytes to put on the bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mapping {
    /// One or more bits inside a manufacturer "as built" configuration block.
    AsBuiltBits {
        /// Configuration block identifier, e.g. `726-01`.
        block: String,
        /// Zero-based byte within the block.
        byte: u8,
        /// Bit mask within that byte.
        mask: u8,
        /// Value of the masked bits meaning "on", already shifted to sit under
        /// the mask.
        on: u8,
        /// Value of the masked bits meaning "off".
        off: u8,
    },
    /// Bits inside a record addressed by a UDS data identifier.
    ///
    /// This is the form the app can actually read and write, because every part
    /// of it is the public ISO 14229 standard: ask module `module` for data
    /// identifier `did`, change the masked bits of byte `byte`, write the whole
    /// record back. Nothing here is manufacturer-specific *code* — the
    /// manufacturer-specific part is the numbers, and those live in a data file
    /// that names its own source.
    ///
    /// [`Mapping::AsBuiltBits`] describes the same idea in a vendor's own
    /// vocabulary and cannot be executed until somebody establishes which data
    /// identifier a given block corresponds to. That translation is exactly the
    /// kind of unverified guess this project refuses to make in code.
    DataIdentifierBits {
        /// Diagnostic request address of the module that owns the setting.
        ///
        /// A string — `"726"` on an 11-bit vehicle, `"18DA10F1"` on a 29-bit
        /// one. It was a `u16`, which silently could not express the second and
        /// meant a configuration write on a 29-bit vehicle went to a truncated
        /// address. That is the same assumption that made the whole app behave
        /// like a single-make scanner, surviving here because this path is the
        /// least travelled.
        ///
        /// Still accepts the number that older profile files were written with,
        /// because people have those files on disk. See [`module_address`].
        #[serde(deserialize_with = "module_address")]
        module: String,
        /// The data identifier holding the record.
        did: u16,
        /// Zero-based byte within the record.
        byte: u8,
        /// Bit mask within that byte.
        mask: u8,
        /// Value of the masked bits meaning "on", already shifted to sit under
        /// the mask.
        on: u8,
        /// Value of the masked bits meaning "off".
        off: u8,
    },
}

impl Mapping {
    /// The executable form of this mapping, if it has one.
    ///
    /// Returns `None` for a mapping this build can describe but not perform, so
    /// callers must handle that case rather than discovering it mid-write.
    pub fn as_data_identifier(&self) -> Option<DataIdentifierTarget> {
        match self {
            Mapping::DataIdentifierBits { module, did, byte, mask, on, off } => {
                Some(DataIdentifierTarget {
                    module: module.clone(),
                    did: *did,
                    byte: *byte,
                    mask: *mask,
                    on: *on,
                    off: *off,
                })
            }
            Mapping::AsBuiltBits { .. } => None,
        }
    }
}

/// Read a module address written either as a string or as a number.
///
/// Profile files predating 29-bit support wrote `module: 0x7A0`, and people
/// have those files on their own machines. Refusing them to gain a cleaner
/// schema would break a mapping somebody measured on their own vehicle, which
/// is exactly the work this project exists to encourage.
///
/// Numbers render as at least three hex digits, matching how 11-bit addresses
/// have always been written. Strings are taken as given, upper-cased, and
/// checked to be hexadecimal — an address that is not is a mistake worth
/// refusing when the file loads rather than when somebody writes to a module.
fn module_address<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Written {
        Number(u64),
        Text(String),
    }

    let address = match Written::deserialize(deserializer)? {
        Written::Number(n) => format!("{n:03X}"),
        Written::Text(s) => s.trim().to_ascii_uppercase(),
    };
    if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(D::Error::custom(format!(
            "{address:?} is not a module address: expected hex digits such as \"726\" or \
             \"18DA10F1\""
        )));
    }
    Ok(address)
}

/// A mapping resolved to something a UDS request can be built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataIdentifierTarget {
    /// Module diagnostic request address, e.g. `726` or `18DA10F1`.
    pub module: String,
    /// Data identifier holding the record.
    pub did: u16,
    /// Zero-based byte within the record.
    pub byte: u8,
    /// Bit mask within that byte.
    pub mask: u8,
    /// Masked value meaning "on".
    pub on: u8,
    /// Masked value meaning "off".
    pub off: u8,
}

impl DataIdentifierTarget {
    /// Apply this mapping to a record read from the vehicle.
    ///
    /// Returns the modified record, or an error naming why it could not be
    /// applied. Deliberately fallible rather than clamping or padding: a record
    /// shorter than the mapping expects means the mapping is wrong for this
    /// vehicle, and writing a padded guess to a module is how modules die.
    pub fn apply(&self, record: &[u8], on: bool) -> Result<Vec<u8>, String> {
        let idx = self.byte as usize;
        if idx >= record.len() {
            return Err(format!(
                "the mapping expects at least {} bytes at data identifier {:04X}, but the module \
                 returned {}. The mapping does not match this vehicle and nothing was written.",
                idx + 1,
                self.did,
                record.len()
            ));
        }
        let wanted = if on { self.on } else { self.off };
        if wanted & !self.mask != 0 {
            return Err(format!(
                "the mapping's value {wanted:#04X} has bits outside its own mask {:#04X}, which \
                 would change settings this feature does not describe",
                self.mask
            ));
        }
        let mut out = record.to_vec();
        out[idx] = (out[idx] & !self.mask) | wanted;
        Ok(out)
    }

    /// Read the current state of this feature out of a record.
    pub fn current(&self, record: &[u8]) -> Option<bool> {
        let masked = record.get(self.byte as usize)? & self.mask;
        if masked == self.on {
            Some(true)
        } else if masked == self.off {
            Some(false)
        } else {
            None
        }
    }
}

/// One thing about a vehicle that can be turned on, off, or set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureDef {
    /// Stable id. This is the only thing a model is allowed to name.
    pub id: String,
    /// Short human name.
    pub name: String,
    /// For someone with no mechanical background.
    pub easy: String,
    /// For someone who could do the work.
    pub technical: String,
    /// What kind of thing changing this touches.
    pub risk: RiskClass,
    /// Which modules own the setting, by conventional abbreviation.
    pub modules: Vec<String>,
    /// Which vehicles this could apply to.
    #[serde(default)]
    pub applies_to: Applicability,
    /// What the adapter and the vehicle must support before this is even
    /// attemptable. Reported to the user as the reason something is greyed out.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Where the bits live. `None` means this build knows the feature exists
    /// but not how to read or change it.
    #[serde(default)]
    pub mapping: Option<Mapping>,
    /// Whether the mapping has been validated against a real vehicle.
    ///
    /// This is evidence about **reading**: somebody read the record, and the
    /// bits described here held what this feature claims they hold.
    #[serde(default)]
    pub verification: VerificationStatus,
    /// Evidence that this setting can actually be *changed* this way.
    ///
    /// Deliberately separate from [`FeatureDef::verification`], and absent by
    /// default, because knowing where a setting lives is not the same as
    /// knowing how to change it safely. A module can serve a record happily and
    /// then refuse to write it, demand an extended session, demand security
    /// access, or accept the write and ignore it. Those are discovered by
    /// trying, on a vehicle, and recording what happened.
    ///
    /// The default matters as much as the field. Every profile written before
    /// this existed said `verification: verified` meaning "I checked the
    /// mapping", and none of them meant "and writing it is proven safe". They
    /// now grant reading and not writing, which is the correct reading of what
    /// their authors actually verified.
    #[serde(default)]
    pub write_verification: Option<OperationEvidence>,
    /// Where the mapping came from, so a wrong one can be traced to its source
    /// rather than blamed on the tool.
    #[serde(default)]
    pub source: Option<String>,
    /// Anything a person should know before touching this.
    #[serde(default)]
    pub notes: Option<String>,
}

/// Which vehicles a feature could apply to.
///
/// Coarse on purpose. This narrows a list for a human to choose from; it is
/// never the thing that authorises a write, because a VIN prefix matching does
/// not mean a particular truck was built with the hardware. Only reading the
/// vehicle's own configuration establishes that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applicability {
    /// Manufacturer names as the VIN decoder reports them.
    #[serde(default)]
    pub makes: Vec<String>,
    /// Inclusive model-year range, `[from, to]`.
    #[serde(default)]
    pub model_years: Option<[u16; 2]>,
    /// World manufacturer identifier prefixes, e.g. `1FT`.
    #[serde(default)]
    pub wmi_prefixes: Vec<String>,
    /// Exact vehicle identification numbers this applies to.
    ///
    /// The narrowest scope there is, and the honest one for a mapping measured
    /// on exactly one vehicle. "Verified on one vehicle" and "verified on this
    /// vehicle" are different claims, and until somebody confirms the second on
    /// a second truck the first is the only one anybody can make.
    ///
    /// Also what lets a profile shipped for the built-in virtual vehicle stay
    /// bound to it, rather than quietly matching a real one that happens to be
    /// the same make and year.
    #[serde(default)]
    pub vins: Vec<String>,
    /// Free-text platform label, e.g. a manufacturer's own name for a chassis.
    ///
    /// Carried for grouping and display, deliberately not used for matching:
    /// platform naming is a manufacturer's own vocabulary and no two agree, so
    /// letting it decide whether a write happens would be a guess wearing a
    /// taxonomy.
    #[serde(default)]
    pub platform: Option<String>,
    /// VIN prefixes this mapping is a *candidate* for, without having been
    /// measured there.
    ///
    /// Characters 1–8 of a VIN are not arbitrary: the standard puts the
    /// manufacturer, line, body style, and engine there, which is most of what
    /// decides whether two vehicles share a configuration layout. Two trucks
    /// with the same first eight characters and the same model year are very
    /// likely to hold a setting in the same bit — likely, not certain.
    ///
    /// This exists because scoping a measured mapping to one exact VIN and
    /// stopping was too strict to be useful. The next identical truck got
    /// nothing at all, when the honest thing is to offer what we know, label it
    /// as coming from a different vehicle, and let this one settle it. That is
    /// the same rule already applied to community definitions from another
    /// model; applying it to strangers' data and not our own was inconsistent.
    ///
    /// A candidate is never writable on that basis alone. It is read, decoded,
    /// and checked against something the owner can see.
    #[serde(default)]
    pub candidate_vin_prefixes: Vec<String>,
}

/// How much a feature's mapping has to do with the vehicle in front of us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingRelevance {
    /// Measured on this exact vehicle.
    Measured,
    /// Measured on a different vehicle that this one closely resembles. A
    /// hypothesis about this vehicle, and it must be said so.
    Candidate,
    /// Not applicable here.
    None,
}

impl MappingRelevance {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            MappingRelevance::Measured => "measured_on_this_vehicle",
            MappingRelevance::Candidate => "measured_on_a_similar_vehicle",
            MappingRelevance::None => "not_applicable",
        }
    }

    /// What this means, in language meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            MappingRelevance::Measured => {
                "This mapping was measured on this exact vehicle, by watching the setting change."
            }
            MappingRelevance::Candidate => {
                "This mapping was measured on a DIFFERENT vehicle that closely resembles yours - \
                 same manufacturer, line, body and engine by VIN, same model year. Configuration \
                 layouts usually match across such vehicles and sometimes do not. Reading it is \
                 safe and will say what it thinks the current setting is; check that against what \
                 your vehicle actually shows before trusting it."
            }
            MappingRelevance::None => "This mapping is not for this vehicle.",
        }
    }
}

impl Applicability {
    /// How much a mapping scoped by this has to do with `vin`.
    ///
    /// Exact VIN is [`MappingRelevance::Measured`]. A vehicle sharing the first
    /// eight VIN characters is a [`MappingRelevance::Candidate`]. Anything else
    /// is nothing, and an unidentified vehicle is nothing — a candidate is a
    /// claim about a *particular* similar vehicle, not a default.
    pub fn relevance(
        &self,
        make: Option<&str>,
        model_year: Option<u16>,
        vin: Option<&str>,
    ) -> MappingRelevance {
        if self.matches(make, model_year, vin) {
            return MappingRelevance::Measured;
        }
        let Some(vin) = vin.filter(|v| v.len() >= 8) else {
            return MappingRelevance::None;
        };
        // Only the exact-VIN constraint may be relaxed. Everything else this
        // applicability states still has to hold, so a candidate cannot escape
        // a make or model-year restriction by the back door.
        let without_vin = Applicability { vins: Vec::new(), ..self.clone() };
        if !without_vin.matches(make, model_year, Some(vin)) {
            return MappingRelevance::None;
        }
        let prefix = &vin[..8];
        if self.candidate_vin_prefixes.iter().any(|p| p.eq_ignore_ascii_case(prefix)) {
            return MappingRelevance::Candidate;
        }
        MappingRelevance::None
    }

    /// Whether a feature could apply to this vehicle.
    ///
    /// An empty constraint matches everything: a feature that does not say it
    /// is Ford-only is not silently treated as Ford-only.
    pub fn matches(&self, make: Option<&str>, model_year: Option<u16>, vin: Option<&str>) -> bool {
        if !self.makes.is_empty() {
            let m = make.unwrap_or("");
            if !self.makes.iter().any(|x| {
                m.eq_ignore_ascii_case(x)
                    || m.to_ascii_lowercase().contains(&x.to_ascii_lowercase())
            }) {
                return false;
            }
        }
        if let Some([from, to]) = self.model_years {
            match model_year {
                Some(y) if y >= from && y <= to => {}
                // An unknown year does not disqualify: plenty of vehicles do
                // not report one, and hiding a feature because of a missing
                // field is worse than showing it with its year range stated.
                None => {}
                _ => return false,
            }
        }
        if !self.wmi_prefixes.is_empty() {
            let v = vin.unwrap_or("");
            if v.len() >= 3 && !self.wmi_prefixes.iter().any(|p| v.starts_with(p.as_str())) {
                return false;
            }
        }
        // Exact VINs are the one constraint that fails closed on a missing
        // value. Every other field here answers "could this apply", where an
        // unknown year or make should not hide a feature. This one answers "was
        // this measured on the vehicle in front of us", and an unidentified
        // vehicle is not that vehicle.
        if !self.vins.is_empty() {
            let Some(v) = vin else { return false };
            if !self.vins.iter().any(|x| x.eq_ignore_ascii_case(v)) {
                return false;
            }
        }
        true
    }
}

/// How far this build can go with a given feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureSupport {
    /// The feature is described but this build has no mapping: it can be
    /// explained, and nothing more.
    DescribedOnly,
    /// A mapping exists but is unverified: current state can be read and shown
    /// as evidence, and no write is possible.
    ReadOnly,
    /// A verified mapping exists. A write is possible if policy allows it.
    Writable,
}

/// What is known about one operation on one feature, and who established it.
///
/// A struct rather than more enum variants. "Verified on one vehicle" and
/// "verified by the community" are the same fact with a different count, and
/// encoding a count as a variant means inventing a new variant every time the
/// number changes shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationEvidence {
    /// Whether this operation has been demonstrated on a real vehicle.
    #[serde(default)]
    pub verification: VerificationStatus,
    /// How many distinct vehicles it has been demonstrated on.
    ///
    /// One is meaningfully different from twelve, and a person deciding whether
    /// to let a tool write to their door module deserves the number rather than
    /// an adjective.
    #[serde(default)]
    pub verified_on_vehicles: u32,
    /// Where the evidence came from, named so a wrong one is traceable.
    #[serde(default)]
    pub source: Option<String>,
    /// When it was last confirmed, as an ISO date.
    #[serde(default)]
    pub last_verified: Option<String>,
    /// Anything the next person should know before relying on it.
    #[serde(default)]
    pub notes: Option<String>,
}

impl OperationEvidence {
    /// Whether this is enough to act on.
    pub fn is_verified(&self) -> bool {
        self.verification == VerificationStatus::Verified && self.verified_on_vehicles > 0
    }
}

impl FeatureDef {
    /// What this build can actually do with this feature.
    ///
    /// Writing needs its own evidence. A mapping verified for reading gets
    /// `ReadOnly` and nothing more, because a module that will show you a
    /// record will not necessarily let you change it.
    pub fn support(&self) -> FeatureSupport {
        match (&self.mapping, self.verification) {
            (None, _) => FeatureSupport::DescribedOnly,
            (Some(_), VerificationStatus::Verified)
                if self.write_verification.as_ref().is_some_and(|w| w.is_verified()) =>
            {
                FeatureSupport::Writable
            }
            (Some(_), VerificationStatus::Verified) => FeatureSupport::ReadOnly,
            (Some(_), _) => FeatureSupport::ReadOnly,
        }
    }

    /// Whether this feature could ever be written by this product.
    ///
    /// Independent of whether a mapping exists: security and programming class
    /// changes are refused by design, so a verified mapping for one would still
    /// not make it writable.
    pub fn writable_in_principle(&self) -> bool {
        self.risk.is_ever_implementable()
    }
}

#[derive(Debug, Deserialize)]
struct FeatureFile {
    #[serde(default)]
    features: Vec<FeatureDef>,
}

/// Every configurable feature this build knows about.
#[derive(Debug, Clone, Default)]
pub struct FeatureCatalog {
    by_id: BTreeMap<String, FeatureDef>,
    sources: Vec<String>,
}

impl FeatureCatalog {
    /// Load the feature definitions embedded in the binary.
    /// One file per manufacturer, plus the reference vehicle. Nothing here is
    /// privileged over a profile a person drops in themselves: these are
    /// loaded first and can be corrected or completed by a later file.
    pub fn embedded() -> AimResult<FeatureCatalog> {
        let mut c = FeatureCatalog::default();
        c.load_yaml(
            include_str!("../../../vehicle-profiles/ford/features.yaml"),
            "embedded:ford/features.yaml",
        )?;
        // Scoped by exact VIN to the vehicle that does not exist, which is what
        // makes it safe to ship with verified write evidence while every real
        // mapping in this project is still null.
        c.load_yaml(
            include_str!("../../../vehicle-profiles/reference-vehicle/features.yaml"),
            "embedded:reference-vehicle/features.yaml",
        )?;
        // The first mapping in this project measured on a real vehicle, scoped
        // by exact VIN to the truck it was measured on. Shipping it is safe for
        // the same reason as the line above: applicability fails closed, so a
        // vehicle that is not that one gets nothing however similar it looks.
        c.load_yaml(
            include_str!("../../../vehicle-profiles/ford-f250-2019/features.yaml"),
            "embedded:ford-f250-2019/features.yaml",
        )?;
        Ok(c)
    }

    /// Merge one YAML document, attributing everything in it to `source`.
    ///
    /// Later definitions replace earlier ones by id, which is what makes a
    /// user-supplied profile able to correct or complete a shipped one.
    pub fn load_yaml(&mut self, yaml: &str, source: &str) -> AimResult<usize> {
        let file: FeatureFile = serde_yaml_ng::from_str(yaml).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("could not parse feature file {source}: {e}"),
            )
        })?;
        let mut n = 0;
        for mut f in file.features {
            if f.id.trim().is_empty() {
                return Err(AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    format!("a feature in {source} has no id"),
                ));
            }
            // A definition that does not say where it came from gets the file
            // it came from. Attribution is not optional: a wrong mapping has to
            // be traceable to whoever supplied it.
            if f.source.is_none() {
                f.source = Some(source.to_string());
            }
            self.by_id.insert(f.id.clone(), f);
            n += 1;
        }
        self.sources.push(source.to_string());
        Ok(n)
    }

    /// Look one up.
    pub fn get(&self, id: &str) -> Option<&FeatureDef> {
        self.by_id.get(id)
    }

    /// Every feature, in id order.
    pub fn all(&self) -> impl Iterator<Item = &FeatureDef> {
        self.by_id.values()
    }

    /// Features that could apply to a particular vehicle.
    pub fn for_vehicle(
        &self,
        make: Option<&str>,
        model_year: Option<u16>,
        vin: Option<&str>,
    ) -> Vec<&FeatureDef> {
        self.by_id.values().filter(|f| f.applies_to.matches(make, model_year, vin)).collect()
    }

    /// How many definitions are loaded.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True when nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The files that were merged in, in load order.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> FeatureCatalog {
        FeatureCatalog::embedded().unwrap()
    }

    fn mapping_from(module_field: &str) -> Mapping {
        let y = format!(
            r#"
version: 1
source: test
features:
  - id: f
    name: "F"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: [BODY]
    mapping:
      kind: data_identifier_bits
      module: {module_field}
      did: 0xDE01
      byte: 0
      mask: 0x01
      on: 0x01
      off: 0x00
"#
        );
        let mut cat = FeatureCatalog::default();
        cat.load_yaml(&y, "test").expect("loads");
        cat.get("f").unwrap().mapping.clone().unwrap()
    }

    /// A write has to reach a 29-bit module. The address was a `u16`, which
    /// silently could not hold one — the same assumption that made the whole
    /// app behave like a single-make scanner, surviving on the least-travelled
    /// path.
    #[test]
    fn a_configuration_write_can_address_a_29_bit_module() {
        let target = mapping_from(r#""18DA10F1""#).as_data_identifier().unwrap();
        assert_eq!(target.module, "18DA10F1");
    }

    /// Profile files predating this were written with a number, and people have
    /// those files on their own machines. Breaking them to gain a cleaner
    /// schema would discard a mapping somebody measured on their own vehicle.
    #[test]
    fn a_module_address_written_as_a_number_still_loads() {
        assert_eq!(mapping_from("0x726").as_data_identifier().unwrap().module, "726");
        assert_eq!(mapping_from("1830").as_data_identifier().unwrap().module, "726");
        // Three digits minimum, matching how 11-bit addresses have always been
        // written, so `7E0` does not become `7E0` in one file and `07E0` in
        // another.
        assert_eq!(mapping_from("0x7E0").as_data_identifier().unwrap().module, "7E0");
    }

    #[test]
    fn a_module_address_is_normalised_and_checked() {
        assert_eq!(
            mapping_from(r#"" 18da10f1 ""#).as_data_identifier().unwrap().module,
            "18DA10F1"
        );
    }

    /// An address that is not hexadecimal is a mistake worth refusing when the
    /// file loads, rather than when somebody writes to a module.
    #[test]
    fn a_module_address_that_is_not_hexadecimal_is_refused_at_load_time() {
        let y = r#"
version: 1
source: test
features:
  - id: f
    name: "F"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: [BODY]
    mapping:
      kind: data_identifier_bits
      module: "the door module"
      did: 0xDE01
      byte: 0
      mask: 0x01
      on: 0x01
      off: 0x00
"#;
        let mut cat = FeatureCatalog::default();
        let err = cat.load_yaml(y, "test").unwrap_err();
        assert!(
            format!("{err:?}").contains("module address"),
            "the failure should name the problem: {err:?}"
        );
    }

    #[test]
    fn the_shipped_catalogue_loads() {
        let c = catalog();
        assert!(!c.is_empty());
        assert!(c.get("mirror_auto_fold").is_some());
    }

    #[test]
    fn no_real_vehicle_feature_claims_a_verified_mapping() {
        // The honest position for this build. Every real-vehicle mapping here
        // came from public description rather than from a vehicle this project
        // has measured, so none may authorise a write. If this fails, somebody
        // added a mapping without measuring it.
        //
        // The one exemption is a feature scoped to specific VINs, which is how
        // the reference profile for the built-in virtual vehicle ships with
        // write evidence: an exact-VIN scope fails closed, so it cannot reach a
        // real truck even of the same make, year and manufacturer prefix.
        for f in catalog().all() {
            if !f.applies_to.vins.is_empty() {
                continue;
            }
            assert_ne!(f.support(), FeatureSupport::Writable, "{} claims a verified mapping", f.id);
        }
    }

    #[test]
    fn every_feature_says_where_it_came_from() {
        for f in catalog().all() {
            assert!(f.source.is_some(), "{} has no source", f.id);
        }
    }

    #[test]
    fn security_and_programming_features_are_never_writable_in_principle() {
        let f = FeatureDef {
            id: "x".into(),
            name: "x".into(),
            easy: "x".into(),
            technical: "x".into(),
            risk: aim_types::RiskClass::Security,
            modules: vec![],
            applies_to: Applicability::default(),
            requires: vec![],
            mapping: Some(Mapping::AsBuiltBits {
                block: "726-01".into(),
                byte: 0,
                mask: 0x01,
                on: 0x01,
                off: 0x00,
            }),
            // Even fully verified, for reading and for writing.
            verification: VerificationStatus::Verified,
            write_verification: Some(OperationEvidence {
                verification: VerificationStatus::Verified,
                verified_on_vehicles: 4,
                source: Some("test".into()),
                last_verified: None,
                notes: None,
            }),
            source: Some("test".into()),
            notes: None,
        };
        assert_eq!(f.support(), FeatureSupport::Writable);
        assert!(!f.writable_in_principle(), "risk class must veto the mapping");
    }

    #[test]
    fn a_later_file_can_correct_an_earlier_one() {
        // This is what makes a user-supplied profile useful: somebody who has
        // verified a mapping on their own vehicle can complete a shipped
        // definition without rebuilding anything.
        let mut c = catalog();
        let before = c.get("mirror_auto_fold").unwrap().clone();
        assert_eq!(before.support(), FeatureSupport::DescribedOnly);

        c.load_yaml(
            r#"
features:
  - id: mirror_auto_fold
    name: "Automatic folding mirrors"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: [DDM]
    verification: verified
    write_verification:
      verification: verified
      verified_on_vehicles: 1
      source: "measured on my own truck"
    mapping:
      kind: as_built_bits
      block: "740-01"
      byte: 2
      mask: 0x30
      on: 0x10
      off: 0x00
"#,
            "user:my-f250.yaml",
        )
        .unwrap();

        let after = c.get("mirror_auto_fold").unwrap();
        assert_eq!(after.support(), FeatureSupport::Writable);
        assert_eq!(after.source.as_deref(), Some("user:my-f250.yaml"));
    }

    #[test]
    fn applicability_narrows_without_hiding_on_missing_data() {
        let a = Applicability {
            makes: vec!["Ford".into()],
            model_years: Some([2017, 2022]),
            wmi_prefixes: vec!["1FT".into()],
            vins: Vec::new(),
            platform: None,
            candidate_vin_prefixes: Vec::new(),
        };
        assert!(a.matches(Some("Ford Motor Company (US, truck)"), Some(2019), Some("1FT7W2BT")));
        assert!(!a.matches(Some("Toyota"), Some(2019), Some("JTD")));
        assert!(!a.matches(Some("Ford"), Some(2005), Some("1FT")));
        // A vehicle that never reported its year is still offered the feature,
        // with the range stated, rather than silently filtered out.
        assert!(a.matches(Some("Ford"), None, Some("1FT")));
    }

    #[test]
    fn an_unknown_feature_id_is_simply_absent() {
        assert!(catalog().get("delete_the_dpf").is_none());
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    fn target() -> DataIdentifierTarget {
        // Bit 2 of byte 3: on = 0b100, off = 0b000.
        DataIdentifierTarget {
            module: String::from("726"),
            did: 0xDE01,
            byte: 3,
            mask: 0b0000_0100,
            on: 0b0000_0100,
            off: 0,
        }
    }

    #[test]
    fn only_the_masked_bits_move() {
        let record = [0xAA, 0xBB, 0xCC, 0b1011_0011, 0xDD];
        let on = target().apply(&record, true).unwrap();
        assert_eq!(on[3], 0b1011_0111, "the masked bit should be set");
        assert_eq!(&on[..3], &record[..3], "earlier bytes must be untouched");
        assert_eq!(&on[4..], &record[4..], "later bytes must be untouched");

        let off = target().apply(&record, false).unwrap();
        assert_eq!(off[3], 0b1011_0011, "already off, so nothing changes");
    }

    /// A record shorter than the mapping means the mapping is for a different
    /// vehicle. Padding it and writing anyway is how a module stops working.
    #[test]
    fn a_short_record_is_an_error_rather_than_a_padded_guess() {
        let err = target().apply(&[0x00, 0x01], true).unwrap_err();
        assert!(err.contains("does not match this vehicle"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
    }

    /// A mapping whose value spills outside its own mask would silently change
    /// settings the feature does not describe.
    #[test]
    fn a_value_outside_its_mask_is_refused() {
        let bad = DataIdentifierTarget {
            module: String::from("726"),
            did: 0xDE01,
            byte: 0,
            mask: 0x0F,
            on: 0x1F,
            off: 0,
        };
        let err = bad.apply(&[0x00], true).unwrap_err();
        assert!(err.contains("outside its own mask"), "{err}");
    }

    #[test]
    fn current_reads_back_what_apply_wrote_and_admits_when_it_cannot_tell() {
        let t = target();
        let record = [0, 0, 0, 0, 0];
        let on = t.apply(&record, true).unwrap();
        assert_eq!(t.current(&on), Some(true));
        assert_eq!(t.current(&record), Some(false));

        // A masked value matching neither on nor off is not guessed at.
        let odd = DataIdentifierTarget {
            module: String::from("001"),
            did: 2,
            byte: 0,
            mask: 0b11,
            on: 0b01,
            off: 0b00,
        };
        assert_eq!(odd.current(&[0b10]), None);
        assert_eq!(odd.current(&[]), None);
    }

    #[test]
    fn an_as_built_mapping_is_not_executable() {
        let m = Mapping::AsBuiltBits { block: "726-01".into(), byte: 0, mask: 1, on: 1, off: 0 };
        assert!(m.as_data_identifier().is_none());
    }
}

#[cfg(test)]
mod operation_evidence_tests {
    use super::*;

    fn feature(yaml_extra: &str) -> FeatureDef {
        let y = format!(
            r#"
features:
  - id: f
    name: "F"
    easy: "e"
    technical: "t"
    risk: convenience
    modules: [BODY]
    mapping:
      kind: data_identifier_bits
      module: 0x726
      did: 0xDE01
      byte: 0
      mask: 0x01
      on: 0x01
      off: 0x00
{yaml_extra}
"#
        );
        let mut cat = FeatureCatalog::default();
        cat.load_yaml(&y, "test").expect("loads");
        cat.get("f").expect("feature").clone()
    }

    /// The migration-safety property, and the reason the field defaults to
    /// absent rather than to the read verification.
    ///
    /// Every profile written before write evidence existed says
    /// `verification: verified` and means "I checked where the setting lives".
    /// None of them means "and writing it is proven safe". If the write gate
    /// read the same field, enabling writes would have silently made every
    /// existing profile writable on evidence nobody gathered.
    #[test]
    fn a_read_verified_mapping_does_not_become_writable_on_its_own() {
        let f = feature("    verification: verified");
        assert_eq!(f.support(), FeatureSupport::ReadOnly);
        assert!(f.write_verification.is_none());
    }

    #[test]
    fn writing_needs_its_own_evidence_on_at_least_one_vehicle() {
        let f = feature(
            "    verification: verified\n    \
             write_verification:\n      verification: verified\n      \
             verified_on_vehicles: 1\n      source: \"measured\"",
        );
        assert_eq!(f.support(), FeatureSupport::Writable);
    }

    /// A claim of verification with no vehicle behind it is not evidence.
    #[test]
    fn write_evidence_verified_on_nothing_is_not_enough() {
        let f = feature(
            "    verification: verified\n    \
             write_verification:\n      verification: verified\n      \
             verified_on_vehicles: 0",
        );
        assert_eq!(f.support(), FeatureSupport::ReadOnly);
    }

    /// Read evidence still gates everything above it: write evidence cannot
    /// rescue a mapping nobody has confirmed the location of.
    #[test]
    fn write_evidence_cannot_substitute_for_an_unverified_mapping() {
        let f = feature(
            "    verification: unverified\n    \
             write_verification:\n      verification: verified\n      \
             verified_on_vehicles: 9",
        );
        assert_eq!(f.support(), FeatureSupport::ReadOnly);
    }

    #[test]
    fn the_vehicle_count_survives_the_round_trip_for_the_interface_to_show() {
        let f = feature(
            "    verification: verified\n    \
             write_verification:\n      verification: verified\n      \
             verified_on_vehicles: 3\n      last_verified: \"2026-09-09\"",
        );
        let w = f.write_verification.expect("evidence");
        assert_eq!(w.verified_on_vehicles, 3);
        assert_eq!(w.last_verified.as_deref(), Some("2026-09-09"));
    }
}

#[cfg(test)]
mod reference_profile_tests {
    use super::*;

    fn reference() -> FeatureDef {
        FeatureCatalog::embedded()
            .expect("embedded catalogue loads")
            .get("reference_body_setting")
            .expect("the reference feature ships")
            .clone()
    }

    /// The property that makes shipping a writable mapping safe at all.
    ///
    /// This profile carries verified write evidence, which every real-vehicle
    /// mapping in this project deliberately does not. It is only defensible
    /// because it cannot match a real vehicle - including one of the same make,
    /// year and manufacturer prefix as the truck the simulator is modelled on.
    #[test]
    fn the_reference_profile_cannot_match_a_real_vehicle() {
        let f = reference();
        assert_eq!(f.support(), FeatureSupport::Writable, "it is writable on the sim");

        // The simulated vehicle.
        assert!(f.applies_to.matches(Some("Ford"), Some(2019), Some("1FT7W2BT6KEC00001")));

        // A real truck of the same make, year and WMI. One digit of VIN apart,
        // and that is the entire difference that must stop the write.
        assert!(
            !f.applies_to.matches(Some("Ford"), Some(2019), Some("1FT7W2BT6KEC00002")),
            "a real vehicle must never pick up the reference mapping"
        );

        // And a vehicle nobody has identified is not the reference vehicle.
        assert!(
            !f.applies_to.matches(Some("Ford"), Some(2019), None),
            "an unidentified vehicle must fail closed against an exact-VIN scope"
        );
    }
}

#[cfg(test)]
mod measured_on_a_real_vehicle {
    use super::*;

    /// The first mapping this project measured on a vehicle rather than
    /// inferring. Pinned because these numbers are evidence: they were observed
    /// changing on a 2019 F-250 twice, in opposite directions, and a silent edit
    /// to any of them would be a claim about a truck nobody re-measured.
    #[test]
    fn the_f250_autolock_mapping_is_exactly_what_was_measured() {
        let catalog = FeatureCatalog::embedded().unwrap();
        let feature = catalog.get("autolock_doors_when_driving").expect("the measured feature");

        let target = feature
            .mapping
            .as_ref()
            .and_then(|m| m.as_data_identifier())
            .expect("a mapping that can actually be executed");

        assert_eq!(target.module, "726", "the module it listens on, not the one it answers on");
        assert_eq!(target.did, 0xDE0E);
        assert_eq!(target.byte, 4);
        assert_eq!(target.mask, 0x01);
        assert_eq!(target.on, 0x01);
        assert_eq!(target.off, 0x00);
    }

    /// Reading and writing are separate claims, and both are now established
    /// for this one bit on this one vehicle.
    ///
    /// Written from this application on 2026-09-11 with an OBDLink MX+ and
    /// confirmed by the owner against the dash menu. The count matters as much
    /// as the status: one vehicle is one vehicle, and the applicability is
    /// still a single VIN.
    #[test]
    fn writing_this_bit_has_been_demonstrated_on_exactly_one_vehicle() {
        let catalog = FeatureCatalog::embedded().unwrap();
        let feature = catalog.get("autolock_doors_when_driving").unwrap();

        assert_eq!(feature.verification, VerificationStatus::Verified, "reading was measured");
        let write = feature.write_verification.as_ref().expect("write evidence is stated");
        assert_eq!(write.verification, VerificationStatus::Verified);
        assert_eq!(write.verified_on_vehicles, 1, "one truck is one truck");
        assert!(write.is_verified(), "which is what promotes this to writable");
    }

    /// The finding that nearly cost us this mapping, pinned so it cannot be
    /// edited away.
    ///
    /// Immediately after the write the dash menu still showed the old value.
    /// Every machine-checkable signal said the write had worked — the module
    /// accepted it, the read-back returned the new bytes, nothing reverted —
    /// and the vehicle behaved as though nothing had changed. It took an
    /// ignition cycle for the module to latch it.
    ///
    /// A profile that records the mapping without recording that has set the
    /// next person up to write the bit, check the menu, see no change, and
    /// conclude the mapping is broken.
    #[test]
    fn the_note_says_the_change_needs_a_key_cycle() {
        let catalog = FeatureCatalog::embedded().unwrap();
        let feature = catalog.get("autolock_doors_when_driving").unwrap();
        let notes = feature
            .write_verification
            .as_ref()
            .and_then(|w| w.notes.as_deref())
            .expect("the write evidence carries notes");

        let lower = notes.to_ascii_lowercase();
        assert!(
            lower.contains("ignition") || lower.contains("key"),
            "the key-cycle requirement must survive: {notes}"
        );
    }

    /// Scoped to one VIN, and failing closed is the whole reason this is safe
    /// to ship. A record's layout can differ between trims of the same year.
    #[test]
    fn another_truck_of_the_same_year_gets_nothing() {
        let catalog = FeatureCatalog::embedded().unwrap();
        let feature = catalog.get("autolock_doors_when_driving").unwrap();

        let vin = |v: Option<&str>| feature.applies_to.matches(Some("Ford"), Some(2019), v);
        assert!(vin(Some("1FT7W2BT7KEF78036")));
        // One character different: a different truck.
        assert!(!vin(Some("1FT7W2BT7KEF78037")));
        // And an unread VIN is not a match, rather than a default.
        assert!(!vin(None));
    }
}
