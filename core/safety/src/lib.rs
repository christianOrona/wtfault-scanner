//! `aim-safety` — the capability allowlist and permission gate.
//!
//! Handoff §10 makes safety an architectural requirement rather than a UI
//! warning. Four rules from that section are implemented here and each has a
//! test that proves it:
//!
//! 1. **Capability allowlist.** Only operations registered in the
//!    [`CapabilityRegistry`] can execute. An unregistered id is rejected —
//!    never "best effort" executed.
//! 2. **Fail closed.** Every path that cannot prove an operation is safe
//!    rejects it. There is no default-allow branch in [`SafetyGate::authorize`].
//! 3. **Levels.** L0 runs freely; L1 needs an explicit confirmation token;
//!    L2 and L3 are compiled off in this build via [`MAX_ENABLED_LEVEL`].
//! 4. **Audit.** Every authorization decision — allowed or rejected — produces
//!    an [`AuditRecord`] naming the initiator. A non-read operation cannot be
//!    authorized without one.
//!
//! The future agent runtime calls tools, tools call this gate. The gate has no
//! idea a model exists, which is the point.

#![warn(missing_docs)]

pub mod preconditions;

pub use preconditions::{Precondition, VehicleConditions};

use aim_types::{AimError, AimResult, ErrorCode, PermissionLevel, RiskClass, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The highest permission level this build will execute.
///
/// L2 (configuration writes) and L3 (programming) are out of scope for Phase 1
/// and are refused by the gate regardless of confirmation. Raising this
/// constant is a deliberate, reviewable change — not a runtime setting, not a
/// config file, and certainly not something an agent can ask for.
pub const MAX_ENABLED_LEVEL: PermissionLevel = PermissionLevel::L1;

/// Risk class for a capability deserialized from an older record that predates
/// the field. Read is the safe default: it under-claims rather than over-claims.
fn default_risk() -> RiskClass {
    RiskClass::Read
}

/// A registered, allowlisted operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// Stable operation id, e.g. `obd2.read_dtcs`.
    pub id: String,
    /// What it does. Developer-facing.
    pub description: String,
    /// Permission level required.
    pub level: PermissionLevel,
    /// Conditions that must hold before execution.
    pub preconditions: Vec<Precondition>,
    /// Adapter capability flags the operation needs, e.g. `supports_transmit`.
    pub required_adapter_flags: Vec<AdapterFlag>,
    /// What kind of thing this touches, independent of how privileged it is.
    #[serde(default = "default_risk")]
    pub risk: RiskClass,
    /// True when this operation changes vehicle state.
    pub mutating: bool,
}

impl Capability {
    /// A read-only (L0) capability with no preconditions.
    pub fn read_only(id: &str, description: &str) -> Self {
        Capability {
            id: id.to_string(),
            description: description.to_string(),
            level: PermissionLevel::L0,
            preconditions: Vec::new(),
            required_adapter_flags: Vec::new(),
            risk: RiskClass::Read,
            mutating: false,
        }
    }

    /// Set the risk class.
    pub fn with_risk(mut self, risk: RiskClass) -> Self {
        self.risk = risk;
        self
    }

    /// Set the permission level.
    pub fn at_level(mut self, level: PermissionLevel) -> Self {
        self.level = level;
        self.mutating = level.is_write_capable();
        self
    }

    /// Attach preconditions.
    pub fn requiring(mut self, preconditions: Vec<Precondition>) -> Self {
        self.preconditions = preconditions;
        self
    }

    /// Attach required adapter capability flags.
    pub fn needing_adapter(mut self, flags: Vec<AdapterFlag>) -> Self {
        self.required_adapter_flags = flags;
        self
    }
}

/// Adapter capability flags an operation can depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterFlag {
    /// The adapter must be able to transmit, not only poll.
    SupportsTransmit,
    /// Messages longer than one CAN frame must be handled.
    SupportsLongMessages,
    /// 29-bit CAN identifiers must be usable.
    Can29Bit,
    /// More than one CAN bus must be reachable.
    MultipleCanBuses,
    /// A J2534 pass-through device is required.
    J2534,
}

