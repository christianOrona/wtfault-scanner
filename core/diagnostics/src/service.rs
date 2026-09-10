//! The diagnostic service: the deterministic core the whole app runs through.
//!
//! Every vehicle operation in the product is a method here, and every one of
//! them follows the same five steps:
//!
//! 1. **Record the intent.** A `tool_invoked` event names the operation, its
//!    arguments and who asked — before anything reaches the vehicle.
//! 2. **Authorize.** The [`SafetyGate`] decides. An unknown operation is
//!    rejected, not attempted. The decision is recorded either way.
//! 3. **Talk to the vehicle** through the adapter, which records every command
//!    and reply into the same event log.
//! 4. **Decode** with the data-driven decoders, so every value carries its raw
//!    bytes, decoder id and verification status.
//! 5. **Persist and return** a [`ToolResult`] — the §7 envelope, identical
//!    whether the caller is the UI, a test, or the future agent runtime.
//!
//! That last point is the seam the handoff asks for. When the agent runtime is
//! built it does not get a new path to the vehicle; it calls these same
//! methods, through the same gate, producing the same recorded evidence.

use crate::recorder::SessionRecorder;
use aim_adapter::{DiagnosticAdapter, EcuMessage, RequestTarget};
use aim_decoders::DecoderSet;
use aim_protocols::{
    decode_dtc_list, decode_monitor_response, decode_supported_pids, strip_dtc_count, ObdRequest,
    ObdResponse, Service,
};
use aim_safety::{OperationRequest, SafetyGate, VehicleConditions};
use aim_session::{measurement_from, SessionStore};
use aim_types::{
    now, AdapterCapabilities, AdapterHealth, AimError, AimResult, ConnectionId, ConnectionState,
    DecodedValue, DtcRecord, DtcStatus, ErrorCode, EventKind, Module, ModuleIdentity, SessionId,
    ToolResult, Value, Vehicle, Warning,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Capability ids, matching [`aim_safety::CapabilityRegistry::phase1`].
pub mod capabilities {
    /// Open and initialize the adapter link.
    pub const CONNECT: &str = "adapter.connect";
    /// Close the adapter link.
    pub const DISCONNECT: &str = "adapter.disconnect";
    /// Read adapter link health.
    pub const HEALTH: &str = "adapter.health";
    /// Read VIN and calibration identifiers.
    pub const IDENTIFY_VEHICLE: &str = "obd2.identify_vehicle";
    /// Discover responding ECUs.
    pub const SCAN_MODULES: &str = "obd2.scan_modules";
    /// Read a module's identity strings.
    pub const MODULE_IDENTITY: &str = "obd2.get_module_identity";
    /// Read stored, pending and permanent DTCs.
    pub const READ_DTCS: &str = "obd2.read_dtcs";
    /// Read freeze frame data.
    pub const READ_FREEZE_FRAME: &str = "obd2.read_freeze_frame";
    /// Read one current-data PID.
    pub const READ_PID: &str = "obd2.read_pid";
    /// Read several PIDs as one sample.
    pub const READ_LIVE_DATA: &str = "obd2.read_live_data";
    /// Enumerate supported PIDs.
    pub const READ_SUPPORTED_PIDS: &str = "obd2.read_supported_pids";
    /// Read service 06 on-board monitor test results.
    pub const READ_MONITOR_TESTS: &str = "obd2.read_monitor_tests";
    /// Discover every module on the bus and read its fault memory.
    pub const SCAN_ALL_MODULES: &str = "obd2.scan_all_modules";
    /// Measure what one module supports, without writing to it.
    pub const PROBE_MODULE_CAPABILITIES: &str = "obd2.probe_module_capabilities";
    /// List community signal definitions that might apply to this vehicle.
    pub const LIST_CATALOG_SIGNALS: &str = "obd2.list_catalog_signals";
    /// Ask the vehicle one community-defined signal and report what it says.
    pub const READ_CATALOG_SIGNAL: &str = "obd2.read_catalog_signal";
    /// List configurable features applicable to this vehicle.
    pub const LIST_FEATURES: &str = "config.list_features";
    /// Read one feature's current setting from the vehicle. L0.
    pub const READ_FEATURE: &str = "config.read_feature";
    /// Evaluate a proposed configuration change without applying it.
    pub const PREVIEW_CHANGE: &str = "config.preview_change";
    /// Change one configurable feature. L2, and never reachable by the agent.
    pub const WRITE_FEATURE: &str = "config.write_feature";
    /// Clear DTCs. L1, requiring an explicit confirmation.
    pub const CLEAR_DTCS: &str = "obd2.clear_dtcs";
}

/// PIDs a freeze frame is worth reading, in the order a technician wants them.
///
/// Only those the module reports as supported are actually requested, so this
/// is a preference list rather than an assumption about any vehicle.
/// The 11-bit diagnostic address range swept by a full-vehicle scan.
///
/// `0x700..=0x7EF` is where ISO 15765-4 puts diagnostic request identifiers.
/// The legislated emissions block is `0x7E0..=0x7E7`; everything below it is
/// where manufacturers put brakes, body, airbag and the rest, and is exactly
/// the region a code reader never looks at.
///
/// Swept exhaustively rather than from a per-manufacturer list, because a list
/// is a guess about a vehicle and a sweep is a measurement of one.
const UDS_SCAN_RANGE: std::ops::RangeInclusive<u16> = 0x700..=0x7EF;

/// Consecutive timeouts before a signal stops being asked for.
///
/// Three, because two can be coincidence on a busy bus and four is another
/// twenty-four seconds of somebody waiting.
const SIGNAL_TIMEOUT_STRIKES: usize = 3;

/// The diagnostic request identifiers a full-vehicle scan sweeps, written in
/// the addressing the negotiated protocol actually uses.
///
/// An 11-bit vehicle is swept over [`UDS_SCAN_RANGE`]. A 29-bit vehicle
/// addresses modules as `18DA<target>F1`, and sweeping 11-bit identifiers there
/// reaches nothing at all — which the scan then reports as a vehicle with no
/// modules, rather than as a scan that asked the wrong question. Measured on a
/// 2023 Odyssey, whose two modules sat at `18DAF110` and `18DAF11E` while the
/// 11-bit sweep found nothing in sixty seconds.
fn scan_addresses(protocol: aim_types::ObdProtocol) -> Vec<String> {
    if protocol.is_29_bit() {
        // 0x33 is the functional target, not a module.
        (0x00..=0xFF_u16)
            .filter(|target| *target != 0x33)
            .map(|target| format!("18DA{target:02X}F1"))
            .collect()
    } else {
        // 0x7DF is the functional broadcast and is not a module: probing it
        // produces a reply from every emissions ECU at once.
        UDS_SCAN_RANGE.filter(|addr| *addr != 0x7DF).map(|addr| format!("{addr:03X}")).collect()
    }
}

/// The second way of asking a module whether it is there.
///
/// `TesterPresent` is the obvious probe and not every module answers it.
/// Measured on a 2019 F-250: the engine controller ignores `0x3E` at `7E0` and
/// answers `0x10 0x01` at the same address, so a sweep that only knows the
/// first concludes the truck has no modules at all — while `read_pid` on that
/// very module returns engine speed.
///
/// Requesting the *default* session is the safe half of a service that can do
/// more: it asks a module to be in the state it is already in. The programming
/// session is never requested anywhere in this build.
fn fallback_probe() -> Vec<u8> {
    aim_protocols::UdsRequest::diagnostic_session_control(0x01).to_bytes()
}

/// Whether a request identifier falls in the legislated emissions block.
///
/// Defined for 11-bit addressing as `0x7E0..=0x7E7`. ISO 15765-4 gives 29-bit
/// addressing no equivalent contiguous block, so rather than invent one the
/// question is left unanswered there.
fn in_legislated_range(request_addr: &str) -> serde_json::Value {
    if request_addr.len() != 3 {
        return serde_json::Value::Null;
    }
    match u16::from_str_radix(request_addr, 16) {
        Ok(addr) => serde_json::json!((0x7E0..=0x7E7).contains(&addr)),
        Err(_) => serde_json::Value::Null,
    }
}

const FREEZE_FRAME_PIDS: [u8; 14] =
    [0x03, 0x04, 0x05, 0x0B, 0x0C, 0x0D, 0x0F, 0x10, 0x11, 0x1F, 0x21, 0x2F, 0x33, 0x42];

/// What one UDS exchange produced, kept as three distinct outcomes.
///
/// "Refused" and "no answer" are different facts and must not collapse into
/// each other: a refusal means the module is there and said no, and carries a
/// reason worth showing.
enum UdsOutcome {
    Positive(Vec<u8>),
    Refused(aim_protocols::NegativeResponseCode),
    NoAnswer(String),
}

impl UdsOutcome {
    fn is_positive(&self) -> bool {
        matches!(self, UdsOutcome::Positive(_))
    }

    /// The stable refusal code, when the module gave a reason.
    fn refusal_code(&self) -> Option<&'static str> {
        match self {
            UdsOutcome::Refused(nrc) => Some(nrc.refusal().code()),
            _ => None,
        }
    }

    /// What happened, in language meant for a person.
    fn detail(&self) -> Option<String> {
        match self {
            UdsOutcome::Positive(_) => None,
            UdsOutcome::Refused(nrc) => Some(nrc.refusal().explain().to_string()),
            UdsOutcome::NoAnswer(why) => Some(why.clone()),
        }
    }
}

/// What reading one module's fault memory produced.
///
/// Carries *why* a module declined rather than only that it did. A module that
/// answers `securityAccessDenied` has said it has fault memory and will not show
/// it; one that answers `serviceNotSupported` has said it does not keep any.
/// Collapsing both to "did not answer" throws away the difference.
#[derive(Debug, Default)]
struct DtcReadOutcome {
    dtcs: Vec<serde_json::Value>,
    /// Human-readable note, when something other than a clean read happened.
    note: Option<String>,
    /// The refusal, classified, when the module gave a reason.
    refusal: Option<aim_protocols::RefusalKind>,
}

impl DtcReadOutcome {
    fn refused(note: String, refusal: Option<aim_protocols::RefusalKind>) -> DtcReadOutcome {
        DtcReadOutcome { dtcs: Vec::new(), note: Some(note), refusal }
    }
}

/// What one operation produced, before it is wrapped in the §7 envelope.
#[derive(Debug, Default)]
struct Payload {
    values: Vec<DecodedValue>,
    data: Option<serde_json::Value>,
    warnings: Vec<Warning>,
    evidence: Option<i64>,
    module: Option<String>,
}

impl Payload {
    fn with_data(data: serde_json::Value) -> Self {
        Payload { data: Some(data), ..Default::default() }
    }
}

/// A decoded DTC as the API and the future agent see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DtcReport {
    /// SAE J2012 code.
    pub code: String,
    /// Which service reported it.
    pub status: DtcStatus,
    /// Module that reported it.
    pub module: String,
    /// Description from the catalog, or `None` when the code is not in it.
    /// The core never invents one.
    pub description: Option<String>,
    /// Structural decoding, always available even for an unlisted code.
    pub structural_summary: String,
    /// Whether the description came from a validated entry.
    pub verification: aim_types::VerificationStatus,
    /// Whether this is a generic SAE code rather than manufacturer-specific.
    pub is_generic: bool,
}

/// The service.
pub struct DiagnosticService {
    adapter: Box<dyn DiagnosticAdapter>,
    store: SessionStore,
    decoders: Arc<DecoderSet>,
    gate: SafetyGate,
    session: aim_types::Session,
    recorder: Arc<SessionRecorder>,
    connection_id: Option<ConnectionId>,
    conditions: VehicleConditions,
    /// Per-module service 01 support, cached after the first enumeration.
    supported_pids: BTreeMap<String, Vec<u8>>,
    /// Per-module service 06 monitor support, cached the same way. Without
    /// this a repeated visit to the Inspect screen re-walks the whole mask
    /// chain and puts dozens of extra requests on the bus.
    supported_mids: BTreeMap<String, Vec<u8>>,
    vehicle: Option<Vehicle>,
    /// Set when any reply this session arrived incomplete. Evidence about the
    /// adapter, consumed by the configuration-change checks.
    saw_truncated_response: bool,
    /// Consecutive timeouts per module and signal, so a parameter that has
    /// repeatedly failed to answer stops being asked.
    ///
    /// Measured from real sessions: three PIDs each hit the six-second request
    /// ceiling across roughly 1,350 requests. A parameter that has timed out
    /// three times running is not about to start answering, and every retry
    /// costs six seconds of somebody standing next to a vehicle.
    signal_timeouts: BTreeMap<String, usize>,
    reference_year: u16,
}

impl DiagnosticService {
    /// Start a session and attach the flight recorder to `adapter`.
    ///
    /// The session exists before the adapter is used, so there is nowhere for
    /// unrecorded traffic to hide.
    pub fn start(
        mut adapter: Box<dyn DiagnosticAdapter>,
        store: SessionStore,
        decoders: Arc<DecoderSet>,
        gate: SafetyGate,
        label: Option<String>,
    ) -> AimResult<DiagnosticService> {
        let session = store.create_session(label)?;
        let recorder = Arc::new(SessionRecorder::new(store.clone(), session.id.clone()));
        adapter.set_observer(recorder.clone());

        Ok(DiagnosticService {
            adapter,
            store,
            decoders,
            gate,
            session,
            recorder,
            connection_id: None,
            conditions: VehicleConditions::unknown(),
            supported_pids: BTreeMap::new(),
            supported_mids: BTreeMap::new(),
            vehicle: None,
            saw_truncated_response: false,
            signal_timeouts: BTreeMap::new(),
            reference_year: 2026,
        })
    }

    /// The session this service records into.
    pub fn session_id(&self) -> &SessionId {
        &self.session.id
    }

    /// The session record.
    pub fn session(&self) -> &aim_types::Session {
        &self.session
    }

    /// The store, for read-only queries by the API.
    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// The safety gate, so the API can publish the capability list.
    pub fn gate(&self) -> &SafetyGate {
        &self.gate
    }

    /// Adapter link state.
    pub fn state(&self) -> ConnectionState {
        self.adapter.state()
    }

    /// Adapter link health.
    pub fn health(&self) -> AdapterHealth {
        self.adapter.health()
    }

    /// Observed adapter capabilities.
    pub fn capabilities(&self) -> AdapterCapabilities {
        self.adapter.capabilities()
    }

    /// Adapter descriptor, e.g. `COM5` or `sim:dpf-regen`.
    pub fn adapter_descriptor(&self) -> String {
        self.adapter.descriptor()
    }

    /// The identified vehicle, when one has been read.
    pub fn vehicle(&self) -> Option<&Vehicle> {
        self.vehicle.as_ref()
    }

    /// Vehicle conditions as currently believed, used by precondition checks.
    pub fn conditions(&self) -> &VehicleConditions {
        &self.conditions
    }

    /// Record an operator note in the flight recorder — handoff §19's
    /// "I just unplugged the sensor" case.
    pub fn note(&self, text: impl Into<String>) -> AimResult<i64> {
        self.store.append_event(&self.session.id, EventKind::UserNote { text: text.into() })
    }

    // ------------------------------------------------------------ plumbing

    fn record_invocation(&self, tool: &str, initiator: &str, arguments: serde_json::Value) {
        let _ = self.store.append_event(
            &self.session.id,
            EventKind::ToolInvoked {
                tool: tool.to_string(),
                arguments,
                initiator: initiator.to_string(),
            },
        );
    }

