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
//! 3. **Two ceilings.** L0 runs freely; L1 and L2 need a [`HumanApproval`],
//!    which only an [`ApprovalDesk`] can make; L3 (programming) is compiled off
//!    via [`MAX_ENABLED_LEVEL`]. A
//!    second, independent ceiling — [`MAX_ENABLED_RISK`] — refuses on
//!    consequence rather than privilege, because a mirror-fold preference and a
//!    brake calibration are the same permission level and must not be the same
//!    decision.
//! 4. **Audit.** Every authorization decision — allowed or rejected — produces
//!    an [`AuditRecord`] naming the initiator. A non-read operation cannot be
//!    authorized without one.
//!
//! 5. **Nothing leaves without permission.** A capability names the
//!    state-changing services it may put on the vehicle bus, and the
//!    [`Authorization`] the gate grants is what the diagnostic core has to show
//!    before it sends one. Only this crate can make an [`Authorization`].
//!
//! The future agent runtime calls tools, tools call this gate. The gate has no
//! idea a model exists, which is the point.

#![warn(missing_docs)]

pub mod approval;
pub mod preconditions;

pub use approval::{ApprovalDesk, Challenge, HumanApproval};
pub use preconditions::{Precondition, VehicleConditions};

use aim_types::{AimError, AimResult, ErrorCode, PermissionLevel, RiskClass, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The highest permission level this build will execute.
///
/// L2 is configuration writes: changing a setting a vehicle already supports,
/// through the same public standard used to read it. Those are enabled, behind
/// a typed confirmation and the full precondition list. L3 is programming —
/// firmware — and stays refused.
///
/// Raising this constant is a deliberate, reviewable change: not a runtime
/// setting, not a config file, and never something an agent can ask for.
pub const MAX_ENABLED_LEVEL: PermissionLevel = PermissionLevel::L2;

/// The most consequential kind of thing this build will touch, whatever its
/// permission level.
///
/// This is a second ceiling, deliberately independent of [`MAX_ENABLED_LEVEL`],
/// because "how privileged is this operation" and "what happens if it is wrong"
/// are different questions. A convenience setting and a brake calibration can
/// both be L2 writes; only one of them can hurt somebody.
///
/// Everything above [`RiskClass::Service`] is refused here **as policy, not as
/// an unfinished feature**. The distinction matters and is worth stating
/// plainly, because the two look identical from the outside:
///
/// - **Safety-critical** — anything in the path of stopping, steering or
///   accelerating. Refused because a diagnostic tool that a person runs in
///   their own driveway should not be able to change how the vehicle stops.
/// - **Security** — immobiliser, keys. Refused because the legitimate uses are
///   indistinguishable, over a wire, from the illegitimate ones.
/// - **Programming** — firmware. Refused because a failed write leaves a module
///   that does not boot, and recovery needs equipment this app cannot assume.
///
/// Raising this is not a matter of writing more code. It is a decision about
/// what this tool is for.
pub const MAX_ENABLED_RISK: RiskClass = RiskClass::Service;

/// OBD-II service 04: clear the emissions codes, freeze frames and monitors.
pub const CLEAR_EMISSIONS_CODES: u8 = 0x04;
/// UDS service 14: clear one module's fault memory.
pub const CLEAR_DIAGNOSTIC_INFORMATION: u8 = 0x14;
/// UDS service 2E: write one data identifier.
pub const WRITE_DATA_BY_IDENTIFIER: u8 = 0x2E;

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
    /// True when a model may not initiate this, whatever else permits it.
    ///
    /// Not the same as "mutating". An agent proposing a self-test that a person
    /// then confirms is the product working as intended. An agent erasing the
    /// readiness monitors, or changing how a vehicle is configured, is not
    /// something a confirmation dialog makes acceptable — the model should not
    /// be the thing that raised the idea.
    #[serde(default)]
    pub agent_forbidden: bool,
    /// The state-changing services this operation may put on the vehicle bus,
    /// by service identifier. Empty for anything that only reads.
    ///
    /// Stated here, beside the level and the preconditions, so that what an
    /// operation is permitted to send has the same single source as whether it
    /// is permitted at all. The diagnostic core will not send a service that is
    /// not a read unless the [`Authorization`] it holds lists it.
    #[serde(default)]
    pub wire_services: Vec<u8>,
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
            agent_forbidden: false,
            wire_services: Vec::new(),
        }
    }

    /// Name the state-changing services this operation sends.
    pub fn sending(mut self, services: Vec<u8>) -> Self {
        self.wire_services = services;
        self
    }

    /// Mark this as something a model may never initiate.
    pub fn never_for_agents(mut self) -> Self {
        self.agent_forbidden = true;
        self
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
                "obd2.scan_all_modules",
                "Discover every module on the diagnostic bus and read its fault memory",
            ),
            Capability::read_only(
                "obd2.read_monitor_tests",
                "Read on-board monitor test results and their pass or fail limits",
            ),
            Capability::read_only(
                "obd2.probe_module_capabilities",
                "Measure which identifiers, sessions and services one module supports",
            ),
            Capability::read_only(
                "obd2.list_catalog_signals",
                "List community signal definitions that might apply to this vehicle",
            ),
            Capability::read_only(
                "obd2.read_catalog_signal",
                "Read one community-defined signal and report what the vehicle answered",
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
            // Reading a file off the user's own disk. It touches no vehicle,
            // and it is registered here anyway so that it is recorded and
            // gated like everything else: an import decides what the app
            // believes about a vehicle's configuration, and something that
            // consequential should not be the one operation with no audit
            // trail.
            Capability::read_only(
                "config.import_as_built",
                "Import a manufacturer as-built file for the connected vehicle",
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
            // Establishes *whether* a module accepts writes at all, by asking
            // it to write to an identifier it has already said it does not
            // have. Nothing can land: there is nowhere for it to go, and the
            // service call is refused before the module reaches for storage.
            //
            // Still declared as a write. It issues WriteDataByIdentifier, and
            // classifying it by its intent rather than by the service it sends
            // is exactly the kind of reasoning that turns into an accident. A
            // person confirms it; an agent may never initiate it.
            Capability::read_only(
                "config.probe_write_gate",
                "Find out whether a module accepts writes, without writing anything",
            )
            .at_level(PermissionLevel::L1)
            .with_risk(RiskClass::Convenience)
            .never_for_agents()
            .sending(vec![WRITE_DATA_BY_IDENTIFIER]),
            Capability::read_only("config.write_feature", "Change a vehicle configuration setting")
                .at_level(PermissionLevel::L2)
                .with_risk(RiskClass::Convenience)
                .never_for_agents()
                .requiring(vec![
                    Precondition::IgnitionOn,
                    Precondition::EngineOff,
                    Precondition::StableConnection,
                    Precondition::VehicleStationary,
                    Precondition::BatteryVoltageAtLeast(12.4),
                ])
                .needing_adapter(vec![AdapterFlag::SupportsTransmit])
                .sending(vec![WRITE_DATA_BY_IDENTIFIER]),
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
            .never_for_agents()
            .requiring(vec![
                Precondition::EngineOff,
                Precondition::VehicleStationary,
                Precondition::StableConnection,
            ])
            .needing_adapter(vec![AdapterFlag::SupportsTransmit])
            .sending(vec![CLEAR_EMISSIONS_CODES, CLEAR_DIAGNOSTIC_INFORMATION]),
            // There is deliberately no unbounded "write any configuration"
            // capability. One existed while L2 was compiled off, as a
            // placeholder whose only job was to be refused explicitly. Enabling
            // L2 turned it into exactly what this design exists to prevent: a
            // write with no feature definition behind it, no preconditions and
            // no read-back. `config.write_feature` is the only way in, and it
            // cannot name a value the catalogue does not describe.
            //
            // L3: registered so the refusal is explicit and auditable rather
            // than an "unknown operation" accident.
            Capability::read_only("program.program_module", "Reprogram a module")
                .at_level(PermissionLevel::L3)
                .with_risk(RiskClass::Programming),
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

    /// What one capability requires of the vehicle.
    ///
    /// Exposed so a preview can check the same list the authorisation will,
    /// from the same source. Two hand-maintained copies of a safety list is how
    /// a preview comes to promise something the write then refuses.
    pub fn preconditions(&self, id: &str) -> Vec<Precondition> {
        self.get(id).map(|c| c.preconditions.clone()).unwrap_or_default()
    }

    /// Capabilities this build will actually execute.
    pub fn enabled(&self) -> Vec<&Capability> {
        self.capabilities.values().filter(|c| c.level <= MAX_ENABLED_LEVEL).collect()
    }
}

/// What kind of caller an initiator label names.
///
/// The label is whatever the caller wrote, so this can only ever be used to
/// refuse. `user:api` proves nothing about who is asking; what it does is fail
/// to be `agent:`, and the operations a model may never start are refused for
/// anything that is not recognisably a person. Permission to change something
/// comes from a [`HumanApproval`] and from nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiatorKind {
    /// `user` or `user:<where>`.
    Person,
    /// `agent` or `agent:<name>`.
    Agent,
    /// `system` or `system:<what>`: the application acting for itself.
    System,
    /// Anything else. Treated as not a person.
    Unrecognised,
}

