//! The physical state of the simulated vehicle, and its SAE encoding.
//!
//! Two things are kept strictly apart here, for the same reason the real stack
//! keeps them apart:
//!
//! * [`VehicleState`] is **physics** — degrees Celsius, rpm, kPa. It is what a
//!   sensor would read.
//! * [`encode_pid`] is **protocol** — the SAE J1979 byte encoding of those
//!   quantities.
//!
//! `encode_pid` is deliberately the exact inverse of the scaling formulas in
//! `vehicle-profiles/generic-obd/pids/mode01.yaml`. That is what makes the
//! end-to-end test meaningful: if either side drifts, decoded values stop
//! matching the physics the simulator intended and the round-trip test fails.

use std::f64::consts::PI;

/// Everything the virtual vehicle is currently doing, in physical units.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleState {
    /// Seconds since the engine started.
    pub run_time_s: f64,
    /// Engine speed, rpm.
    pub rpm: f64,
    /// Engine coolant temperature, degrees Celsius.
    pub coolant_c: f64,
    /// Engine oil temperature, degrees Celsius.
    pub oil_temp_c: f64,
    /// Intake air temperature, degrees Celsius.
    pub intake_air_c: f64,
    /// Ambient air temperature, degrees Celsius.
    pub ambient_c: f64,
    /// Intake manifold absolute pressure, kPa. On a turbodiesel this sits near
    /// barometric at idle and rises with boost.
    pub map_kpa: f64,
    /// Barometric pressure, kPa.
    pub baro_kpa: f64,
    /// Mass air flow, grams per second.
    pub maf_gs: f64,
    /// Calculated engine load, percent.
    pub load_pct: f64,
    /// Absolute load, percent.
    pub absolute_load_pct: f64,
    /// Road speed, km/h.
    pub speed_kph: f64,
    /// Accelerator pedal position, percent.
    pub pedal_pct: f64,
    /// Control module supply voltage, volts.
    pub battery_v: f64,
    /// Fuel tank level, percent.
    pub fuel_level_pct: f64,
    /// Common rail fuel pressure, kPa gauge.
    pub fuel_rail_kpa: f64,
    /// Engine fuel rate, litres per hour.
    pub fuel_rate_lph: f64,
    /// Commanded EGR, percent.
    pub egr_pct: f64,
    /// Exhaust gas temperature, bank 1 sensor 1, degrees Celsius.
    pub egt_c: f64,
    /// Diesel particulate filter inlet temperature, degrees Celsius.
    pub dpf_inlet_c: f64,
    /// Modelled DPF soot loading, percent. Not an OBD-II PID — it drives the
    /// scenario, and is exposed only through the temperatures it causes.
    pub dpf_soot_pct: f64,
    /// True while the scenario is running a particulate filter regeneration.
    pub regen_active: bool,
    /// Malfunction indicator lamp state.
    pub mil_on: bool,
    /// Confirmed DTC count reported in PID 01.
    pub dtc_count: u8,
    /// Distance travelled with the MIL on, km.
    pub distance_mil_km: u16,
    /// Distance since codes were cleared, km.
    pub distance_cleared_km: u16,
    /// Warm-up cycles since codes were cleared.
    pub warmups: u8,
    /// Actual engine torque, percent of reference.
    pub torque_pct: f64,
    /// Driver demanded torque, percent of reference.
    pub demand_torque_pct: f64,
    /// Reference engine torque, Nm.
    pub reference_torque_nm: u16,
}

impl VehicleState {
    /// A stopped, cold vehicle. Scenarios build on this.
    pub fn cold_idle() -> Self {
        VehicleState {
            run_time_s: 0.0,
            rpm: 0.0,
            coolant_c: 18.0,
            oil_temp_c: 18.0,
            intake_air_c: 18.0,
            ambient_c: 16.0,
            map_kpa: 101.0,
            baro_kpa: 101.0,
            maf_gs: 0.0,
            load_pct: 0.0,
            absolute_load_pct: 0.0,
            speed_kph: 0.0,
            pedal_pct: 0.0,
            battery_v: 12.4,
            fuel_level_pct: 62.0,
            fuel_rail_kpa: 0.0,
            fuel_rate_lph: 0.0,
            egr_pct: 0.0,
            egt_c: 16.0,
            dpf_inlet_c: 16.0,
            dpf_soot_pct: 0.0,
            regen_active: false,
            mil_on: false,
            dtc_count: 0,
            distance_mil_km: 0,
            distance_cleared_km: 0,
            warmups: 0,
            torque_pct: 0.0,
            demand_torque_pct: 0.0,
            reference_torque_nm: 1152,
        }
    }
}