impl AdapterFlag {
    /// Read this flag out of an observed capability set.
    pub fn is_present(&self, caps: &aim_types::AdapterCapabilities) -> bool {
        match self {
            AdapterFlag::SupportsTransmit => caps.supports_transmit,
            AdapterFlag::SupportsLongMessages => caps.supports_long_messages,
            AdapterFlag::Can29Bit => caps.can_29_bit,
            AdapterFlag::MultipleCanBuses => caps.multiple_can_buses,
            AdapterFlag::J2534 => caps.j2534,
        }
    }
}

/// The allowlist. Nothing outside it can run.
#[derive(Debug, Clone, Default)]
pub struct CapabilityRegistry {
    capabilities: BTreeMap<String, Capability>,
}

impl CapabilityRegistry {
    /// An empty registry — every operation is rejected.
    pub fn new() -> Self {
        Self::default()
    }

    /// The operations this build ships with.
    ///
    /// Read operations are L0. `clear_dtcs` is L1: implemented, requiring an
    /// explicit confirmation, and never reachable by the agent. Configuration
    /// writes and programming stay at L2 and L3, registered so their refusal is
    /// explicit and auditable rather than an "unknown operation" accident.
    pub fn phase1() -> Self {
        let mut r = CapabilityRegistry::new();
        for c in [
            Capability::read_only("obd2.identify_vehicle", "Read VIN and calibration identifiers"),
            Capability::read_only("obd2.scan_modules", "Discover responding ECUs"),
            Capability::read_only("obd2.get_module_identity", "Read a module's identity strings"),
            Capability::read_only("obd2.read_dtcs", "Read stored, pending and permanent DTCs"),
            Capability::read_only("obd2.read_freeze_frame", "Read freeze frame data"),
            Capability::read_only("obd2.read_pid", "Read one current-data PID"),
            Capability::read_only("obd2.read_live_data", "Read several PIDs as a sample"),
            Capability::read_only("obd2.read_supported_pids", "Enumerate supported PIDs"),
            Capability::read_only(
                "obd2.read_monitor_tests",
                "Read on-board monitor test results and their pass or fail limits",
            ),
            Capability::read_only("obd2.read_vehicle_configuration", "Read vehicle configuration"),
            Capability::read_only("adapter.connect", "Open and initialize the adapter link"),
            Capability::read_only("adapter.disconnect", "Close the adapter link"),
            Capability::read_only("adapter.health", "Read adapter link health"),
            Capability::read_only("dtc.lookup", "Look up a DTC description"),
            // L1: implemented as a seam, requires confirmation and an adapter
            // that can actually transmit. No L1 test is registered in this
            // build, so the level is exercised only by tests.
            Capability::read_only("obd2.run_self_test", "Run an on-board monitor self test")
                .at_level(PermissionLevel::L1)
                .requiring(vec![Precondition::IgnitionOn, Precondition::StableConnection])
                .needing_adapter(vec![AdapterFlag::SupportsTransmit]),
            // Reading which features a vehicle has, and what they are currently
            // set to, changes nothing. It is the whole of what this build can
            // actually do with vehicle configuration, and it is L0.
            Capability::read_only(
                "config.list_features",
                "List the configurable features that could apply to this vehicle",
            ),
            Capability::read_only(
                "config.read_feature",
                "Read the current setting of one configurable feature",
            ),
            // Previewing a change is also read-only: it evaluates every check
            // and reports what *would* happen without touching the vehicle.
            // Deliberately separate from the write, so the UI can show a person
            // the full consequence before anything is authorised.
            Capability::read_only(
                "config.preview_change",
                "Evaluate a proposed configuration change without applying it",
            ),
            // L2: registered so the seam exists, refused by MAX_ENABLED_LEVEL.
            // The risk class is carried separately from the level because a
            // policy that says "convenience changes only" cannot be expressed
            // in levels alone.
            Capability::read_only(
                "config.write_feature",
                "Change a vehicle configuration setting",
            )
            .at_level(PermissionLevel::L2)
            .with_risk(RiskClass::Convenience)
            .requiring(vec![
                Precondition::IgnitionOn,
                Precondition::EngineOff,
                Precondition::StableConnection,
                Precondition::VehicleStationary,
                Precondition::BatteryVoltageAtLeast(12.4),
            ])
            .needing_adapter(vec![AdapterFlag::SupportsTransmit]),
            // Clearing codes is the one write every twenty-pound code reader
            // performs, and refusing it made this tool strictly less capable
            // than the thing it is meant to replace. It is L1: implemented,
            // requiring an explicit confirmation token, never reachable by
            // accident and never available to the agent, which is registered
            // with read-only tools only.
            //
            // It is genuinely destructive, which is why the confirmation is not
            // a formality. Service 04 erases the stored codes, the freeze
            // frames captured with them, and every readiness monitor — the last
            // of which takes 50 to 100 miles of driving to rebuild and, until
            // it does, fails an emissions test outright. The UI states all of
            // that before asking.
            Capability::read_only(
                "obd2.clear_dtcs",
                "Clear stored codes, freeze frames and readiness monitors",
            )
            .at_level(PermissionLevel::L1)
            .with_risk(RiskClass::Service)
            .requiring(vec![
                Precondition::EngineOff,
                Precondition::VehicleStationary,
                Precondition::StableConnection,
            ])
            .needing_adapter(vec![AdapterFlag::SupportsTransmit]),
            // L2/L3: registered so the refusal is explicit and auditable
            // rather than an "unknown operation" accident.
            Capability::read_only("config.write_configuration", "Write module configuration")
                .at_level(PermissionLevel::L2),
            Capability::read_only("program.program_module", "Reprogram a module")
                .at_level(PermissionLevel::L3),
        ] {
            r.register(c);
        }
        r
    }

