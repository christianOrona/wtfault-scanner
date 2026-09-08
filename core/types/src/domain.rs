//! The handoff §12 core data model.
//!
//! These structs are both the API representation and the shape persisted by
//! `aim-session`. They are plain data: no behaviour beyond small constructors.

use crate::{
    AdapterCapabilities, ConnectionId, DiagnosisId, ModuleId, ObdProtocol, PermissionLevel,
    SessionId, TestRunId, Timestamp, TransportKind, VehicleId,
};
use serde::{Deserialize, Serialize};

/// A vehicle, identified from VIN where possible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    /// Stable id.
    pub id: VehicleId,
    /// 17-character VIN, when one could be read.
    pub vin: Option<String>,
    /// Manufacturer, derived from the VIN's WMI. `None` when unknown.
    pub make: Option<String>,
    /// Model. Only set when a validated profile supplies it.
    pub model: Option<String>,
    /// Model year, derived from VIN position 10.
    pub year: Option<u16>,
    /// Trim. Only set when a validated profile supplies it.
    pub trim: Option<String>,
    /// Engine description. Only set when a validated profile supplies it.
    pub engine: Option<String>,
    /// Transmission description. Only set when a validated profile supplies it.
    pub transmission: Option<String>,
    /// When this vehicle was first seen.
    pub discovered_at: Timestamp,
}

impl Vehicle {
    /// A vehicle known only by VIN; everything else stays `None` until a
    /// validated profile fills it in. We do not guess model/trim from a VIN.
    pub fn from_vin(vin: Option<String>) -> Self {
        Vehicle {
            id: VehicleId::new(),
            vin,
            make: None,
            model: None,
            year: None,
            trim: None,
            engine: None,
            transmission: None,
            discovered_at: crate::now(),
        }
    }
}

/// One adapter connection lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Stable id.
    pub id: ConnectionId,
    /// Session this connection belongs to.
    pub session_id: SessionId,
    /// Adapter identity string (port name, simulator scenario, transcript path).
    pub adapter_id: String,
    /// Physical link kind.
    pub transport: TransportKind,
    /// When the transport opened.
    pub connected_at: Timestamp,
    /// When it closed, if it has.
    pub disconnected_at: Option<Timestamp>,
    /// Adapter identification banner.
    pub firmware: Option<String>,
    /// Capabilities observed for this connection.
    pub capabilities: AdapterCapabilities,
}

/// A diagnostic session: the unit the Flight Recorder replays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Stable id.
    pub id: SessionId,
    /// Vehicle, when identified.
    pub vehicle_id: Option<VehicleId>,
    /// When the session started.
    pub started_at: Timestamp,
    /// When it ended, if it has.
    pub ended_at: Option<Timestamp>,
    /// Free-form user label.
    pub label: Option<String>,
}

/// An ECU discovered during a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    /// Stable id.
    pub id: ModuleId,
    /// Session the module was discovered in.
    pub session_id: SessionId,
    /// Short module key, e.g. `ECU_7E8`.
    pub module_key: String,
    /// Best-known name. Falls back to the key when no profile identifies it.
    pub name: String,
    /// Response CAN identifier / addressing string as observed.
    pub address: String,
    /// Protocol the module answered on.
    pub protocol: ObdProtocol,
    /// Identity strings read from the module (CALID, CVN, ECU name).
    pub identity: ModuleIdentity,
    /// Software/calibration version, when read.
    pub software_version: Option<String>,
    /// When it was discovered.
    pub discovered_at: Timestamp,
}

/// Identity strings read from a module via OBD-II service 09.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleIdentity {
    /// Service 09 PID 0A ECU name.
    pub ecu_name: Option<String>,
    /// Service 09 PID 04 calibration identification(s).
    pub calibration_ids: Vec<String>,
    /// Service 09 PID 06 calibration verification number(s), hex.
    pub calibration_verification_numbers: Vec<String>,
}

/// Status of a diagnostic trouble code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DtcStatus {
    /// Service 03: confirmed / MIL-illuminating.
    Confirmed,
    /// Service 07: pending, detected this or last drive cycle.
    Pending,
    /// Service 0A: permanent, cannot be cleared by a scan tool.
    Permanent,
}

impl DtcStatus {
    /// The OBD-II service that reports this status.
    pub fn service(&self) -> u8 {
        match self {
            DtcStatus::Confirmed => 0x03,
            DtcStatus::Pending => 0x07,
            DtcStatus::Permanent => 0x0A,
        }
    }

    /// Stable wire form.
    pub fn as_str(&self) -> &'static str {
        match self {
            DtcStatus::Confirmed => "confirmed",
            DtcStatus::Pending => "pending",
            DtcStatus::Permanent => "permanent",
        }
    }
}

