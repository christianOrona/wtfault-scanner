//! The append-only session event log — the Diagnostic Flight Recorder (§9).
//!
//! Everything the core does emits an event: connection transitions, every
//! adapter request and response, every decoded reading, every safety decision.
//! The log is append-only at the database level (SQLite triggers reject UPDATE
//! and DELETE), so a replay of a session is a replay of exactly what happened.

use crate::{SessionId, Timestamp};
use serde::{Deserialize, Serialize};

/// A chronological entry in a session's event log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Row id, assigned by the store on insert. `None` before persistence.
    pub id: Option<i64>,
    /// Session the event belongs to.
    pub session_id: SessionId,
    /// Monotonic per-session sequence number.
    pub seq: i64,
    /// When it happened.
    pub timestamp: Timestamp,
    /// What happened.
    pub kind: EventKind,
}

impl SessionEvent {
    /// Build an unpersisted event; the store assigns `id` and `seq`.
    pub fn new(session_id: SessionId, kind: EventKind) -> Self {
        SessionEvent { id: None, session_id, seq: 0, timestamp: crate::now(), kind }
    }

    /// Stable discriminant string, used for indexing and filtering.
    pub fn kind_name(&self) -> &'static str {
        self.kind.name()
    }
}

/// The event taxonomy. Adding a variant is backward compatible; renaming one
/// breaks stored sessions, so variants are append-only too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// A session was opened.
    SessionStarted {
        /// Optional user label.
        label: Option<String>,
    },
    /// A session was closed.
    SessionEnded,
    /// The connection state machine moved.
    ConnectionStateChanged {
        /// Previous state name.
        from: String,
        /// New state.
        to: crate::ConnectionState,
    },
    /// The adapter was identified and its capabilities determined.
    AdapterIdentified {
        /// Observed capabilities.
        capabilities: crate::AdapterCapabilities,
    },
    /// A command was sent to the adapter. Recorded for every single exchange.
    AdapterRequest {
        /// The literal command line sent, minus the terminating CR.
        command: String,
    },
    /// The adapter's reply to the preceding request.
    AdapterResponse {
        /// The literal command line this answers.
        command: String,
        /// Raw reply lines, prompt and echo stripped.
        lines: Vec<String>,
        /// Round-trip time.
        elapsed_ms: u64,
        /// Classification, e.g. `ok`, `no_data`, `unable_to_connect`.
        classification: String,
    },
    /// An adapter or transport level failure.
    AdapterFailure {
        /// The command that failed, when there was one.
        command: Option<String>,
        /// Structured error.
        error: crate::AimError,
    },
    /// A vehicle was identified.
    VehicleIdentified {
        /// VIN, when readable.
        vin: Option<String>,
        /// Vehicle id assigned.
        vehicle_id: String,
    },
    /// A module was discovered by a scan.
    ModuleDiscovered {
        /// Module key.
        module_key: String,
        /// Observed address.
        address: String,
    },
    /// A DTC was read.
    DtcRead {
        /// Module key.
        module_key: String,
        /// SAE code.
        code: String,
        /// Status class.
        status: crate::DtcStatus,
    },
    /// A signal was decoded.
    MeasurementRecorded {
        /// Module key.
        module_key: String,
        /// Signal id.
        signal_id: String,
        /// Numeric value, when numeric.
        value: Option<f64>,
        /// Unit symbol.
        unit: Option<String>,
        /// Hex of the decoded bytes.
        raw_hex: String,
    },
    /// The safety gate made a decision. Recorded for every gated call,
    /// allowed or rejected — handoff §10 requires the audit trail.
    SafetyDecision {
        /// Operation id requested.
        operation: String,
        /// Permission level of the operation.
        level: crate::PermissionLevel,
        /// Whether it was allowed.
        allowed: bool,
        /// Who or what asked.
        initiator: String,
        /// Whether a user confirmation token accompanied the request.
        confirmed: bool,
        /// Rejection reason code, when rejected.
        reason: Option<String>,
    },
    /// A tool executed through the registry.
    ToolInvoked {
        /// Tool name.
        tool: String,
        /// Serialized arguments.
        arguments: serde_json::Value,
        /// Who or what invoked it.
        initiator: String,
    },
    /// A tool produced a result.
    ToolCompleted {
        /// Tool name.
        tool: String,
        /// Whether it succeeded.
        success: bool,
        /// Duration.
        execution_time_ms: u64,
        /// Warning codes attached to the result.
        warnings: Vec<String>,
    },
    /// A test run started.
    TestStarted {
        /// Test id.
        test_id: String,
        /// Test run id.
        run_id: String,
    },
    /// A test run finished.
    TestFinished {
        /// Test run id.
        run_id: String,
        /// Outcome.
        result: crate::domain::TestOutcome,
    },
    /// An agent message was recorded. Unused until the agent runtime lands.
    AgentMessage {
        /// Role.
        role: String,
        /// Content.
        content: String,
    },
    /// A user-supplied context note ("I just unplugged the sensor").
    UserNote {
        /// Note text.
        text: String,
    },
}

impl EventKind {
    /// Stable discriminant string.
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::SessionStarted { .. } => "session_started",
            EventKind::SessionEnded => "session_ended",
            EventKind::ConnectionStateChanged { .. } => "connection_state_changed",
            EventKind::AdapterIdentified { .. } => "adapter_identified",
            EventKind::AdapterRequest { .. } => "adapter_request",
            EventKind::AdapterResponse { .. } => "adapter_response",
            EventKind::AdapterFailure { .. } => "adapter_failure",
            EventKind::VehicleIdentified { .. } => "vehicle_identified",
            EventKind::ModuleDiscovered { .. } => "module_discovered",
            EventKind::DtcRead { .. } => "dtc_read",
            EventKind::MeasurementRecorded { .. } => "measurement_recorded",
            EventKind::SafetyDecision { .. } => "safety_decision",
            EventKind::ToolInvoked { .. } => "tool_invoked",
            EventKind::ToolCompleted { .. } => "tool_completed",
            EventKind::TestStarted { .. } => "test_started",
            EventKind::TestFinished { .. } => "test_finished",
            EventKind::AgentMessage { .. } => "agent_message",
            EventKind::UserNote { .. } => "user_note",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_through_json_with_a_kind_tag() {
        let e = SessionEvent::new(
            SessionId::from_string("ses_1"),
            EventKind::AdapterResponse {
                command: "0100".into(),
                lines: vec!["41 00 BE 3F A8 13".into()],
                elapsed_ms: 42,
                classification: "ok".into(),
            },
        );
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"kind\":\"adapter_response\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.kind_name(), "adapter_response");
    }

    #[test]
    fn safety_decisions_record_the_initiator() {
        let e = EventKind::SafetyDecision {
            operation: "clear_dtcs".into(),
            level: crate::PermissionLevel::L2,
            allowed: false,
            initiator: "agent:planner".into(),
            confirmed: false,
            reason: Some("permission_level_disabled".into()),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["initiator"], "agent:planner");
        assert_eq!(v["allowed"], false);
    }
}