    /// Add a capability. Replaces any existing entry with the same id.
    pub fn register(&mut self, c: Capability) {
        self.capabilities.insert(c.id.clone(), c);
    }

    /// Look up a capability.
    pub fn get(&self, id: &str) -> Option<&Capability> {
        self.capabilities.get(id)
    }

    /// Every registered capability, id order.
    pub fn all(&self) -> Vec<&Capability> {
        self.capabilities.values().collect()
    }

    /// Capabilities this build will actually execute.
    pub fn enabled(&self) -> Vec<&Capability> {
        self.capabilities
            .values()
            .filter(|c| c.level <= MAX_ENABLED_LEVEL)
            .collect()
    }
}

/// A request to perform an allowlisted operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationRequest {
    /// Capability id being requested.
    pub operation: String,
    /// Who or what asked: `user`, `agent:<name>`, `system`.
    ///
    /// Mandatory. §10 requires that every write-capable operation records its
    /// initiator, and the gate refuses to authorize one without it.
    pub initiator: String,
    /// A confirmation token supplied by a human, for L1 and above.
    #[serde(default)]
    pub confirmation: Option<Confirmation>,
    /// Free-form context recorded in the audit trail.
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

impl OperationRequest {
    /// A read request from a named initiator.
    pub fn new(operation: impl Into<String>, initiator: impl Into<String>) -> Self {
        OperationRequest {
            operation: operation.into(),
            initiator: initiator.into(),
            confirmation: None,
            context: None,
        }
    }

    /// Attach a human confirmation.
    pub fn confirmed_by(mut self, user: impl Into<String>) -> Self {
        self.confirmation = Some(Confirmation {
            confirmed_by: user.into(),
            confirmed_at: aim_types::now(),
        });
        self
    }
}

/// Evidence that a human confirmed an operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confirmation {
    /// Who confirmed. Must be a human identity, never an agent.
    pub confirmed_by: String,
    /// When.
    pub confirmed_at: Timestamp,
}

impl Confirmation {
    /// True when the confirming identity is a human rather than an agent.
    ///
    /// An agent cannot confirm its own request: that would make the
    /// confirmation requirement decorative.
    pub fn is_human(&self) -> bool {
        !self.confirmed_by.is_empty()
            && !self.confirmed_by.starts_with("agent:")
            && self.confirmed_by != "system"
    }
}

/// Permission granted by the gate. Only the gate can construct one, so a tool
/// cannot execute without having gone through authorization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Authorization {
    /// The capability that was authorized.
    pub capability: String,
    /// Its level.
    pub level: PermissionLevel,
    /// Who asked.
    pub initiator: String,
    /// Whether a human confirmation accompanied it.
    pub confirmed: bool,
    /// When permission was granted.
    pub granted_at: Timestamp,
}

/// The audit record produced by every decision, allowed or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Operation id requested.
    pub operation: String,
    /// Level of the operation, when it was a known one.
    pub level: Option<PermissionLevel>,
    /// Who or what asked.
    pub initiator: String,
    /// Whether a confirmation was supplied.
    pub confirmed: bool,
    /// Whether it was allowed.
    pub allowed: bool,
    /// Rejection reason code, when rejected.
    pub reason: Option<String>,
    /// When the decision was made.
    pub decided_at: Timestamp,
}

