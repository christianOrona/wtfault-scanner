//! The typed tool seam (handoff §7) and the permission levels (§10) that gate it.
//!
//! No model code lives here or anywhere else in this build. What lives here is
//! the *envelope* a future agent runtime will see: a tool call always returns a
//! [`ToolResult`] with values, warnings, the capability that authorized it, and
//! a pointer to raw evidence.

use crate::{DecodedValue, SessionId, Timestamp};
use serde::{Deserialize, Serialize};

/// Permission level of an operation — handoff §10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PermissionLevel {
    /// Read-only: VIN, ECU ids, DTCs, freeze frame, live data. Allowed.
    L0,
    /// Non-invasive test: self tests, sensor checks. Requires confirmation.
    L1,
    /// Configuration write. Disabled in this build.
    L2,
    /// Programming / flashing. Disabled in this build.
    L3,
}

/// What *kind* of thing an operation touches, independent of how privileged it
/// is.
///
/// The permission level answers "how carefully must this be gated?". The risk
/// class answers "what is the worst thing that happens if it goes wrong?".
/// Those are different questions and conflating them loses information: a
/// startup-chime change and an ABS calibration are both configuration writes at
/// the same level, and no sane product should present them the same way.
///
/// Ordering is by consequence, so `>=` comparisons are meaningful and a policy
/// can say "nothing above Convenience" without enumerating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    /// Reads only. Nothing about the vehicle changes.
    Read,
    /// Appearance and sound: startup animation, chimes, gauge styling. Wrong
    /// values are annoying, never dangerous.
    Cosmetic,
    /// Comfort behaviour: mirror auto-fold, lighting, lock preferences. A wrong
    /// value is inconvenient and reversible.
    Convenience,
    /// Maintenance resets, relearns, calibrations of non-safety systems.
    Service,
    /// Commanding a component to move for testing: actuators, solenoids, fans.
    /// Physically consequential while it runs.
    DiagnosticControl,
    /// Anything in the path of stopping, steering or accelerating the vehicle.
    SafetyCritical,
    /// Immobiliser, keys, security access.
    Security,
    /// Firmware. A failed write leaves a module that no longer boots.
    Programming,
}

impl RiskClass {
    /// Human-facing label.
    pub fn label(&self) -> &'static str {
        match self {
            RiskClass::Read => "read only",
            RiskClass::Cosmetic => "cosmetic",
            RiskClass::Convenience => "convenience",
            RiskClass::Service => "service",
            RiskClass::DiagnosticControl => "component test",
            RiskClass::SafetyCritical => "safety critical",
            RiskClass::Security => "security",
            RiskClass::Programming => "programming",
        }
    }

    /// Whether this build will ever carry an implementation for this class.
    ///
    /// `false` is a design decision, not a gap waiting to be filled: security
    /// access and firmware programming are how a diagnostic tool turns into a
    /// theft tool and a brick factory respectively, and neither belongs in
    /// something distributed openly.
    pub fn is_ever_implementable(&self) -> bool {
        !matches!(self, RiskClass::Security | RiskClass::Programming)
    }
}

impl PermissionLevel {
    /// Whether reaching this level requires an explicit user confirmation token.
    pub fn requires_confirmation(&self) -> bool {
        !matches!(self, PermissionLevel::L0)
    }

    /// Whether this level mutates vehicle state and therefore must be audited
    /// with an initiator recorded (handoff §10, "record who/what initiated").
    pub fn is_write_capable(&self) -> bool {
        !matches!(self, PermissionLevel::L0)
    }

    /// Stable wire form.
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionLevel::L0 => "L0",
            PermissionLevel::L1 => "L1",
            PermissionLevel::L2 => "L2",
            PermissionLevel::L3 => "L3",
        }
    }
}

/// Severity of a warning attached to a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningSeverity {
    /// Contextual note; nothing is wrong.
    Info,
    /// Something is degraded or unvalidated; do not present as fact.
    Caution,
    /// The result is materially incomplete or suspect.
    Serious,
}

/// A caveat that travels with a tool result instead of being logged and lost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable warning code, e.g. `unverified_decoder`, `partial_read`.
    pub code: String,
    /// Developer-facing detail.
    pub message: String,
    /// How much this should reduce confidence.
    pub severity: WarningSeverity,
}

impl Warning {
    /// Informational note.
    pub fn info(code: impl Into<String>, message: impl Into<String>) -> Self {
        Warning { code: code.into(), message: message.into(), severity: WarningSeverity::Info }
    }

    /// Reduce-confidence note.
    pub fn caution(code: impl Into<String>, message: impl Into<String>) -> Self {
        Warning { code: code.into(), message: message.into(), severity: WarningSeverity::Caution }
    }

    /// Result is materially suspect.
    pub fn serious(code: impl Into<String>, message: impl Into<String>) -> Self {
        Warning { code: code.into(), message: message.into(), severity: WarningSeverity::Serious }
    }
}

/// Handoff §7 tool result envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Name of the tool that ran.
    pub tool: String,
    /// When the tool finished.
    pub timestamp: Timestamp,
    /// Session the call belongs to.
    pub vehicle_session_id: SessionId,
    /// Module addressed, when the tool is module-scoped.
    pub module: Option<String>,
    /// Whether the tool produced a usable result.
    pub success: bool,
    /// Decoded values produced. Empty on failure.
    pub values: Vec<DecodedValue>,
    /// Arbitrary structured payload for tools whose output is not a value list
    /// (module scans, DTC lists, supported-PID bitmaps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Pointer into the append-only event log holding the raw exchange.
    pub raw_evidence_ref: Option<i64>,
    /// Caveats. Never empty when the result is partial or unverified.
    pub warnings: Vec<Warning>,
    /// The allowlisted capability id that authorized this execution.
    pub capability_used: String,
    /// Wall time spent inside the tool.
    pub execution_time_ms: u64,
    /// Structured error, present exactly when `success` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::AimError>,
}