    /// Record a refusal that happened before the gate was reached.
    ///
    /// The tool interface can refuse a call on its own — a tool disabled for
    /// models, whatever the underlying capability permits for a person. That
    /// refusal must still appear in the flight recorder, or the audit trail
    /// would show a request arriving and nothing happening to it, which is the
    /// one shape a log must never have.
    ///
    /// Written as an invocation followed by a rejected decision, so it reads
    /// identically to a gate refusal to anything consuming the log.
    pub fn record_refusal(
        &self,
        tool: &str,
        capability: &str,
        initiator: &str,
        reason: &str,
        arguments: serde_json::Value,
    ) {
        self.record_invocation(tool, initiator, arguments);
        let _ = self.store.append_event(
            &self.session.id,
            EventKind::SafetyDecision {
                operation: capability.to_string(),
                level: aim_types::PermissionLevel::L2,
                allowed: false,
                initiator: initiator.to_string(),
                confirmed: false,
                reason: Some(reason.to_string()),
            },
        );
    }

    /// Ask the gate. The decision is recorded whichever way it goes, because
    /// handoff §10 requires an audit trail for refusals as much as approvals.
    fn authorize(
        &mut self,
        capability: &str,
        initiator: &str,
        confirmation: Option<&str>,
    ) -> AimResult<()> {
        let mut request = OperationRequest::new(capability, initiator);
        if let Some(user) = confirmation {
            request = request.confirmed_by(user);
        }
        let caps = self.adapter.capabilities();
        let decision = self.gate.authorize(&request, Some(&caps), &self.conditions);

        let _ = self.store.append_event(
            &self.session.id,
            EventKind::SafetyDecision {
                operation: decision.audit.operation.clone(),
                level: decision.audit.level.unwrap_or(aim_types::PermissionLevel::L0),
                allowed: decision.audit.allowed,
                initiator: decision.audit.initiator.clone(),
                confirmed: decision.audit.confirmed,
                reason: decision.audit.reason.clone(),
            },
        );
        decision.into_result().map(|_| ())
    }

    /// Wrap a completed operation in the §7 envelope and record its completion.
    fn finish(
        &self,
        tool: &str,
        capability: &str,
        started: Instant,
        outcome: AimResult<Payload>,
    ) -> ToolResult {
        let elapsed = started.elapsed().as_millis() as u64;
        let result = match outcome {
            Ok(p) => {
                let mut r = ToolResult::success(tool, self.session.id.clone(), capability, elapsed)
                    .with_values(p.values);
                if let Some(d) = p.data {
                    r = r.with_data(d);
                }
                if let Some(e) = p.evidence {
                    r = r.with_evidence(e);
                }
                if let Some(m) = p.module {
                    r = r.with_module(m);
                }
                for w in p.warnings {
                    r = r.warn(w);
                }
                r
            }
            Err(e) => {
                let mut r =
                    ToolResult::failure(tool, self.session.id.clone(), capability, elapsed, e);
                // Even a failure gets its evidence pointer: the adapter
                // exchange that failed is often the whole story.
                if let Some(id) = self.recorder.last_response_event() {
                    r = r.with_evidence(id);
                }
                r
            }
        };

        let _ = self.store.append_event(
            &self.session.id,
            EventKind::ToolCompleted {
                tool: tool.to_string(),
                success: result.success,
                execution_time_ms: elapsed,
                warnings: result.warnings.iter().map(|w| w.code.clone()).collect(),
            },
        );
        result
    }

    /// Issue a request and return the messages, tagging each with the evidence
    /// row for the adapter exchange that produced it.
    fn request(
        &mut self,
        request: &ObdRequest,
        target: &RequestTarget,
    ) -> AimResult<Vec<EcuMessage>> {
        self.adapter.request(request, target)
    }

    /// Request from one specific module.
    ///
    /// Prefers physical addressing. When a module's address has no conventional
    /// request counterpart the request goes out functionally and the answer is
    /// filtered by address — which is honest, if slower, and never guesses an
    /// address that might belong to something else.
    fn request_module(
        &mut self,
        module: &Module,
        request: &ObdRequest,
    ) -> AimResult<(EcuMessage, Option<i64>)> {
        let target = RequestTarget::from_response_address(&module.address)
            .unwrap_or(RequestTarget::Functional);
        let messages = self.request(request, &target)?;
        let evidence = self.recorder.last_response_event();
        messages
            .into_iter()
            .find(|m| m.address == module.address)
            .map(|m| (m, evidence))
            .ok_or_else(|| {
                AimError::new(
                    ErrorCode::NoData,
                    format!(
                        "module {} did not answer service {:02X}",
                        module.module_key,
                        request.service.id()
                    ),
                )
            })
    }

    /// Peel the response header off a message, surfacing negative responses.
    ///
    /// Returns owned bytes: the parsed [`ObdResponse`] owns its payload, so a
    /// borrow would not outlive this function. Payloads are at most a few
    /// hundred bytes and one copy per exchange is not worth an arena.
    fn payload_of(message: &EcuMessage, request: &ObdRequest) -> AimResult<Vec<u8>> {
        let response = ObdResponse::parse(&message.payload, request.pid.is_some())?;
        response.payload_for(request).map(|p| p.to_vec())
    }

    fn module_by_key(&self, key: &str) -> AimResult<Module> {
        self.store.modules(&self.session.id)?.into_iter().find(|m| m.module_key == key).ok_or_else(
            || {
                AimError::not_found(format!(
                    "no module {key:?} in this session; run scan_modules first"
                ))
            },
        )
    }

    fn require_usable(&self) -> AimResult<()> {
        if self.adapter.state().is_usable() {
            Ok(())
        } else {
            Err(AimError::new(
                ErrorCode::NoActiveSession,
                format!("adapter is {}", self.adapter.state().name()),
            )
            .with_capabilities(self.adapter.capabilities()))
        }
    }

    // ----------------------------------------------------------- lifecycle

    /// Connect the adapter, record the connection and its capabilities.
    pub fn connect(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("connect", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::CONNECT, initiator, None)
            .and_then(|_| self.connect_inner());
        self.finish("connect", capabilities::CONNECT, t0, outcome)
    }

