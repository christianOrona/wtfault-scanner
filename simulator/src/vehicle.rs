//! Deterministic virtual ECUs.
//!
//! [`VirtualVehicle`] answers OBD-II service requests the way a small network
//! of ECUs would: a broadcast request is answered by every module that
//! implements it, a physically addressed request by exactly one, and a request
//! for a PID nobody implements is answered by nobody — which is what produces
//! `NO DATA` at the adapter.
//!
//! # What is modelled and what is not
//!
//! The modelled truck is a 2019 F-250 with the 6.7 L diesel, but everything it
//! answers is **generic SAE J1979**: standard service 01/02/03/04/07/09/0A
//! requests at the standard ISO 15765-4 addresses `0x7E8`–`0x7EF`. No
//! Ford-specific module address, PID, or service is simulated, because none has
//! been validated by this project. The two modules besides the engine
//! controller are deliberately named by address rather than by guessed
//! function.
//!
//! The VIN is synthetic. Its check digit is valid so that the VIN decoder has
//! something real to verify, but it identifies no actual vehicle, and the core
//! deliberately does not infer model or trim from it.

use crate::scenario::{Scenario, ScenarioId};
use crate::state::{encode_pid, VehicleState};
use aim_protocols::{encode_supported_pids, Service};
use aim_types::DtcStatus;
use std::time::Instant;

/// A synthetic VIN with a valid SAE J272 check digit. Identifies no real
/// vehicle; see the module docs.
pub const SIMULATED_VIN: &str = "1FT7W2BT6KEC00001";

/// How the simulated vehicle tells the time.
#[derive(Debug, Clone)]
pub enum TimeSource {
    /// Simulated time advances a fixed step per request. Fully reproducible:
    /// the same sequence of requests always yields the same readings, which is
    /// what golden transcripts and the end-to-end test depend on.
    Ticks {
        /// Requests served so far.
        tick: u64,
        /// Simulated milliseconds per request.
        ms_per_tick: u64,
    },
    /// Simulated time follows the wall clock. Used when a human is watching a
    /// live-data graph and wants it to move at a believable rate.
    Wall {
        /// When the vehicle "started".
        started: Instant,
    },
}

impl TimeSource {
    /// The default deterministic clock: a quarter second of simulated time per
    /// request.
    pub fn deterministic() -> Self {
        TimeSource::Ticks {
            tick: 0,
            ms_per_tick: 250,
        }
    }

    /// A wall-clock source starting now.
    pub fn wall() -> Self {
        TimeSource::Wall {
            started: Instant::now(),
        }
    }

    fn advance(&mut self) {
        if let TimeSource::Ticks { tick, .. } = self {
            *tick += 1;
        }
    }

    fn seconds(&self) -> f64 {
        match self {
            TimeSource::Ticks { tick, ms_per_tick } => (tick * ms_per_tick) as f64 / 1000.0,
            TimeSource::Wall { started } => started.elapsed().as_secs_f64(),
        }
    }
}

/// One simulated electronic control unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualEcu {
    /// The identifier this ECU answers on, e.g. `0x7E8`.
    pub response_id: u16,
    /// Developer-facing label. Modules other than the engine controller are
    /// named by address because their function is vehicle-specific and
    /// unverified.
    pub label: String,
    /// Service 09 PID 0A "ECU name", exactly 20 ASCII characters.
    pub ecu_name: Option<String>,
    /// Service 09 PID 04 calibration identifiers, 16 ASCII characters each.
    pub calibration_ids: Vec<String>,
    /// Service 09 PID 06 calibration verification numbers.
    pub cvns: Vec<u32>,
    /// Service 01 PIDs this ECU implements.
    pub supported_service01: Vec<u8>,
    /// Service 09 info types this ECU implements.
    pub supported_service09: Vec<u8>,
    /// Whether this ECU reports diagnostic trouble codes.
    pub reports_dtcs: bool,
    /// Whether this ECU runs service 06 on-board monitors. Only the engine
    /// controller does, which is also how a real vehicle behaves.
    pub runs_monitors: bool,
    /// Whether this ECU answers the VIN request.
    pub reports_vin: bool,
}