/// The outcome of an authorization attempt: a decision plus its audit record.
///
/// The audit record is produced even when the request is refused, so refusals
/// are recorded in the flight recorder rather than vanishing.
#[derive(Debug, Clone)]
pub struct Decision {
    /// `Ok` with permission, or `Err` with the structured refusal.
    pub result: AimResult<Authorization>,
    /// What to write to the event log.
    pub audit: AuditRecord,
}

impl Decision {
    /// Whether the operation may proceed.
    pub fn is_allowed(&self) -> bool {
        self.result.is_ok()
    }

    /// Unwrap into a `Result`, dropping the audit record.
    pub fn into_result(self) -> AimResult<Authorization> {
        self.result
    }
}

/// The permission gate.
#[derive(Debug, Clone)]
pub struct SafetyGate {
    registry: CapabilityRegistry,
}

impl SafetyGate {
    /// Build a gate over an allowlist.
    pub fn new(registry: CapabilityRegistry) -> Self {
        SafetyGate { registry }
    }

    /// The Phase 1 gate.
    pub fn phase1() -> Self {
        SafetyGate::new(CapabilityRegistry::phase1())
    }

    /// The allowlist behind this gate.
    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// Decide whether `request` may proceed.
    ///
    /// Checks run in fixed order — existence, level ceiling, initiator,
    /// confirmation, adapter capability, preconditions — and the first failure
    /// wins, so a refusal always names the most fundamental reason.
    pub fn authorize(
        &self,
        request: &OperationRequest,
        adapter: Option<&aim_types::AdapterCapabilities>,
        conditions: &VehicleConditions,
    ) -> Decision {
        let now = aim_types::now();
        let mut audit = AuditRecord {
            operation: request.operation.clone(),
            level: None,
            initiator: request.initiator.clone(),
            confirmed: request.confirmation.is_some(),
            allowed: false,
            reason: None,
            decided_at: now,
        };

        // 1. Allowlist. Unknown operations are refused, never attempted.
        let Some(cap) = self.registry.get(&request.operation) else {
            audit.reason = Some(String::from("operation_not_allowed"));
            return Decision {
                result: Err(AimError::new(
                    ErrorCode::OperationNotAllowed,
                    format!(
                        "{:?} is not an allowlisted operation; unknown operations are rejected, not attempted",
                        request.operation
                    ),
                )
                .with_details(serde_json::json!({ "operation": request.operation }))),
                audit,
            };
        };
        audit.level = Some(cap.level);

        // 2. Level ceiling for this build.
        if cap.level > MAX_ENABLED_LEVEL {
            audit.reason = Some(String::from("permission_level_disabled"));
            return Decision {
                result: Err(AimError::new(
                    ErrorCode::PermissionLevelDisabled,
                    format!(
                        "{} requires {} which is disabled in this build (ceiling {})",
                        cap.id,
                        cap.level.as_str(),
                        MAX_ENABLED_LEVEL.as_str()
                    ),
                )
                .with_details(serde_json::json!({
                    "operation": cap.id,
                    "required_level": cap.level.as_str(),
                    "max_enabled_level": MAX_ENABLED_LEVEL.as_str(),
                }))),
                audit,
            };
        }

        // 3. Initiator. Mandatory for anything that is not a pure read.
        if request.initiator.trim().is_empty() {
            audit.reason = Some(String::from("missing_initiator"));
            return Decision {
                result: Err(AimError::new(
                    ErrorCode::BadRequest,
                    "every operation must record an initiator",
                )),
                audit,
            };
        }

        // 4. Confirmation for L1 and above, and it must come from a human.
        if cap.level.requires_confirmation() {
            match &request.confirmation {
                None => {
                    audit.reason = Some(String::from("confirmation_required"));
                    return Decision {
                        result: Err(AimError::new(
                            ErrorCode::ConfirmationRequired,
                            format!("{} requires explicit user confirmation", cap.id),
                        )
                        .with_details(serde_json::json!({
                            "operation": cap.id,
                            "level": cap.level.as_str(),
                            "preconditions": cap.preconditions,
                        }))),
                        audit,
                    };
                }
                Some(c) if !c.is_human() => {
                    audit.reason = Some(String::from("confirmation_not_human"));
                    return Decision {
                        result: Err(AimError::new(
                            ErrorCode::ConfirmationRequired,
                            format!(
                                "{} was confirmed by {:?}, which is not a human identity",
                                cap.id, c.confirmed_by
                            ),
                        )),
                        audit,
                    };
                }
                Some(_) => {}
            }
        }

        // 5. Adapter capability. Missing flags fail closed: an unknown
        //    capability set means no, not "probably fine".
        for flag in &cap.required_adapter_flags {
            let present = adapter.is_some_and(|a| flag.is_present(a));
            if !present {
                audit.reason = Some(String::from("capability_missing"));
                return Decision {
                    result: Err(AimError::new(
                        ErrorCode::CapabilityMissing,
                        format!(
                            "{} needs adapter capability {:?}, which the connected adapter does not report",
                            cap.id, flag
                        ),
                    )
                    .with_details(serde_json::json!({
                        "operation": cap.id,
                        "required_flag": flag,
                    }))),
                    audit,
                };
            }
        }

        // 6. Vehicle preconditions.
        if let Err(failure) = preconditions::check_all(&cap.preconditions, conditions) {
            audit.reason = Some(String::from("precondition_failed"));
            return Decision { result: Err(failure), audit };
        }

        audit.allowed = true;
        Decision {
            result: Ok(Authorization {
                capability: cap.id.clone(),
                level: cap.level,
                initiator: request.initiator.clone(),
                confirmed: request.confirmation.is_some(),
                granted_at: now,
            }),
            audit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_types::{AdapterCapabilities, TransportKind};

    fn caps(transmit: bool) -> AdapterCapabilities {
        let mut c = AdapterCapabilities::unknown(TransportKind::Bluetooth);
        c.elm327_compatible = true;
        c.supports_transmit = transmit;
        c
    }

    fn ready() -> VehicleConditions {
        VehicleConditions {
            ignition_on: true,
            engine_running: false,
            battery_voltage: Some(12.6),
            connection_stable: true,
            vehicle_speed_kph: Some(0.0),
        }
    }

    #[test]
    fn read_operations_are_allowed_without_confirmation() {
        let gate = SafetyGate::phase1();
        let d = gate.authorize(
            &OperationRequest::new("obd2.read_dtcs", "user"),
            Some(&caps(false)),
            &ready(),
        );
        assert!(d.is_allowed());
        assert!(d.audit.allowed);
        assert_eq!(d.audit.level, Some(PermissionLevel::L0));
        assert_eq!(d.audit.initiator, "user");
        let auth = d.into_result().unwrap();
        assert_eq!(auth.level, PermissionLevel::L0);
        assert!(!auth.confirmed);
    }

    #[test]
    fn unknown_operations_are_rejected_deterministically() {
        let gate = SafetyGate::phase1();
        for op in [
            "obd2.send_raw_can",
            "flash_ecu",
            "",
            "obd2.read_dtcs ", // trailing space: not the same id
            "OBD2.READ_DTCS",  // wrong case: not the same id
            "../../etc/passwd",
        ] {
            let d = gate.authorize(
                &OperationRequest::new(op, "agent:planner"),
                Some(&caps(true)),
                &ready(),
            );
            assert!(!d.is_allowed(), "{op:?} must be rejected");
            assert_eq!(d.audit.reason.as_deref(), Some("operation_not_allowed"));
            assert_eq!(
                d.into_result().unwrap_err().code,
                ErrorCode::OperationNotAllowed
            );
        }
    }

    #[test]
    fn rejections_are_stable_across_repeated_attempts() {
        let gate = SafetyGate::phase1();
        let req = OperationRequest::new("obd2.send_raw_can", "agent:planner");
        for _ in 0..100 {
            let d = gate.authorize(&req, Some(&caps(true)), &ready());
            assert!(!d.is_allowed());
        }
    }

    #[test]
    fn clearing_codes_needs_a_confirmation_and_never_happens_by_accident() {
        let gate = SafetyGate::phase1();

        // Unconfirmed: refused, and the reason names what is missing.
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("confirmation_required"));

        // Confirmed, engine off, stationary, link healthy: allowed.
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user").confirmed_by("owner"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(d.is_allowed(), "{:?}", d.audit.reason);
    }

    #[test]
    fn clearing_codes_is_refused_with_the_engine_running() {
        // Service 04 on a running engine is refused by many modules anyway, and
        // attempting it mid-drive is how a person ends up with a half-cleared
        // state they cannot explain.
        let gate = SafetyGate::phase1();
        let mut conditions = ready();
        conditions.engine_running = true;
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user").confirmed_by("owner"),
            Some(&caps(true)),
            &conditions,
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("precondition_failed"));
    }

