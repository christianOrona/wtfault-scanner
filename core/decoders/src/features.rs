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
        /// Diagnostic request address of the module that owns the setting,
        /// e.g. `0x726`.
        module: u16,
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
        match *self {
            Mapping::DataIdentifierBits { module, did, byte, mask, on, off } => {
                Some(DataIdentifierTarget { module, did, byte, mask, on, off })
            }
            Mapping::AsBuiltBits { .. } => None,
        }
    }
}

/// A mapping resolved to something a UDS request can be built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataIdentifierTarget {
    /// Module diagnostic request address.
    pub module: u16,
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
    #[serde(default)]
    pub verification: VerificationStatus,
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
}

impl Applicability {
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

impl FeatureDef {
    /// What this build can actually do with this feature.
    pub fn support(&self) -> FeatureSupport {
        match (&self.mapping, self.verification) {
            (None, _) => FeatureSupport::DescribedOnly,
            (Some(_), VerificationStatus::Verified) => FeatureSupport::Writable,
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
    pub fn embedded() -> AimResult<FeatureCatalog> {
        let mut c = FeatureCatalog::default();
        c.load_yaml(
            include_str!("../../../vehicle-profiles/ford/features.yaml"),
            "embedded:ford/features.yaml",
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

    #[test]
    fn the_shipped_catalogue_loads() {
        let c = catalog();
        assert!(!c.is_empty());
        assert!(c.get("mirror_auto_fold").is_some());
    }

    #[test]
    fn no_shipped_feature_claims_a_verified_mapping() {
        // The honest position for this build. Every mapping here came from
        // public description rather than from a vehicle this project has
        // measured, so none of them may authorise a write. If this test ever
        // fails, someone added a mapping without verifying it.
        for f in catalog().all() {
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
            // Even fully verified.
            verification: VerificationStatus::Verified,
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
            module: 0x726,
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
            module: 0x726,
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
        let odd =
            DataIdentifierTarget { module: 1, did: 2, byte: 0, mask: 0b11, on: 0b01, off: 0b00 };
        assert_eq!(odd.current(&[0b10]), None);
        assert_eq!(odd.current(&[]), None);
    }

    #[test]
    fn an_as_built_mapping_is_not_executable() {
        let m = Mapping::AsBuiltBits { block: "726-01".into(), byte: 0, mask: 1, on: 1, off: 0 };
        assert!(m.as_data_identifier().is_none());
    }
}