/// A stored diagnostic trouble code reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DtcRecord {
    /// Session it was read in.
    pub session_id: SessionId,
    /// Module that reported it.
    pub module_id: ModuleId,
    /// SAE code string, e.g. `P0401`.
    pub code: String,
    /// Which service reported it.
    pub status: DtcStatus,
    /// Description from the DTC knowledge file, when the code is known.
    pub description: Option<String>,
    /// How many times this code has been seen across this session.
    pub occurrence: u32,
    /// Event-log row holding the raw service response.
    pub freeze_frame_ref: Option<i64>,
    /// When it was read.
    pub read_at: Timestamp,
}

/// A single stored signal reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Session it belongs to.
    pub session_id: SessionId,
    /// Module it was read from.
    pub module_id: ModuleId,
    /// When it was sampled.
    pub timestamp: Timestamp,
    /// Signal identifier.
    pub signal_id: String,
    /// Decoded numeric value, when the signal is numeric.
    pub value: Option<f64>,
    /// Decoded text value, when the signal is textual.
    pub text_value: Option<String>,
    /// Unit symbol.
    pub unit: Option<String>,
    /// Hex of the exact bytes decoded.
    pub raw_value: String,
}

/// Result of a diagnostic test execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    /// Test completed and passed its criteria.
    Pass,
    /// Test completed and failed its criteria.
    Fail,
    /// Test ran but the result is not conclusive.
    Inconclusive,
    /// Test was rejected before execution (safety gate, preconditions).
    Rejected,
    /// Test aborted after starting.
    Aborted,
}

/// One execution of a registered test (handoff §12 `TestRun`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRun {
    /// Stable id.
    pub id: TestRunId,
    /// Session it belongs to.
    pub session_id: SessionId,
    /// Module the test addressed.
    pub module_id: Option<ModuleId>,
    /// Registered test/capability id.
    pub test_id: String,
    /// Permission level the test required.
    pub risk_level: PermissionLevel,
    /// Who asked: `user`, `agent:<name>`, `system`. Mandatory for audit.
    pub requested_by: String,
    /// Whether a human confirmation token was supplied.
    pub confirmed_by_user: bool,
    /// Start time.
    pub started_at: Timestamp,
    /// End time, when finished.
    pub ended_at: Option<Timestamp>,
    /// Outcome.
    pub result: TestOutcome,
    /// Event-log row holding the raw exchange.
    pub evidence_ref: Option<i64>,
    /// Detail (rejection reason, measured values summary).
    pub detail: Option<serde_json::Value>,
}

/// One entry in the agent conversation trace (handoff §12 `AgentTrace`).
///
/// The agent runtime is not built in this phase; the table and type exist so
/// the Flight Recorder schema is stable when it lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTrace {
    /// Session it belongs to.
    pub session_id: SessionId,
    /// Stable message id within the session.
    pub message_id: String,
    /// `user`, `assistant`, `tool`, `system`.
    pub role: String,
    /// Message text, when the entry is a message.
    pub content: Option<String>,
    /// Tool name, when the entry is a tool call.
    pub tool_name: Option<String>,
    /// Event-log row holding the serialized tool arguments.
    pub tool_args_ref: Option<i64>,
    /// Event-log row holding the serialized tool result.
    pub tool_result_ref: Option<i64>,
    /// Model identifier, when a model produced the entry.
    pub model: Option<String>,
    /// Versioned prompt identifier.
    pub prompt_version: Option<String>,
    /// When the entry was created.
    pub timestamp: Timestamp,
}

/// An evidence-backed diagnosis (handoff §12 `Diagnosis`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Stable id.
    pub id: DiagnosisId,
    /// Session it belongs to.
    pub session_id: SessionId,
    /// The claim being made.
    pub hypothesis: String,
    /// Confidence 0.0–1.0.
    pub confidence: f64,
    /// Event-log rows backing the claim. A diagnosis with no evidence refs is
    /// by construction unsupported.
    pub evidence_refs: Vec<i64>,
    /// Alternative hypotheses not ruled out.
    pub alternatives: Vec<String>,
    /// Recommended next action or repair.
    pub recommendation: String,
    /// Questions the evidence did not answer.
    pub unresolved_questions: Vec<String>,
    /// When it was created.
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtc_status_maps_to_services() {
        assert_eq!(DtcStatus::Confirmed.service(), 0x03);
        assert_eq!(DtcStatus::Pending.service(), 0x07);
        assert_eq!(DtcStatus::Permanent.service(), 0x0A);
    }

    #[test]
    fn vehicle_from_vin_does_not_guess_model() {
        let v = Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()));
        assert_eq!(v.vin.as_deref(), Some("1FT7W2BT6KEC00001"));
        assert!(v.model.is_none());
        assert!(v.trim.is_none());
        assert!(v.engine.is_none());
    }
}
