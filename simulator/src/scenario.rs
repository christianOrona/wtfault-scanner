//! Injectable fault scenarios.
//!
//! A scenario is the whole behaviour of the simulated truck: what its sensors
//! read over time, which codes are stored, whether the bus answers at all.
//! Scenarios are selected at startup (`--scenario dpf-regen`) so that the same
//! binary can reproduce a healthy truck, a truck in particulate-filter
//! regeneration, or an adapter plugged into a vehicle that is not talking.
//!
//! # Modelling honesty
//!
//! The scenarios model *generic OBD-II observable behaviour* — coolant
//! temperature rising, exhaust temperature climbing during a regeneration,
//! generic SAE codes being stored. They do not model Ford-specific modules,
//! PIDs, or service procedures, because this project has not validated any.
//! The DPF scenario is a plausible shape for a diesel particulate filter
//! event, not a recording of one.

use crate::state::{approach, wobble, VehicleState};
use aim_types::DtcStatus;
use serde::{Deserialize, Serialize};

/// A stored diagnostic trouble code in a scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimDtc {
    /// SAE J2012 code, e.g. `P2463`.
    pub code: String,
    /// Which service reports it.
    pub status: DtcStatus,
}

impl SimDtc {
    /// A confirmed (service 03) code.
    pub fn confirmed(code: &str) -> Self {
        SimDtc { code: code.to_string(), status: DtcStatus::Confirmed }
    }

    /// A pending (service 07) code.
    pub fn pending(code: &str) -> Self {
        SimDtc { code: code.to_string(), status: DtcStatus::Pending }
    }

    /// A permanent (service 0A) code.
    pub fn permanent(code: &str) -> Self {
        SimDtc { code: code.to_string(), status: DtcStatus::Permanent }
    }
}

/// One service 06 on-board monitor test result in a scenario.
///
/// The numbers here are shaped to exercise the decoding, not copied from any
/// vehicle. What they demonstrate is the property the feature exists for: a
/// result can sit inside its own limits — so no code is stored and nothing is
/// wrong yet — while being close enough to the limit that it is worth saying so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimMonitorTest {
    /// Monitor id.
    pub mid: u8,
    /// Test id within that monitor.
    pub tid: u8,
    /// Unit-and-scaling id.
    pub uasid: u8,
    /// Measured value, in raw counts.
    pub value: u16,
    /// Lower limit, in raw counts.
    pub min: u16,
    /// Upper limit, in raw counts.
    pub max: u16,
}

impl SimMonitorTest {
    /// A result, in raw counts.
    pub fn new(mid: u8, tid: u8, uasid: u8, value: u16, min: u16, max: u16) -> Self {
        SimMonitorTest { mid, tid, uasid, value, min, max }
    }
}

/// Which scenario the simulator is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioId {
    /// Warm engine, no faults, nothing stored.
    Healthy,
    /// Particulate filter regeneration in progress, with related generic codes
    /// stored and the MIL on.
    DpfRegen,
    /// The adapter works but the vehicle never answers — key off, unplugged
    /// bus, or a truck that does not speak the negotiated protocol.
    BusSilent,
}

impl ScenarioId {
    /// Parse the command-line spelling.
    pub fn parse(s: &str) -> Option<ScenarioId> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "healthy" => Some(ScenarioId::Healthy),
            "dpf-regen" | "dpf" | "regen" => Some(ScenarioId::DpfRegen),
            "bus-silent" | "silent" | "no-vehicle" => Some(ScenarioId::BusSilent),
            _ => None,
        }
    }

    /// The command-line spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScenarioId::Healthy => "healthy",
            ScenarioId::DpfRegen => "dpf-regen",
            ScenarioId::BusSilent => "bus-silent",
        }
    }

    /// Every scenario, for `--list-scenarios` and the API.
    pub fn all() -> [ScenarioId; 3] {
        [ScenarioId::Healthy, ScenarioId::DpfRegen, ScenarioId::BusSilent]
    }
}