impl VirtualEcu {
    /// The request identifier that physically addresses this ECU.
    pub fn request_id(&self) -> u16 {
        self.response_id.saturating_sub(8)
    }

    fn supports_01(&self, pid: u8) -> bool {
        is_mask_pid(pid) || self.supported_service01.contains(&pid)
    }
}

/// True for the service 01 PIDs that carry a supported-PID bitmask.
fn is_mask_pid(pid: u8) -> bool {
    matches!(pid, 0x00 | 0x20 | 0x40 | 0x60 | 0x80 | 0xA0 | 0xC0)
}

/// One ECU's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcuReply {
    /// Identifier the reply is sent from, e.g. `0x7E8`.
    pub response_id: u16,
    /// Full response PDU, service byte first.
    pub payload: Vec<u8>,
}

/// The simulated vehicle.
#[derive(Debug, Clone)]
pub struct VirtualVehicle {
    /// Vehicle identification number this vehicle reports.
    pub vin: String,
    /// The active fault scenario.
    pub scenario: Scenario,
    /// The modules on the bus.
    pub ecus: Vec<VirtualEcu>,
    /// Simulated clock.
    pub time: TimeSource,
    /// Set by a service 04 request. The safety gate is supposed to make this
    /// unreachable; the flag exists so a test can prove it stayed false.
    pub dtcs_cleared: bool,
}

impl VirtualVehicle {
    /// The development vehicle: an engine controller plus two other modules
    /// answering at standard OBD-II addresses.
    pub fn f250_2019(scenario: ScenarioId) -> VirtualVehicle {
        let engine = VirtualEcu {
            response_id: 0x7E8,
            label: String::from("Engine control module"),
            ecu_name: Some(pad_ascii("SIM ENGINE CONTROL", 20)),
            calibration_ids: vec![pad_ascii("SIMULATED-CAL-01", 16)],
            cvns: vec![0x1A2B_3C4D],
            // 0x20/0x40/0x60 are the "more PIDs follow" markers. A real ECU
            // reports each one inside the preceding mask so a tool can walk
            // the chain, and so does this one.
            supported_service01: vec![
                0x01, 0x03, 0x04, 0x05, 0x0B, 0x0C, 0x0D, 0x0F, 0x10, 0x11, 0x1C, 0x1F, 0x20,
                0x21, 0x23, 0x2C, 0x2D, 0x2F, 0x30, 0x31, 0x33, 0x40, 0x42, 0x43, 0x45, 0x46,
                0x49, 0x4A, 0x51, 0x5A, 0x5C, 0x5E, 0x60, 0x61, 0x62, 0x63, 0x78, 0x7C,
            ],
            supported_service09: vec![0x02, 0x04, 0x06, 0x0A],
            reports_dtcs: true,
            runs_monitors: true,
            reports_vin: true,
        };
        // Two further modules answer the standard broadcast. Their function is
        // not asserted: on a given vehicle 7EA and 7EB could be almost
        // anything, and guessing would be exactly the kind of invention the
        // handoff forbids.
        let second = VirtualEcu {
            response_id: 0x7EA,
            label: String::from("OBD module at 7EA"),
            ecu_name: Some(pad_ascii("SIM MODULE 7EA", 20)),
            calibration_ids: vec![pad_ascii("SIMULATED-CAL-02", 16)],
            cvns: vec![0x5566_7788],
            supported_service01: vec![0x01, 0x05, 0x0C, 0x0D, 0x1C, 0x20, 0x40, 0x42],
            supported_service09: vec![0x0A],
            reports_dtcs: true,
            runs_monitors: false,
            reports_vin: false,
        };
        let third = VirtualEcu {
            response_id: 0x7EB,
            label: String::from("OBD module at 7EB"),
            ecu_name: Some(pad_ascii("SIM MODULE 7EB", 20)),
            calibration_ids: Vec::new(),
            cvns: Vec::new(),
            supported_service01: vec![0x01, 0x0C, 0x1C, 0x20, 0x40, 0x42],
            supported_service09: vec![0x0A],
            reports_dtcs: false,
            runs_monitors: false,
            reports_vin: false,
        };

        VirtualVehicle {
            vin: String::from(SIMULATED_VIN),
            scenario: Scenario::new(scenario),
            ecus: vec![engine, second, third],
            time: TimeSource::deterministic(),
            dtcs_cleared: false,
        }
    }