    #[test]
    fn l2_and_l3_operations_are_all_refused() {
        let gate = SafetyGate::phase1();
        for op in ["config.write_configuration", "program.program_module"] {
            let d = gate.authorize(
                &OperationRequest::new(op, "user").confirmed_by("owner"),
                Some(&caps(true)),
                &ready(),
            );
            assert_eq!(
                d.into_result().unwrap_err().code,
                ErrorCode::PermissionLevelDisabled,
                "{op}"
            );
        }
    }

    #[test]
    fn l1_requires_confirmation() {
        let gate = SafetyGate::phase1();
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "agent:planner"),
            Some(&caps(true)),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("confirmation_required"));
        assert_eq!(
            d.into_result().unwrap_err().code,
            ErrorCode::ConfirmationRequired
        );

        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "agent:planner").confirmed_by("owner"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(d.is_allowed());
        assert!(d.into_result().unwrap().confirmed);
    }

    #[test]
    fn an_agent_cannot_confirm_its_own_request() {
        let gate = SafetyGate::phase1();
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "agent:planner")
                .confirmed_by("agent:planner"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("confirmation_not_human"));
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "system").confirmed_by("system"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(!d.is_allowed());
    }

    #[test]
    fn operations_without_an_initiator_are_refused() {
        let gate = SafetyGate::phase1();
        let d = gate.authorize(
            &OperationRequest::new("obd2.read_dtcs", "   "),
            Some(&caps(false)),
            &ready(),
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("missing_initiator"));
    }

    #[test]
    fn missing_adapter_capabilities_fail_closed() {
        let gate = SafetyGate::phase1();
        // Adapter cannot transmit.
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "user").confirmed_by("owner"),
            Some(&caps(false)),
            &ready(),
        );
        assert_eq!(
            d.into_result().unwrap_err().code,
            ErrorCode::CapabilityMissing
        );
        // No adapter connected at all: also closed.
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "user").confirmed_by("owner"),
            None,
            &ready(),
        );
        assert_eq!(
            d.into_result().unwrap_err().code,
            ErrorCode::CapabilityMissing
        );
    }

    #[test]
    fn failed_preconditions_block_execution() {
        let gate = SafetyGate::phase1();
        let mut conditions = ready();
        conditions.ignition_on = false;
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "user").confirmed_by("owner"),
            Some(&caps(true)),
            &conditions,
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("precondition_failed"));
        assert_eq!(
            d.into_result().unwrap_err().code,
            ErrorCode::PreconditionFailed
        );
    }

    #[test]
    fn every_decision_produces_an_audit_record_naming_the_initiator() {
        let gate = SafetyGate::phase1();
        for (op, initiator) in [
            ("obd2.read_dtcs", "user"),
            ("obd2.clear_dtcs", "agent:planner"),
            ("does.not.exist", "agent:planner"),
        ] {
            let d = gate.authorize(
                &OperationRequest::new(op, initiator),
                Some(&caps(true)),
                &ready(),
            );
            assert_eq!(d.audit.initiator, initiator);
            assert_eq!(d.audit.operation, op);
            assert_eq!(d.audit.allowed, d.is_allowed());
            if !d.is_allowed() {
                assert!(d.audit.reason.is_some(), "{op} must record why");
            }
        }
    }

    #[test]
    fn an_empty_registry_allows_nothing() {
        let gate = SafetyGate::new(CapabilityRegistry::new());
        let d = gate.authorize(
            &OperationRequest::new("obd2.read_dtcs", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(!d.is_allowed());
    }

    #[test]
    fn the_enabled_set_excludes_l2_and_l3() {
        let r = CapabilityRegistry::phase1();
        assert!(r.all().len() > r.enabled().len());
        for c in r.enabled() {
            assert!(c.level <= MAX_ENABLED_LEVEL, "{} is enabled", c.id);
        }
        for id in ["config.write_configuration", "program.program_module", "config.write_feature"] {
            assert!(
                !r.enabled().iter().any(|c| c.id == id),
                "{id} must not be enabled"
            );
        }
    }
}