    fn connect_inner(&mut self) -> AimResult<Payload> {
        self.adapter.connect()?;
        let caps = self.adapter.capabilities();
        let state = self.adapter.state();

        let connection = aim_types::Connection {
            id: ConnectionId::new(),
            session_id: self.session.id.clone(),
            adapter_id: self.adapter.descriptor(),
            transport: caps.transport,
            connected_at: now(),
            disconnected_at: None,
            firmware: caps.firmware.clone(),
            capabilities: caps.clone(),
        };
        self.store.record_connection(&connection)?;
        self.connection_id = Some(connection.id.clone());

        // What we now believe about the vehicle, from evidence only.
        let health = self.adapter.health();
        self.conditions.connection_stable = matches!(state, ConnectionState::Ready);
        self.conditions.ignition_on = matches!(state, ConnectionState::Ready);
        self.conditions.battery_voltage = health.battery_voltage;

        let mut warnings: Vec<Warning> =
            caps.caveats.iter().map(|c| Warning::caution("adapter_caveat", c.clone())).collect();
        if let ConnectionState::Degraded { reason } = &state {
            warnings.push(Warning::serious("adapter_degraded", reason.clone()));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "connection_id": connection.id,
                "adapter": self.adapter.descriptor(),
                "state": state,
                "protocol": self.adapter.protocol(),
                "protocol_label": self.adapter.protocol().label(),
                "capabilities": caps,
            })),
            warnings,
            ..Default::default()
        })
    }

    /// Disconnect and close the connection record.
    pub fn disconnect(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("disconnect", initiator, serde_json::json!({}));
        let outcome = self.authorize(capabilities::DISCONNECT, initiator, None).and_then(|_| {
            self.adapter.disconnect()?;
            if let Some(id) = &self.connection_id {
                self.store.close_connection(id)?;
            }
            self.conditions = VehicleConditions::unknown();
            Ok(Payload::with_data(serde_json::json!({
                "state": self.adapter.state(),
            })))
        });
        self.finish("disconnect", capabilities::DISCONNECT, t0, outcome)
    }

    /// End the session. Called once, at shutdown.
    pub fn end_session(&mut self) -> AimResult<()> {
        self.store.end_session(&self.session.id)
    }

    /// Adapter health as a tool result, so the UI polls one shape of thing.
    pub fn adapter_health(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        // Recorded like every other tool. This was the one operation that
        // reached the vehicle without appearing in the log as an invocation,
        // which made it invisible to anything reading the flight recorder to
        // see what the agent had done — and the recorder is meant to be the
        // complete account of a session, not most of one.
        self.record_invocation("adapter_health", initiator, serde_json::json!({}));
        let outcome = self.authorize(capabilities::HEALTH, initiator, None).map(|_| {
            let health = self.adapter.health();
            Payload::with_data(serde_json::json!({
                "health": health,
                "capabilities": self.adapter.capabilities(),
                "descriptor": self.adapter.descriptor(),
            }))
        });
        self.finish("adapter_health", capabilities::HEALTH, t0, outcome)
    }

    // -------------------------------------------------------------- tools

    /// Read the VIN and calibration identifiers, and record the vehicle.
    pub fn identify_vehicle(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("identify_vehicle", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::IDENTIFY_VEHICLE, initiator, None)
            .and_then(|_| self.identify_vehicle_inner());
        self.finish("identify_vehicle", capabilities::IDENTIFY_VEHICLE, t0, outcome)
    }

    fn identify_vehicle_inner(&mut self) -> AimResult<Payload> {
        self.require_usable()?;
        let request = ObdRequest::vehicle_info(0x02);
        let messages = self.request(&request, &RequestTarget::Functional)?;
        let evidence = self.recorder.last_response_event();

        let message = messages
            .first()
            .ok_or_else(|| AimError::no_data("no module answered the VIN request"))?
            .clone();
        let payload = Self::payload_of(&message, &request)?;
        let observed_at = now();
        let values = self
            .decoders
            .pids
            .decode(0x09, 0x02, &payload, observed_at)?
            .into_iter()
            .map(|mut v| {
                if let Some(e) = evidence {
                    v.provenance = v.provenance.clone().with_evidence_ref(e);
                }
                v
            })
            .collect::<Vec<_>>();

        let vin = values
            .iter()
            .find(|v| v.signal_id == "vin")
            .and_then(|v| v.value.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    "the VIN response did not decode to a VIN",
                )
            })?;

        let mut warnings = Vec::new();
        // The VIN's own check digit is a self-check the standard provides. A
        // failure does not stop identification, but it is never hidden.
        let info = match aim_decoders::decode_vin_info(&vin, self.reference_year) {
            Ok(info) => {
                if !info.check_digit_valid {
                    warnings.push(Warning::serious(
                        "vin_check_digit_invalid",
                        format!("VIN {vin} failed its SAE check-digit test"),
                    ));
                }
                Some(info)
            }
            Err(e) => {
                warnings.push(Warning::serious(
                    "vin_structurally_invalid",
                    format!("VIN {vin:?} is not structurally valid: {}", e.message),
                ));
                None
            }
        };

        // Only fields the VIN standard actually encodes are filled in. Model,
        // trim, engine and transmission stay unknown: this project does not
        // have a licensed VIN-decoding database and will not guess.
        let mut vehicle = Vehicle::from_vin(Some(vin.clone()));
        if let Some(i) = &info {
            vehicle.make = i.manufacturer.clone();
            vehicle.year = i.model_year;
        }
        let vehicle = self.store.upsert_vehicle(&vehicle)?;
        self.store.attach_vehicle(&self.session.id, &vehicle.id)?;
        self.session.vehicle_id = Some(vehicle.id.clone());
        self.vehicle = Some(vehicle.clone());
        let _ = self.store.append_event(
            &self.session.id,
            EventKind::VehicleIdentified {
                vin: Some(vin.clone()),
                vehicle_id: vehicle.id.to_string(),
            },
        );

        // Calibration identifiers come from the same module that gave the VIN.
        let module_address = message.address.clone();
        let mut calibration_ids = Vec::new();
        let mut cvns = Vec::new();
        for (info_type, sink) in [(0x04u8, &mut calibration_ids), (0x06u8, &mut cvns)] {
            let req = ObdRequest::vehicle_info(info_type);
            match self.request(&req, &RequestTarget::Functional) {
                Ok(msgs) => match msgs.iter().find(|m| m.address == module_address) {
                    Some(m) => match Self::payload_of(m, &req)
                        .and_then(|p| self.decoders.pids.decode(0x09, info_type, &p, now()))
                    {
                        Ok(vals) => {
                            for v in vals {
                                if let Value::Text(s) = &v.value {
                                    sink.push(s.trim().to_string());
                                } else if let Value::Raw(s) = &v.value {
                                    sink.push(s.clone());
                                }
                            }
                        }
                        Err(e) => warnings.push(Warning::info(
                            "calibration_decode_failed",
                            format!("service 09 info type {info_type:02X}: {}", e.message),
                        )),
                    },
                    None => warnings.push(Warning::info(
                        "calibration_not_reported",
                        format!("module {module_address} did not answer info type {info_type:02X}"),
                    )),
                },
                Err(e) => {
                    // An incomplete multi-frame reply is the signature of an
                    // adapter mishandling flow control on a long response. It is
                    // harmless here — the CVN is a tamper checksum, not a
                    // diagnostic input — but it is evidence about the adapter,
                    // and a configuration write is exactly the operation that
                    // must not be attempted on a link that drops frames.
                    if e.message.contains("incomplete multi-frame")
                        || e.code == ErrorCode::ProtocolMalformedResponse
                    {
                        self.saw_truncated_response = true;
                    }
                    warnings.push(Warning::info(
                        "calibration_unavailable",
                        format!("service 09 info type {info_type:02X}: {}", e.message),
                    ))
                }
            }
        }

        Ok(Payload {
            values,
            data: Some(serde_json::json!({
                "vin": vin,
                "vehicle_id": vehicle.id,
                "vin_decoded": info,
                "reported_by": module_address,
                "calibration_ids": calibration_ids,
                "calibration_verification_numbers": cvns,
            })),
            warnings,
            evidence,
            module: Some(module_address),
        })
    }

    /// Discover which modules answer, and name them from what they report.
    pub fn scan_modules(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("scan_modules", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::SCAN_MODULES, initiator, None)
            .and_then(|_| self.scan_modules_inner());
        self.finish("scan_modules", capabilities::SCAN_MODULES, t0, outcome)
    }

    fn scan_modules_inner(&mut self) -> AimResult<Payload> {
        self.require_usable()?;
        // Service 01 PID 00 is the one request every OBD-II module must answer,
        // which makes it the discovery probe.
        let request = ObdRequest::current_data(0x00);
        let messages = self.request(&request, &RequestTarget::Functional)?;
        let evidence = self.recorder.last_response_event();
        let protocol = self.adapter.protocol();

        let mut discovered = Vec::new();
        for message in &messages {
            let key = format!("ECU_{}", message.address);
            let module = Module {
                id: aim_types::ModuleId::new(),
                session_id: self.session.id.clone(),
                module_key: key.clone(),
                // Named by address until the module tells us otherwise. Which
                // module sits at which OBD address is vehicle-specific, and
                // guessing would be an invention.
                name: format!("OBD module at {}", message.address),
                address: message.address.clone(),
                protocol,
                identity: ModuleIdentity::default(),
                software_version: None,
                discovered_at: now(),
            };
            let stored = self.store.upsert_module(&module)?;
            let _ = self.store.append_event(
                &self.session.id,
                EventKind::ModuleDiscovered {
                    module_key: key.clone(),
                    address: message.address.clone(),
                },
            );
            discovered.push(stored);
        }

        if discovered.is_empty() {
            return Err(AimError::no_data("no module answered the discovery request"));
        }

        // Ask each module for its own name. A module that answers gets named
        // by evidence; one that does not keeps its address-based label.
        let mut named = Vec::new();
        for module in discovered {
            let mut module = module;
            if let Ok(name) = self.read_ecu_name(&module) {
                module.name = name;
                module = self.store.upsert_module(&module)?;
            }
            named.push(module);
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "modules": named,
                "protocol": protocol,
                "protocol_label": protocol.label(),
            })),
            evidence,
            ..Default::default()
        })
    }

    fn read_ecu_name(&mut self, module: &Module) -> AimResult<String> {
        let request = ObdRequest::vehicle_info(0x0A);
        let (message, _) = self.request_module(module, &request)?;
        let payload = Self::payload_of(&message, &request)?;
        let values = self.decoders.pids.decode(0x09, 0x0A, &payload, now())?;
        values
            .into_iter()
            .find_map(|v| v.value.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AimError::no_data("module reported an empty ECU name"))
    }

    /// Read a module's identity: ECU name, calibration ids, CVNs.
    pub fn get_module_identity(&mut self, module_key: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "get_module_identity",
            initiator,
            serde_json::json!({ "module": module_key }),
        );
        let outcome = self
            .authorize(capabilities::MODULE_IDENTITY, initiator, None)
            .and_then(|_| self.module_identity_inner(module_key));
        self.finish("get_module_identity", capabilities::MODULE_IDENTITY, t0, outcome)
    }

    fn module_identity_inner(&mut self, module_key: &str) -> AimResult<Payload> {
        self.require_usable()?;
        let mut module = self.module_by_key(module_key)?;
        let mut warnings = Vec::new();
        let mut identity = ModuleIdentity::default();

        for (info_type, label) in [(0x0Au8, "ecu_name"), (0x04, "calibration_id"), (0x06, "cvn")] {
            let request = ObdRequest::vehicle_info(info_type);
            let decoded = self
                .request_module(&module, &request)
                .and_then(|(m, _)| Self::payload_of(&m, &request))
                .and_then(|p| self.decoders.pids.decode(0x09, info_type, &p, now()));
            match decoded {
                Ok(values) => {
                    for v in values {
                        match (&v.value, info_type) {
                            (Value::Text(s), 0x0A) => identity.ecu_name = Some(s.trim().into()),
                            (Value::Text(s), 0x04) => {
                                identity.calibration_ids.push(s.trim().into())
                            }
                            (Value::Raw(s), 0x06) | (Value::Text(s), 0x06) => {
                                identity.calibration_verification_numbers.push(s.clone())
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => warnings.push(Warning::info(
                    "identity_field_unavailable",
                    format!("{label}: {}", e.message),
                )),
            }
        }

        if let Some(name) = &identity.ecu_name {
            if !name.is_empty() {
                module.name = name.clone();
            }
        }
        module.identity = identity.clone();
        let module = self.store.upsert_module(&module)?;

        Ok(Payload {
            data: Some(serde_json::json!({ "module": module, "identity": identity })),
            warnings,
            evidence: self.recorder.last_response_event(),
            module: Some(module_key.to_string()),
            ..Default::default()
        })
    }

    /// Enumerate the service 01 PIDs a module supports.
    pub fn read_supported_pids(&mut self, module_key: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_supported_pids",
            initiator,
            serde_json::json!({ "module": module_key }),
        );
        let outcome = self
            .authorize(capabilities::READ_SUPPORTED_PIDS, initiator, None)
            .and_then(|_| self.supported_pids_inner(module_key));
        self.finish("read_supported_pids", capabilities::READ_SUPPORTED_PIDS, t0, outcome)
    }

    fn supported_pids_inner(&mut self, module_key: &str) -> AimResult<Payload> {
        self.require_usable()?;
        let module = self.module_by_key(module_key)?;
        let pids = self.enumerate_supported(&module)?;

        // Report which of the supported PIDs this build can actually decode.
        // A PID the vehicle offers and the decoders do not know is a gap worth
        // showing rather than hiding.
        let described: Vec<serde_json::Value> = pids
            .iter()
            .map(|pid| match self.decoders.pids.get(0x01, *pid) {
                Some(c) => serde_json::json!({
                    "pid": pid,
                    "hex": format!("{pid:02X}"),
                    "signal_id": c.def.signal_id,
                    "name": c.def.name,
                    "unit": c.def.unit,
                    // A dashboard needs to know a gauge from a bitmask. Without
                    // this the UI drew "PIDs supported: 01,04,05,0B,0C,0D,0F,10"
                    // in 22px monospace and it overflowed its card.
                    "kind": c.def.kind,
                    "is_measurement": matches!(
                        c.def.kind,
                        aim_decoders::PidKind::Numeric | aim_decoders::PidKind::Enum
                    ),
                    "verification": c.def.verification,
                    "decoder_available": true,
                }),
                None => serde_json::json!({
                    "pid": pid,
                    "hex": format!("{pid:02X}"),
                    "decoder_available": false,
                }),
            })
            .collect();
        let undecodable = described.iter().filter(|d| d["decoder_available"] == false).count();

        let mut warnings = Vec::new();
        if undecodable > 0 {
            warnings.push(Warning::info(
                "pids_without_decoders",
                format!("{undecodable} supported PIDs have no decoder definition in this build"),
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "module": module_key,
                "count": pids.len(),
                "pids": described,
            })),
            warnings,
            evidence: self.recorder.last_response_event(),
            module: Some(module_key.to_string()),
            ..Default::default()
        })
    }

    /// Read service 06 on-board monitor test results.
    ///
    /// This is the reading a code reader cannot give you. A DTC says a monitor
    /// has already failed; service 06 gives the monitor's *measured value next
    /// to the limit it is judged by*, so a catalyst at 0.58 against a 0.60 limit
    /// shows up as passing-and-nearly-gone months before it sets P0420.
    ///
    /// Every result carries `passed` and `margin`, which are exact regardless of
    /// scaling: the value and both limits arrive in the same unit whatever that
    /// unit is. The scaled numbers are supporting evidence and marked as
    /// unverified, because this build's UAS table has not been checked against a
    /// real vehicle.
    pub fn read_monitor_tests(&mut self, module_key: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_monitor_tests",
            initiator,
            serde_json::json!({ "module": module_key }),
        );
        let outcome = self
            .authorize(capabilities::READ_MONITOR_TESTS, initiator, None)
            .and_then(|_| self.monitor_tests_inner(module_key));
        self.finish("read_monitor_tests", capabilities::READ_MONITOR_TESTS, t0, outcome)
    }

    fn monitor_tests_inner(&mut self, module_key: &str) -> AimResult<Payload> {
        self.require_usable()?;
        let module = self.module_by_key(module_key)?;

        let mids = self.enumerate_supported_mids(&module)?;
        let mut warnings = Vec::new();
        if mids.is_empty() {
            // Plenty of vehicles answer service 06 with nothing useful, and a
            // few do not implement it at all. That is a fact about the vehicle,
            // not a failure of the scan.
            warnings.push(Warning::info(
                "no_monitor_tests",
                "this module reports no service 06 monitors; many vehicles \
                 built before roughly 2005 do not implement it",
            ));
            return Ok(Payload {
                data: Some(serde_json::json!({
                    "module": module_key,
                    "supported": false,
                    "monitors": [],
                })),
                warnings,
                evidence: self.recorder.last_response_event(),
                module: Some(module_key.to_string()),
                ..Default::default()
            });
        }

        let mut readings = Vec::new();
        let mut unanswered = Vec::new();
        let mut truncated = 0usize;
        let mut bus_errors = 0usize;
        let mut abandoned: Option<u8> = None;

        for mid in &mids {
            // Stop after repeated bus errors rather than grinding through the
            // rest of the list.
            //
            // Measured on a real truck: a `CAN ERROR` reply leaves the link
            // unhappy for a moment, and continuing to hammer it made the *next*
            // unrelated reads fail too — so a monitor sweep could knock out the
            // live data the user was actually looking at. Backing off keeps the
            // damage to the sweep that caused it.
            if bus_errors >= 2 {
                abandoned = Some(*mid);
                break;
            }
            let request = ObdRequest::monitor_results(*mid);
            let message = match self.request_module(&module, &request) {
                Ok((m, _)) => m,
                // A MID listed in the mask that then refuses to answer is worth
                // naming rather than dropping: it is usually a monitor that has
                // not run yet this drive cycle.
                Err(e) => {
                    if matches!(e.code, ErrorCode::AdapterError | ErrorCode::VehicleNotResponding) {
                        bus_errors += 1;
                    }
                    unanswered.push(format!("{mid:02X}"));
                    continue;
                }
            };
            let payload = match Self::payload_of(&message, &request) {
                Ok(p) => p,
                Err(_) => {
                    unanswered.push(format!("{mid:02X}"));
                    continue;
                }
            };
            let (tests, leftover) = decode_monitor_response(*mid, &payload);
            truncated += leftover;
            readings.extend(self.decoders.monitors.interpret_all(&tests));
        }

        if !unanswered.is_empty() {
            warnings.push(Warning::info(
                "monitors_did_not_answer",
                format!(
                    "{} of {} supported monitors returned no result (usually a \
                     monitor that has not run yet this drive cycle): {}",
                    unanswered.len(),
                    mids.len(),
                    unanswered.join(", ")
                ),
            ));
        }
        if truncated > 0 {
            warnings.push(Warning::caution(
                "monitor_reply_truncated",
                format!("{truncated} trailing bytes did not form a whole test record"),
            ));
        }
        if let Some(stopped_at) = abandoned {
            warnings.push(Warning::caution(
                "monitor_sweep_abandoned",
                format!(
                    "the bus reported errors, so the sweep stopped at monitor {stopped_at:02X} \
                     rather than continuing. Results above are complete; the rest were not asked \
                     for. Re-check with the engine running and the adapter seated."
                ),
            ));
        }
        if readings.iter().any(|r| r.unknown_scaling) {
            warnings.push(Warning::info(
                "monitor_scaling_unknown",
                "some results use a scaling this build does not have; their \
                 pass or fail verdict is still exact, but the numbers are shown \
                 as raw counts",
            ));
        }

        let failing = readings.iter().filter(|r| !r.passed).count();
        // Under a tenth of the limit band left is the interesting case: still
        // passing, so no code, but not for much longer.
        const MARGINAL: f64 = 0.10;
        let marginal =
            readings.iter().filter(|r| r.passed && r.margin.is_some_and(|m| m < MARGINAL)).count();

        Ok(Payload {
            data: Some(serde_json::json!({
                "module": module_key,
                "supported": true,
                "monitors": readings,
                "failing": failing,
                "marginal": marginal,
                "marginal_threshold": MARGINAL,
                "scaling_verification": "unverified",
            })),
            warnings,
            evidence: self.recorder.last_response_event(),
            module: Some(module_key.to_string()),
            ..Default::default()
        })
    }

    /// Ask which monitor ids a module supports, walking the mask chain.
    ///
    /// Service 06 mirrors service 01 here: MID 0x00 returns a four-byte mask
    /// covering 0x01–0x20, and bit 0x20 means "ask again at 0x20". Only the
    /// masks the module actually acknowledges are followed.
    fn enumerate_supported_mids(&mut self, module: &Module) -> AimResult<Vec<u8>> {
        // Cached for the session, exactly as the service 01 mask chain is.
        //
        // Measured on a real truck: without this, one visit to the Inspect
        // screen re-walked the whole chain every time the card mounted, and a
        // session that opened the screen four times put ninety service 06
        // requests on the bus. Which monitors a module has does not change
        // while the key is on.
        if let Some(cached) = self.supported_mids.get(&module.module_key) {
            return Ok(cached.clone());
        }
        let mut found: BTreeSet<u8> = BTreeSet::new();
        let mut base: u8 = 0x00;
        loop {
            let request = ObdRequest::monitor_results(base);
            let message = match self.request_module(module, &request) {
                Ok((m, _)) => m,
                // Unlike service 01, a module refusing MID 0x00 is not an
                // error: service 06 is optional in practice and the caller is
                // told it is unsupported.
                Err(_) => break,
            };
            let payload = match Self::payload_of(&message, &request) {
                Ok(p) => p,
                Err(_) => break,
            };
            let mids = match decode_supported_pids(base, &payload) {
                Ok(m) => m,
                Err(_) => break,
            };
            found.extend(mids.iter().copied());

            let next = base.wrapping_add(0x20);
            if next > base && mids.contains(&next) {
                base = next;
            } else {
                break;
            }
        }
        // 0x00 and the continuation markers are questions, not monitors.
        let list: Vec<u8> = found.into_iter().filter(|m| *m != 0x00 && m % 0x20 != 0).collect();
        self.supported_mids.insert(module.module_key.clone(), list.clone());
        Ok(list)
    }

    /// List the configurable features that could apply to this vehicle.
    ///
    /// "Could apply" is doing real work here. A VIN prefix and a model-year
    /// range narrow the list; they do not establish that a particular truck was
    /// built with the hardware. Only reading the vehicle's own configuration
    /// does that, and this build cannot, so every entry says how far it can go.
    pub fn list_features(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("list_features", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::LIST_FEATURES, initiator, None)
            .map(|_| self.features_inner());
        self.finish("list_features", capabilities::LIST_FEATURES, t0, outcome)
    }

    fn features_inner(&mut self) -> Payload {
        let (make, year, vin) = match &self.vehicle {
            Some(v) => (v.make.clone(), v.year, v.vin.clone()),
            None => (None, None, None),
        };
        let features: Vec<serde_json::Value> = self
            .decoders
            .features
            .for_vehicle(make.as_deref(), year, vin.as_deref())
            .into_iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id,
                    "name": f.name,
                    "easy": f.easy,
                    "technical": f.technical,
                    "risk": f.risk,
                    "risk_label": f.risk.label(),
                    "modules": f.modules,
                    "requires": f.requires,
                    "support": f.support(),
                    "verification": f.verification,
                    "source": f.source,
                    "notes": f.notes,
                    "writable_in_principle": f.writable_in_principle(),
                })
            })
            .collect();

        let mut warnings = Vec::new();
        if vin.is_none() {
            warnings.push(Warning::info(
                "vehicle_not_identified",
                "No VIN has been read this session, so this list is not narrowed \
                 to your vehicle. Identify the vehicle for a shorter, more \
                 relevant list.",
            ));
        }
        Payload {
            data: Some(serde_json::json!({
                "features": features,
                "catalog_size": self.decoders.features.len(),
                "narrowed_to_vehicle": vin.is_some(),
            })),
            warnings,
            ..Default::default()
        }
    }

    /// Evaluate a proposed configuration change without performing it.
    ///
    /// Read-only by construction: it inspects catalogue data and what has
    /// already been observed about this session, and sends nothing. The result
    /// is the full list of checks so a person can see every reason a change
    /// would or would not go ahead, rather than a bare "unavailable".
    pub fn preview_configuration_change(
        &mut self,
        feature_id: &str,
        desired: crate::config::DesiredValue,
        initiator: &str,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "preview_configuration_change",
            initiator,
            serde_json::json!({ "feature_id": feature_id, "desired": desired }),
        );
        let outcome = self
            .authorize(capabilities::PREVIEW_CHANGE, initiator, None)
            .and_then(|_| self.preview_inner(feature_id, desired));
        self.finish("preview_configuration_change", capabilities::PREVIEW_CHANGE, t0, outcome)
    }

    fn preview_inner(
        &mut self,
        feature_id: &str,
        desired: crate::config::DesiredValue,
    ) -> AimResult<Payload> {
        let modules: Vec<String> =
            self.store.modules(&self.session.id)?.into_iter().map(|m| m.module_key).collect();
        let caps = self.adapter.capabilities();
        let request = crate::config::ChangeRequest { feature_id: feature_id.to_string(), desired };
        let ctx = crate::config::ChangeContext {
            vehicle: self.vehicle.as_ref(),
            adapter: Some(&caps),
            modules_present: &modules,
            saw_truncated_response: self.saw_truncated_response,
            battery_voltage: self.conditions.battery_voltage,
            max_level: aim_safety::MAX_ENABLED_LEVEL,
        };
        let plan =
            crate::config::plan_change(&request, self.decoders.features.get(feature_id), &ctx);

        let mut warnings = Vec::new();
        if !plan.can_apply {
            let permanent = plan.checks.iter().any(|c| !c.passed && c.blocking_by_design);
            warnings.push(if permanent {
                Warning::info(
                    "change_refused_by_design",
                    "This change will not be made by this build. The preview \
                     shows every check so the reason is visible.",
                )
            } else {
                Warning::info(
                    "change_not_possible_yet",
                    "This change cannot be made in the current situation. Each \
                     failed check says what would have to be true.",
                )
            });
        }

        Ok(Payload {
            data: Some(serde_json::to_value(&plan).unwrap_or(serde_json::Value::Null)),
            warnings,
            ..Default::default()
        })
    }

    /// Read one feature's current setting from the vehicle.
    ///
    /// This is the operation everything else about configuration stands on. A
    /// change needs a before-state to show and an after-state to verify
    /// against, and a mapping nobody has measured yet cannot be investigated
    /// without being able to read the record it supposedly lives in.
    ///
    /// It is L0 and read-only, and the assistant may use it.
    ///
    /// A feature with no executable mapping is **not an error**. It answers
    /// with what is known, what is not, and what would establish the rest —
    /// because "this app does not support that" and "nobody has measured where
    /// that bit lives on your vehicle" are different sentences, and only one of
    /// them is a dead end.
    pub fn read_feature(&mut self, feature_id: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_feature",
            initiator,
            serde_json::json!({ "feature_id": feature_id }),
        );
        let outcome = self
            .authorize(capabilities::READ_FEATURE, initiator, None)
            .and_then(|_| self.read_feature_inner(feature_id));
        self.finish("read_feature", capabilities::READ_FEATURE, t0, outcome)
    }

    fn read_feature_inner(&mut self, feature_id: &str) -> AimResult<Payload> {
        let Some(feature) = self.decoders.features.get(feature_id) else {
            return Err(AimError::new(
                ErrorCode::NotFound,
                format!(
                    "no feature {feature_id} in the catalogue. A feature has to be described in \
                     a data file before it can be read."
                ),
            ));
        };
        let name = feature.name.clone();
        let modules = feature.modules.clone();
        let source = feature.source.clone();
        let verification = feature.verification;
        let write_evidence = feature.write_verification.clone();
        let writable = feature.support() == aim_decoders::FeatureSupport::Writable;
        let target = feature.mapping.as_ref().and_then(|m| m.as_data_identifier());

        // No executable mapping. Report it as an open question with the steps
        // that would close it, rather than as a refusal.
        let Some(target) = target else {
            return Ok(Payload {
                data: Some(serde_json::json!({
                    "feature_id": feature_id,
                    "name": name,
                    "known": true,
                    "readable": false,
                    "state": serde_json::Value::Null,
                    "owning_modules": modules,
                    "what_is_known": "This vehicle family is described as having this feature.",
                    "what_is_missing":
                        "Which data identifier and which bits hold the setting on this vehicle. \
                         Nobody has measured it, and this app will not guess at one.",
                    "how_to_establish": [
                        "Read and save the owning module's configuration.",
                        "Change the setting once with a tool already known to do it correctly.",
                        "Read the configuration again.",
                        "The bits that moved are the mapping. Record where it came from.",
                    ],
                })),
                warnings: vec![Warning::info(
                    "mapping_not_measured",
                    "The setting cannot be read yet because nobody has recorded where it lives \
                     on this vehicle. That is a gap rather than a refusal, and a profile file \
                     closes it without a new version of the app.",
                )],
                ..Default::default()
            });
        };

        let addr = RequestTarget::Physical(target.module.clone());
        let request = aim_protocols::UdsRequest::read_data_by_identifier(target.did).to_bytes();
        let record = self.read_did(&request, &addr, Duration::from_millis(2000), target.did)?;
        let state = target.current(&record);

        let mut warnings = Vec::new();
        if state.is_none() {
            warnings.push(Warning::caution(
                "value_matches_neither_state",
                "The module answered, but the bits this mapping describes hold a value that is \
                 neither the documented on nor the documented off. The mapping may be wrong for \
                 this vehicle; nothing was changed.",
            ));
        }
        if verification != aim_types::VerificationStatus::Verified {
            warnings.push(Warning::caution(
                "mapping_unverified",
                "This mapping has not been verified against a known-good tool on this model. It \
                 can be read and shown; it cannot be written.",
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "feature_id": feature_id,
                "name": name,
                "known": true,
                "readable": true,
                "state": state,
                "module": target.module,
                "did": format!("{:04X}", target.did),
                "record": aim_types::hex(&record),
                "owning_modules": modules,
                "mapping_source": source,
                "verification": verification,
                "write_evidence": write_evidence,
                "writable": writable,
            })),
            warnings,
            ..Default::default()
        })
    }

    /// Read a set of configuration records off one module.
    ///
    /// The first half of the procedure that turns an unmapped feature into a
    /// mapped one: capture, change the setting with a tool that already knows
    /// how, capture again, and compare.
    ///
    /// The identifiers are supplied rather than discovered. Sweeping all 65536
    /// of them is tens of minutes of bus traffic for a result that is mostly
    /// negative responses, and there is no standard that says where a given
    /// manufacturer keeps configuration - so the caller says which to read, and
    /// a module that does not have one answers so without drama.
    ///
    /// Read-only. L0, and the assistant may use it.
    pub fn capture_configuration(
        &mut self,
        module: &str,
        dids: &[u16],
        label: Option<String>,
        initiator: &str,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "capture_configuration",
            initiator,
            serde_json::json!({ "module": module, "identifiers": dids.len() }),
        );
        let outcome = self
            .authorize(capabilities::READ_FEATURE, initiator, None)
            .and_then(|_| self.capture_inner(module, dids, label));
        self.finish("capture_configuration", capabilities::READ_FEATURE, t0, outcome)
    }

    fn capture_inner(
        &mut self,
        module: &str,
        dids: &[u16],
        label: Option<String>,
    ) -> AimResult<Payload> {
        // A cap, because this is a loop over caller-supplied input that puts
        // traffic on a vehicle bus.
        const MAX_IDENTIFIERS: usize = 64;
        if dids.is_empty() {
            return Err(AimError::new(
                ErrorCode::BadRequest,
                "capturing configuration needs at least one data identifier to read",
            ));
        }
        if dids.len() > MAX_IDENTIFIERS {
            return Err(AimError::new(
                ErrorCode::BadRequest,
                format!("at most {MAX_IDENTIFIERS} identifiers can be read in one capture"),
            ));
        }

        let addr = RequestTarget::Physical(module.to_string());
        let request_budget = Duration::from_millis(2000);
        let mut records = std::collections::BTreeMap::new();
        let mut missing = Vec::new();

        for &did in dids {
            let request = aim_protocols::UdsRequest::read_data_by_identifier(did).to_bytes();
            match self.read_did(&request, &addr, request_budget, did) {
                Ok(record) => {
                    records.insert(did, record);
                }
                // A module that does not hold this identifier is a fact about
                // the module, not a failure of the capture.
                Err(_) => missing.push(format!("{did:04X}")),
            }
        }

        if records.is_empty() {
            return Err(AimError::new(
                ErrorCode::NoData,
                format!(
                    "the module at {module} returned none of the {} identifiers asked for. \
                     Either it holds no configuration, or it keeps it somewhere else.",
                    dids.len()
                ),
            ));
        }

        let capture = crate::capture::ConfigCapture {
            module: module.to_string(),
            records: records.clone(),
            taken_at: aim_types::now().0.to_string(),
            label,
        };

        let mut warnings = Vec::new();
        if !missing.is_empty() {
            warnings.push(Warning::info(
                "identifiers_not_held",
                format!(
                    "{} of the identifiers asked for are not held by this module: {}",
                    missing.len(),
                    missing.join(", ")
                ),
            ));
        }

        // Stored, so that this can be the "before" of a comparison made next
        // week. A capture that lived only in an HTTP response could only ever
        // be compared with another one taken in the same sitting, which is not
        // how somebody changes a setting on their own vehicle.
        let stored_id = match serde_json::to_string(&capture) {
            Ok(json) => self
                .store
                .record_capture(
                    &self.session.id,
                    self.session.vehicle_id.as_ref().map(|v| v.as_str()),
                    &format!("ECU_{module}"),
                    capture.label.as_deref(),
                    &capture.taken_at,
                    &json,
                )
                .ok(),
            Err(_) => None,
        };
        if stored_id.is_none() {
            warnings.push(Warning::caution(
                "capture_not_stored",
                "This capture could not be saved, so it will not be available to compare \
                 against later. It is still returned in full here — keep it if you need it.",
            ));
        }
        if self.session.vehicle_id.is_none() {
            warnings.push(Warning::info(
                "capture_not_linked_to_a_vehicle",
                "No vehicle has been identified in this session, so this capture is saved \
                 without one and will not be found by a later search for this vehicle's \
                 captures. Running identify_vehicle first avoids that.",
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "capture_id": stored_id,
                "capture": capture,
                "records_read": records.len(),
                "hex": records
                    .iter()
                    .map(|(d, r)| (format!("{d:04X}"), aim_types::hex(r)))
                    .collect::<std::collections::BTreeMap<_, _>>(),
                "next_step":
                    "Change the setting once, using the vehicle's own controls or a tool already \
                     known to do it correctly. Then capture the same identifiers again and \
                     compare the two. The comparison reports only the bits that moved.",
            })),
            warnings,
            ..Default::default()
        })
    }

    /// Every capture stored for the vehicle in this session, newest first.
    ///
    /// This is what makes the loop usable across days: today's capture finds
    /// last week's without anybody having kept a JSON blob in a text file.
    pub fn list_captures(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("list_captures", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::READ_FEATURE, initiator, None)
            .and_then(|_| self.list_captures_inner());
        self.finish("list_captures", capabilities::READ_FEATURE, t0, outcome)
    }

    fn list_captures_inner(&mut self) -> AimResult<Payload> {
        let Some(vehicle_id) = self.session.vehicle_id.clone() else {
            return Err(AimError::new(
                ErrorCode::PreconditionFailed,
                "no vehicle has been identified in this session, so there is nothing to look up \
                 captures for. Run identify_vehicle first.",
            ));
        };
        let captures = self.store.captures_for_vehicle(vehicle_id.as_str())?;
        Ok(Payload {
            data: Some(serde_json::json!({
                "vehicle_id": vehicle_id.as_str(),
                "count": captures.len(),
                "captures": captures
                    .iter()
                    .map(|c| serde_json::json!({
                        "id": c.id,
                        "module": c.module_key,
                        "label": c.label,
                        "taken_at": c.taken_at,
                        "same_session": c.session_id == self.session.id.as_str(),
                    }))
                    .collect::<Vec<_>>(),
            })),
            ..Default::default()
        })
    }

    /// Change a vehicle setting, having been told to by a person.
    ///
    /// This is the only write path in the app that touches configuration, and
    /// every constraint on it is deliberate:
    ///
    /// * The caller names a **feature id and a value**. It cannot name a module,
    ///   a data identifier, a byte or a bit — those come from the catalogue, so
    ///   an arbitrary write is not expressible rather than merely refused.
    /// * The same plan the preview showed is recomputed here and must still
    ///   pass. A preview that passed ten minutes ago is not authority to write
    ///   now, because the engine may have been started since.
    /// * The record is **read, modified and written whole**, then **read back
    ///   and verified**. A write this app reports as successful is one it has
    ///   seen take effect, not one that merely returned a positive response.
    /// * `confirmation` is mandatory and must name a human. The agent is
    ///   registered with read-only tools and cannot reach this function at all.
    pub fn apply_configuration_change(
        &mut self,
        feature_id: &str,
        desired: crate::config::DesiredValue,
        initiator: &str,
        confirmation: &str,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "apply_configuration_change",
            initiator,
            serde_json::json!({ "feature_id": feature_id, "desired": desired }),
        );
        let outcome = self
            .authorize(capabilities::WRITE_FEATURE, initiator, Some(confirmation))
            .and_then(|_| self.apply_change_inner(feature_id, desired));
        self.finish("apply_configuration_change", capabilities::WRITE_FEATURE, t0, outcome)
    }

    fn apply_change_inner(
        &mut self,
        feature_id: &str,
        desired: crate::config::DesiredValue,
    ) -> AimResult<Payload> {
        // 1. The plan, recomputed now rather than trusted from the preview.
        let plan_payload = self.preview_inner(feature_id, desired)?;
        let plan: crate::config::ChangePlan =
            plan_payload.data.clone().and_then(|v| serde_json::from_value(v).ok()).ok_or_else(
                || AimError::new(ErrorCode::Internal, "could not re-evaluate the change plan"),
            )?;
        if !plan.can_apply {
            let failed: Vec<&str> =
                plan.checks.iter().filter(|c| !c.passed).map(|c| c.id.as_str()).collect();
            return Err(AimError::new(
                ErrorCode::PreconditionFailed,
                format!(
                    "this change cannot be applied right now; failed checks: {}",
                    failed.join(", ")
                ),
            )
            .with_details(serde_json::to_value(&plan).unwrap_or(serde_json::Value::Null)));
        }

        // 2. The mapping, which must be executable and verified.
        // Copied out rather than borrowed: everything below needs `self`
        // mutably to talk to the adapter, and the catalogue entry is only
        // needed for these two facts.
        let (target, verification) = {
            let feature = self.decoders.features.get(feature_id).ok_or_else(|| {
                AimError::new(ErrorCode::NotFound, format!("no feature {feature_id}"))
            })?;
            let target =
                feature.mapping.as_ref().and_then(|m| m.as_data_identifier()).ok_or_else(|| {
                    AimError::new(
                        ErrorCode::PreconditionFailed,
                        "this feature has no mapping that can be executed. Nobody has recorded \
                         which data identifier and bits hold this setting on this vehicle, and \
                         this app will not guess at one.",
                    )
                })?;
            (target, feature.verification)
        };
        let want_on = matches!(desired, crate::config::DesiredValue::On);
        let addr = RequestTarget::Physical(target.module.clone());
        let budget = Duration::from_millis(2000);

        // 3. An extended session. Most modules refuse writes in the default
        //    session, and a refusal here costs nothing.
        let session = aim_protocols::UdsRequest::diagnostic_session_control(0x03).to_bytes();
        let _ = self.adapter.request_pdu(&session, &addr, budget);

        // 4. Read the record as it stands.
        let read = aim_protocols::UdsRequest::read_data_by_identifier(target.did).to_bytes();
        let before = self.read_did(&read, &addr, budget, target.did)?;
        let before_state = target.current(&before);

        let after_bytes = target
            .apply(&before, want_on)
            .map_err(|e| AimError::new(ErrorCode::PreconditionFailed, e))?;

        // Already correct. Reporting this as a no-op is more honest than
        // writing identical bytes and calling it a change.
        if after_bytes == before {
            return Ok(Payload {
                data: Some(serde_json::json!({
                    "feature_id": feature_id,
                    "changed": false,
                    "reason": "already_set",
                    "before": aim_types::hex(&before),
                    "state": before_state,
                })),
                warnings: vec![Warning::info(
                    "already_set",
                    "This setting is already what you asked for. Nothing was written.",
                )],
                ..Default::default()
            });
        }

        // 5. Write the whole record back.
        let write = aim_protocols::UdsRequest::write_data_by_identifier(target.did, &after_bytes)
            .to_bytes();
        let write_reply = self.adapter.request_pdu(&write, &addr, budget)?;
        let wrote_ok = write_reply.iter().any(|m| {
            m.payload.first()
                == Some(&aim_protocols::UdsService::WriteDataByIdentifier.response_id())
        });
        if !wrote_ok {
            return Err(AimError::new(
                ErrorCode::VehicleNotResponding,
                "the module did not accept the write. Nothing about the earlier read suggests \
                 the setting changed, but verify it before assuming so.",
            ));
        }

        // 6. Read it back. A write is not believed until it is seen.
        let verify = self.read_did(&read, &addr, budget, target.did)?;
        let now_state = target.current(&verify);
        let verified = verify == after_bytes;

        let mut warnings = Vec::new();
        if !verified {
            warnings.push(Warning::caution(
                "write_not_verified",
                "The module accepted the write but reading the setting back did not return \
                 what was written. Treat this change as not having happened, and check the \
                 vehicle before trying again.",
            ));
        }
        if verification != aim_types::VerificationStatus::Verified {
            warnings.push(Warning::caution(
                "mapping_unverified",
                "This mapping has not been verified against a known-good tool on this model.",
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "feature_id": feature_id,
                "changed": verified,
                "verified": verified,
                "module": target.module,
                "did": format!("{:04X}", target.did),
                "before": aim_types::hex(&before),
                "after": aim_types::hex(&verify),
                "state_before": before_state,
                "state_after": now_state,
            })),
            warnings,
            ..Default::default()
        })
    }

    /// Send a ReadDataByIdentifier and return the record without its header.
    fn read_did(
        &mut self,
        request: &[u8],
        addr: &RequestTarget,
        budget: Duration,
        did: u16,
    ) -> AimResult<Vec<u8>> {
        let replies = self.adapter.request_pdu(request, addr, budget)?;
        let expected = aim_protocols::UdsService::ReadDataByIdentifier.response_id();
        for m in replies {
            // Positive response: 0x62, then the echoed DID, then the record.
            if m.payload.first() == Some(&expected) && m.payload.len() >= 3 {
                let echoed = u16::from_be_bytes([m.payload[1], m.payload[2]]);
                if echoed == did {
                    return Ok(m.payload[3..].to_vec());
                }
            }
        }
        Err(AimError::new(
            ErrorCode::NoData,
            format!("the module did not return data identifier {did:04X}"),
        ))
    }

    /// Walk the supported-PID mask chain, caching the result for the session.
    fn enumerate_supported(&mut self, module: &Module) -> AimResult<Vec<u8>> {
        if let Some(cached) = self.supported_pids.get(&module.module_key) {
            return Ok(cached.clone());
        }
        let mut found: BTreeSet<u8> = BTreeSet::new();
        let mut base: u8 = 0x00;
        loop {
            let request = ObdRequest::current_data(base);
            let (message, _) = match self.request_module(module, &request) {
                Ok(v) => v,
                // A module that does not answer this mask simply supports
                // nothing above it. That is an answer, not a failure — unless
                // it is the very first mask, which every module must answer.
                Err(e) if base == 0x00 => return Err(e),
                Err(_) => break,
            };
            let payload = Self::payload_of(&message, &request)?;
            let pids = decode_supported_pids(base, &payload)?;
            found.insert(base);
            found.extend(pids.iter().copied());

            let next = base.wrapping_add(0x20);
            if next > base && pids.contains(&next) {
                base = next;
            } else {
                break;
            }
        }
        let list: Vec<u8> = found.into_iter().collect();
        self.supported_pids.insert(module.module_key.clone(), list.clone());
        Ok(list)
    }

    /// Read stored, pending and permanent DTCs.
    ///
    /// `module_key` of `None` reads every discovered module.
    pub fn read_dtcs(&mut self, module_key: Option<&str>, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("read_dtcs", initiator, serde_json::json!({ "module": module_key }));
        let outcome = self
            .authorize(capabilities::READ_DTCS, initiator, None)
            .and_then(|_| self.read_dtcs_inner(module_key));
        self.finish("read_dtcs", capabilities::READ_DTCS, t0, outcome)
    }

    fn read_dtcs_inner(&mut self, module_key: Option<&str>) -> AimResult<Payload> {
        self.require_usable()?;
        let modules = match module_key {
            Some(k) => vec![self.module_by_key(k)?],
            None => {
                let all = self.store.modules(&self.session.id)?;
                if all.is_empty() {
                    return Err(AimError::not_found(
                        "no modules discovered yet; run scan_modules first",
                    ));
                }
                all
            }
        };

        let mut reports: Vec<DtcReport> = Vec::new();
        let mut warnings = Vec::new();
        let mut evidence = None;

        for module in &modules {
            for status in [DtcStatus::Confirmed, DtcStatus::Pending, DtcStatus::Permanent] {
                let service = match status {
                    DtcStatus::Confirmed => Service::StoredDtcs,
                    DtcStatus::Pending => Service::PendingDtcs,
                    DtcStatus::Permanent => Service::PermanentDtcs,
                };
                let request = ObdRequest::bare(service);
                let (message, ev) = match self.request_module(module, &request) {
                    Ok(v) => v,
                    Err(e) if e.code == ErrorCode::NoData => {
                        // A module that ignores one DTC service is common and
                        // is reported, not treated as "no codes".
                        warnings.push(Warning::info(
                            "dtc_service_unanswered",
                            format!(
                                "{} did not answer service {:02X} ({})",
                                module.module_key,
                                service.id(),
                                status.as_str()
                            ),
                        ));
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                evidence = ev.or(evidence);

                let payload = Self::payload_of(&message, &request)?;
                let (code_bytes, claimed) = strip_dtc_count(&payload);
                let codes = decode_dtc_list(code_bytes)?;
                if let Some(claimed) = claimed {
                    if claimed as usize != codes.len() {
                        warnings.push(Warning::caution(
                            "dtc_count_mismatch",
                            format!(
                                "{} claimed {claimed} {} codes but reported {}",
                                module.module_key,
                                status.as_str(),
                                codes.len()
                            ),
                        ));
                    }
                }

                for code in codes {
                    let info = self.decoders.dtcs.describe(&code)?;
                    if info.description.is_none() {
                        warnings.push(Warning::caution(
                            "dtc_not_in_catalog",
                            Self::unknown_dtc_next_steps(&code, &module.module_key, &info),
                        ));
                    }
                    self.store.record_dtc(&DtcRecord {
                        session_id: self.session.id.clone(),
                        module_id: module.id.clone(),
                        code: code.clone(),
                        status,
                        description: info.description.clone(),
                        occurrence: 1,
                        freeze_frame_ref: None,
                        read_at: now(),
                    })?;
                    let _ = self.store.append_event(
                        &self.session.id,
                        EventKind::DtcRead {
                            module_key: module.module_key.clone(),
                            code: code.clone(),
                            status,
                        },
                    );
                    reports.push(DtcReport {
                        code,
                        status,
                        module: module.module_key.clone(),
                        description: info.description,
                        structural_summary: info.structural_summary,
                        verification: info.verification,
                        is_generic: info.is_generic,
                    });
                }
            }
        }

        let confirmed = reports.iter().filter(|r| r.status == DtcStatus::Confirmed).count();
        Ok(Payload {
            data: Some(serde_json::json!({
                "dtcs": reports,
                "confirmed_count": confirmed,
                "modules_read": modules.iter().map(|m| &m.module_key).collect::<Vec<_>>(),
            })),
            warnings,
            evidence,
            module: module_key.map(|k| k.to_string()),
            ..Default::default()
        })
    }

    /// Read freeze frame `frame` from a module.
    pub fn read_freeze_frame(
        &mut self,
        module_key: &str,
        frame: u8,
        initiator: &str,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_freeze_frame",
            initiator,
            serde_json::json!({ "module": module_key, "frame": frame }),
        );
        let outcome = self
            .authorize(capabilities::READ_FREEZE_FRAME, initiator, None)
            .and_then(|_| self.freeze_frame_inner(module_key, frame));
        self.finish("read_freeze_frame", capabilities::READ_FREEZE_FRAME, t0, outcome)
    }

    fn freeze_frame_inner(&mut self, module_key: &str, frame: u8) -> AimResult<Payload> {
        self.require_usable()?;
        let module = self.module_by_key(module_key)?;

        // PID 02 of a freeze frame is the code that caused it. Without it,
        // there is no frame to read.
        let dtc_request = ObdRequest::freeze_frame(0x02, frame);
        let (message, evidence) = self.request_module(&module, &dtc_request)?;
        let payload = Self::payload_of(&message, &dtc_request)?;
        // Payload is <frame> <dtc hi> <dtc lo>.
        if payload.len() < 3 {
            return Err(AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!(
                    "freeze frame {frame} DTC response was {} bytes, expected 3",
                    payload.len()
                ),
            ));
        }
        let cause = aim_protocols::decode_dtc(payload[1], payload[2]);
        let info = self.decoders.dtcs.describe(&cause)?;

        let supported = self.enumerate_supported(&module).unwrap_or_default();
        let mut values = Vec::new();
        let mut warnings = Vec::new();
        for pid in FREEZE_FRAME_PIDS {
            if !supported.contains(&pid) {
                continue;
            }
            let request = ObdRequest::freeze_frame(pid, frame);
            let (message, ev) = match self.request_module(&module, &request) {
                Ok(v) => v,
                Err(e) if e.code == ErrorCode::NoData => continue,
                Err(e) => return Err(e),
            };
            let payload = match Self::payload_of(&message, &request) {
                Ok(p) => p,
                Err(e) => {
                    warnings.push(Warning::info(
                        "freeze_frame_pid_unreadable",
                        format!("PID {pid:02X}: {}", e.message),
                    ));
                    continue;
                }
            };
            // Skip the echoed frame number to reach the data.
            let data = &payload[1..];
            match self.decoders.pids.decode(0x01, pid, data, now()) {
                Ok(decoded) => {
                    for mut v in decoded {
                        if let Some(e) = ev {
                            v.provenance = v.provenance.clone().with_evidence_ref(e);
                        }
                        values.push(v);
                    }
                }
                Err(e) => warnings.push(Warning::info(
                    "freeze_frame_pid_undecodable",
                    format!("PID {pid:02X}: {}", e.message),
                )),
            }
        }

        // A freeze frame is a snapshot from the past. It is deliberately not
        // written to `measurements`, which is a time series of live readings —
        // mixing the two would put a stale sample on a live graph.
        Ok(Payload {
            values,
            data: Some(serde_json::json!({
                "module": module_key,
                "frame": frame,
                "dtc": cause,
                "dtc_description": info.description,
                "dtc_verification": info.verification,
            })),
            warnings,
            evidence,
            module: Some(module_key.to_string()),
        })
    }

    /// Read one signal by its id (`engine_rpm`) or PID (`0x0C`, `12`).
    pub fn read_pid(&mut self, module_key: &str, signal: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_pid",
            initiator,
            serde_json::json!({ "module": module_key, "signal": signal }),
        );
        let outcome = self.authorize(capabilities::READ_PID, initiator, None).and_then(|_| {
            let module = self.module_by_key(module_key)?;
            let pid = self.resolve_signal(signal)?;
            let (values, evidence) = self.sample(&module, pid)?;
            Ok(Payload {
                values,
                data: Some(serde_json::json!({
                    "module": module_key,
                    "pid": pid,
                    "hex": format!("{pid:02X}"),
                })),
                evidence,
                module: Some(module_key.to_string()),
                ..Default::default()
            })
        });
        self.finish("read_pid", capabilities::READ_PID, t0, outcome)
    }

    /// Read several signals as one sample.
    ///
    /// Each is a separate request: ELM327-class adapters support multi-PID
    /// requests inconsistently, and a clone that silently truncates one would
    /// produce readings attributed to the wrong signal.
    pub fn read_live_data(
        &mut self,
        module_key: &str,
        signals: &[String],
        initiator: &str,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_live_data",
            initiator,
            serde_json::json!({ "module": module_key, "signals": signals }),
        );
        let outcome = self
            .authorize(capabilities::READ_LIVE_DATA, initiator, None)
            .and_then(|_| self.live_data_inner(module_key, signals));
        self.finish("read_live_data", capabilities::READ_LIVE_DATA, t0, outcome)
    }

    fn live_data_inner(&mut self, module_key: &str, signals: &[String]) -> AimResult<Payload> {
        self.require_usable()?;
        if signals.is_empty() {
            return Err(AimError::bad_request("read_live_data needs at least one signal"));
        }
        let module = self.module_by_key(module_key)?;
        let mut values = Vec::new();
        let mut warnings = Vec::new();
        let mut evidence = None;

        // A wedged bus must not be hammered for the rest of the sample.
        //
        // The same mistake as the service 06 sweep, in a worse place: this runs
        // on a timer. One bus error used to become a warning and the loop
        // carried straight on to the next signal, so an unhappy link got a
        // fresh burst of requests every interval and never recovered, while the
        // graph kept drawing whichever signals still answered. Stopping the
        // sample gives the bus a gap and makes the problem visible.
        let mut bus_errors = 0usize;
        let mut skipped = Vec::new();

        let mut given_up = Vec::new();

        for signal in signals {
            if bus_errors >= 2 {
                skipped.push(signal.clone());
                continue;
            }
            // A parameter that has timed out repeatedly is not going to start
            // answering. Deliberately *not* recorded as unsupported: a timeout
            // is not a negative response, and the vehicle never said no.
            let key = format!("{}:{signal}", module.module_key);
            if self.signal_timeouts.get(&key).copied().unwrap_or(0) >= SIGNAL_TIMEOUT_STRIKES {
                given_up.push(signal.clone());
                continue;
            }
            let pid = match self.resolve_signal(signal) {
                Ok(p) => p,
                Err(e) => {
                    warnings.push(Warning::caution(
                        "unknown_signal",
                        format!("{signal:?}: {}", e.message),
                    ));
                    continue;
                }
            };
            match self.sample(&module, pid) {
                Ok((mut decoded, ev)) => {
                    evidence = ev.or(evidence);
                    values.append(&mut decoded);
                    // It answered, so whatever went wrong before has passed.
                    self.signal_timeouts.remove(&key);
                }
                Err(e) => {
                    if matches!(e.code, ErrorCode::AdapterError | ErrorCode::VehicleNotResponding) {
                        bus_errors += 1;
                    }
                    if e.code == ErrorCode::TransportTimeout {
                        *self.signal_timeouts.entry(key).or_insert(0) += 1;
                    }
                    warnings.push(Warning::caution(
                        "signal_unavailable",
                        format!("{signal}: {}", e.message),
                    ));
                }
            }
        }

        if !given_up.is_empty() {
            warnings.push(Warning::info(
                "signal_stopped_being_asked",
                format!(
                    "{} signal{} stopped being asked for after timing out {SIGNAL_TIMEOUT_STRIKES} \
                     times in a row ({}). This is not the vehicle saying it does not support \
                     them - it never answered either way. Reconnect to try again.",
                    given_up.len(),
                    if given_up.len() == 1 { "" } else { "s" },
                    given_up.join(", ")
                ),
            ));
        }

        if !skipped.is_empty() {
            warnings.push(Warning::caution(
                "sample_cut_short",
                format!(
                    "the bus returned errors, so {} signal{} were not asked for this time \
                     ({}). The values shown are real; the missing ones were skipped rather \
                     than retried into an unhappy bus.",
                    skipped.len(),
                    if skipped.len() == 1 { "" } else { "s" },
                    skipped.join(", ")
                ),
            ));
        }

        // A sample where nothing at all could be read is a failure, not an
        // empty success: a live-data graph must never silently flatline.
        if values.is_empty() {
            return Err(AimError::no_data(format!(
                "none of the {} requested signals could be read",
                signals.len()
            )));
        }

        Ok(Payload {
            values,
            data: Some(serde_json::json!({
                "module": module_key,
                "requested": signals,
            })),
            warnings,
            evidence,
            module: Some(module_key.to_string()),
        })
    }

    /// Find every module on the bus, and read its faults.
    ///
    /// This is the difference between a code reader and a scan tool, and it
    /// needs no manufacturer secrets to do.
    ///
    /// Service 03 — everything this app used before — is legislated *emissions*
    /// diagnostics. It reaches the engine and transmission controllers and
    /// nothing else, which is why a scan of a vehicle with a dead airbag
    /// module, a faulty wheel speed sensor and a failing body controller comes
    /// back clean. Those modules are on the same wires, answering the same way;
    /// nobody was asking them.
    ///
    /// UDS `ReadDTCInformation` (0x19) asks them. It is ISO 14229, a public
    /// standard, and it works identically on every manufacturer — the parts of
    /// UDS that are proprietary are the data identifiers and the security
    /// algorithms, neither of which this needs.
    ///
    /// Two steps:
    ///
    /// 1. **Discovery.** `TesterPresent` (0x3E 0x00) to each address in the
    ///    diagnostic range. It is the most inert request in the protocol — a
    ///    keep-alive that does nothing — and anything that answers it is a
    ///    module that exists. Which address it *replies* on is recorded rather
    ///    than assumed, because the request-plus-eight convention holds for the
    ///    legislated range and is only a convention elsewhere.
    /// 2. **Fault memory.** `0x19 0x02 0xFF` to each module that answered.
    pub fn scan_all_modules(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("scan_all_modules", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::SCAN_ALL_MODULES, initiator, None)
            .and_then(|_| self.scan_all_inner());
        self.finish("scan_all_modules", capabilities::SCAN_ALL_MODULES, t0, outcome)
    }

    fn scan_all_inner(&mut self) -> AimResult<Payload> {
        self.require_usable()?;
        let budget = self.adapter.capabilities().discovery_budget();

        let mut found = Vec::new();
        let mut warnings = Vec::new();
        let probe = aim_protocols::UdsRequest::tester_present(false).to_bytes();

        // Addressed the way this vehicle is actually addressed. Sweeping
        // 11-bit identifiers on a 29-bit vehicle reaches nothing and reports it
        // as an empty vehicle.
        let addresses = scan_addresses(self.adapter.protocol());

        // Calibrate the deadline against a module already known to be there.
        //
        // The discovery budget is derived from measured OBD-II throughput, and
        // a UDS request is not an OBD-II request. Measured on a 2019 F-250: the
        // engine controller answers a PID in 22 ms and did not answer session
        // control inside the 90 ms that produced, so a sweep at that deadline
        // reported a truck with no modules while `read_pid` on one of them was
        // returning engine speed.
        //
        // Asking a known module how long it actually takes turns that from a
        // constant somebody has to tune into a measurement.
        let (probe_budget, calibrated_with) = self.calibrate_probe_budget(budget.probe);
        if let Some(ms) = calibrated_with {
            warnings.push(Warning::info(
                "discovery_deadline_measured",
                format!(
                    "A module known to be present needed {ms} ms to answer a discovery probe, so \
                     the sweep waits that long at each address. This vehicle's modules are \
                     slower to answer diagnostic requests than to answer emissions PIDs."
                ),
            ));
        }

        for (attempt, probe) in [(0usize, probe.clone()), (1, fallback_probe())].into_iter() {
            for addr in &addresses {
                let target = RequestTarget::Physical(addr.clone());
                let replies = match self.adapter.request_pdu(&probe, &target, probe_budget) {
                    Ok(r) => r,
                    // A link that has genuinely fallen over should stop the
                    // sweep rather than produce 240 identical failures.
                    Err(e) if e.code == ErrorCode::TransportDisconnected => return Err(e),
                    Err(_) => continue,
                };
                for m in replies {
                    found.push((addr.clone(), m.address.clone()));
                }
            }
            if !found.is_empty() {
                if attempt == 1 {
                    warnings.push(Warning::info(
                        "discovery_used_the_fallback_probe",
                        "No module answered TesterPresent, so discovery asked again with \
                         DiagnosticSessionControl. Both are standard ways to ask a module \
                         whether it is there, and modules differ in which they answer.",
                    ));
                }
                break;
            }
        }

        if found.is_empty() {
            return Err(AimError::no_data(
                "no module answered on any diagnostic address, to either of the two standard \
                 ways of asking; the vehicle may be asleep, or this adapter may not reach the \
                 bus the modules are on",
            ));
        }

        // Read fault memory from each responder.
        let dtc_request = aim_protocols::UdsRequest::read_dtc_by_status_mask(0xFF).to_bytes();
        let mut modules = Vec::new();
        let mut total_faults = 0usize;
        let mut refused = 0usize;
        // Counted by reason, so the scan can say what kind of wall it hit
        // rather than how many times it hit something.
        let mut refusals: std::collections::BTreeMap<aim_protocols::RefusalKind, usize> =
            std::collections::BTreeMap::new();

        for (request_addr, response_addr) in &found {
            let target = RequestTarget::Physical(request_addr.clone());
            let reply = self.adapter.request_pdu(&dtc_request, &target, budget.read);

            let outcome = match reply {
                Ok(messages) => match messages.into_iter().find(|m| &m.address == response_addr) {
                    Some(m) => self.decode_uds_dtcs(&m.payload, &format!("ECU_{response_addr}")),
                    None => DtcReadOutcome::refused(
                        String::from("did not answer the fault request"),
                        None,
                    ),
                },
                Err(e) => DtcReadOutcome::refused(e.message.clone(), None),
            };
            let DtcReadOutcome { dtcs, note, refusal } = outcome;
            if note.is_some() {
                refused += 1;
            }
            if let Some(kind) = refusal {
                *refusals.entry(kind).or_insert(0usize) += 1;
            }
            total_faults += dtcs.len();
            let fault_count = dtcs.len();

            // A module that answered the full scan is a module that is present,
            // and it belongs in the session's module list like any other.
            // Without this it was discovered, reported, and then invisible to
            // everything that asks "is this module on this vehicle" - including
            // the check that guards a configuration write, which is exactly the
            // case the full scan exists to reach.
            let key = format!("ECU_{response_addr}");
            let record = Module {
                id: aim_types::ModuleId::new(),
                session_id: self.session.id.clone(),
                module_key: key.clone(),
                // Described, never named. See below.
                name: format!("Module at {response_addr}"),
                address: response_addr.clone(),
                protocol: self.adapter.protocol(),
                identity: ModuleIdentity::default(),
                software_version: None,
                discovered_at: now(),
            };
            if self.store.upsert_module(&record).is_ok() {
                let _ = self.store.append_event(
                    &self.session.id,
                    EventKind::ModuleDiscovered { module_key: key, address: response_addr.clone() },
                );
            }

            // Described, never named. A module at 0x760 is "the module at 760"
            // until something it says identifies it — guessing that it is the
            // ABS controller because it usually is on some vehicles is exactly
            // the invention this project refuses.
            modules.push(serde_json::json!({
                "request_address": request_addr,
                "address": response_addr,
                "name": format!("Module at {response_addr}"),
                "in_legislated_range": in_legislated_range(request_addr),
                "faults": dtcs,
                "fault_count": fault_count,
                "note": note,
            }));
        }

        if refused > 0 {
            warnings.push(Warning::info(
                "modules_without_fault_memory",
                format!(
                    "{refused} of {} modules answered the discovery probe but not the fault \
                     request. That is normal: not every module implements the standard fault \
                     service, and some only answer it in a diagnostic session this build does \
                     not open.",
                    found.len()
                ),
            ));
        }

        // One warning per distinct reason, carrying what it means. A count of
        // refusals tells a user nothing they can act on; "four modules have
        // fault memory and will not show it without manufacturer security"
        // tells them where they stand.
        for (kind, count) in &refusals {
            warnings
                .push(Warning::info(kind.code(), format!("{count} module(s): {}", kind.explain())));
        }
        warnings.push(Warning::info(
            "uds_scan_scope",
            "This reads every module that answers on the standard diagnostic addresses, not \
             only the emissions ones. Codes outside the emissions system are reported with \
             their raw identifier when this build has no description for them.",
        ));

        Ok(Payload {
            data: Some(serde_json::json!({
                "modules": modules,
                "module_count": modules.len(),
                "fault_count": total_faults,
                "addresses_probed": addresses.len(),
            })),
            warnings,
            evidence: self.recorder.last_response_event(),
            ..Default::default()
        })
    }

    /// Turn a `0x19 0x02` reply into described faults.
    ///
    /// Returns the note separately so a module that answered something other
    /// than a positive response is reported as such rather than as "no faults",
    /// which are very different things to a person deciding whether to worry.
    /// Measure what one module supports. Reads only.
    pub fn probe_module_capabilities(&mut self, module_key: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "probe_module_capabilities",
            initiator,
            serde_json::json!({ "module": module_key }),
        );
        let outcome = self
            .authorize(capabilities::PROBE_MODULE_CAPABILITIES, initiator, None)
            .and_then(|_| self.probe_capabilities_inner(module_key));
        self.finish(
            "probe_module_capabilities",
            capabilities::PROBE_MODULE_CAPABILITIES,
            t0,
            outcome,
        )
    }

    /// Ask a module what it has, rather than reasoning about what it probably
    /// has.
    ///
    /// Everything here is a read. The result is a map of this module's actual
    /// surface — which identifiers exist, which sessions it grants, whether it
    /// implements security at all — measured on the vehicle in front of us.
    ///
    /// The programming session (0x02) is deliberately never requested. Entering
    /// it can stop a module behaving normally until it is power-cycled, and
    /// nothing in this build needs it. "We did not ask" is a better answer than
    /// a stalled module in somebody's driveway.
    fn probe_capabilities_inner(&mut self, module_key: &str) -> AimResult<Payload> {
        self.require_usable()?;
        let module = self.module_by_key(module_key)?;
        // Modules are recorded by the address they answered on; a request goes
        // to the matching request address.
        let addr = RequestTarget::from_response_address(&module.address)
            .unwrap_or_else(|| RequestTarget::Physical(module.address.clone()));
        let budget = Duration::from_millis(1500);
        let mut warnings = Vec::new();

        // 1. Which sessions this module grants.
        let mut sessions = Vec::new();
        for (id, name) in [(0x01u8, "default"), (0x03u8, "extended")] {
            let request = aim_protocols::UdsRequest::diagnostic_session_control(id).to_bytes();
            let granted = match self.adapter.request_pdu(&request, &addr, budget) {
                Ok(replies) => Self::first_uds_outcome(&replies),
                Err(e) => UdsOutcome::NoAnswer(e.message),
            };
            sessions.push(serde_json::json!({
                "session": name,
                "sub_function": format!("{id:02X}"),
                "granted": granted.is_positive(),
                "refused_because": granted.refusal_code(),
                "detail": granted.detail(),
            }));
        }

        // 2. Whether security is implemented at all.
        //
        // Requesting a seed changes nothing — the module either offers one or
        // says it will not. This does not attempt a key, and this build has no
        // manufacturer key algorithm to attempt one with. What it establishes
        // is whether `securityAccessDenied` elsewhere is a real lock on this
        // module or a red herring.
        let seed_request = aim_protocols::UdsRequest::security_access_request_seed(0x01).to_bytes();
        let seed = match self.adapter.request_pdu(&seed_request, &addr, budget) {
            Ok(replies) => Self::first_uds_outcome(&replies),
            Err(e) => UdsOutcome::NoAnswer(e.message),
        };
        let security = serde_json::json!({
            "implements_security_access": seed.is_positive(),
            "refused_because": seed.refusal_code(),
            "detail": seed.detail(),
            "note": "A seed was requested and nothing was sent back. This build has no \
                     manufacturer key algorithm and does not attempt one.",
        });

        // 3. The identification block.
        let mut identifiers = Vec::new();
        let mut strikes = 0usize;
        for did in 0xF180..=0xF1FFu16 {
            let request = aim_protocols::UdsRequest::read_data_by_identifier(did).to_bytes();
            let outcome = match self.adapter.request_pdu(&request, &addr, budget) {
                Ok(replies) => {
                    strikes = 0;
                    Self::first_uds_outcome(&replies)
                }
                Err(e) => {
                    strikes += 1;
                    UdsOutcome::NoAnswer(e.message)
                }
            };
            // A module that has stopped answering is not a module full of
            // absent identifiers, and asking it another 100 times says nothing.
            if strikes >= SIGNAL_TIMEOUT_STRIKES {
                warnings.push(Warning::info(
                    "identifier_sweep_stopped_early",
                    format!(
                        "Stopped at {did:04X}: the module went quiet for \
                         {SIGNAL_TIMEOUT_STRIKES} consecutive requests. What is listed was \
                         measured; what is missing was not asked."
                    ),
                ));
                break;
            }
            if let UdsOutcome::Positive(payload) = &outcome {
                // 0x62, echoed DID, then the record.
                let record = payload.get(3..).unwrap_or(&[]);
                identifiers.push(serde_json::json!({
                    "did": format!("{did:04X}"),
                    "length": record.len(),
                    "bytes": record.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
                    "text": Self::printable_ascii(record),
                }));
            }
        }

        // Leave the module as it was found. An extended session lapses on its
        // own, but waiting for a timeout is not the same as putting it back.
        let restore = aim_protocols::UdsRequest::diagnostic_session_control(0x01).to_bytes();
        let _ = self.adapter.request_pdu(&restore, &addr, budget);

        warnings.push(Warning::info(
            "probe_is_read_only",
            "Nothing was written. Sessions were opened and closed, a security seed was \
             requested and discarded, and identifiers were read.",
        ));

        Ok(Payload {
            data: Some(serde_json::json!({
                "module": module_key,
                "address": module.address,
                "sessions": sessions,
                "security": security,
                "identifiers": identifiers,
                "identifier_count": identifiers.len(),
                "identifiers_probed": "F180-F1FF",
            })),
            warnings,
            evidence: self.recorder.last_response_event(),
            ..Default::default()
        })
    }

    /// Turn an uncatalogued code into the next thing that can be measured.
    ///
    /// "No description for U0284" ends a session. Working out the undocumented
    /// parts is the whole point of this application, so stopping at the edge of
    /// the catalogue is a strange place to stop.
    ///
    /// Every suggestion here is something this build can actually do and that
    /// produces a *measurement*. None of them is a guess at what the code
    /// means: a manufacturer-specific code has no generic meaning, and the
    /// structural decoding below is all that can be said without inventing one.
    fn unknown_dtc_next_steps(
        code: &str,
        module_key: &str,
        info: &aim_decoders::DtcInfo,
    ) -> String {
        let origin = if info.is_generic {
            "It is a generic code, so it should have a standard meaning; this build's catalogue \
             simply does not carry it."
        } else {
            "It is manufacturer-specific, which means no generic catalogue defines it. Its \
             meaning comes from the carmaker, and this build will not guess one."
        };
        format!(
            "{code} has no description in the generic SAE catalog. What is known structurally: \
             {}. {origin} What can be measured next: read the freeze frame from {module_key} to \
             see the conditions recorded when it set; run probe_module_capabilities on \
             {module_key} to see what that module can be asked; and compare against a capture \
             from when the vehicle was behaving, which shows what changed rather than what the \
             code is called.",
            info.structural_summary
        )
    }

    /// Find a deadline a module that is definitely there can actually meet.
    ///
    /// Tries the derived budget first, then progressively longer ones, against
    /// a module already discovered in this session. Returns the deadline to
    /// sweep with, and how long the module needed when that was longer than the
    /// derived budget.
    ///
    /// Falls back to the derived budget when there is nothing to calibrate
    /// against or nothing answers: an uncalibrated sweep is still worth running,
    /// and a slow sweep of the whole range on a vehicle that will not answer
    /// anything is worse than a quick one.
    fn calibrate_probe_budget(&mut self, derived: Duration) -> (Duration, Option<u128>) {
        let Some(known) = self.store.modules(&self.session.id).ok().and_then(|m| m.into_iter().next())
        else {
            return (derived, None);
        };
        let Some(target) = RequestTarget::from_response_address(&known.address) else {
            return (derived, None);
        };

        // Deliberately does not try `derived`. That number comes from measured
        // OBD-II throughput, and a UDS request is not an OBD-II request:
        // measured on a 2019 F-250, the engine controller answered a PID in
        // 22 ms and needed far longer for session control. Worse, `derived` sat
        // right on the edge — the module beat it once, the sweep was calibrated
        // to it, and the next scan of the same truck found nothing. A deadline
        // that works one time in two is worse than one that is simply too long.
        //
        // Capped at a second: past that the sweep costs more time than the
        // information is worth, and something else is wrong.
        const CANDIDATES: [Duration; 3] = [
            Duration::from_millis(250),
            Duration::from_millis(600),
            Duration::from_millis(1000),
        ];
        // Twice in a row, because a single answer inside a deadline does not
        // establish that the deadline is enough.
        const CONSECUTIVE: usize = 2;

        for probe in [aim_protocols::UdsRequest::tester_present(false).to_bytes(), fallback_probe()]
        {
            for budget in CANDIDATES {
                let consistent = (0..CONSECUTIVE).all(|_| {
                    self.adapter
                        .request_pdu(&probe, &target, budget)
                        .map(|r| !r.is_empty())
                        .unwrap_or(false)
                });
                if consistent {
                    return (budget, Some(budget.as_millis()));
                }
            }
        }
        // Nothing answered consistently at any deadline. Sweep at the longest
        // tried rather than at `derived`: we have just watched a module that is
        // definitely present fail to meet the shorter ones.
        (CANDIDATES[CANDIDATES.len() - 1], None)
    }

    /// Community signal definitions that might apply to this vehicle.
    ///
    /// Touches no vehicle: this is a lookup.
    pub fn list_catalog_signals(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("list_catalog_signals", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::LIST_CATALOG_SIGNALS, initiator, None)
            .and_then(|_| self.list_catalog_inner());
        self.finish("list_catalog_signals", capabilities::LIST_CATALOG_SIGNALS, t0, outcome)
    }

    fn list_catalog_inner(&mut self) -> AimResult<Payload> {
        let vehicle = self.vehicle().cloned();
        let (make, model, year) = match &vehicle {
            Some(v) => (v.make.clone(), v.model.clone(), v.year),
            None => (None, None, None),
        };

        let candidates = self.decoders.catalog.candidates(make.as_deref(), model.as_deref(), year);
        let mut warnings = Vec::new();

        if make.is_none() {
            warnings.push(Warning::info(
                "vehicle_not_identified",
                "No manufacturer is known for this vehicle, so no community definitions can be \
                 matched to it. Run identify_vehicle first.",
            ));
        } else if candidates.is_empty() {
            warnings.push(Warning::info(
                "no_community_definitions",
                format!(
                    "This build has no community signal definitions for {}. That is the normal \
                     case: the catalogue covers a few hundred vehicles and many entries are \
                     still empty. Nothing is wrong with your vehicle or your adapter.",
                    make.as_deref().unwrap_or("this make")
                ),
            ));
        }
        if year.is_none() && !candidates.is_empty() {
            warnings.push(Warning::caution(
                "model_year_unknown",
                "The model year is not known, so definitions scoped to particular years are \
                 being withheld. That is usually most of them - of 116 definitions recorded for \
                 one vehicle, only 7 carried no year range.",
            ));
        }

        let related = candidates
            .iter()
            .filter(|c| c.relevance == aim_decoders::catalog::Relevance::RelatedModel)
            .count();
        if related > 0 {
            warnings.push(Warning::caution(
                "definitions_from_another_model",
                format!(
                    "{related} of these come from a DIFFERENT model by the same manufacturer. \
                     {}",
                    aim_decoders::catalog::Relevance::RelatedModel.explain()
                ),
            ));
        }

        let signals: Vec<serde_json::Value> = candidates
            .iter()
            .flat_map(|c| {
                c.command.signals.iter().map(move |s| {
                    serde_json::json!({
                        "signal_id": s.id,
                        "name": s.name,
                        "group": s.path,
                        "unit": s.fmt.unit,
                        "suggested_metric": s.suggested_metric,
                        "module": c.command.hdr,
                        "asks": c.command.describe(),
                        "from_catalog": c.source_key,
                        "relevance": c.relevance.as_str(),
                        // Said on every entry rather than once at the top,
                        // because these get read one at a time.
                        "verification": "unverified",
                    })
                })
            })
            .collect();

        Ok(Payload {
            data: Some(serde_json::json!({
                "make": make,
                "model": model,
                "year": year,
                "count": signals.len(),
                "signals": signals,
                "note":
                    "These are community-recorded claims about what this vehicle answers, not \
                     measurements and not a standard. Reading one is safe - it is a read, and a \
                     module that does not have the identifier says so. What comes back is a \
                     reading to check, never a fact about your vehicle.",
            })),
            warnings,
            ..Default::default()
        })
    }

    /// Ask the vehicle one community-defined signal.
    pub fn read_catalog_signal(&mut self, signal_id: &str, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "read_catalog_signal",
            initiator,
            serde_json::json!({ "signal": signal_id }),
        );
        let outcome = self
            .authorize(capabilities::READ_CATALOG_SIGNAL, initiator, None)
            .and_then(|_| self.read_catalog_inner(signal_id));
        self.finish("read_catalog_signal", capabilities::READ_CATALOG_SIGNAL, t0, outcome)
    }

    fn read_catalog_inner(&mut self, signal_id: &str) -> AimResult<Payload> {
        self.require_usable()?;
        let vehicle = self.vehicle().cloned();
        let (make, model, year) = match &vehicle {
            Some(v) => (v.make.clone(), v.model.clone(), v.year),
            None => (None, None, None),
        };

        // Resolved and copied out before touching the adapter, so nothing holds
        // a borrow of the decoder set across the request.
        let found =
            self.decoders
                .catalog
                .candidates(make.as_deref(), model.as_deref(), year)
                .into_iter()
                .find_map(|c| {
                    c.command.signals.iter().find(|s| s.id == signal_id).map(|s| {
                        (c.command.clone(), s.clone(), c.relevance, c.source_key.to_string())
                    })
                });
        let Some((command, signal, relevance, source_key)) = found else {
            return Err(AimError::not_found(format!(
                "no community definition {signal_id:?} applies to this vehicle. \
                 list_catalog_signals shows the ones that do."
            )));
        };

        let Some(request) = command.request_bytes() else {
            return Err(AimError::new(
                ErrorCode::PreconditionFailed,
                format!("the catalogue entry for {signal_id:?} is malformed and cannot be sent"),
            ));
        };

        let target = RequestTarget::Physical(command.hdr.clone());
        let budget = self.adapter.capabilities().discovery_budget().read;
        let replies = self.adapter.request_pdu(&request, &target, budget)?;

        // A positive response echoes the service with 0x40 added, then the
        // parameter. Anything else is not an answer to this question.
        let expected_service = request[0].wrapping_add(0x40);
        let echo_len = request.len();
        let answer = replies.iter().find(|m| {
            m.payload.first() == Some(&expected_service)
                && m.payload.len() > echo_len
                && m.payload[1..echo_len] == request[1..]
        });

        let Some(answer) = answer else {
            // Say which of the two it was. A module that refused has told us
            // something; a module that said nothing has not.
            let refusal =
                replies.iter().find_map(|m| match aim_protocols::UdsResponse::parse(&m.payload) {
                    Ok(aim_protocols::UdsResponse::Negative { nrc, .. }) => Some(nrc.refusal()),
                    _ => None,
                });
            let detail = match refusal {
                Some(kind) => format!(" The module refused: {}", kind.explain()),
                None => String::new(),
            };
            return Ok(Payload {
                data: Some(serde_json::json!({
                    "signal_id": signal_id,
                    "answered": false,
                    "from_catalog": source_key,
                    "relevance": relevance.as_str(),
                    "refused_because": refusal.map(|r| r.code()),
                })),
                warnings: vec![Warning::info(
                    "definition_did_not_apply",
                    format!(
                        "The module at {} did not answer service {}.{detail} That is a real \
                         result rather than a failure: it is evidence this definition does not \
                         describe this vehicle.",
                        command.hdr,
                        command
                            .cmd
                            .iter()
                            .next()
                            .map(|(s, p)| format!("{s} parameter {p}"))
                            .unwrap_or_else(|| String::from("?")),
                    ),
                )],
                evidence: self.recorder.last_response_event(),
                ..Default::default()
            });
        };

        let data = &answer.payload[echo_len..];
        let Some(reading) = signal.decode(data, 0) else {
            return Ok(Payload {
                data: Some(serde_json::json!({
                    "signal_id": signal_id,
                    "answered": true,
                    "decoded": false,
                    "bytes": aim_types::hex(data),
                })),
                warnings: vec![Warning::caution(
                    "definition_does_not_fit_the_reply",
                    format!(
                        "The module answered with {} bytes, which is too few for this \
                         definition. The definition describes a different vehicle, or a \
                         different model year of this one.",
                        data.len()
                    ),
                )],
                evidence: self.recorder.last_response_event(),
                ..Default::default()
            });
        };

        let mut warnings = vec![Warning::caution(
            "reading_from_a_community_definition",
            format!(
                "The bytes are measured; what they mean is not. {} This value should be treated \
                 as a reading to check - does it look plausible for this vehicle right now? - \
                 rather than as something the vehicle reported.",
                relevance.explain()
            ),
        )];
        if reading.out_of_stated_range {
            warnings.push(Warning::caution(
                "outside_the_definitions_own_range",
                format!(
                    "{} decoded to {:.3}, which is outside the range the definition itself \
                     states. The definition and this vehicle disagree, so this number should \
                     not be used.",
                    signal.name, reading.value
                ),
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "signal_id": reading.id,
                "name": reading.name,
                "answered": true,
                "decoded": true,
                "value": reading.value,
                "unit": reading.unit,
                "label": reading.label,
                "out_of_stated_range": reading.out_of_stated_range,
                "bytes": aim_types::hex(data),
                "module": answer.address,
                "from_catalog": source_key,
                "relevance": relevance.as_str(),
                "source": "profile_data",
                "verification": "unverified",
            })),
            warnings,
            evidence: self.recorder.last_response_event(),
            ..Default::default()
        })
    }

    /// Classify the first usable UDS reply in a batch.
    fn first_uds_outcome(replies: &[aim_adapter::EcuMessage]) -> UdsOutcome {
        use aim_protocols::UdsResponse;
        for m in replies {
            match UdsResponse::parse(&m.payload) {
                Ok(UdsResponse::Positive { .. }) => return UdsOutcome::Positive(m.payload.clone()),
                Ok(UdsResponse::Negative { nrc, .. }) => return UdsOutcome::Refused(nrc),
                Err(_) => continue,
            }
        }
        UdsOutcome::NoAnswer(String::from("no parsable response"))
    }

    /// The printable run of a record, when it plainly is text.
    ///
    /// Returned only when every byte is printable, so a part number is shown as
    /// a part number and a bitfield is not dressed up as mojibake.
    /// Trailing NULs are padding, not content.
    ///
    /// Measured on a 2019 F-250: every identification record is NUL-padded to a
    /// fixed width, so requiring every byte to be printable hid the text in all
    /// of them — including `F190`, which is the VIN sitting in plain ASCII
    /// behind nine zero bytes.
    fn printable_ascii(bytes: &[u8]) -> Option<String> {
        // All NULs, or empty. Either way there is no text here.
        let last = bytes.iter().rposition(|b| *b != 0x00)?;
        let trimmed: &[u8] = &bytes[..=last];
        // A single leading non-printable byte is common in these records: some
        // hold a one-byte format or version marker before the text. It is
        // skipped rather than allowed to hide the rest.
        let body = match trimmed.first() {
            Some(b) if !(0x20..0x7F).contains(b) && trimmed.len() > 1 => &trimmed[1..],
            _ => trimmed,
        };
        if body.is_empty() || !body.iter().all(|b| (0x20..0x7F).contains(b)) {
            return None;
        }
        Some(String::from_utf8_lossy(body).trim().to_string())
    }

    fn decode_uds_dtcs(&self, payload: &[u8], module_key: &str) -> DtcReadOutcome {
        use aim_protocols::UdsResponse;
        let parsed = match UdsResponse::parse(payload) {
            Ok(p) => p,
            Err(e) => return DtcReadOutcome::refused(e.message, None),
        };
        let data = match &parsed {
            UdsResponse::Positive { data, .. } => data.clone(),
            // The refusal reason is kept, not flattened to "declined". A module
            // that says securityAccessDenied has told us it *has* fault memory
            // and will not show it, which is a different fact from one that says
            // the service does not exist.
            UdsResponse::Negative { nrc, .. } => {
                return DtcReadOutcome::refused(
                    format!("declined: {}", nrc.description()),
                    Some(nrc.refusal()),
                )
            }
        };
        // Skip the echoed sub-function byte before the availability mask.
        let body = data.split_first().map(|(_, rest)| rest).unwrap_or(&[]);
        let dtcs = aim_protocols::decode_dtc_by_status_mask(body);
        DtcReadOutcome {
            dtcs: dtcs
                .iter()
                .map(|d| {
                    // Looked up on the two-byte base code, because that is what
                    // the catalogue is keyed on. A code with no entry keeps its
                    // structural decoding and gets no description rather than
                    // an invented one — the same rule service 03 follows, and
                    // it matters more here: a body or chassis code from a
                    // module nobody legislated is exactly the case where this
                    // build is most likely to have nothing to say.
                    let info = self.decoders.dtcs.describe(&d.base_code).ok();
                    serde_json::json!({
                        "code": d.code,
                        "base_code": d.base_code,
                        "description": info.as_ref().and_then(|i| i.description.clone()),
                        "structural_summary": info.as_ref().map(|i| i.structural_summary.clone()),
                        // An uncatalogued code carries what can be done about
                        // it rather than ending the trail. This is the case
                        // that matters most here: a body or chassis code from
                        // a module nobody legislated is exactly where this
                        // build is most likely to have nothing to say.
                        "next_steps": info
                            .as_ref()
                            .filter(|i| i.description.is_none())
                            .map(|i| Self::unknown_dtc_next_steps(&d.code, module_key, i)),
                        "is_generic": info.as_ref().map(|i| i.is_generic),
                        "status": d.status,
                        "status_summary": d.status_summary(),
                        "failing_now": d.test_failed(),
                        "confirmed": d.confirmed(),
                        "warning_lamp": d.warning_indicator_requested(),
                    })
                })
                .collect(),
            note: None,
            refusal: None,
        }
    }

    /// Emissions readiness from every module that keeps it.
    ///
    /// Reading this from one module is what the app used to do, and it was
    /// quietly wrong. Measured on a real 2019 truck: the engine controller and
    /// the transmission controller both answer PID 01, and they disagree —
    /// 195 warm-ups against 218, 11601 km since the codes were cleared against
    /// 11605. Neither is faulty. Each module keeps its own counters and clears
    /// them on its own schedule, so a card that reads "the" readiness state and
    /// shows one number is picking a winner without saying so.
    ///
    /// This reads all of them and reports the disagreement as a fact, because
    /// that is what it is.
    pub fn read_readiness(&mut self, initiator: &str) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation("read_readiness", initiator, serde_json::json!({}));
        let outcome = self
            .authorize(capabilities::READ_LIVE_DATA, initiator, None)
            .and_then(|_| self.readiness_inner());
        self.finish("read_readiness", capabilities::READ_LIVE_DATA, t0, outcome)
    }

    fn readiness_inner(&mut self) -> AimResult<Payload> {
        self.require_usable()?;
        const SIGNALS: [&str; 4] = [
            "monitor_status",
            "distance_since_cleared",
            "warmups_since_cleared",
            "distance_with_mil_on",
        ];

        let modules = self.store.modules(&self.session.id)?;
        if modules.is_empty() {
            return Err(AimError::not_found(
                "no modules have been discovered yet; run a scan first",
            ));
        }

        let mut per_module = Vec::new();
        let mut warnings = Vec::new();
        let mut values = Vec::new();
        let mut evidence = None;

        for module in &modules {
            let mut read = Vec::new();
            for signal in SIGNALS {
                let Ok(pid) = self.resolve_signal(signal) else { continue };
                if let Ok((decoded, ev)) = self.sample(module, pid) {
                    evidence = ev.or(evidence);
                    read.extend(decoded);
                }
            }
            // A module that answers none of them simply does not keep readiness
            // state. That is normal for a body or transmission controller and
            // is not worth a warning.
            if read.is_empty() {
                continue;
            }
            per_module.push(serde_json::json!({
                "module": module.module_key,
                "address": module.address,
                "name": module.name,
                "values": read,
            }));
            values.extend(read);
        }

        if per_module.is_empty() {
            return Err(AimError::no_data("no module reported emissions readiness"));
        }

        // Do any two modules disagree about how far the vehicle has gone since
        // its codes were cleared? That is the number a buyer leans on, so a
        // split in it has to be visible rather than averaged away.
        let distances: Vec<(String, f64)> = per_module
            .iter()
            .filter_map(|m| {
                let key = m["module"].as_str()?.to_string();
                let v = m["values"]
                    .as_array()?
                    .iter()
                    .find(|v| v["signal_id"] == "distance_since_cleared")?
                    .get("value")?
                    .get("value")?
                    .as_f64()?;
                Some((key, v))
            })
            .collect();
        let disagreement = match (
            distances.iter().map(|(_, v)| *v).fold(f64::MAX, f64::min),
            distances.iter().map(|(_, v)| *v).fold(f64::MIN, f64::max),
        ) {
            (lo, hi) if distances.len() > 1 && (hi - lo) > 1.0 => Some(hi - lo),
            _ => None,
        };
        if let Some(spread) = disagreement {
            warnings.push(Warning::info(
                "modules_disagree_on_readiness",
                format!(
                    "the modules differ by {spread:.0} km on distance since the codes were \
                     cleared. This is normal — each module keeps its own counters — but it \
                     means there is no single answer, so both are shown."
                ),
            ));
        }

        Ok(Payload {
            data: Some(serde_json::json!({
                "modules": per_module,
                "module_count": per_module.len(),
                "disagreement_km": disagreement,
            })),
            values,
            warnings,
            evidence,
            module: None,
        })
    }

    /// Request one service 01 PID, decode it, and record every value.
    fn sample(&mut self, module: &Module, pid: u8) -> AimResult<(Vec<DecodedValue>, Option<i64>)> {
        let request = ObdRequest::current_data(pid);
        let (message, evidence) = self.request_module(module, &request)?;
        let payload = Self::payload_of(&message, &request)?;
        let observed_at = now();
        let mut values = self.decoders.pids.decode(0x01, pid, &payload, observed_at)?;

        for value in &mut values {
            if let Some(e) = evidence {
                value.provenance = value.provenance.clone().with_evidence_ref(e);
            }
            self.store.record_measurement(&measurement_from(
                &self.session.id,
                &module.id,
                value,
            ))?;
            let _ = self.store.append_event(
                &self.session.id,
                EventKind::MeasurementRecorded {
                    module_key: module.module_key.clone(),
                    signal_id: value.signal_id.clone(),
                    value: value.value.as_f64(),
                    unit: value.unit.clone(),
                    raw_hex: value.provenance.raw_hex.clone(),
                },
            );
        }

        // Readings that tell us about vehicle state feed the precondition
        // checks, so an L1 test's "engine off" requirement is evaluated
        // against observation rather than an assumption.
        self.observe_conditions(&values);
        Ok((values, evidence))
    }

    fn observe_conditions(&mut self, values: &[DecodedValue]) {
        for v in values {
            match v.signal_id.as_str() {
                "engine_rpm" => {
                    if let Some(rpm) = v.value.as_f64() {
                        self.conditions.engine_running = rpm > 250.0;
                        self.conditions.ignition_on = true;
                    }
                }
                "vehicle_speed" => self.conditions.vehicle_speed_kph = v.value.as_f64(),
                "control_module_voltage" => self.conditions.battery_voltage = v.value.as_f64(),
                _ => {}
            }
        }
    }

    /// Resolve a signal id or PID spelling to a service 01 PID number.
    ///
    /// A signal id (`engine_rpm`) is always preferred and is what the UI and
    /// the agent should send. A bare number is a convenience for exploration,
    /// and its spelling decides the base in one fixed order, so the same string
    /// always means the same PID:
    ///
    /// | Spelling | Base | Example |
    /// |----------|------|---------|
    /// | `0x`-prefixed | hex | `0x0C` → PID 0x0C |
    /// | all decimal digits | decimal | `12` → PID 0x0C |
    /// | contains a hex letter | hex | `0C`, `1F` → PID 0x0C, 0x1F |
    ///
    /// Decimal is tried before bare hex deliberately. Reading the wrong sensor
    /// and reporting it confidently is the worst failure this layer can have,
    /// so the rule is fixed and documented rather than inferred per call.
    fn resolve_signal(&self, signal: &str) -> AimResult<u8> {
        if let Some((service, pid)) = self.decoders.pids.address_of(signal) {
            if service != 0x01 {
                return Err(AimError::bad_request(format!(
                    "{signal:?} is a service {service:02X} signal, not live data"
                )));
            }
            return Ok(pid);
        }
        let t = signal.trim();
        let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some(hex) => u8::from_str_radix(hex, 16).ok(),
            None => t.parse::<u8>().ok().or_else(|| u8::from_str_radix(t, 16).ok()),
        };
        parsed.ok_or_else(|| {
            AimError::new(
                ErrorCode::DecoderNotFound,
                format!("{signal:?} is neither a known signal id nor a PID number"),
            )
        })
    }

    /// Clear diagnostic trouble codes.
    ///
    /// Implemented end to end and permanently refused: the capability is
    /// registered at L2, which is above this build's ceiling. Handoff §10 asks
    /// for exactly this — the operation exists, is visible, is auditable, and
    /// cannot run. The refusal happens before any byte reaches the vehicle.
    pub fn clear_dtcs(
        &mut self,
        module_key: Option<&str>,
        initiator: &str,
        confirmation: Option<&str>,
    ) -> ToolResult {
        let t0 = Instant::now();
        self.record_invocation(
            "clear_dtcs",
            initiator,
            serde_json::json!({ "module": module_key, "confirmed": confirmation.is_some() }),
        );
        let outcome = self
            .authorize(capabilities::CLEAR_DTCS, initiator, confirmation)
            .and_then(|_| self.clear_dtcs_inner(module_key));
        self.finish("clear_dtcs", capabilities::CLEAR_DTCS, t0, outcome)
    }

    fn clear_dtcs_inner(&mut self, module_key: Option<&str>) -> AimResult<Payload> {
        self.require_usable()?;
        let request = ObdRequest::bare(Service::ClearDtcs);
        let target = match module_key {
            Some(k) => {
                let module = self.module_by_key(k)?;
                RequestTarget::from_response_address(&module.address)
                    .unwrap_or(RequestTarget::Functional)
            }
            None => RequestTarget::Functional,
        };
        let messages = self.request(&request, &target)?;
        Ok(Payload {
            data: Some(serde_json::json!({
                "cleared_by": messages.iter().map(|m| &m.address).collect::<Vec<_>>(),
            })),
            evidence: self.recorder.last_response_event(),
            module: module_key.map(|k| k.to_string()),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_safety::CapabilityRegistry;

    /// A full scan must ask in the addressing the vehicle answers in.
    ///
    /// Measured on a 2023 Odyssey: its modules sit at `18DAF110` and
    /// `18DAF11E`, and the 11-bit sweep spent sixty seconds finding nothing
    /// before reporting the vehicle had no modules at all.
    #[test]
    fn a_full_scan_addresses_a_29_bit_vehicle_in_29_bit() {
        let addrs = scan_addresses(aim_types::ObdProtocol::Iso15765Can29_500);
        // The engine and transmission controllers on the vehicle this was
        // measured against, addressed for a request rather than a response.
        assert!(addrs.contains(&String::from("18DA10F1")), "the engine controller is unreachable");
        assert!(addrs.contains(&String::from("18DA1EF1")));
        // The functional target is a broadcast, not a module.
        assert!(!addrs.contains(&String::from("18DA33F1")));
        assert_eq!(addrs.len(), 255);

        // An 11-bit vehicle keeps the range it had.
        let eleven = scan_addresses(aim_types::ObdProtocol::Iso15765Can11_500);
        assert!(eleven.contains(&String::from("7E0")));
        assert!(!eleven.contains(&String::from("7DF")), "the broadcast id is not a module");
        assert_eq!(eleven.len(), UDS_SCAN_RANGE.count() - 1);

        // Nothing 11-bit leaks into the 29-bit sweep or the reverse.
        assert!(addrs.iter().all(|a| a.len() == 8));
        assert!(eleven.iter().all(|a| a.len() == 3));
    }

    /// A record is shown as text only when it plainly is text, so a part number
    /// reads as one and a bitfield is not dressed up as mojibake.
    #[test]
    fn only_printable_records_are_offered_as_text() {
        assert_eq!(
            DiagnosticService::printable_ascii(b"37805-5MR-C120"),
            Some(String::from("37805-5MR-C120"))
        );
        assert_eq!(DiagnosticService::printable_ascii(&[0x01, 0xFF, 0x00]), None);
        // One unprintable byte is enough: a record is text or it is not.
        assert_eq!(DiagnosticService::printable_ascii(b"AB\x00CD"), None);
        assert_eq!(DiagnosticService::printable_ascii(&[]), None);
    }

    /// A refusal and a silence are different facts and must not merge.
    #[test]
    fn a_refusal_carries_its_reason_and_a_silence_does_not_pretend_to() {
        use aim_protocols::NegativeResponseCode;

        let locked = UdsOutcome::Refused(NegativeResponseCode::SecurityAccessDenied);
        assert!(!locked.is_positive());
        assert_eq!(locked.refusal_code(), Some("module_refused_security_required"));
        assert!(locked.detail().is_some_and(|d| d.contains("seed/key")));

        let absent = UdsOutcome::Refused(NegativeResponseCode::RequestOutOfRange);
        assert_eq!(absent.refusal_code(), Some("module_refused_not_present"));

        // Silence gives no reason, and none is invented for it.
        let quiet = UdsOutcome::NoAnswer(String::from("timed out"));
        assert_eq!(quiet.refusal_code(), None);

        assert!(UdsOutcome::Positive(vec![0x62]).is_positive());
        assert_eq!(UdsOutcome::Positive(vec![0x62]).detail(), None);
    }

    /// The legislated block is an 11-bit concept; 29-bit gets no invented one.
    #[test]
    fn the_legislated_range_is_left_unstated_where_it_is_undefined() {
        assert_eq!(in_legislated_range("7E0"), serde_json::json!(true));
        assert_eq!(in_legislated_range("7E7"), serde_json::json!(true));
        assert_eq!(in_legislated_range("760"), serde_json::json!(false));
        assert_eq!(in_legislated_range("18DA10F1"), serde_json::Value::Null);
    }

    /// Every capability id this service uses must exist in the registry.
    /// A typo here would mean an operation is refused as "unknown" at runtime
    /// rather than at compile time, so it is checked once, here.
    #[test]
    fn every_capability_the_service_uses_is_registered() {
        let registry = CapabilityRegistry::phase1();
        for id in [
            capabilities::CONNECT,
            capabilities::DISCONNECT,
            capabilities::HEALTH,
            capabilities::IDENTIFY_VEHICLE,
            capabilities::SCAN_MODULES,
            capabilities::MODULE_IDENTITY,
            capabilities::READ_DTCS,
            capabilities::READ_FREEZE_FRAME,
            capabilities::READ_PID,
            capabilities::READ_LIVE_DATA,
            capabilities::READ_SUPPORTED_PIDS,
            capabilities::CLEAR_DTCS,
        ] {
            assert!(
                registry.get(id).is_some(),
                "capability {id} is used by the service but not registered"
            );
        }
    }

    #[test]
    fn clearing_codes_is_enabled_for_a_person_but_never_for_the_agent() {
        // The asymmetry is the design. A person can clear codes; the model
        // cannot, because the readiness monitors it destroys are the evidence a
        // used-car buyer most needs and rebuilding them costs 50 to 100 miles.
        let registry = CapabilityRegistry::phase1();
        let cap = registry.get(capabilities::CLEAR_DTCS).unwrap();
        assert!(cap.level <= aim_safety::MAX_ENABLED_LEVEL, "a person must be able to clear codes");
        assert!(cap.level.requires_confirmation(), "and never without saying so explicitly");
        assert!(cap.mutating, "it changes the vehicle and must be audited as such");
    }
}