impl InitiatorKind {
    /// Classify an initiator label.
    ///
    /// Case and surrounding space are ignored, so `Agent:planner` and
    /// ` agent:planner` are an agent, as they would be to anybody reading the
    /// log. A label that fits none of the three families is unrecognised
    /// rather than assumed to be a person.
    pub fn of(initiator: &str) -> InitiatorKind {
        let label = initiator.trim().to_ascii_lowercase();
        match label.split(':').next().unwrap_or_default() {
            "user" => InitiatorKind::Person,
            "agent" => InitiatorKind::Agent,
            "system" => InitiatorKind::System,
            _ => InitiatorKind::Unrecognised,
        }
    }
}

/// A request to perform an allowlisted operation.
#[derive(Debug)]
pub struct OperationRequest {
    /// Capability id being requested.
    pub operation: String,
    /// Who or what asked: `user`, `agent:<name>`, `system`.
    ///
    /// Mandatory. §10 requires that every write-capable operation records its
    /// initiator, and the gate refuses to authorize one without it. It is a
    /// label for the audit trail: see [`InitiatorKind`] for what it can and
    /// cannot decide.
    pub initiator: String,
    /// What the operation would act on, in the words an approval names it by.
    /// Empty for an operation that needs no approval.
    pub subject: String,
    /// A person's approval, for L1 and above.
    pub approval: Option<HumanApproval>,
    /// Free-form context recorded in the audit trail.
    pub context: Option<serde_json::Value>,
}