/// Smooth, bounded, deterministic wobble in `[-1, 1]`.
///
/// Two incommensurable sine terms so the signal never repeats on a short
/// period and never looks like a sawtooth counter, while staying an exact
/// function of `t` — the same tick always produces the same reading.
pub fn wobble(t: f64, primary_hz: f64, secondary_hz: f64) -> f64 {
    0.65 * (2.0 * PI * primary_hz * t).sin() + 0.35 * (2.0 * PI * secondary_hz * t).sin()
}

/// First-order approach from `start` to `target` with time constant `tau`.
pub fn approach(start: f64, target: f64, t: f64, tau: f64) -> f64 {
    target + (start - target) * (-t / tau).exp()
}

// ---- SAE J1979 encoders -------------------------------------------------
//
// Each helper is the inverse of the corresponding profile formula. Values are
// clamped to the encodable range rather than wrapping, because a wrapped byte
// would silently produce a plausible wrong reading.

fn u8_clamped(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn u16_clamped(v: f64) -> [u8; 2] {
    let n = v.round().clamp(0.0, 65535.0) as u16;
    [(n >> 8) as u8, (n & 0xFF) as u8]
}

/// `A - 40` — the temperature encoding used by PIDs 05, 0F, 46, 5C.
pub fn enc_temp(c: f64) -> u8 {
    u8_clamped(c + 40.0)
}

/// `A * 100 / 255` — the percent encoding used by PIDs 04, 11, 2C, 2F, 45...
pub fn enc_percent(p: f64) -> u8 {
    u8_clamped(p * 255.0 / 100.0)
}

/// `(256A + B) / 4` — engine speed, PID 0C.
pub fn enc_rpm(rpm: f64) -> [u8; 2] {
    u16_clamped(rpm * 4.0)
}

/// `(256A + B) / 10 - 40` — the wide temperature encoding used by PID 3C and
/// by the (unverified) exhaust and DPF temperature PIDs.
pub fn enc_temp_wide(c: f64) -> [u8; 2] {
    u16_clamped((c + 40.0) * 10.0)
}

/// The 9-byte layout used by PIDs 78 and 7C: a sensor-support mask followed by
/// four wide-temperature readings.
///
/// This project has **not** validated that layout against SAE J1979 — the
/// profile marks both PIDs `unverified` for exactly that reason. The simulator
/// encodes what the profile says it will decode, so the round trip is
/// self-consistent; it is not evidence that a real truck answers this way.
pub fn enc_temp_bank(readings: [f64; 4]) -> Vec<u8> {
    let mut v = vec![0x0F];
    for r in readings {
        v.extend_from_slice(&enc_temp_wide(r));
    }
    v
}

/// Encode one service 01 PID from the vehicle's physical state.
///
/// `None` means this simulated vehicle does not implement the PID, which is
/// how the supported-PID bitmask and the answers stay consistent with each
/// other: both are derived from this one function plus the ECU's declared list.
pub fn encode_pid(pid: u8, s: &VehicleState) -> Option<Vec<u8>> {
    Some(match pid {
        // Monitor status. Bit 7 of A is the MIL; the low seven bits are the
        // confirmed DTC count. Bytes B..D are monitor availability bits, left
        // at a plain "compression ignition monitors supported" pattern.
        0x01 => vec![(if s.mil_on { 0x80 } else { 0x00 }) | (s.dtc_count & 0x7F), 0x07, 0xE1, 0x00],
        // Fuel system status: a compression-ignition engine has no closed-loop
        // fuelling in the spark-ignition sense, so both loops report 0.
        0x03 => vec![0x00, 0x00],
        0x04 => vec![enc_percent(s.load_pct)],
        0x05 => vec![enc_temp(s.coolant_c)],
        0x0B => vec![u8_clamped(s.map_kpa)],
        0x0C => enc_rpm(s.rpm).to_vec(),
        0x0D => vec![u8_clamped(s.speed_kph)],
        0x0F => vec![enc_temp(s.intake_air_c)],
        0x10 => u16_clamped(s.maf_gs * 100.0).to_vec(),
        0x11 => vec![enc_percent(s.pedal_pct)],
        // OBD-II as defined by CARB.
        0x1C => vec![0x01],
        0x1F => u16_clamped(s.run_time_s).to_vec(),
        0x21 => u16_clamped(s.distance_mil_km as f64).to_vec(),
        0x23 => u16_clamped(s.fuel_rail_kpa / 10.0).to_vec(),
        0x2C => vec![enc_percent(s.egr_pct)],
        0x2D => vec![u8_clamped((0.0 + 100.0) * 1.28)],
        0x2F => vec![enc_percent(s.fuel_level_pct)],
        0x30 => vec![s.warmups],
        0x31 => u16_clamped(s.distance_cleared_km as f64).to_vec(),
        0x33 => vec![u8_clamped(s.baro_kpa)],
        0x42 => u16_clamped(s.battery_v * 1000.0).to_vec(),
        0x43 => u16_clamped(s.absolute_load_pct * 255.0 / 100.0).to_vec(),
        0x45 => vec![enc_percent(s.pedal_pct)],
        0x46 => vec![enc_temp(s.ambient_c)],
        0x49 => vec![enc_percent(s.pedal_pct)],
        0x4A => vec![enc_percent(s.pedal_pct * 0.98)],
        // Diesel.
        0x51 => vec![0x04],
        0x5A => vec![enc_percent(s.pedal_pct)],
        0x5C => vec![enc_temp(s.oil_temp_c)],
        0x5E => u16_clamped(s.fuel_rate_lph * 20.0).to_vec(),
        0x61 => vec![u8_clamped(s.demand_torque_pct + 125.0)],
        0x62 => vec![u8_clamped(s.torque_pct + 125.0)],
        0x63 => u16_clamped(s.reference_torque_nm as f64).to_vec(),
        0x78 => enc_temp_bank([s.egt_c, s.egt_c - 30.0, s.egt_c - 55.0, s.egt_c - 80.0]),
        0x7C => enc_temp_bank([
            s.dpf_inlet_c,
            s.dpf_inlet_c - 25.0,
            s.dpf_inlet_c - 45.0,
            s.dpf_inlet_c - 70.0,
        ]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoders_invert_the_profile_formulas() {
        // A - 40
        assert_eq!(enc_temp(88.0), 128);
        // (256A + B) / 4
        assert_eq!(enc_rpm(1200.0), [0x12, 0xC0]);
        assert_eq!((u16::from_be_bytes(enc_rpm(1200.0)) as f64) / 4.0, 1200.0);
        // A * 100 / 255
        let load = enc_percent(40.0);
        assert!(((load as f64) * 100.0 / 255.0 - 40.0).abs() < 0.5);
        // (256A + B) / 10 - 40
        let t = enc_temp_wide(560.0);
        assert!(((u16::from_be_bytes(t) as f64) / 10.0 - 40.0 - 560.0).abs() < 0.05);
    }

    #[test]
    fn encoders_clamp_instead_of_wrapping() {
        assert_eq!(enc_temp(9000.0), 255);
        assert_eq!(enc_temp(-9000.0), 0);
        assert_eq!(enc_rpm(1.0e9), [0xFF, 0xFF]);
    }

    #[test]
    fn unimplemented_pids_return_none() {
        let s = VehicleState::cold_idle();
        assert!(encode_pid(0xFE, &s).is_none());
        assert!(encode_pid(0x0C, &s).is_some());
    }

    #[test]
    fn the_mil_bit_and_dtc_count_share_byte_a() {
        let mut s = VehicleState::cold_idle();
        s.mil_on = true;
        s.dtc_count = 2;
        let v = encode_pid(0x01, &s).unwrap();
        assert_eq!(v[0], 0x82);
        s.mil_on = false;
        assert_eq!(encode_pid(0x01, &s).unwrap()[0], 0x02);
    }

    #[test]
    fn wobble_is_bounded_and_deterministic() {
        for i in 0..500 {
            let t = i as f64 * 0.37;
            let w = wobble(t, 0.14, 0.031);
            assert!((-1.0..=1.0).contains(&w), "wobble out of range at t={t}");
            assert_eq!(w, wobble(t, 0.14, 0.031));
        }
    }

    #[test]
    fn approach_converges_toward_the_target() {
        assert!((approach(20.0, 90.0, 0.0, 100.0) - 20.0).abs() < 1e-9);
        assert!((approach(20.0, 90.0, 1000.0, 100.0) - 90.0).abs() < 0.01);
        assert!(approach(20.0, 90.0, 50.0, 100.0) > 20.0);
    }
}