    /// Use the wall clock instead of the deterministic tick clock.
    pub fn with_wall_clock(mut self) -> Self {
        self.time = TimeSource::wall();
        self
    }

    /// Simulated seconds since the engine started.
    pub fn elapsed_s(&self) -> f64 {
        self.time.seconds()
    }

    /// The vehicle's current physical state.
    pub fn state(&self) -> VehicleState {
        self.scenario.state_at(self.elapsed_s())
    }

    /// Serve one request PDU addressed to `target_id`.
    ///
    /// `target_id` is `0x7DF` for a broadcast, or a physical request id. The
    /// returned vector is empty when nothing answers, which the adapter layer
    /// turns into `NO DATA`.
    pub fn handle(&mut self, target_id: u16, request: &[u8]) -> Vec<EcuReply> {
        self.time.advance();
        if !self.scenario.vehicle_answers || request.is_empty() {
            return Vec::new();
        }
        let state = self.state();

        // Snapshot what each ECU needs before the borrow, so answering can
        // take &self.
        let addressed: Vec<usize> = self
            .ecus
            .iter()
            .enumerate()
            .filter(|(_, e)| target_id == 0x7DF || target_id == e.request_id())
            .map(|(i, _)| i)
            .collect();

        let mut replies = Vec::new();
        for i in addressed {
            if let Some(payload) = self.answer(&self.ecus[i], request, &state) {
                replies.push(EcuReply {
                    response_id: self.ecus[i].response_id,
                    payload,
                });
            }
        }

        // Service 04 is the one request that changes vehicle state. It is
        // implemented so that the safety gate is what prevents it, not a
        // missing feature.
        if request[0] == Service::ClearDtcs.id() && !replies.is_empty() {
            self.dtcs_cleared = true;
        }
        replies
    }

