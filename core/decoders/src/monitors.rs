//! Service 06 on-board monitor test results, made readable.
//!
//! The protocol layer hands over raw records — monitor id, test id, a scaling
//! id, and three 16-bit counts. This turns them into something a person can act
//! on, without inventing anything it does not know.
//!
//! Three rules, in descending order of how load-bearing they are:
//!
//! 1. **Pass/fail and margin never depend on the scaling.** The value and both
//!    limits arrive in the same unit, so the comparison is exact even when the
//!    unit is unknown. That is the useful part of service 06 and it is always
//!    trustworthy.
//! 2. **Scaled values are marked unverified.** The UAS table has not been
//!    validated against a real vehicle by this project, so a scaled number is
//!    supporting evidence, never a measurement.
//! 3. **An unknown monitor is named by its number.** Manufacturers define their
//!    own above 0xA0 and this project does not guess, exactly as a module at an
//!    unknown address is called "OBD module at 7EA".

use aim_protocols::obd2::MonitorTest;
use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one monitor id tests.
#[derive(Debug, Clone, Deserialize)]
struct MonitorDef {
    mid: u8,
    name: String,
    system: String,
}

/// How to turn a raw count into a physical value.
#[derive(Debug, Clone, Deserialize)]
struct UnitDef {
    uasid: u8,
    #[serde(default)]
    unit: Option<String>,
    scale: f64,
    #[serde(default)]
    offset: f64,
    #[serde(default)]
    signed: bool,
}

#[derive(Debug, Deserialize)]
struct MonitorFile {
    monitors: Vec<MonitorDef>,
    units: Vec<UnitDef>,
}

/// The service 06 catalogue.
#[derive(Debug, Clone)]
pub struct MonitorCatalog {
    monitors: BTreeMap<u8, MonitorDef>,
    units: BTreeMap<u8, UnitDef>,
}

/// One test result, interpreted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorReading {
    /// Monitor id as reported.
    pub mid: u8,
    /// Test id within that monitor.
    pub tid: u8,
    /// What this monitor tests, or a fallback naming the number.
    pub name: String,
    /// Which system it belongs to, when known.
    pub system: Option<String>,
    /// Whether the vehicle's own limits were met. Always trustworthy.
    pub passed: bool,
    /// How close to failing, as a fraction of the limit band. `None` when the
    /// band is degenerate.
    pub margin: Option<f64>,
    /// Scaled measured value, when the scaling id is known.
    pub value: Option<f64>,
    /// Scaled lower limit.
    pub min: Option<f64>,
    /// Scaled upper limit.
    pub max: Option<f64>,
    /// Unit of the scaled values, when the scaling declares one.
    pub unit: Option<String>,
    /// Raw counts, always present, because the scaling is unvalidated.
    pub raw: RawCounts,
    /// True when this build has no definition for the monitor id.
    pub unknown_monitor: bool,
    /// True when this build has no scaling for the unit id, so only the raw
    /// counts and the pass/fail verdict are meaningful.
    pub unknown_scaling: bool,
}

/// The three numbers exactly as the vehicle sent them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCounts {
    /// Measured value.
    pub value: u16,
    /// Lower limit.
    pub min: u16,
    /// Upper limit.
    pub max: u16,
    /// Unit and scaling id, so an unknown one can be looked up later.
    pub uasid: u8,
}

impl MonitorCatalog {
    /// Load the generic SAE catalogue embedded in the binary.
    pub fn generic_obd() -> AimResult<MonitorCatalog> {
        const SRC: &str =
            include_str!("../../../vehicle-profiles/generic-obd/monitors/mode06.yaml");
        let file: MonitorFile = serde_yaml_ng::from_str(SRC).map_err(|e| {
            AimError::new(ErrorCode::DecoderInputInvalid, format!("mode06.yaml is not valid: {e}"))
        })?;
        Ok(MonitorCatalog {
            monitors: file.monitors.into_iter().map(|m| (m.mid, m)).collect(),
            units: file.units.into_iter().map(|u| (u.uasid, u)).collect(),
        })
    }