impl OperationRequest {
    /// A read request from a named initiator.
    pub fn new(operation: impl Into<String>, initiator: impl Into<String>) -> Self {
        OperationRequest {
            operation: operation.into(),
            initiator: initiator.into(),
            subject: String::new(),
            approval: None,
            context: None,
        }
    }

    /// Say what the operation would act on.
    pub fn about(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Attach a person's approval.
    pub fn approved(mut self, approval: HumanApproval) -> Self {
        self.approval = Some(approval);
        self
    }
}

/// Permission granted by the gate.
///
/// Only the gate can make one: the fields are private, nothing outside this
/// crate constructs it, it cannot be cloned and it cannot be read out of JSON.
/// That is what lets the diagnostic core treat holding one as proof that
/// authorization happened, and ask to see it before a state-changing service
/// is sent.
#[derive(Debug)]
pub struct Authorization {
    capability: String,
    level: PermissionLevel,
    initiator: String,
    confirmed: bool,
    granted_at: Timestamp,
    wire_services: Vec<u8>,
}

impl Authorization {
    /// The capability that was authorized.
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// Its level.
    pub fn level(&self) -> PermissionLevel {
        self.level
    }

    /// Who asked.
    pub fn initiator(&self) -> &str {
        &self.initiator
    }

    /// Whether a person's approval accompanied it.
    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    /// When permission was granted.
    pub fn granted_at(&self) -> Timestamp {
        self.granted_at
    }

    /// Whether this permits sending the state-changing service `service`.
    ///
    /// From the capability's own definition, so an authorization to clear
    /// codes is not one to write a setting.
    pub fn permits_service(&self, service: u8) -> bool {
        self.wire_services.contains(&service)
    }
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
    /// Whether a person's approval, from the desk this gate trusts and for
    /// this operation, came with the request.
    pub confirmed: bool,
    /// What that person was told they were approving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
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
#[derive(Debug)]
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
#[derive(Clone)]
pub struct SafetyGate {
    registry: CapabilityRegistry,
    /// The one desk whose approvals count here.
    trusted_desk: Option<approval::DeskId>,
}

impl std::fmt::Debug for SafetyGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SafetyGate")
            .field("registry", &self.registry)
            .field("trusts_a_desk", &self.trusted_desk.is_some())
            .finish()
    }
}

impl SafetyGate {
    /// Build a gate over an allowlist.
    ///
    /// It trusts no [`ApprovalDesk`], so everything that needs a person's
    /// approval is refused until [`SafetyGate::trusting`] names one.
    pub fn new(registry: CapabilityRegistry) -> Self {
        SafetyGate { registry, trusted_desk: None }
    }

    /// The Phase 1 gate.
    pub fn phase1() -> Self {
        SafetyGate::new(CapabilityRegistry::phase1())
    }

    /// Accept approvals issued by `desk`, and by no other.
    ///
    /// Which desk is trusted is decided once, where the gate is built, by
    /// whatever also decides who is handed the desk. An approval from any
    /// other desk is refused, so making one is not a way round this.
    pub fn trusting(mut self, desk: &ApprovalDesk) -> Self {
        self.trusted_desk = Some(desk.id());
        self
    }

    /// The allowlist behind this gate.
    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// The audit record for a refusal made before this gate was asked.
    ///
    /// The tool interface refuses some calls on its own. Their record is built
    /// here so the level in it comes from the same registry entry a decision
    /// would have used, instead of being written down a second time somewhere
    /// that can disagree.
    pub fn refusal(&self, operation: &str, initiator: &str, reason: &str) -> AuditRecord {
        AuditRecord {
            operation: operation.to_string(),
            level: self.registry.get(operation).map(|c| c.level),
            initiator: initiator.to_string(),
            confirmed: false,
            approval: None,
            allowed: false,
            reason: Some(reason.to_string()),
            decided_at: aim_types::now(),
        }
    }