    fn answer(&self, ecu: &VirtualEcu, request: &[u8], state: &VehicleState) -> Option<Vec<u8>> {
        let service = request[0];
        match Service::from_id(service) {
            Some(Service::CurrentData) => {
                let pid = *request.get(1)?;
                if !ecu.supports_01(pid) {
                    return None;
                }
                if is_mask_pid(pid) {
                    // Only answer a mask that actually has bits set: a real ECU
                    // does not answer 0x60 when it supports nothing above 0x60.
                    let mask = encode_supported_pids(pid, &ecu.supported_service01);
                    let next_mask_supported =
                        ecu.supported_service01.iter().any(|p| *p > pid && *p <= pid + 32);
                    if !next_mask_supported {
                        return None;
                    }
                    let mut v = vec![0x41, pid];
                    v.extend_from_slice(&mask);
                    return Some(v);
                }
                let data = encode_pid(pid, state)?;
                let mut v = vec![0x41, pid];
                v.extend_from_slice(&data);
                Some(v)
            }
            Some(Service::FreezeFrame) => {
                let pid = *request.get(1)?;
                let frame = *request.get(2).unwrap_or(&0);
                if !ecu.reports_dtcs || frame != 0 {
                    return None;
                }
                let ff = self.scenario.freeze_frame.as_ref()?;
                let mut v = vec![0x42, pid, frame];
                if pid == 0x02 {
                    // PID 02 of a freeze frame is the DTC that caused it.
                    let code = self
                        .scenario
                        .dtcs_with_status(DtcStatus::Confirmed)
                        .first()
                        .map(|d| d.code.clone())?;
                    let bytes = aim_protocols::encode_dtc(&code).ok()?;
                    v.extend_from_slice(&bytes);
                } else {
                    if !ecu.supports_01(pid) {
                        return None;
                    }
                    v.extend_from_slice(&encode_pid(pid, ff)?);
                }
                Some(v)
            }
            Some(Service::MonitorResults) => {
                let mid = *request.get(1)?;
                if !ecu.runs_monitors || self.scenario.monitor_tests.is_empty() {
                    return None;
                }
                if is_mask_pid(mid) {
                    let supported = self.supported_mids();
                    // As with service 01, a mask with nothing above it is not
                    // answered at all rather than answered with zeroes.
                    if !supported
                        .iter()
                        .any(|m| *m > mid && *m <= mid.saturating_add(32))
                    {
                        return None;
                    }
                    let mask = encode_supported_pids(mid, &supported);
                    let mut v = vec![0x46, mid];
                    v.extend_from_slice(&mask);
                    return Some(v);
                }
                // Every test for this monitor comes back in one response, and
                // each record restates the monitor id — including the first,
                // whose id doubles as the response's echoed byte.
                let tests: Vec<_> = self
                    .scenario
                    .monitor_tests
                    .iter()
                    .filter(|t| t.mid == mid)
                    .collect();
                if tests.is_empty() {
                    return None;
                }
                let mut v = vec![0x46];
                for t in tests {
                    v.extend_from_slice(&[t.mid, t.tid, t.uasid]);
                    v.extend_from_slice(&t.value.to_be_bytes());
                    v.extend_from_slice(&t.min.to_be_bytes());
                    v.extend_from_slice(&t.max.to_be_bytes());
                }
                Some(v)
            }
            Some(Service::StoredDtcs) => self.dtc_response(ecu, 0x43, DtcStatus::Confirmed),
            Some(Service::PendingDtcs) => self.dtc_response(ecu, 0x47, DtcStatus::Pending),
            Some(Service::PermanentDtcs) => self.dtc_response(ecu, 0x4A, DtcStatus::Permanent),
            Some(Service::ClearDtcs) => {
                if ecu.reports_dtcs {
                    Some(vec![0x44])
                } else {
                    None
                }
            }
            Some(Service::VehicleInfo) => {
                let info = *request.get(1)?;
                if info != 0x00 && !ecu.supported_service09.contains(&info) {
                    return None;
                }
                match info {
                    0x00 => {
                        let mask = encode_supported_pids(0x00, &ecu.supported_service09);
                        let mut v = vec![0x49, 0x00];
                        v.extend_from_slice(&mask);
                        Some(v)
                    }
                    0x02 if ecu.reports_vin => {
                        let mut v = vec![0x49, 0x02, 0x01];
                        v.extend_from_slice(self.vin.as_bytes());
                        Some(v)
                    }
                    0x04 if !ecu.calibration_ids.is_empty() => {
                        let mut v = vec![0x49, 0x04, ecu.calibration_ids.len() as u8];
                        for c in &ecu.calibration_ids {
                            v.extend_from_slice(c.as_bytes());
                        }
                        Some(v)
                    }
                    0x06 if !ecu.cvns.is_empty() => {
                        let mut v = vec![0x49, 0x06, ecu.cvns.len() as u8];
                        for c in &ecu.cvns {
                            v.extend_from_slice(&c.to_be_bytes());
                        }
                        Some(v)
                    }
                    0x0A => {
                        let name = ecu.ecu_name.as_ref()?;
                        let mut v = vec![0x49, 0x0A, 0x01];
                        v.extend_from_slice(name.as_bytes());
                        Some(v)
                    }
                    _ => None,
                }
            }
            // An unimplemented service gets a proper negative response from a
            // module that is listening, so the negative-response path is
            // exercised rather than being indistinguishable from silence.
            None if ecu.reports_dtcs => Some(vec![0x7F, service, 0x11]),
            None => None,
        }
    }

    /// The monitor ids the engine controller advertises in its service 06
    /// masks: the monitors it actually has, plus the mask boundaries needed to
    /// reach them.
    ///
    /// A real ECU reports each continuation boundary inside the preceding mask
    /// so a tool can walk the chain — the same convention this simulator already
    /// follows for service 01 — and a monitor high in the range is unreachable
    /// without them.
    fn supported_mids(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self.scenario.monitor_tests.iter().map(|t| t.mid).collect();
        let highest = v.iter().copied().max().unwrap_or(0);
        v.extend((1..=6).map(|n| n * 0x20).filter(|b| *b < highest));
        v.sort_unstable();
        v.dedup();
        v
    }