/// A fully described scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    /// Which scenario this is.
    pub id: ScenarioId,
    /// One-line description, safe to show in a picker.
    pub description: String,
    /// Codes the engine controller reports.
    pub dtcs: Vec<SimDtc>,
    /// Whether the vehicle answers requests at all.
    pub vehicle_answers: bool,
    /// State captured when the first confirmed code was stored, served by
    /// service 02. `None` when nothing is stored.
    pub freeze_frame: Option<VehicleState>,
    /// Service 06 on-board monitor test results the engine controller reports.
    pub monitor_tests: Vec<SimMonitorTest>,
}

impl Scenario {
    /// Build a scenario from its id.
    pub fn new(id: ScenarioId) -> Scenario {
        match id {
            ScenarioId::Healthy => Scenario {
                id,
                description: String::from(
                    "Warm engine at idle, no stored codes, particulate filter loading normally",
                ),
                dtcs: Vec::new(),
                vehicle_answers: true,
                freeze_frame: None,
                // Everything comfortably inside its limits, so the monitor
                // panel has a "nothing to say" case to render.
                monitor_tests: vec![
                    SimMonitorTest::new(0x71, 0x80, 0x24, 30, 0, 200),
                    SimMonitorTest::new(0x86, 0x80, 0x1A, 4, 0, 40),
                    SimMonitorTest::new(0x86, 0x81, 0x24, 120, 0, 900),
                ],
            },
            ScenarioId::DpfRegen => {
                // The freeze frame is the state at the moment the code was
                // stored, which is not the state now — that difference is the
                // whole point of a freeze frame, so it is modelled explicitly
                // rather than by reusing the live state.
                let mut ff = Scenario::state_for(ScenarioId::DpfRegen, 120.0);
                ff.rpm = 748.0;
                ff.egt_c = 302.0;
                ff.dpf_inlet_c = 268.0;
                ff.dpf_soot_pct = 96.0;
                ff.coolant_c = 89.0;
                ff.regen_active = false;
                Scenario {
                    id,
                    description: String::from(
                        "Particulate filter regeneration in progress: exhaust temperatures \
                         elevated, soot loading high, generic filter codes stored",
                    ),
                    dtcs: vec![
                        SimDtc::confirmed("P2463"),
                        SimDtc::confirmed("P242F"),
                        SimDtc::pending("P2002"),
                        SimDtc::permanent("P2463"),
                    ],
                    vehicle_answers: true,
                    freeze_frame: Some(ff),
                    // The point of this set: MID 0x86 test 0x80 is *passing*,
                    // so no code exists for it, yet it sits at 38 of a 40-count
                    // limit. That is the reading service 06 exists to give and
                    // that a code reader cannot. Test 0x82 has actually failed,
                    // so the panel shows both states side by side. MID 0xC4 is
                    // deliberately not in the catalogue: it exercises the path
                    // where an unnamed monitor is still reported by number with
                    // its verdict intact.
                    monitor_tests: vec![
                        SimMonitorTest::new(0x71, 0x80, 0x24, 96, 0, 200),
                        SimMonitorTest::new(0x86, 0x80, 0x1A, 38, 0, 40),
                        SimMonitorTest::new(0x86, 0x82, 0x24, 940, 0, 900),
                        SimMonitorTest::new(0xC4, 0x80, 0xFE, 12, 0, 100),
                    ],
                }
            }
            ScenarioId::BusSilent => Scenario {
                id,
                description: String::from(
                    "Adapter responds but the vehicle does not answer any request",
                ),
                dtcs: Vec::new(),
                vehicle_answers: false,
                freeze_frame: None,
                monitor_tests: Vec::new(),
            },
        }
    }

    /// Confirmed codes only — what service 03 reports.
    pub fn dtcs_with_status(&self, status: DtcStatus) -> Vec<&SimDtc> {
        self.dtcs.iter().filter(|d| d.status == status).collect()
    }

    /// The vehicle's physical state `t_s` seconds into the session.
    pub fn state_at(&self, t_s: f64) -> VehicleState {
        Scenario::state_for(self.id, t_s)
    }

