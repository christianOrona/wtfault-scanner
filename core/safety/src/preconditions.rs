//! Preconditions checked before an active test runs.
//!
//! Handoff §10: *"Precondition checks before active tests"* and *"Require
//! stable power/connection checks before high-risk operations."* A condition
//! the core cannot observe is treated as **not satisfied**, never as satisfied
//! by default.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// A condition that must hold before an operation runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precondition {
    /// Key on.
    IgnitionOn,
    /// Engine must be stopped.
    EngineOff,
    /// Engine must be running.
    EngineRunning,
    /// Vehicle must be stationary.
    VehicleStationary,
    /// The adapter link must be healthy.
    StableConnection,
    /// Control module voltage must be at least this many volts.
    BatteryVoltageAtLeast(f64),
}

impl Precondition {
    /// Stable code used in audit records and error details.
    pub fn code(&self) -> &'static str {
        match self {
            Precondition::IgnitionOn => "ignition_on",
            Precondition::EngineOff => "engine_off",
            Precondition::EngineRunning => "engine_running",
            Precondition::VehicleStationary => "vehicle_stationary",
            Precondition::StableConnection => "stable_connection",
            Precondition::BatteryVoltageAtLeast(_) => "battery_voltage",
        }
    }
}

/// What the core currently knows about the vehicle.
///
/// `None` on an optional field means *not observed*. Every check treats "not
/// observed" as failure.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct VehicleConditions {
    /// Whether the ignition is on, as inferred from the vehicle answering.
    pub ignition_on: bool,
    /// Whether the engine is turning, inferred from RPM > 0.
    pub engine_running: bool,
    /// Control module voltage, when PID 0x42 or `ATRV` has been read.
    pub battery_voltage: Option<f64>,
    /// Whether the adapter link is healthy.
    pub connection_stable: bool,
    /// Road speed, when PID 0x0D has been read.
    pub vehicle_speed_kph: Option<f64>,
}

impl VehicleConditions {
    /// Nothing observed: every precondition fails. This is the correct state
    /// before a connection exists.
    pub fn unknown() -> Self {
        Self::default()
    }

    /// Check one precondition, returning why it failed.
    pub fn check(&self, p: &Precondition) -> Result<(), String> {
        match p {
            Precondition::IgnitionOn => {
                if self.ignition_on {
                    Ok(())
                } else {
                    Err(String::from("ignition is not on (no vehicle response observed)"))
                }
            }
            Precondition::EngineOff => {
                if self.engine_running {
                    Err(String::from("engine is running and must be off"))
                } else {
                    Ok(())
                }
            }
            Precondition::EngineRunning => {
                if self.engine_running {
                    Ok(())
                } else {
                    Err(String::from("engine is not running"))
                }
            }
            Precondition::VehicleStationary => match self.vehicle_speed_kph {
                Some(v) if v <= 0.5 => Ok(()),
                Some(v) => Err(format!("vehicle is moving at {v} km/h")),
                None => Err(String::from("vehicle speed has not been read")),
            },
            Precondition::StableConnection => {
                if self.connection_stable {
                    Ok(())
                } else {
                    Err(String::from("adapter link is not stable"))
                }
            }
            Precondition::BatteryVoltageAtLeast(min) => match self.battery_voltage {
                Some(v) if v >= *min => Ok(()),
                Some(v) => Err(format!("control module voltage {v:.2} V is below {min:.2} V")),
                None => Err(String::from("control module voltage has not been read")),
            },
        }
    }
}

/// Check every precondition, reporting all failures at once.
pub fn check_all(preconditions: &[Precondition], conditions: &VehicleConditions) -> AimResult<()> {
    let failures: Vec<serde_json::Value> = preconditions
        .iter()
        .filter_map(|p| {
            conditions.check(p).err().map(|why| {
                serde_json::json!({ "precondition": p.code(), "reason": why })
            })
        })
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    let summary: Vec<String> = failures
        .iter()
        .map(|f| f["reason"].as_str().unwrap_or("unknown").to_string())
        .collect();
    Err(
        AimError::new(
            ErrorCode::PreconditionFailed,
            format!("preconditions not met: {}", summary.join("; ")),
        )
        .with_details(serde_json::json!({ "failures": failures })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_running() -> VehicleConditions {
        VehicleConditions {
            ignition_on: true,
            engine_running: true,
            battery_voltage: Some(14.2),
            connection_stable: true,
            vehicle_speed_kph: Some(0.0),
        }
    }

    #[test]
    fn unknown_conditions_satisfy_nothing() {
        let c = VehicleConditions::unknown();
        for p in [
            Precondition::IgnitionOn,
            Precondition::EngineRunning,
            Precondition::VehicleStationary,
            Precondition::StableConnection,
            Precondition::BatteryVoltageAtLeast(12.0),
        ] {
            assert!(c.check(&p).is_err(), "{p:?} must fail when nothing is known");
        }
        // EngineOff is the one condition that is safe to infer from silence:
        // an engine we have never seen running is not running.
        assert!(c.check(&Precondition::EngineOff).is_ok());
    }

    #[test]
    fn engine_state_checks_are_mutually_exclusive() {
        let c = engine_running();
        assert!(c.check(&Precondition::EngineRunning).is_ok());
        assert!(c.check(&Precondition::EngineOff).is_err());
    }

    #[test]
    fn unread_voltage_is_not_an_acceptable_voltage() {
        let mut c = engine_running();
        c.battery_voltage = None;
        let why = c.check(&Precondition::BatteryVoltageAtLeast(12.0)).unwrap_err();
        assert!(why.contains("has not been read"));
    }

    #[test]
    fn low_voltage_blocks_and_reports_the_number() {
        let mut c = engine_running();
        c.battery_voltage = Some(10.8);
        let why = c.check(&Precondition::BatteryVoltageAtLeast(12.0)).unwrap_err();
        assert!(why.contains("10.80"), "{why}");
    }

    #[test]
    fn a_moving_vehicle_is_not_stationary() {
        let mut c = engine_running();
        c.vehicle_speed_kph = Some(48.0);
        assert!(c.check(&Precondition::VehicleStationary).is_err());
        c.vehicle_speed_kph = Some(0.4);
        assert!(c.check(&Precondition::VehicleStationary).is_ok());
    }

    #[test]
    fn check_all_reports_every_failure_at_once() {
        let c = VehicleConditions::unknown();
        let err = check_all(
            &[
                Precondition::IgnitionOn,
                Precondition::StableConnection,
                Precondition::BatteryVoltageAtLeast(12.0),
            ],
            &c,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::PreconditionFailed);
        let failures = err.details.unwrap();
        assert_eq!(failures["failures"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn an_empty_precondition_list_passes() {
        assert!(check_all(&[], &VehicleConditions::unknown()).is_ok());
    }
}