    fn dtc_response(&self, ecu: &VirtualEcu, response: u8, status: DtcStatus) -> Option<Vec<u8>> {
        if !ecu.reports_dtcs {
            return None;
        }
        let codes: Vec<&crate::scenario::SimDtc> = if self.dtcs_cleared {
            Vec::new()
        } else {
            self.scenario.dtcs_with_status(status)
        };
        // ISO 15765-4 puts a code count between the service byte and the
        // codes; the pre-CAN protocols do not. This vehicle speaks CAN.
        let mut v = vec![response, codes.len() as u8];
        for d in codes {
            match aim_protocols::encode_dtc(&d.code) {
                Ok(bytes) => v.extend_from_slice(&bytes),
                Err(_) => return None,
            }
        }
        Some(v)
    }
}

/// Right-pad (or truncate) to exactly `n` ASCII characters, as service 09
/// fixed-width records require.
fn pad_ascii(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    while out.len() < n {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_protocols::{decode_dtc_list, decode_supported_pids, decode_vin};

    fn vehicle(id: ScenarioId) -> VirtualVehicle {
        VirtualVehicle::f250_2019(id)
    }

    #[test]
    fn the_simulated_vin_has_a_valid_check_digit() {
        // The VIN decoder is real, so the VIN it is fed must be too.
        let info = aim_decoders::decode_vin_info(SIMULATED_VIN, 2026).unwrap();
        assert!(info.check_digit_valid, "simulated VIN check digit is wrong");
        assert_eq!(info.wmi, "1FT");
    }

    #[test]
    fn a_broadcast_is_answered_by_every_module_that_implements_the_pid() {
        let mut v = vehicle(ScenarioId::Healthy);
        // PID 0C (engine rpm) is implemented by all three.
        let replies = v.handle(0x7DF, &[0x01, 0x0C]);
        assert_eq!(replies.len(), 3);
        // PID 5C (oil temperature) only by the engine controller.
        let replies = v.handle(0x7DF, &[0x01, 0x5C]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].response_id, 0x7E8);
    }

    #[test]
    fn a_physical_request_is_answered_by_exactly_one_module() {
        let mut v = vehicle(ScenarioId::Healthy);
        let replies = v.handle(0x7E2, &[0x01, 0x0C]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].response_id, 0x7EA);
    }

    #[test]
    fn nobody_answers_an_unimplemented_pid() {
        let mut v = vehicle(ScenarioId::Healthy);
        assert!(v.handle(0x7DF, &[0x01, 0xFE]).is_empty());
    }

    #[test]
    fn the_supported_pid_mask_agrees_with_what_the_vehicle_actually_answers() {
        // This is the invariant that keeps the simulator honest: a scan tool
        // that trusts the mask must never hit a PID that answers nothing.
        let mut v = vehicle(ScenarioId::Healthy);
        for base in [0x00u8, 0x20, 0x40, 0x60] {
            let replies = v.handle(0x7E0, &[0x01, base]);
            if replies.is_empty() {
                continue;
            }
            let advertised = decode_supported_pids(base, &replies[0].payload[2..]).unwrap();
            for pid in advertised {
                if is_mask_pid(pid) {
                    continue;
                }
                let answer = v.handle(0x7E0, &[0x01, pid]);
                assert!(
                    !answer.is_empty(),
                    "PID {pid:02X} is advertised as supported but nothing answers it"
                );
                assert_eq!(answer[0].payload[1], pid, "wrong PID echoed");
            }
        }
    }

    #[test]
    fn the_engine_controller_reports_the_vin() {
        let mut v = vehicle(ScenarioId::Healthy);
        let replies = v.handle(0x7DF, &[0x09, 0x02]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].response_id, 0x7E8);
        // payload = 49 02 <NODI> <17 chars>
        let vin = decode_vin(&replies[0].payload[2..]).unwrap();
        assert_eq!(vin, SIMULATED_VIN);
    }

    #[test]
    fn dtc_services_report_the_scenario_codes_with_a_can_count_byte() {
        let mut v = vehicle(ScenarioId::DpfRegen);
        let stored = v.handle(0x7E0, &[0x03]).remove(0).payload;
        assert_eq!(stored[0], 0x43);
        assert_eq!(stored[1], 2, "count byte");
        let codes = decode_dtc_list(&stored[2..]).unwrap();
        assert_eq!(codes, vec!["P2463", "P242F"]);

        let pending = v.handle(0x7E0, &[0x07]).remove(0).payload;
        assert_eq!(decode_dtc_list(&pending[2..]).unwrap(), vec!["P2002"]);

        let permanent = v.handle(0x7E0, &[0x0A]).remove(0).payload;
        assert_eq!(decode_dtc_list(&permanent[2..]).unwrap(), vec!["P2463"]);
    }

    #[test]
    fn a_healthy_vehicle_reports_an_empty_dtc_list_rather_than_silence() {
        let mut v = vehicle(ScenarioId::Healthy);
        let stored = v.handle(0x7E0, &[0x03]).remove(0).payload;
        assert_eq!(stored, vec![0x43, 0x00]);
    }

    #[test]
    fn the_freeze_frame_reports_the_causing_code_and_frozen_values() {
        let mut v = vehicle(ScenarioId::DpfRegen);
        let dtc = v.handle(0x7E0, &[0x02, 0x02, 0x00]).remove(0).payload;
        assert_eq!(&dtc[..3], &[0x42, 0x02, 0x00]);
        assert_eq!(aim_protocols::decode_dtc(dtc[3], dtc[4]), "P2463");

        let rpm = v.handle(0x7E0, &[0x02, 0x0C, 0x00]).remove(0).payload;
        let raw = u16::from_be_bytes([rpm[3], rpm[4]]) as f64 / 4.0;
        // The frozen rpm is the idle speed at the time of the fault, not the
        // raised idle the truck is running at now.
        assert!((raw - 748.0).abs() < 1.0, "frozen rpm was {raw}");
    }

    #[test]
    fn a_healthy_vehicle_has_no_freeze_frame_to_give() {
        let mut v = vehicle(ScenarioId::Healthy);
        assert!(v.handle(0x7E0, &[0x02, 0x0C, 0x00]).is_empty());
    }

    #[test]
    fn an_unknown_service_gets_a_negative_response_not_silence() {
        let mut v = vehicle(ScenarioId::Healthy);
        let replies = v.handle(0x7E0, &[0x22, 0xF1, 0x90]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].payload, vec![0x7F, 0x22, 0x11]);
    }

    #[test]
    fn a_silent_bus_answers_nothing_at_all() {
        let mut v = vehicle(ScenarioId::BusSilent);
        assert!(v.handle(0x7DF, &[0x01, 0x00]).is_empty());
        assert!(v.handle(0x7DF, &[0x09, 0x02]).is_empty());
        assert!(v.handle(0x7E0, &[0x03]).is_empty());
    }

    #[test]
    fn live_values_move_between_requests_but_are_reproducible() {
        let mut a = vehicle(ScenarioId::Healthy);
        let mut b = vehicle(ScenarioId::Healthy);
        let mut first = Vec::new();
        let mut second = Vec::new();
        for _ in 0..25 {
            first.push(a.handle(0x7E0, &[0x01, 0x0C]).remove(0).payload);
            second.push(b.handle(0x7E0, &[0x01, 0x0C]).remove(0).payload);
        }
        assert_eq!(first, second, "the simulator must be reproducible");
        assert!(
            first.iter().collect::<std::collections::BTreeSet<_>>().len() > 3,
            "live values should actually move"
        );
    }

    #[test]
    fn clearing_codes_works_but_is_recorded_so_the_gate_can_be_proven() {
        let mut v = vehicle(ScenarioId::DpfRegen);
        assert!(!v.dtcs_cleared);
        let reply = v.handle(0x7E0, &[0x04]);
        assert_eq!(reply[0].payload, vec![0x44]);
        assert!(v.dtcs_cleared);
        let stored = v.handle(0x7E0, &[0x03]).remove(0).payload;
        assert_eq!(stored, vec![0x43, 0x00]);
    }

    #[test]
    fn service_09_records_are_fixed_width() {
        let mut v = vehicle(ScenarioId::Healthy);
        let calid = v.handle(0x7E0, &[0x09, 0x04]).remove(0).payload;
        assert_eq!(calid.len(), 3 + 16);
        let name = v.handle(0x7E0, &[0x09, 0x0A]).remove(0).payload;
        assert_eq!(name.len(), 3 + 20);
        let cvn = v.handle(0x7E0, &[0x09, 0x06]).remove(0).payload;
        assert_eq!(cvn.len(), 3 + 4);
    }
}