    fn state_for(id: ScenarioId, t: f64) -> VehicleState {
        let mut s = VehicleState::cold_idle();
        s.run_time_s = t;

        match id {
            ScenarioId::Healthy | ScenarioId::BusSilent => {
                // A warm truck idling in a workshop bay.
                s.rpm = 712.0 + 22.0 * wobble(t, 0.21, 0.043);
                s.coolant_c = approach(64.0, 88.0, t, 140.0) + 0.8 * wobble(t, 0.017, 0.0061);
                s.oil_temp_c = approach(58.0, 94.0, t, 260.0) + 0.6 * wobble(t, 0.013, 0.005);
                s.intake_air_c = 24.0 + 1.4 * wobble(t, 0.011, 0.0037);
                s.ambient_c = 16.0 + 0.4 * wobble(t, 0.004, 0.0013);
                s.map_kpa = 102.0 + 1.6 * wobble(t, 0.19, 0.037);
                s.maf_gs = 13.5 + 1.1 * wobble(t, 0.23, 0.047);
                s.load_pct = 21.0 + 2.5 * wobble(t, 0.17, 0.039);
                s.absolute_load_pct = 18.0 + 2.0 * wobble(t, 0.17, 0.039);
                s.pedal_pct = 0.0;
                s.battery_v = 14.12 + 0.06 * wobble(t, 0.33, 0.071);
                s.fuel_rail_kpa = 34_500.0 + 900.0 * wobble(t, 0.27, 0.053);
                s.fuel_rate_lph = 2.4 + 0.25 * wobble(t, 0.19, 0.041);
                s.egr_pct = 22.0 + 3.0 * wobble(t, 0.09, 0.021);
                s.egt_c = approach(180.0, 268.0, t, 90.0) + 6.0 * wobble(t, 0.07, 0.019);
                s.dpf_inlet_c = approach(150.0, 232.0, t, 120.0) + 5.0 * wobble(t, 0.06, 0.017);
                // Soot accumulates slowly and never triggers anything here.
                s.dpf_soot_pct = (14.0 + t * 0.004).min(35.0);
                s.torque_pct = 9.0 + 1.5 * wobble(t, 0.17, 0.039);
                s.demand_torque_pct = 8.0 + 1.5 * wobble(t, 0.17, 0.039);
                s.distance_cleared_km = 4821;
                s.warmups = 41;
            }
            ScenarioId::DpfRegen => {
                // Active regeneration: the controller raises idle speed and
                // post-injects to drive exhaust temperature up, so the tell is
                // high, still-climbing exhaust temperatures at a fast idle.
                s.rpm = 1094.0 + 28.0 * wobble(t, 0.19, 0.041);
                s.coolant_c = approach(86.0, 94.0, t, 90.0) + 0.7 * wobble(t, 0.015, 0.006);
                s.oil_temp_c = approach(92.0, 108.0, t, 200.0) + 0.9 * wobble(t, 0.012, 0.005);
                s.intake_air_c = 31.0 + 1.6 * wobble(t, 0.011, 0.0037);
                s.ambient_c = 16.0 + 0.4 * wobble(t, 0.004, 0.0013);
                s.map_kpa = 118.0 + 3.2 * wobble(t, 0.19, 0.037);
                s.maf_gs = 22.8 + 1.7 * wobble(t, 0.23, 0.047);
                s.load_pct = 38.0 + 3.5 * wobble(t, 0.17, 0.039);
                s.absolute_load_pct = 34.0 + 3.0 * wobble(t, 0.17, 0.039);
                s.pedal_pct = 0.0;
                s.battery_v = 14.05 + 0.07 * wobble(t, 0.33, 0.071);
                s.fuel_rail_kpa = 61_200.0 + 1_800.0 * wobble(t, 0.27, 0.053);
                s.fuel_rate_lph = 5.9 + 0.4 * wobble(t, 0.19, 0.041);
                // EGR is closed down during regeneration.
                s.egr_pct = 3.0 + 1.0 * wobble(t, 0.09, 0.021);
                s.egt_c = approach(420.0, 612.0, t, 150.0) + 11.0 * wobble(t, 0.07, 0.019);
                s.dpf_inlet_c = approach(380.0, 578.0, t, 170.0) + 9.0 * wobble(t, 0.06, 0.017);
                // Soot burns off as the regeneration proceeds.
                s.dpf_soot_pct = (96.0 - t * 0.055).max(22.0);
                s.regen_active = true;
                s.torque_pct = 17.0 + 2.0 * wobble(t, 0.17, 0.039);
                s.demand_torque_pct = 8.0 + 1.5 * wobble(t, 0.17, 0.039);
                s.mil_on = true;
                s.dtc_count = 2;
                s.distance_mil_km = 213;
                s.distance_cleared_km = 11_640;
                s.warmups = 96;
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_ids_round_trip_through_their_command_line_spelling() {
        for id in ScenarioId::all() {
            assert_eq!(ScenarioId::parse(id.as_str()), Some(id));
        }
        assert_eq!(ScenarioId::parse("DPF_REGEN"), Some(ScenarioId::DpfRegen));
        assert_eq!(ScenarioId::parse("nope"), None);
    }

    #[test]
    fn state_is_a_pure_function_of_time() {
        let s = Scenario::new(ScenarioId::DpfRegen);
        assert_eq!(s.state_at(37.5), s.state_at(37.5));
        assert_ne!(s.state_at(37.5), s.state_at(38.5));
    }

    #[test]
    fn the_healthy_scenario_stores_nothing_and_keeps_the_lamp_off() {
        let s = Scenario::new(ScenarioId::Healthy);
        assert!(s.dtcs.is_empty());
        assert!(s.freeze_frame.is_none());
        let st = s.state_at(300.0);
        assert!(!st.mil_on);
        assert_eq!(st.dtc_count, 0);
    }

    #[test]
    fn regeneration_looks_like_regeneration() {
        let healthy = Scenario::new(ScenarioId::Healthy).state_at(600.0);
        let regen = Scenario::new(ScenarioId::DpfRegen).state_at(600.0);
        // Raised idle, much hotter exhaust, filter burning clean.
        assert!(regen.rpm > healthy.rpm + 250.0);
        assert!(regen.egt_c > healthy.egt_c + 250.0);
        assert!(regen.dpf_inlet_c > 500.0);
        assert!(regen.regen_active);
        assert!(regen.mil_on);
        assert!(
            regen.dpf_soot_pct < Scenario::new(ScenarioId::DpfRegen).state_at(0.0).dpf_soot_pct
        );
    }

    #[test]
    fn the_regen_scenario_stores_codes_across_all_three_dtc_services() {
        let s = Scenario::new(ScenarioId::DpfRegen);
        assert_eq!(s.dtcs_with_status(DtcStatus::Confirmed).len(), 2);
        assert_eq!(s.dtcs_with_status(DtcStatus::Pending).len(), 1);
        assert_eq!(s.dtcs_with_status(DtcStatus::Permanent).len(), 1);
    }

    #[test]
    fn the_freeze_frame_differs_from_the_live_state() {
        let s = Scenario::new(ScenarioId::DpfRegen);
        let ff = s.freeze_frame.clone().unwrap();
        let live = s.state_at(600.0);
        assert!(ff.egt_c < live.egt_c);
        assert!(ff.dpf_soot_pct > live.dpf_soot_pct);
        assert!(!ff.regen_active);
    }

    #[test]
    fn the_silent_scenario_does_not_answer() {
        assert!(!Scenario::new(ScenarioId::BusSilent).vehicle_answers);
        assert!(Scenario::new(ScenarioId::Healthy).vehicle_answers);
    }

    #[test]
    fn physical_values_stay_inside_plausible_bounds_over_a_long_session() {
        for id in [ScenarioId::Healthy, ScenarioId::DpfRegen] {
            let s = Scenario::new(id);
            for i in 0..2000 {
                let st = s.state_at(i as f64 * 0.5);
                assert!((600.0..=1400.0).contains(&st.rpm), "{id:?} rpm {}", st.rpm);
                assert!((50.0..=120.0).contains(&st.coolant_c));
                assert!((11.0..=15.0).contains(&st.battery_v));
                assert!((0.0..=900.0).contains(&st.egt_c));
                assert!((0.0..=100.0).contains(&st.dpf_soot_pct));
            }
        }
    }
}
