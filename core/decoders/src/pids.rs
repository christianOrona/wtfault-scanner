//! Data-file-driven PID decoding.
//!
//! A [`PidRegistry`] is built from YAML files under `vehicle-profiles/`. There
//! is no `match pid { 0x0C => ... }` anywhere in this crate: adding a PID means
//! adding a data-file entry, and every entry declares whether it has been
//! validated.

use crate::expr::Formula;
use aim_types::{
    hex, AimError, AimResult, DecodedValue, ErrorCode, Provenance, SourceKind, Timestamp,
    ValidRange, Value, VerificationStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a PID's payload should be interpreted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PidKind {
    /// Scalar with a scaling formula.
    Numeric,
    /// A 4-byte "PIDs supported" bitmask.
    SupportedPids,
    /// Named bits, optionally with derived numeric signals.
    Bitfield,
    /// A single byte selecting a label from a table.
    Enum,
    /// The 17-character VIN.
    Vin,
    /// One or more fixed-width ASCII records.
    Ascii,
    /// One or more fixed-width records reported as hex.
    HexRecords,
}

/// One named bit within a bitfield PID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagDef {
    /// Stable flag id.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Payload byte index.
    pub byte: usize,
    /// Bit position, 0 = least significant.
    pub bit: u8,
}

/// An extra numeric signal computed from the same payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedDef {
    /// Stable signal id.
    pub signal_id: String,
    /// Human-readable name.
    pub name: String,
    /// Scaling formula.
    pub formula: String,
    /// Unit symbol.
    #[serde(default)]
    pub unit: Option<String>,
    /// Declared plausible range.
    #[serde(default)]
    pub range: Option<ValidRange>,
}

/// A PID definition as it appears in a data file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PidDefinition {
    /// PID / info-type number.
    pub pid: u8,
    /// Stable signal id used throughout the API.
    pub signal_id: String,
    /// Human-readable name.
    pub name: String,
    /// Interpretation strategy.
    pub kind: PidKind,
    /// Expected payload length in bytes, where fixed.
    #[serde(default)]
    pub bytes: Option<usize>,
    /// Unit symbol.
    #[serde(default)]
    pub unit: Option<String>,
    /// Scaling formula for `numeric` PIDs.
    #[serde(default)]
    pub formula: Option<String>,
    /// Declared plausible range.
    #[serde(default)]
    pub range: Option<ValidRange>,
    /// Validation status. Defaults to `unverified` when a file omits it —
    /// fail closed rather than assume a definition has been checked.
    #[serde(default)]
    pub verification: VerificationStatus,
    /// Free-form note, surfaced with unverified values.
    #[serde(default)]
    pub notes: Option<String>,
    /// Named bits for `bitfield` PIDs.
    #[serde(default)]
    pub flags: Vec<FlagDef>,
    /// Extra numeric signals from the same payload.
    #[serde(default)]
    pub derived: Vec<DerivedDef>,
    /// Which byte holds the selector for `enum` PIDs.
    #[serde(default)]
    pub enum_byte: Option<usize>,
    /// Label table for `enum` PIDs.
    #[serde(default)]
    pub values: BTreeMap<u32, String>,
    /// Record width for `ascii` / `hex_records` PIDs.
    #[serde(default)]
    pub record_width: Option<usize>,
}

/// The top-level shape of a PID data file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PidFile {
    /// Data-file version, recorded in every value's provenance.
    pub version: u32,
    /// Where the definitions came from.
    pub source: String,
    /// OBD-II service these definitions belong to.
    pub service: u8,
    /// The definitions.
    pub pids: Vec<PidDefinition>,
}

/// A definition plus its compiled formulas and its file's version.
#[derive(Debug, Clone)]
pub struct CompiledPid {
    /// The data-file definition.
    pub def: PidDefinition,
    /// Version of the file it came from.
    pub file_version: u32,
    formula: Option<Formula>,
    derived: Vec<(DerivedDef, Formula)>,
}

impl CompiledPid {
    /// Decoder id recorded in provenance, e.g. `obd2.mode01.pid0C`.
    pub fn decoder_id(&self, service: u8) -> String {
        format!("obd2.mode{:02X}.pid{:02X}", service, self.def.pid)
    }
}

/// All PID definitions for all services, indexed by `(service, pid)`.
#[derive(Debug, Clone, Default)]
pub struct PidRegistry {
    by_service_pid: BTreeMap<(u8, u8), CompiledPid>,
    by_signal_id: BTreeMap<String, (u8, u8)>,
    sources: Vec<String>,
}