    /// Interpret one raw test result.
    pub fn interpret(&self, t: &MonitorTest) -> MonitorReading {
        let def = self.monitors.get(&t.mid);
        let unit = self.units.get(&t.uasid);

        let scale = |raw: u16| -> Option<f64> {
            let u = unit?;
            // A signed scaling reads the same bits as two's complement.
            let n = if u.signed { raw as i16 as f64 } else { raw as f64 };
            Some(n * u.scale + u.offset)
        };

        MonitorReading {
            mid: t.mid,
            tid: t.tid,
            name: def
                .map(|d| d.name.clone())
                // Named by number rather than guessed at. Manufacturer-specific
                // monitors live above 0xA0 and this project has not validated
                // any of them.
                .unwrap_or_else(|| format!("Manufacturer monitor 0x{:02X}", t.mid)),
            system: def.map(|d| d.system.clone()),
            passed: t.passed(),
            margin: t.margin(),
            value: scale(t.value),
            min: scale(t.min),
            max: scale(t.max),
            unit: unit.and_then(|u| u.unit.clone()),
            raw: RawCounts { value: t.value, min: t.min, max: t.max, uasid: t.uasid },
            unknown_monitor: def.is_none(),
            unknown_scaling: unit.is_none(),
        }
    }

    /// Interpret a whole service 06 payload.
    pub fn interpret_all(&self, tests: &[MonitorTest]) -> Vec<MonitorReading> {
        tests.iter().map(|t| self.interpret(t)).collect()
    }

    /// How many monitors this build can name, for reporting coverage honestly.
    pub fn known_monitors(&self) -> usize {
        self.monitors.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MonitorCatalog {
        MonitorCatalog::generic_obd().unwrap()
    }

    fn test(mid: u8, uasid: u8, value: u16, min: u16, max: u16) -> MonitorTest {
        MonitorTest { mid, tid: 0x80, uasid, value, min, max }
    }

    #[test]
    fn a_known_monitor_is_named_and_scaled() {
        // Catalyst bank 1, volts at 0.001 per count.
        let r = catalog().interpret(&test(0x21, 0x0B, 580, 0, 600));
        assert_eq!(r.name, "Catalytic converter, bank 1");
        assert_eq!(r.system.as_deref(), Some("Emissions"));
        assert_eq!(r.unit.as_deref(), Some("V"));
        assert_eq!(r.value, Some(0.58));
        assert_eq!(r.max, Some(0.6));
        assert!(r.passed);
        assert!(!r.unknown_monitor);
        assert!(!r.unknown_scaling);
    }

    #[test]
    fn the_margin_warns_before_a_code_would() {
        // Passing, but three percent from the limit - the whole point of
        // service 06.
        let r = catalog().interpret(&test(0x21, 0x0B, 970, 0, 1000));
        assert!(r.passed);
        assert_eq!(r.margin, Some(0.03));
    }

    #[test]
    fn an_unknown_monitor_is_named_by_its_number_not_guessed() {
        let r = catalog().interpret(&test(0xC7, 0x0B, 10, 0, 100));
        assert_eq!(r.name, "Manufacturer monitor 0xC7");
        assert!(r.unknown_monitor);
        assert_eq!(r.system, None);
        // Still fully usable: the verdict does not need the name.
        assert!(r.passed);
    }

    #[test]
    fn an_unknown_scaling_keeps_the_verdict_and_drops_the_units() {
        // 0xFE is not in the table. The comparison is still exact because all
        // three numbers share whatever unit it is.
        let r = catalog().interpret(&test(0x21, 0xFE, 58, 0, 60));
        assert!(r.unknown_scaling);
        assert_eq!(r.value, None);
        assert_eq!(r.unit, None);
        assert!(r.passed, "pass/fail never depends on the scaling");
        // 2 counts of headroom out of a 60-count band.
        assert_eq!(r.margin, Some(2.0 / 60.0));
        // Raw counts survive so the reading can still be checked by hand.
        assert_eq!(r.raw.value, 58);
        assert_eq!(r.raw.max, 60);
        assert_eq!(r.raw.uasid, 0xFE);
    }

    #[test]
    fn a_failing_test_is_reported_as_failing() {
        let r = catalog().interpret(&test(0x31, 0x0B, 700, 0, 600));
        assert!(!r.passed);
        assert_eq!(r.margin, Some(0.0));
        assert_eq!(r.name, "Exhaust gas recirculation, bank 1");
    }

    #[test]
    fn a_signed_scaling_reads_negative_temperatures() {
        // UASID 0x16 is degC at 0.1 with a -40 offset, read signed.
        let r = catalog().interpret(&test(0xA1, 0x16, 0, 0, 1000));
        assert_eq!(r.unit.as_deref(), Some("degC"));
        assert_eq!(r.value, Some(-40.0));
    }

    #[test]
    fn the_catalogue_loads_and_covers_the_sae_monitors() {
        let c = catalog();
        // Sanity: the SAE-defined range is present rather than an empty file
        // silently loading.
        assert!(c.known_monitors() >= 30, "only {} monitors", c.known_monitors());
        assert_eq!(c.interpret(&test(0x86, 0x01, 1, 0, 2)).name, "Particulate filter, bank 1");
    }
}