    /// Decide whether `request` may proceed.
    ///
    /// Checks run in fixed order — existence, level ceiling, initiator,
    /// approval, adapter capability, preconditions — and the first failure
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
            confirmed: false,
            approval: None,
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

        // 2b. Risk ceiling. Independent of the level, and deliberately so: a
        // convenience setting and a brake calibration are both L2 writes.
        // Refusals here are policy and say so, because "this build will not do
        // that" and "that is not written yet" are different sentences and a
        // person deserves to know which one they are being told.
        if cap.risk > MAX_ENABLED_RISK {
            audit.reason = Some(String::from("risk_class_refused"));
            return Decision {
                result: Err(AimError::new(
                    ErrorCode::OperationNotAllowed,
                    format!(
                        "{} is classed {}, and this tool does not perform {} operations. \
                         That is a deliberate limit rather than an unfinished feature: \
                         the ceiling is {}.",
                        cap.id,
                        cap.risk.label(),
                        cap.risk.label(),
                        MAX_ENABLED_RISK.label()
                    ),
                )
                .with_details(serde_json::json!({
                    "operation": cap.id,
                    "risk": cap.risk,
                    "max_enabled_risk": MAX_ENABLED_RISK,
                    "policy": true,
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

        // 3b. A model may not change a vehicle. Ever, by any route.
        //
        // This used to be arranged indirectly, by giving the destructive tools
        // a permission level the build did not execute. That held right up
        // until the build started executing that level, at which point raising
        // an unrelated ceiling would have handed an agent the ability to erase
        // somebody's readiness monitors. A property this important should not
        // be a side effect of an unrelated constant, so it is stated here as
        // itself: the operation is closed to models, the caller is not
        // recognisably a person, refused.
        //
        // Not "the caller is an agent". That was a prefix test on a label the
        // caller writes, and `Agent:planner` walked through it. Anything that
        // is not a person is refused, which a misspelling cannot get round.
        //
        // Refused *after* the initiator check so the audit record names who
        // asked, and before the approval so that a model cannot get further by
        // presenting one.
        if cap.agent_forbidden {
            let kind = InitiatorKind::of(&request.initiator);
            if kind != InitiatorKind::Person {
                let (reason, message) = if kind == InitiatorKind::Agent {
                    (
                        "agent_may_not_mutate",
                        format!(
                            "{} changes the vehicle, and a model is not permitted to do that by \
                             any route. A person can, from the interface, after being told what \
                             it costs.",
                            cap.id
                        ),
                    )
                } else {
                    (
                        "initiator_not_a_person",
                        format!(
                            "{} changes the vehicle and can only be started by a person. {:?} is \
                             not recognised as one.",
                            cap.id, request.initiator
                        ),
                    )
                };
                audit.reason = Some(String::from(reason));
                return Decision {
                    result: Err(AimError::new(ErrorCode::OperationNotAllowed, message)
                        .with_details(serde_json::json!({
                            "operation": cap.id,
                            "initiator": request.initiator,
                        }))),
                    audit,
                };
            }
        }

        // 4. A person's approval, for L1 and above.
        //
        // Not a string. An approval is a value only the trusted desk can make,
        // for this operation on this subject, and it goes stale.
        if cap.level.requires_confirmation() {
            let refusal = match &request.approval {
                None => Some((
                    "confirmation_required",
                    format!("{} requires a person's approval", cap.id),
                )),
                Some(a) if self.trusted_desk != Some(a.desk()) => Some((
                    "approval_not_trusted",
                    format!("{} was approved somewhere this gate does not trust", cap.id),
                )),
                Some(a) if a.operation() != cap.id || a.subject() != request.subject => Some((
                    "approval_for_something_else",
                    format!(
                        "the approval presented is for {} on {:?}, not for {} on {:?}",
                        a.operation(),
                        a.subject(),
                        cap.id,
                        request.subject
                    ),
                )),
                Some(a) if !a.is_fresh() => Some((
                    "approval_expired",
                    format!("the approval for {} was given too long ago; confirm it again", cap.id),
                )),
                Some(a) => {
                    audit.confirmed = true;
                    audit.approval = Some(a.statement().to_string());
                    None
                }
            };
            if let Some((reason, message)) = refusal {
                audit.reason = Some(String::from(reason));
                return Decision {
                    result: Err(AimError::new(ErrorCode::ConfirmationRequired, message)
                        .with_details(serde_json::json!({
                            "operation": cap.id,
                            "level": cap.level.as_str(),
                            "preconditions": cap.preconditions,
                            "reason": reason,
                        }))),
                    audit,
                };
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
                confirmed: audit.confirmed,
                granted_at: now,
                wire_services: cap.wire_services.clone(),
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

    /// A gate and the desk it trusts.
    fn gate_and_desk() -> (SafetyGate, ApprovalDesk) {
        let desk = ApprovalDesk::new();
        (SafetyGate::phase1().trusting(&desk), desk)
    }

    /// `operation` asked for by `initiator`, approved by a person at `desk`.
    fn approved(desk: &ApprovalDesk, operation: &str, initiator: &str) -> OperationRequest {
        let challenge = desk.issue(&[operation], "the vehicle", "YES", "approved in a test");
        let approval = desk.redeem(&challenge.id, "YES", operation, "the vehicle").unwrap();
        OperationRequest::new(operation, initiator).about("the vehicle").approved(approval)
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
        assert_eq!(auth.level(), PermissionLevel::L0);
        assert!(!auth.confirmed());
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
            assert_eq!(d.audit.level, None, "an unknown operation has no level to record");
            assert_eq!(d.into_result().unwrap_err().code, ErrorCode::OperationNotAllowed);
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
    fn clearing_codes_needs_an_approval_and_never_happens_by_accident() {
        let (gate, desk) = gate_and_desk();

        // Unapproved: refused, and the reason names what is missing.
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("confirmation_required"));
        assert!(!d.audit.confirmed);

        // Approved, engine off, stationary, link healthy: allowed.
        let d = gate.authorize(
            &approved(&desk, "obd2.clear_dtcs", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(d.is_allowed(), "{:?}", d.audit.reason);
        assert!(d.audit.confirmed);
        assert_eq!(d.audit.approval.as_deref(), Some("approved in a test"));
    }

    #[test]
    fn clearing_codes_is_refused_with_the_engine_running() {
        // Service 04 on a running engine is refused by many modules anyway, and
        // attempting it mid-drive is how a person ends up with a half-cleared
        // state they cannot explain.
        let (gate, desk) = gate_and_desk();
        let mut conditions = ready();
        conditions.engine_running = true;
        let d = gate.authorize(
            &approved(&desk, "obd2.clear_dtcs", "user"),
            Some(&caps(true)),
            &conditions,
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("precondition_failed"));
    }

    #[test]
    fn programming_stays_refused_by_level() {
        let (gate, desk) = gate_and_desk();
        let d = gate.authorize(
            &approved(&desk, "program.program_module", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::PermissionLevelDisabled);
    }

    /// The risk ceiling is not the level ceiling, and it has to bite on its own.
    ///
    /// Once L2 is enabled, level alone stops being a defence: a brake
    /// calibration and a mirror-fold preference are the same level. This proves
    /// the second ceiling refuses on consequence, and says it is policy.
    #[test]
    fn a_dangerous_change_is_refused_on_risk_even_at_an_enabled_level() {
        let desk = ApprovalDesk::new();
        let mut registry = CapabilityRegistry::phase1();
        registry.register(
            Capability::read_only("config.calibrate_brakes", "Calibrate the ABS module")
                .at_level(PermissionLevel::L2)
                .with_risk(RiskClass::SafetyCritical),
        );
        let gate = SafetyGate::new(registry).trusting(&desk);

        let d = gate.authorize(
            &approved(&desk, "config.calibrate_brakes", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("risk_class_refused"));
        let err = d.into_result().unwrap_err();
        assert_eq!(err.code, ErrorCode::OperationNotAllowed);
        assert!(
            err.message.contains("deliberate limit"),
            "a policy refusal must not read as an unfinished feature: {}",
            err.message
        );
    }

    /// The change this whole seam exists for now actually goes through.
    #[test]
    fn a_convenience_write_is_allowed_once_a_person_approves_it() {
        let (gate, desk) = gate_and_desk();

        let unapproved = gate.authorize(
            &OperationRequest::new("config.write_feature", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert_eq!(
            unapproved.into_result().unwrap_err().code,
            ErrorCode::ConfirmationRequired,
            "a write must never happen without an approval"
        );

        let d = gate.authorize(
            &approved(&desk, "config.write_feature", "user"),
            Some(&caps(true)),
            &ready(),
        );
        let auth = d.into_result().expect("an approved convenience write should be authorised");
        assert!(auth.confirmed());
        assert!(auth.permits_service(WRITE_DATA_BY_IDENTIFIER));
        assert!(
            !auth.permits_service(CLEAR_EMISSIONS_CODES),
            "permission to write a setting is not permission to clear codes"
        );
    }

    #[test]
    fn l1_requires_an_approval() {
        let (gate, desk) = gate_and_desk();
        let d = gate.authorize(
            &OperationRequest::new("obd2.run_self_test", "agent:planner"),
            Some(&caps(true)),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("confirmation_required"));
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::ConfirmationRequired);

        // A model proposing a self test that a person then approves is the
        // product working as intended: this one is not closed to models.
        let d = gate.authorize(
            &approved(&desk, "obd2.run_self_test", "agent:planner"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(d.is_allowed());
        assert!(d.into_result().unwrap().confirmed());
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
        let (gate, desk) = gate_and_desk();
        // Adapter cannot transmit.
        let d = gate.authorize(
            &approved(&desk, "obd2.run_self_test", "user"),
            Some(&caps(false)),
            &ready(),
        );
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::CapabilityMissing);
        // No adapter connected at all: also closed.
        let d = gate.authorize(&approved(&desk, "obd2.run_self_test", "user"), None, &ready());
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::CapabilityMissing);
    }

    #[test]
    fn failed_preconditions_block_execution() {
        let (gate, desk) = gate_and_desk();
        let mut conditions = ready();
        conditions.ignition_on = false;
        let d = gate.authorize(
            &approved(&desk, "obd2.run_self_test", "user"),
            Some(&caps(true)),
            &conditions,
        );
        assert!(!d.is_allowed());
        assert_eq!(d.audit.reason.as_deref(), Some("precondition_failed"));
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::PreconditionFailed);
    }

    #[test]
    fn every_decision_produces_an_audit_record_naming_the_initiator() {
        let gate = SafetyGate::phase1();
        for (op, initiator) in [
            ("obd2.read_dtcs", "user"),
            ("obd2.clear_dtcs", "agent:planner"),
            ("does.not.exist", "agent:planner"),
        ] {
            let d =
                gate.authorize(&OperationRequest::new(op, initiator), Some(&caps(true)), &ready());
            assert_eq!(d.audit.initiator, initiator);
            assert_eq!(d.audit.operation, op);
            assert_eq!(d.audit.allowed, d.is_allowed());
            if !d.is_allowed() {
                assert!(d.audit.reason.is_some(), "{op} must record why");
            }
        }
    }

    /// The level in an audit record is the registry's, whoever writes the
    /// record and whichever way the decision went.
    #[test]
    fn the_audit_level_is_the_capabilitys_own_level() {
        let (gate, desk) = gate_and_desk();
        for cap in CapabilityRegistry::phase1().all() {
            let refused = gate.authorize(
                &OperationRequest::new(&cap.id, "agent:planner"),
                None,
                &VehicleConditions::unknown(),
            );
            assert_eq!(refused.audit.level, Some(cap.level), "{} decided", cap.id);

            let early = gate.refusal(&cap.id, "agent:planner", "permission_level_disabled");
            assert_eq!(early.level, Some(cap.level), "{} refused before the gate", cap.id);
            assert!(!early.allowed);
            assert!(!early.confirmed);
        }
        // The two the audit names: clearing codes is L1, a setting change L2.
        let cleared = gate.authorize(
            &approved(&desk, "obd2.clear_dtcs", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(cleared.audit.allowed);
        assert_eq!(cleared.audit.level, Some(PermissionLevel::L1));
        let written = gate.authorize(
            &approved(&desk, "config.write_feature", "user"),
            Some(&caps(true)),
            &ready(),
        );
        assert!(written.audit.allowed);
        assert_eq!(written.audit.level, Some(PermissionLevel::L2));
        // And nothing is made up for an operation that is not in the registry.
        assert_eq!(gate.refusal("does.not.exist", "user", "x").level, None);
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
    fn the_enabled_set_stops_at_both_ceilings() {
        let r = CapabilityRegistry::phase1();
        assert!(r.all().len() > r.enabled().len());
        for c in r.enabled() {
            assert!(c.level <= MAX_ENABLED_LEVEL, "{} is enabled", c.id);
        }
        // Programming is still off, by level.
        assert!(
            !r.enabled().iter().any(|c| c.id == "program.program_module"),
            "programming must not be enabled"
        );
        // The configuration write this build does support is on.
        assert!(
            r.enabled().iter().any(|c| c.id == "config.write_feature"),
            "config.write_feature should be enabled now that L2 is"
        );
    }

    /// Nothing above the risk ceiling may be reachable, whatever its level.
    ///
    /// Written as a sweep over the registry rather than a list of ids, so that
    /// registering a new safety-critical capability fails this test instead of
    /// quietly becoming reachable.
    #[test]
    fn no_enabled_capability_exceeds_the_risk_ceiling() {
        for c in CapabilityRegistry::phase1().enabled() {
            assert!(
                c.risk <= MAX_ENABLED_RISK,
                "{} is enabled at risk {:?}, above the ceiling {:?}",
                c.id,
                c.risk,
                MAX_ENABLED_RISK
            );
        }
    }
}

/// A person's approval cannot be made out of words.
#[cfg(test)]
mod approval_tests {
    use super::*;

    fn caps() -> aim_types::AdapterCapabilities {
        let mut c = aim_types::AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        c.supports_transmit = true;
        c
    }

    fn ready() -> VehicleConditions {
        VehicleConditions {
            ignition_on: true,
            engine_running: false,
            battery_voltage: Some(13.0),
            connection_stable: true,
            vehicle_speed_kph: Some(0.0),
        }
    }

    const NEEDS_A_PERSON: [&str; 3] =
        ["obd2.clear_dtcs", "config.write_feature", "config.probe_write_gate"];

    /// There is nowhere to put a string. Whatever a caller calls itself, a
    /// request with no approval is refused for lacking one.
    #[test]
    fn calling_yourself_a_person_approves_nothing() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        for initiator in ["user", "user:api", "user:owner", "user:human", "USER", "User:the-owner"]
        {
            for operation in NEEDS_A_PERSON {
                let d = gate.authorize(
                    &OperationRequest::new(operation, initiator).about("owner"),
                    Some(&caps()),
                    &ready(),
                );
                assert!(!d.is_allowed(), "{operation} as {initiator:?}");
                assert_eq!(d.audit.reason.as_deref(), Some("confirmation_required"));
                assert!(!d.audit.confirmed);
            }
        }
    }

    /// An approval from a desk the gate was not built to trust is worth
    /// nothing, so making a desk is not a way to make an approval.
    #[test]
    fn an_approval_from_another_desk_is_refused() {
        let trusted = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&trusted);
        let mine = ApprovalDesk::new();
        for operation in NEEDS_A_PERSON {
            let c = mine.issue(&[operation], "it", "YES", "self-issued");
            let approval = mine.redeem(&c.id, "YES", operation, "it").unwrap();
            let d = gate.authorize(
                &OperationRequest::new(operation, "user").about("it").approved(approval),
                Some(&caps()),
                &ready(),
            );
            assert!(!d.is_allowed(), "{operation}");
            assert_eq!(d.audit.reason.as_deref(), Some("approval_not_trusted"));
            assert!(!d.audit.confirmed);
        }
    }

    /// A gate nobody told which desk to trust approves nothing that needs a
    /// person. Forgetting to wire the desk fails closed.
    #[test]
    fn a_gate_that_trusts_no_desk_refuses_every_approval() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1();
        let c = desk.issue(&["obd2.clear_dtcs"], "it", "YES", "x");
        let approval = desk.redeem(&c.id, "YES", "obd2.clear_dtcs", "it").unwrap();
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user").about("it").approved(approval),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("approval_not_trusted"));
    }

    /// What was approved is what may be done, and nothing next to it.
    #[test]
    fn an_approval_covers_one_operation_on_one_subject() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        let approval_for = |operation: &str, subject: &str| {
            let c = desk.issue(&[operation], subject, "YES", "x");
            desk.redeem(&c.id, "YES", operation, subject).unwrap()
        };

        // Approved turning one setting on; asked to turn another on.
        let d = gate.authorize(
            &OperationRequest::new("config.write_feature", "user")
                .about("feature:brakes=on")
                .approved(approval_for("config.write_feature", "feature:chime=on")),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("approval_for_something_else"));

        // Approved clearing codes; asked to write a setting.
        let d = gate.authorize(
            &OperationRequest::new("config.write_feature", "user")
                .about("it")
                .approved(approval_for("obd2.clear_dtcs", "it")),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("approval_for_something_else"));
    }

    #[test]
    fn a_stale_approval_is_refused() {
        let desk =
            ApprovalDesk::with_lifetimes(approval::CHALLENGE_LIFETIME, std::time::Duration::ZERO);
        let gate = SafetyGate::phase1().trusting(&desk);
        let c = desk.issue(&["obd2.clear_dtcs"], "it", "YES", "x");
        let approval = desk.redeem(&c.id, "YES", "obd2.clear_dtcs", "it").unwrap();
        let d = gate.authorize(
            &OperationRequest::new("obd2.clear_dtcs", "user").about("it").approved(approval),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(d.audit.reason.as_deref(), Some("approval_expired"));
        assert_eq!(d.into_result().unwrap_err().code, ErrorCode::ConfirmationRequired);
    }

    /// Only an operation closed to models may put a state-changing service on
    /// the bus, and reads may put none.
    #[test]
    fn only_operations_closed_to_models_may_send_a_state_changing_service() {
        for cap in CapabilityRegistry::phase1().all() {
            if cap.wire_services.is_empty() {
                continue;
            }
            assert!(cap.agent_forbidden, "{} sends a write and is open to a model", cap.id);
            assert!(cap.level.requires_confirmation(), "{} sends a write unapproved", cap.id);
            assert!(cap.mutating, "{} sends a write and is not declared mutating", cap.id);
        }
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        let read = gate
            .authorize(&OperationRequest::new("obd2.read_catalog_signal", "user"), None, &ready())
            .into_result()
            .unwrap();
        for service in 0..=u8::MAX {
            assert!(!read.permits_service(service), "a read permits service {service:02X}");
        }
    }
}

#[cfg(test)]
mod agent_write_tests {
    use super::*;
    use crate::preconditions::VehicleConditions;

    fn caps() -> aim_types::AdapterCapabilities {
        let mut c = aim_types::AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        c.supports_transmit = true;
        c.supports_long_messages = true;
        c.can_29_bit = true;
        c.multiple_can_buses = true;
        c
    }

    fn ready() -> VehicleConditions {
        VehicleConditions {
            ignition_on: true,
            engine_running: false,
            battery_voltage: Some(13.0),
            connection_stable: true,
            vehicle_speed_kph: Some(0.0),
        }
    }

    /// `operation` with a real approval from the trusted desk attached.
    fn approved(desk: &ApprovalDesk, operation: &str, initiator: &str) -> OperationRequest {
        let c = desk.issue(&[operation], "it", "YES", "the owner approved");
        let approval = desk.redeem(&c.id, "YES", operation, "it").unwrap();
        OperationRequest::new(operation, initiator).about("it").approved(approval)
    }

    /// The property, stated over the whole registry rather than a list of ids.
    ///
    /// A new mutating capability added later is covered by this without anyone
    /// remembering to extend a list, which is the only way a rule like this
    /// survives contact with future edits.
    ///
    /// Every one of these requests carries a genuine approval. The refusal is
    /// for who is asking, and no spelling of "agent" gets round it.
    #[test]
    fn nothing_marked_agent_forbidden_is_reachable_by_an_agent() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        for cap in CapabilityRegistry::phase1().all() {
            if !cap.agent_forbidden {
                continue;
            }
            for initiator in [
                "agent:planner",
                "agent",
                "Agent:planner",
                "AGENT:planner",
                " agent:planner",
                "agent:user",
                "assistant",
                "model:claude",
                "system",
                "system:preconditions",
                "the-owner",
                "human",
            ] {
                let decision =
                    gate.authorize(&approved(&desk, &cap.id, initiator), Some(&caps()), &ready());
                assert!(
                    !decision.is_allowed(),
                    "{} is closed to models and {initiator:?} was allowed to run it",
                    cap.id
                );
            }
        }
    }

    #[test]
    fn an_initiator_is_a_person_only_when_it_says_user() {
        for (label, kind) in [
            ("user", InitiatorKind::Person),
            ("user:api", InitiatorKind::Person),
            (" User:test ", InitiatorKind::Person),
            ("agent", InitiatorKind::Agent),
            ("agent:planner", InitiatorKind::Agent),
            ("Agent:planner", InitiatorKind::Agent),
            (" agent:x", InitiatorKind::Agent),
            ("system", InitiatorKind::System),
            ("system:preconditions", InitiatorKind::System),
            ("", InitiatorKind::Unrecognised),
            ("owner", InitiatorKind::Unrecognised),
            ("human", InitiatorKind::Unrecognised),
            ("username", InitiatorKind::Unrecognised),
            ("agents", InitiatorKind::Unrecognised),
        ] {
            assert_eq!(InitiatorKind::of(label), kind, "{label:?}");
        }
    }

    /// The flag has to actually be set on the operations that matter.
    ///
    /// Without this, the sweep above passes vacuously the moment somebody
    /// forgets to mark a new destructive capability.
    #[test]
    fn the_destructive_operations_are_the_ones_marked() {
        let r = CapabilityRegistry::phase1();
        for id in ["obd2.clear_dtcs", "config.write_feature", "config.probe_write_gate"] {
            assert!(
                r.get(id).expect("registered").agent_forbidden,
                "{id} must be closed to the agent"
            );
        }
    }

    /// And the same operation, asked for by a person, is allowed - otherwise
    /// the test above would pass on a gate that simply refused everything.
    #[test]
    fn a_person_can_still_do_what_the_agent_cannot() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        let decision =
            gate.authorize(&approved(&desk, "obd2.clear_dtcs", "user"), Some(&caps()), &ready());
        assert!(decision.is_allowed(), "a person must still be able to clear codes");
    }

    #[test]
    fn the_refusal_names_who_asked_so_it_is_auditable() {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        let decision = gate.authorize(
            &approved(&desk, "obd2.clear_dtcs", "agent:planner"),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(decision.audit.reason.as_deref(), Some("agent_may_not_mutate"));
        assert_eq!(decision.audit.initiator, "agent:planner");
        assert!(!decision.audit.confirmed, "refused before the approval was looked at");

        let decision = gate.authorize(
            &approved(&desk, "obd2.clear_dtcs", "the-owner"),
            Some(&caps()),
            &ready(),
        );
        assert_eq!(decision.audit.reason.as_deref(), Some("initiator_not_a_person"));
    }
}