impl PidRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the generic SAE definitions embedded in the binary.
    ///
    /// Embedding keeps the core self-contained: the API server has no runtime
    /// dependency on the checkout layout, but the same files can be reloaded
    /// from disk with [`PidRegistry::load_yaml`] for profile development.
    pub fn generic_obd() -> AimResult<PidRegistry> {
        let mut r = PidRegistry::new();
        r.load_yaml(include_str!("../../../vehicle-profiles/generic-obd/pids/mode01.yaml"))?;
        r.load_yaml(include_str!("../../../vehicle-profiles/generic-obd/pids/mode09.yaml"))?;
        Ok(r)
    }

    /// Merge one YAML data file into the registry.
    pub fn load_yaml(&mut self, yaml: &str) -> AimResult<()> {
        let file: PidFile = serde_yaml_ng::from_str(yaml).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("could not parse PID data file: {e}"),
            )
        })?;
        for def in file.pids {
            let formula = match &def.formula {
                Some(src) => Some(Formula::parse(src)?),
                None => None,
            };
            if def.kind == PidKind::Numeric && formula.is_none() {
                return Err(AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    format!("numeric PID {} has no formula", def.signal_id),
                ));
            }
            let mut derived = Vec::new();
            for d in &def.derived {
                derived.push((d.clone(), Formula::parse(&d.formula)?));
            }
            let key = (file.service, def.pid);
            if self.by_service_pid.contains_key(&key) {
                return Err(AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    format!(
                        "duplicate definition for service {:02X} PID {:02X}",
                        file.service, def.pid
                    ),
                ));
            }
            self.by_signal_id.insert(def.signal_id.clone(), key);
            self.by_service_pid
                .insert(key, CompiledPid { def, file_version: file.version, formula, derived });
        }
        self.sources.push(file.source);
        Ok(())
    }

    /// Load every `*.yaml` file in a directory tree. Used for profile
    /// development against a working copy rather than the embedded copy.
    pub fn load_dir(&mut self, dir: &std::path::Path) -> AimResult<usize> {
        let mut count = 0;
        let entries = std::fs::read_dir(dir).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderNotFound,
                format!("could not read PID directory {}: {e}", dir.display()),
            )
        })?;
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                let text = std::fs::read_to_string(&path).map_err(|e| {
                    AimError::new(
                        ErrorCode::DecoderNotFound,
                        format!("could not read {}: {e}", path.display()),
                    )
                })?;
                self.load_yaml(&text)?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Number of definitions loaded.
    pub fn len(&self) -> usize {
        self.by_service_pid.len()
    }

    /// True when nothing has been loaded.
    pub fn is_empty(&self) -> bool {
        self.by_service_pid.is_empty()
    }

    /// The data-file sources that were merged in.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Look up by service and PID.
    pub fn get(&self, service: u8, pid: u8) -> Option<&CompiledPid> {
        self.by_service_pid.get(&(service, pid))
    }

    /// Every definition loaded, in service and PID order.
    pub fn iter(&self) -> impl Iterator<Item = &PidDefinition> {
        self.by_service_pid.values().map(|c| &c.def)
    }

    /// Look up by stable signal id.
    pub fn by_signal(&self, signal_id: &str) -> Option<&CompiledPid> {
        self.by_signal_id.get(signal_id).and_then(|k| self.by_service_pid.get(k))
    }

    /// The `(service, pid)` pair behind a signal id.
    pub fn address_of(&self, signal_id: &str) -> Option<(u8, u8)> {
        self.by_signal_id.get(signal_id).copied()
    }

    /// Every definition for a service, in PID order.
    pub fn for_service(&self, service: u8) -> Vec<&CompiledPid> {
        self.by_service_pid.iter().filter(|((s, _), _)| *s == service).map(|(_, v)| v).collect()
    }

    /// Decode a payload for one PID into one or more values.
    ///
    /// A single PID can yield several signals (a bitfield with derived
    /// counters), which is why this returns a `Vec`.
    pub fn decode(
        &self,
        service: u8,
        pid: u8,
        payload: &[u8],
        observed_at: Timestamp,
    ) -> AimResult<Vec<DecodedValue>> {
        let c = self.get(service, pid).ok_or_else(|| {
            AimError::new(
                ErrorCode::DecoderNotFound,
                format!("no decoder definition for service {service:02X} PID {pid:02X}"),
            )
        })?;
        let decoder_id = c.decoder_id(service);
        let version = c.file_version.to_string();
        let prov = |bytes: &[u8]| {
            Provenance::decoded(
                bytes,
                decoder_id.clone(),
                version.clone(),
                c.def.verification,
                observed_at,
            )
        };

        if let Some(expected) = c.def.bytes {
            // Short payloads are a hard error; long ones are common (padding)
            // and are simply not consumed.
            if payload.len() < expected && c.def.kind != PidKind::Ascii {
                return Err(AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    format!(
                        "{} expects at least {expected} payload bytes, got {}: {}",
                        c.def.signal_id,
                        payload.len(),
                        hex(payload)
                    ),
                )
                .with_provenance(prov(payload)));
            }
        }

        let mut out = Vec::new();
        match c.def.kind {
            PidKind::Numeric => {
                let f = c.formula.as_ref().expect("checked at load time");
                let v = f.eval(payload)?;
                out.push(DecodedValue::new(
                    c.def.signal_id.clone(),
                    c.def.name.clone(),
                    Value::Number(round6(v)),
                    c.def.unit.clone(),
                    c.def.range,
                    prov(payload),
                ));
            }
            PidKind::SupportedPids => {
                let pids = aim_protocols::obd2::decode_supported_pids(pid, payload)?;
                out.push(DecodedValue::new(
                    c.def.signal_id.clone(),
                    c.def.name.clone(),
                    Value::Text(
                        pids.iter().map(|p| format!("{p:02X}")).collect::<Vec<_>>().join(","),
                    ),
                    None,
                    None,
                    prov(&payload[..4]),
                ));
            }
            PidKind::Bitfield => {
                let flags: Vec<aim_types::value::Flag> = c
                    .def
                    .flags
                    .iter()
                    .filter(|f| f.byte < payload.len())
                    .map(|f| aim_types::value::Flag {
                        id: f.id.clone(),
                        label: f.label.clone(),
                        set: payload[f.byte] & (1 << f.bit) != 0,
                    })
                    .collect();
                out.push(DecodedValue::new(
                    c.def.signal_id.clone(),
                    c.def.name.clone(),
                    Value::Flags(flags),
                    None,
                    None,
                    prov(payload),
                ));
                for (d, f) in &c.derived {
                    let v = f.eval(payload)?;
                    out.push(DecodedValue::new(
                        d.signal_id.clone(),
                        d.name.clone(),
                        Value::Number(round6(v)),
                        d.unit.clone(),
                        d.range,
                        prov(payload),
                    ));
                }
            }
            PidKind::Enum => {
                let idx = c.def.enum_byte.unwrap_or(0);
                let raw = *payload.get(idx).ok_or_else(|| {
                    AimError::new(
                        ErrorCode::DecoderInputInvalid,
                        format!("{} needs payload byte {idx}", c.def.signal_id),
                    )
                })? as u32;
                let label = c
                    .def
                    .values
                    .get(&raw)
                    .cloned()
                    // An unmapped selector is reported honestly, not guessed at.
                    .unwrap_or_else(|| format!("unmapped value 0x{raw:02X}"));
                out.push(DecodedValue::new(
                    c.def.signal_id.clone(),
                    c.def.name.clone(),
                    Value::Text(label),
                    None,
                    None,
                    prov(payload),
                ));
            }
            PidKind::Vin => {
                let vin = aim_protocols::obd2::decode_vin(payload)?;
                out.push(DecodedValue::new(
                    c.def.signal_id.clone(),
                    c.def.name.clone(),
                    Value::Text(vin),
                    None,
                    None,
                    prov(payload),
                ));
            }
            PidKind::Ascii => {
                let width = c.def.record_width.unwrap_or(16);
                let records = aim_protocols::obd2::decode_ascii_records(payload, width);
                for (i, rec) in records.iter().enumerate() {
                    let id = if records.len() > 1 {
                        format!("{}_{}", c.def.signal_id, i + 1)
                    } else {
                        c.def.signal_id.clone()
                    };
                    out.push(DecodedValue::new(
                        id,
                        c.def.name.clone(),
                        Value::Text(rec.clone()),
                        None,
                        None,
                        prov(payload),
                    ));
                }
            }
            PidKind::HexRecords => {
                let width = c.def.record_width.unwrap_or(4);
                let body = if payload.len() % width == 1 { &payload[1..] } else { payload };
                for (i, chunk) in body.chunks(width).enumerate() {
                    let id = if body.len() > width {
                        format!("{}_{}", c.def.signal_id, i + 1)
                    } else {
                        c.def.signal_id.clone()
                    };
                    out.push(DecodedValue::new(
                        id,
                        c.def.name.clone(),
                        Value::Raw(hex(chunk)),
                        None,
                        None,
                        prov(chunk),
                    ));
                }
            }
        }
        if out.is_empty() {
            return Err(AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("{} produced no values from {}", c.def.signal_id, hex(payload)),
            ));
        }
        Ok(out)
    }
}