impl ToolResult {
    /// A successful result skeleton; fill in `values`/`data` afterwards.
    pub fn success(
        tool: impl Into<String>,
        session: SessionId,
        capability_used: impl Into<String>,
        execution_time_ms: u64,
    ) -> Self {
        ToolResult {
            tool: tool.into(),
            timestamp: crate::now(),
            vehicle_session_id: session,
            module: None,
            success: true,
            values: Vec::new(),
            data: None,
            raw_evidence_ref: None,
            warnings: Vec::new(),
            capability_used: capability_used.into(),
            execution_time_ms,
            error: None,
        }
    }

    /// A failed result carrying the structured error.
    pub fn failure(
        tool: impl Into<String>,
        session: SessionId,
        capability_used: impl Into<String>,
        execution_time_ms: u64,
        error: crate::AimError,
    ) -> Self {
        ToolResult {
            tool: tool.into(),
            timestamp: crate::now(),
            vehicle_session_id: session,
            module: None,
            success: false,
            values: Vec::new(),
            data: None,
            raw_evidence_ref: None,
            warnings: Vec::new(),
            capability_used: capability_used.into(),
            execution_time_ms,
            error: Some(error),
        }
    }

    /// Set the addressed module.
    pub fn with_module(mut self, module: impl Into<String>) -> Self {
        self.module = Some(module.into());
        self
    }

    /// Attach decoded values, propagating an `unverified_decoder` warning for
    /// any value whose decoder definition has not been validated.
    pub fn with_values(mut self, values: Vec<DecodedValue>) -> Self {
        let unverified: Vec<&str> = values
            .iter()
            .filter(|v| !v.provenance.verification.is_trustworthy())
            .map(|v| v.signal_id.as_str())
            .collect();
        if !unverified.is_empty() {
            self.warnings.push(Warning::caution(
                "unverified_decoder",
                format!(
                    "decoder definitions not validated for: {}",
                    unverified.join(", ")
                ),
            ));
        }
        let out_of_range: Vec<&str> = values
            .iter()
            .filter(|v| v.out_of_range)
            .map(|v| v.signal_id.as_str())
            .collect();
        if !out_of_range.is_empty() {
            self.warnings.push(Warning::serious(
                "value_out_of_range",
                format!(
                    "decoded outside declared valid range: {}",
                    out_of_range.join(", ")
                ),
            ));
        }
        self.values = values;
        self
    }

    /// Attach a structured payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Attach the event-log row holding the raw exchange.
    pub fn with_evidence(mut self, row_id: i64) -> Self {
        self.raw_evidence_ref = Some(row_id);
        self
    }

    /// Append a warning.
    pub fn warn(mut self, w: Warning) -> Self {
        self.warnings.push(w);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{provenance::VerificationStatus, Provenance, Value};

    fn value(id: &str, status: VerificationStatus, v: f64, range: Option<crate::ValidRange>) -> DecodedValue {
        DecodedValue::new(
            id,
            id,
            Value::Number(v),
            Some("x".into()),
            range,
            Provenance::decoded(&[0x41], id, "1", status, crate::now()),
        )
    }

    #[test]
    fn permission_levels_order_and_gate() {
        assert!(PermissionLevel::L0 < PermissionLevel::L1);
        assert!(PermissionLevel::L2 < PermissionLevel::L3);
        assert!(!PermissionLevel::L0.requires_confirmation());
        assert!(PermissionLevel::L1.requires_confirmation());
        assert!(!PermissionLevel::L0.is_write_capable());
        assert!(PermissionLevel::L2.is_write_capable());
    }

    #[test]
    fn unverified_values_force_a_warning_onto_the_envelope() {
        let r = ToolResult::success("read_pid", SessionId::from_string("ses_1"), "obd2.read_pid", 3)
            .with_values(vec![value("dpf_temp", VerificationStatus::Unverified, 300.0, None)]);
        assert!(r.warnings.iter().any(|w| w.code == "unverified_decoder"));
        assert_eq!(r.warnings[0].severity, WarningSeverity::Caution);
    }

    #[test]
    fn out_of_range_values_force_a_serious_warning() {
        let r = ToolResult::success("read_pid", SessionId::from_string("ses_1"), "obd2.read_pid", 3)
            .with_values(vec![value(
                "coolant",
                VerificationStatus::Verified,
                9000.0,
                Some(crate::ValidRange { min: 0.0, max: 100.0 }),
            )]);
        assert!(r.warnings.iter().any(|w| w.code == "value_out_of_range"
            && w.severity == WarningSeverity::Serious));
    }

    #[test]
    fn clean_verified_values_produce_no_warnings() {
        let r = ToolResult::success("read_pid", SessionId::from_string("ses_1"), "obd2.read_pid", 3)
            .with_values(vec![value(
                "coolant",
                VerificationStatus::Verified,
                90.0,
                Some(crate::ValidRange { min: 0.0, max: 200.0 }),
            )]);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn failure_carries_the_error() {
        let r = ToolResult::failure(
            "read_pid",
            SessionId::from_string("ses_1"),
            "obd2.read_pid",
            1,
            crate::AimError::no_data("nothing answered"),
        );
        assert!(!r.success);
        assert_eq!(r.error.unwrap().code, crate::ErrorCode::NoData);
    }
}