/// Round to six decimal places so serialized values do not carry float noise.
fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Provenance for a value that came from a profile data file rather than the
/// vehicle. Used by the vehicle-identification path.
pub fn profile_provenance(
    decoder_id: &str,
    version: &str,
    verification: VerificationStatus,
    observed_at: Timestamp,
) -> Provenance {
    Provenance {
        source: SourceKind::ProfileData,
        raw_hex: String::new(),
        decoder_id: decoder_id.to_string(),
        decoder_version: version.to_string(),
        verification,
        observed_at,
        evidence_ref: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> PidRegistry {
        PidRegistry::generic_obd().expect("embedded generic OBD definitions must load")
    }

    #[test]
    fn the_shipped_data_files_load_and_compile() {
        let r = registry();
        assert!(r.len() > 40, "loaded {} definitions", r.len());
        assert!(!r.is_empty());
        assert_eq!(r.sources().len(), 2);
        assert!(r.get(1, 0x0C).is_some());
        assert!(r.get(9, 0x02).is_some());
    }

    #[test]
    fn every_definition_declares_a_usable_shape() {
        let r = registry();
        for c in r.by_service_pid.values() {
            match c.def.kind {
                PidKind::Numeric => assert!(
                    c.formula.is_some(),
                    "{} is numeric but has no formula",
                    c.def.signal_id
                ),
                PidKind::Enum => assert!(
                    !c.def.values.is_empty(),
                    "{} is an enum with no value table",
                    c.def.signal_id
                ),
                PidKind::Bitfield => assert!(
                    !c.def.flags.is_empty() || !c.def.derived.is_empty(),
                    "{} is a bitfield with nothing to decode",
                    c.def.signal_id
                ),
                _ => {}
            }
        }
    }

    #[test]
    fn signal_ids_are_unique_and_addressable() {
        let r = registry();
        assert_eq!(r.address_of("engine_rpm"), Some((1, 0x0C)));
        assert_eq!(r.address_of("vin"), Some((9, 0x02)));
        assert_eq!(r.address_of("not_a_signal"), None);
        assert_eq!(r.by_signal("coolant_temp").unwrap().def.pid, 0x05);
    }

    #[test]
    fn numeric_pids_decode_to_documented_values() {
        let r = registry();
        let ts = aim_types::now();
        let v = &r.decode(1, 0x0C, &[0x1A, 0xF8], ts).unwrap()[0];
        assert_eq!(v.signal_id, "engine_rpm");
        assert_eq!(v.value, Value::Number(1726.0));
        assert_eq!(v.unit.as_deref(), Some("rpm"));
        assert!(!v.out_of_range);
        assert!(v.is_trustworthy());
        assert_eq!(v.provenance.raw_hex, "1af8");
        assert_eq!(v.provenance.decoder_id, "obd2.mode01.pid0C");

        let v = &r.decode(1, 0x05, &[0x5A], ts).unwrap()[0];
        assert_eq!(v.value, Value::Number(50.0));
        assert_eq!(v.unit.as_deref(), Some("degC"));
    }

    #[test]
    fn unverified_definitions_produce_untrustworthy_values() {
        let r = registry();
        let payload = [0x01, 0x0D, 0xAC, 0, 0, 0, 0, 0, 0];
        let v = &r.decode(1, 0x7C, &payload, aim_types::now()).unwrap()[0];
        assert_eq!(v.provenance.verification, VerificationStatus::Unverified);
        assert!(
            !v.is_trustworthy(),
            "an unverified DPF temperature must never be presented as fact"
        );
    }

    #[test]
    fn out_of_range_values_are_flagged_not_clamped() {
        let mut r = PidRegistry::new();
        r.load_yaml(
            r#"
version: 1
source: test
service: 1
pids:
  - pid: 0x05
    signal_id: coolant_temp
    name: Coolant
    kind: numeric
    bytes: 1
    unit: degC
    formula: "A * 10"
    range: { min: -40, max: 215 }
    verification: verified
"#,
        )
        .unwrap();
        let v = &r.decode(1, 0x05, &[0xFF], aim_types::now()).unwrap()[0];
        assert_eq!(v.value, Value::Number(2550.0), "value is reported as decoded");
        assert!(v.out_of_range);
        assert!(!v.is_trustworthy());
    }

    #[test]
    fn supported_pid_masks_decode_to_a_pid_list() {
        let r = registry();
        let v = &r.decode(1, 0x00, &[0xBE, 0x3F, 0xA8, 0x13], aim_types::now()).unwrap()[0];
        let text = v.value.as_str().unwrap();
        assert!(text.starts_with("01,03,04,05,06,07"), "got {text}");
    }

    #[test]
    fn bitfields_decode_flags_and_derived_counters() {
        let r = registry();
        // MIL on, 3 confirmed DTCs.
        let values = r.decode(1, 0x01, &[0x83, 0x07, 0x65, 0x00], aim_types::now()).unwrap();
        assert_eq!(values.len(), 2);
        match &values[0].value {
            Value::Flags(flags) => {
                assert_eq!(flags[0].id, "mil_on");
                assert!(flags[0].set);
            }
            other => panic!("expected flags, got {other:?}"),
        }
        assert_eq!(values[1].signal_id, "dtc_count");
        assert_eq!(values[1].value, Value::Number(3.0));
    }

    #[test]
    fn enums_report_unmapped_selectors_honestly() {
        let r = registry();
        let v = &r.decode(1, 0x03, &[0x02, 0x00], aim_types::now()).unwrap()[0];
        assert_eq!(v.value.as_str().unwrap(), "Closed loop, using oxygen sensor feedback");
        let v = &r.decode(1, 0x03, &[0x77, 0x00], aim_types::now()).unwrap()[0];
        assert_eq!(v.value.as_str().unwrap(), "unmapped value 0x77");
    }

    #[test]
    fn vin_decodes_through_the_registry() {
        let r = registry();
        let mut payload = vec![0x01];
        payload.extend_from_slice(b"1FT7W2BT6KEC00001");
        let v = &r.decode(9, 0x02, &payload, aim_types::now()).unwrap()[0];
        assert_eq!(v.value.as_str().unwrap(), "1FT7W2BT6KEC00001");
    }

    #[test]
    fn unknown_pids_are_a_decoder_not_found_error() {
        let r = registry();
        let err = r.decode(1, 0xEE, &[0x00], aim_types::now()).unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderNotFound);
    }

    #[test]
    fn short_payloads_are_rejected_with_provenance_attached() {
        let r = registry();
        let err = r.decode(1, 0x0C, &[0x1A], aim_types::now()).unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
        assert_eq!(err.provenance.unwrap().raw_hex, "1a");
    }

    #[test]
    fn duplicate_pid_definitions_are_rejected_at_load() {
        let mut r = PidRegistry::new();
        let yaml = r#"
version: 1
source: test
service: 1
pids:
  - { pid: 0x05, signal_id: a, name: A, kind: numeric, bytes: 1, formula: "A", verification: verified }
"#;
        r.load_yaml(yaml).unwrap();
        assert!(r.load_yaml(yaml).is_err(), "second load must collide");
    }

    #[test]
    fn a_numeric_pid_without_a_formula_is_rejected_at_load() {
        let mut r = PidRegistry::new();
        let err = r
            .load_yaml(
                r#"
version: 1
source: test
service: 1
pids:
  - { pid: 0x05, signal_id: a, name: A, kind: numeric, bytes: 1, verification: verified }
"#,
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
    }

    #[test]
    fn omitting_verification_defaults_to_unverified() {
        let mut r = PidRegistry::new();
        r.load_yaml(
            r#"
version: 1
source: test
service: 1
pids:
  - { pid: 0x05, signal_id: a, name: A, kind: numeric, bytes: 1, formula: "A" }
"#,
        )
        .unwrap();
        let v = &r.decode(1, 0x05, &[1], aim_types::now()).unwrap()[0];
        assert_eq!(v.provenance.verification, VerificationStatus::Unverified);
    }

    #[test]
    fn definitions_can_also_be_loaded_from_the_working_copy() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vehicle-profiles/generic-obd/pids");
        let mut r = PidRegistry::new();
        let n = r.load_dir(&dir).unwrap();
        assert_eq!(n, 2);
        assert_eq!(r.len(), registry().len(), "disk and embedded copies agree");
    }
}
