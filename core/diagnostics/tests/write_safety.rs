//! What reaches the wire, and what is said about it afterwards.
//!
//! Every test here is about a vehicle write that must or must not happen, and
//! every one of them decides that by looking at what the adapter was actually
//! handed to send, not at what the service said it did. A result that reads
//! "refused" over a request that went out is the failure this file exists to
//! catch.
//!
//! The transport in between is honest or not, as each test says: the simulated
//! vehicle behind it always does what a module would, and the wrapper decides
//! what the adapter gets to hear.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_decoders::DecoderSet;
use aim_diagnostics::{approvals, capabilities, DesiredValue, DiagnosticService};
use aim_safety::ApprovalDesk;
use aim_session::{FindingOutcome, SessionStore};
use aim_simulator::{ConfigWriteBehaviour, ScenarioId, SharedEmulator, SimulatedTransport};
use aim_transport::{Transport, TransportStats};
use aim_types::{AimResult, ErrorCode, EventKind, PermissionLevel, ToolResult, TransportKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

const USER: &str = "user:test";
const FEATURE: &str = "test_body_setting";
const ON: DesiredValue = DesiredValue::On;
/// The record the simulated body module holds, and what turning the setting on
/// makes of it: bit 2 of byte 3.
const BEFORE: [u8; 6] = [0x00, 0x11, 0x22, 0xB3, 0x44, 0x55];
const AFTER: [u8; 6] = [0x00, 0x11, 0x22, 0xB7, 0x44, 0x55];

// ------------------------------------------------------------------ the wire

/// What the wire does to the write, or to the read that follows it.
#[derive(Clone)]
enum Wire {
    Honest,
    /// The write lands. The read that checks it gets `NO DATA`, every time.
    ReadBackSilent,
    /// The write lands, and the module then holds these bytes instead.
    ReadBackReturns(Vec<u8>),
    /// The write lands. The read that checks it is answered from another
    /// module's address.
    ReadBackFromAnotherModule,
    /// The write lands. All the adapter hears is `7F 2E 78`: working on it.
    WriteAnsweredPendingOnly,
    /// The write lands. Its answer is lost.
    WriteReplyLost,
    /// The write never reaches the vehicle, and nothing answers.
    WriteLost,
}

/// A transport that records every line the adapter sends and can interfere
/// with the two exchanges that matter.
struct Recording {
    inner: SimulatedTransport,
    emulator: SharedEmulator,
    wire: Wire,
    line: Vec<u8>,
    saw_write: bool,
    out: VecDeque<u8>,
    sent: Arc<Mutex<Vec<String>>>,
}

impl Recording {
    fn forward(&mut self, raw: &[u8]) -> AimResult<()> {
        self.inner.write_all(raw)?;
        self.inner.write_all(b"\r")
    }

    fn take_reply(&mut self) -> String {
        let mut reply = Vec::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = self.inner.read(&mut buf, Duration::ZERO) {
            if n == 0 {
                break;
            }
            reply.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&reply).to_string()
    }

    fn answer(&mut self, text: &str) {
        self.out.extend(text.as_bytes().iter());
    }
}

impl Transport for Recording {
    fn kind(&self) -> TransportKind {
        self.inner.kind()
    }
    fn descriptor(&self) -> String {
        self.inner.descriptor()
    }
    fn open(&mut self) -> AimResult<()> {
        self.inner.open()
    }
    fn close(&mut self) -> AimResult<()> {
        self.inner.close()
    }
    fn is_open(&self) -> bool {
        self.inner.is_open()
    }
    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        for &b in data {
            if b != b'\r' && b != b'\n' {
                self.line.push(b);
                continue;
            }
            let raw = std::mem::take(&mut self.line);
            let command: String = String::from_utf8_lossy(&raw)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_uppercase();
            if !command.is_empty() {
                self.sent.lock().unwrap().push(command.clone());
            }
            let is_write = command.contains("2EDE01");
            let is_read_back = command == "22DE01" && self.saw_write;

            match (self.wire.clone(), is_write, is_read_back) {
                (Wire::ReadBackSilent, _, true) => self.answer("NO DATA\r\r>"),
                (Wire::ReadBackReturns(bytes), _, true) => {
                    {
                        let mut e = self.emulator.lock().unwrap();
                        for ecu in e.vehicle.ecus.iter_mut() {
                            if ecu.response_id == 0x7A8 {
                                ecu.config_records.insert(0xDE01, bytes.clone());
                            }
                        }
                    }
                    self.forward(&raw)?;
                }
                (Wire::ReadBackFromAnotherModule, _, true) => {
                    self.forward(&raw)?;
                    let real = self.take_reply();
                    assert!(real.contains("7A8"), "expected the body module to answer: {real:?}");
                    self.answer(&real.replace("7A8", "7B8"));
                }
                (Wire::WriteAnsweredPendingOnly, true, _) => {
                    self.forward(&raw)?;
                    let real = self.take_reply();
                    let heard = real.replace("6E DE 01", "7F 2E 78").replace("6EDE01", "7F2E78");
                    assert_ne!(real, heard, "expected a positive response to replace: {real:?}");
                    self.answer(&heard);
                }
                (Wire::WriteReplyLost, true, _) => {
                    self.forward(&raw)?;
                    let _ = self.take_reply();
                    self.answer("NO DATA\r\r>");
                }
                (Wire::WriteLost, true, _) => self.answer("NO DATA\r\r>"),
                _ => self.forward(&raw)?,
            }
            if is_write {
                self.saw_write = true;
            }
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        if self.out.is_empty() {
            return self.inner.read(buf, timeout);
        }
        let mut n = 0;
        while n < buf.len() {
            match self.out.pop_front() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
    fn flush_input(&mut self) -> AimResult<()> {
        self.inner.flush_input()
    }
    fn stats(&self) -> TransportStats {
        self.inner.stats()
    }
}

/// A vehicle, the service over it, and a record of everything sent to it.
struct Bench {
    service: DiagnosticService,
    emulator: SharedEmulator,
    sent: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

/// How much stands behind the mapping under test.
#[derive(Clone, Copy)]
struct Mapping {
    risk: &'static str,
    verified: bool,
    write_verified: bool,
}

/// Measured, and shown to be writable.
const VERIFIED: Mapping = Mapping { risk: "convenience", verified: true, write_verified: true };
/// Documented by somebody else, and never checked on a vehicle.
const UNVERIFIED: Mapping = Mapping { risk: "convenience", verified: false, write_verified: false };

fn profile(m: Mapping) -> String {
    let verification = if m.verified { "verified" } else { "unverified" };
    let write = if m.write_verified {
        "    write_verification:\n      verification: verified\n      verified_on_vehicles: 1\n      source: \"test\"\n"
    } else {
        ""
    };
    format!(
        r#"
version: 1
profile: test
features:
  - id: test_body_setting
    name: "A body module setting"
    risk: {risk}
    easy: "A setting held in the body module."
    technical: "Bit 2 of byte 3 of data identifier DE01 on the module at 7A0."
    modules: ["7A8"]
    verification: {verification}
    source: "test"
{write}    mapping:
      kind: data_identifier_bits
      module: 0x7A0
      did: 0xDE01
      byte: 3
      mask: 0x04
      on: 0x04
      off: 0x00
"#,
        risk = m.risk
    )
}

impl Bench {
    /// Connected, identified and scanned, with the engine off: everything a
    /// write is allowed to assume, so each test names the one thing it changes.
    fn ready(mapping: Mapping, wire: Wire) -> Bench {
        let mut bench = Bench::connected(mapping, wire);
        bench.emulator.lock().unwrap().vehicle.engine_stopped = true;
        bench.scan();
        bench
    }

    fn connected(mapping: Mapping, wire: Wire) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test-profile.yaml"), profile(mapping)).unwrap();
        let decoders = DecoderSet::with_profiles(dir.path()).unwrap();
        assert!(decoders.profiles.failures().next().is_none(), "the profile did not load");

        let inner = SimulatedTransport::new(ScenarioId::Healthy);
        let emulator = inner.emulator();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let transport = Recording {
            inner,
            emulator: Arc::clone(&emulator),
            wire,
            line: Vec::new(),
            saw_write: false,
            out: VecDeque::new(),
            sent: Arc::clone(&sent),
        };
        let adapter: Box<dyn DiagnosticAdapter> =
            Box::new(Elm327Adapter::new(Box::new(transport), Elm327Config::fast()));
        let mut service = DiagnosticService::start(
            adapter,
            SessionStore::open_in_memory().unwrap(),
            Arc::new(decoders),
            common::gate(),
            None,
        )
        .unwrap();
        assert!(service.connect(USER).success);
        assert!(service.identify_vehicle(USER).success);
        Bench { service, emulator, sent, _dir: dir }
    }

    fn scan(&mut self) {
        self.service.scan_modules(USER);
        assert!(self.service.scan_all_modules(USER).success);
    }

    /// The record the module really holds, whatever anybody was told.
    fn record(&self) -> Vec<u8> {
        self.emulator
            .lock()
            .unwrap()
            .vehicle
            .ecus
            .iter()
            .find(|e| e.response_id == 0x7A8)
            .unwrap()
            .config_records[&0xDE01]
            .clone()
    }

    /// Every request the adapter was handed that is not a read, in order.
    ///
    /// The physical boundary: this is what was put on the transport. Adapter
    /// commands (`AT…`, `ST…`) configure the adapter and carry nothing to the
    /// vehicle; everything else is a service request and is judged by its
    /// first byte.
    fn changing_requests(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|line| !line.starts_with("AT") && !line.starts_with("ST"))
            .filter(|line| !is_a_read(line))
            .cloned()
            .collect()
    }

    /// The write-gate probe is itself a write, to an identifier the module has
    /// just said it does not have. Everything else that is not a read.
    fn writes_to_the_setting(&self) -> Vec<String> {
        self.changing_requests().into_iter().filter(|l| l.starts_with("2EDE01")).collect()
    }

    fn apply(
        &mut self,
        initiator: &str,
        approval: Option<aim_safety::HumanApproval>,
    ) -> ToolResult {
        self.service.apply_configuration_change(FEATURE, ON, initiator, approval)
    }

    fn apply_approved(&mut self) -> ToolResult {
        self.apply(USER, common::approve_change(FEATURE, ON))
    }

    fn open_the_gate(&mut self) {
        let probe = self.service.probe_write_gate_for_feature(
            FEATURE,
            ON,
            USER,
            common::approve_gate_for_change(FEATURE, ON),
        );
        assert!(probe.success, "the probe failed: {:?}", probe.error);
        assert_eq!(probe.data.as_ref().unwrap()["writes_open_without_security"], true);
    }

    fn plan(&mut self) -> serde_json::Value {
        let r = self.service.preview_configuration_change(FEATURE, ON, USER);
        assert!(r.success, "{:?}", r.error);
        r.data.unwrap()
    }

    fn events(&self) -> Vec<EventKind> {
        self.service
            .store()
            .events_since(self.service.session_id(), 0, 100_000)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    /// The safety decisions recorded for the configuration write.
    fn write_decisions(&self) -> Vec<(Option<PermissionLevel>, bool, bool, Option<String>)> {
        self.events()
            .into_iter()
            .filter_map(|k| match k {
                EventKind::SafetyDecision {
                    operation, level, allowed, confirmed, reason, ..
                } if operation == capabilities::WRITE_FEATURE => {
                    Some((level, allowed, confirmed, reason))
                }
                _ => None,
            })
            .collect()
    }

    /// What the flight recorder says became of each write that was sent.
    fn recorded_writes(&self) -> Vec<(String, Option<String>, String)> {
        self.events()
            .into_iter()
            .filter_map(|k| match k {
                EventKind::ConfigWrite { reply, read_back, outcome, .. } => {
                    Some((reply, read_back, outcome))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether the session recorded the setting as written and read back.
    fn says_it_was_written(&self) -> bool {
        self.service.knowledge().iter().any(|f| f.subject == "feature.test_body_setting.written")
    }
}

/// Whether a request line is one of the reads this application sends.
fn is_a_read(line: &str) -> bool {
    let service = &line[..line.len().min(2)];
    match service {
        "01" | "02" | "03" | "06" | "07" | "09" | "0A" | "19" | "21" | "22" | "3E" => true,
        "10" => line == "1001" || line == "1003",
        "27" => line == "2701",
        _ => false,
    }
}

fn code(r: &ToolResult) -> Option<ErrorCode> {
    r.error.as_ref().map(|e| e.code)
}

fn warnings(r: &ToolResult) -> Vec<&str> {
    r.warnings.iter().map(|w| w.code.as_str()).collect()
}

// --------------------------------------------- a person's approval, and only that

/// The honest path, so every refusal below is a refusal of something that
/// would otherwise have happened.
#[test]
fn a_verified_setting_a_person_approved_is_written_once_and_read_back() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);
    let r = bench.apply_approved();

    assert!(r.success, "{:?}", r.error);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["verified"], true);
    assert_eq!(data["changed"], true);
    assert_eq!(data["outcome"], "verified");
    assert_eq!(bench.record(), AFTER);
    assert_eq!(bench.changing_requests(), vec!["2EDE01001122B74455"], "one write, and only it");

    // And the trail says so, at the level the operation has, with what the
    // person approved rather than anything the caller said about itself.
    assert_eq!(
        bench.write_decisions(),
        vec![(Some(PermissionLevel::L2), true, true, None)],
        "one decision: L2, allowed, approved"
    );
    let approval = bench.events().into_iter().find_map(|k| match k {
        EventKind::SafetyDecision { operation, approval, .. }
            if operation == capabilities::WRITE_FEATURE =>
        {
            approval
        }
        _ => None,
    });
    assert_eq!(approval.as_deref(), Some("approved in a test"));
    assert_eq!(
        bench.recorded_writes(),
        vec![(
            String::from("accepted"),
            Some(String::from("001122b74455")),
            String::from("verified")
        )]
    );
    assert!(bench.says_it_was_written());
}

#[test]
fn nothing_is_written_without_a_persons_approval() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);
    let r = bench.apply(USER, None);

    assert_eq!(code(&r), Some(ErrorCode::ConfirmationRequired));
    assert!(bench.changing_requests().is_empty(), "{:?}", bench.changing_requests());
    assert_eq!(bench.record(), BEFORE);
    assert_eq!(
        bench.write_decisions(),
        vec![(
            Some(PermissionLevel::L2),
            false,
            false,
            Some(String::from("confirmation_required"))
        )]
    );
}

/// An approval for something else, from somewhere else, or used a second time
/// writes nothing. Each of these is a real approval from a real desk; what is
/// wrong with it is what it is for.
#[test]
fn an_approval_for_anything_but_this_change_writes_nothing() {
    let other_desk = ApprovalDesk::new();
    let self_issued = {
        let subject = approvals::change(FEATURE, ON);
        let c = other_desk.issue(&[capabilities::WRITE_FEATURE], &subject, "YES", "self-issued");
        other_desk.redeem(&c.id, "YES", capabilities::WRITE_FEATURE, &subject).ok()
    };
    let cases = [
        (
            "the other value",
            common::approve_change(FEATURE, DesiredValue::Off),
            "approval_for_something_else",
        ),
        (
            "another setting",
            common::approve_change("another_setting", ON),
            "approval_for_something_else",
        ),
        ("clearing codes", common::approve_clear(None), "approval_for_something_else"),
        (
            "asking the module, not changing it",
            common::approve_gate_for_change(FEATURE, ON),
            "approval_for_something_else",
        ),
        ("a desk the gate does not trust", self_issued, "approval_not_trusted"),
    ];
    for (what, approval, reason) in cases {
        let mut bench = Bench::ready(VERIFIED, Wire::Honest);
        let r = bench.apply(USER, approval);
        assert_eq!(code(&r), Some(ErrorCode::ConfirmationRequired), "{what}");
        assert!(bench.changing_requests().is_empty(), "{what}: {:?}", bench.changing_requests());
        assert_eq!(bench.record(), BEFORE, "{what}");
        let decisions = bench.write_decisions();
        assert_eq!(decisions.len(), 1, "{what}");
        assert_eq!(decisions[0].3.as_deref(), Some(reason), "{what}");
        assert!(!decisions[0].2, "{what}: recorded as approved");
    }
}

/// Whatever a caller calls itself, a model does not change a vehicle, and a
/// genuine approval does not change that.
#[test]
fn nothing_but_a_person_can_start_a_write_even_with_an_approval() {
    for initiator in
        ["agent:planner", "agent", "Agent:planner", " agent:x", "assistant", "system", "owner"]
    {
        let mut bench = Bench::ready(VERIFIED, Wire::Honest);
        let r = bench.apply(initiator, common::approve_change(FEATURE, ON));
        assert_eq!(code(&r), Some(ErrorCode::OperationNotAllowed), "{initiator:?}");
        assert!(bench.changing_requests().is_empty(), "{initiator:?}");
        assert_eq!(bench.record(), BEFORE, "{initiator:?}");
    }
}

// ----------------------------------------------------- the vehicle's own state

/// Approved, verified, and the engine is running.
#[test]
fn an_approved_write_is_not_sent_while_a_precondition_fails() {
    let mut bench = Bench::connected(VERIFIED, Wire::Honest);
    // Every scenario idles. Nothing here switches the engine off.
    bench.scan();
    let r = bench.apply_approved();

    assert_eq!(code(&r), Some(ErrorCode::PreconditionFailed));
    assert!(bench.changing_requests().is_empty(), "{:?}", bench.changing_requests());
    assert_eq!(bench.record(), BEFORE);
    let decisions = bench.write_decisions();
    assert_eq!(
        decisions,
        vec![(Some(PermissionLevel::L2), false, true, Some(String::from("precondition_failed")))]
    );
}

// --------------------------------------------- a mapping nobody has verified

/// With an approval, every precondition met and an adapter that can send, a
/// mapping nobody verified is still not written.
#[test]
fn an_unverified_mapping_is_not_written_on_approval_alone() {
    let mut bench = Bench::ready(UNVERIFIED, Wire::Honest);
    assert_eq!(bench.plan()["can_apply"], false);
    let r = bench.apply_approved();

    assert_eq!(code(&r), Some(ErrorCode::PreconditionFailed));
    assert!(bench.changing_requests().is_empty(), "{:?}", bench.changing_requests());
    assert_eq!(bench.record(), BEFORE);
}

/// "The module accepts writes" cannot be typed in. A person's statement is
/// stored as a statement, the measured authorities are refused from outside,
/// and neither opens the gate.
#[test]
fn a_statement_that_the_gate_is_open_does_not_open_it() {
    let mut bench = Bench::ready(UNVERIFIED, Wire::Honest);

    let claimed = bench.service.record_finding(
        "module.7A0.write_gate",
        FindingOutcome::Established,
        "typed in, not measured",
        "none",
        "measured_this_session",
    );
    assert_eq!(claimed.unwrap_err().code, ErrorCode::BadRequest);

    bench
        .service
        .record_finding(
            "module.7A0.write_gate",
            FindingOutcome::Established,
            "typed in, not measured",
            "none",
            aim_diagnostics::knowledge::USER_STATEMENT,
        )
        .unwrap();
    assert_eq!(bench.plan()["can_apply"], false, "a statement opened the gate");

    let r = bench.apply_approved();
    assert_eq!(code(&r), Some(ErrorCode::PreconditionFailed));
    assert!(bench.changing_requests().is_empty(), "{:?}", bench.changing_requests());
    assert_eq!(bench.record(), BEFORE);
}

/// The experiment is for settings that cannot hurt anybody. An unverified
/// mapping on anything else stays unwritten however open its module is.
#[test]
fn an_unverified_mapping_on_more_than_a_convenience_is_never_written() {
    let mut bench = Bench::ready(Mapping { risk: "service", ..UNVERIFIED }, Wire::Honest);
    // The module is measured accepting writes. That is not what is missing.
    let probe = bench.service.probe_write_gate("ECU_7A8", USER, common::approve_probe("ECU_7A8"));
    assert!(probe.success, "{:?}", probe.error);
    assert_eq!(probe.data.as_ref().unwrap()["writes_open_without_security"], true);

    assert_eq!(bench.plan()["can_apply"], false);
    let r = bench.apply_approved();
    assert_eq!(code(&r), Some(ErrorCode::PreconditionFailed));
    assert!(bench.writes_to_the_setting().is_empty(), "{:?}", bench.writes_to_the_setting());
    assert_eq!(bench.record(), BEFORE);
}

/// The one way an unverified mapping is written, kept on purpose: a
/// convenience setting, on a module measured accepting writes, as an
/// experiment the result says is one.
///
/// Measuring a mapping means changing the setting some other way first, and
/// for anything a vehicle's own menus do not offer there is no other way. So
/// the write is the measurement, and it is never called anything else.
#[test]
fn an_unverified_convenience_mapping_is_written_only_as_a_measured_experiment() {
    let mut bench = Bench::ready(UNVERIFIED, Wire::Honest);
    assert_eq!(bench.plan()["can_apply"], false, "before the module has been asked");
    assert_eq!(bench.plan()["experiment"], true);
    assert!(bench.writes_to_the_setting().is_empty());

    bench.open_the_gate();
    let plan = bench.plan();
    assert_eq!(plan["can_apply"], true);
    assert_eq!(plan["experiment"], true, "and it is still an experiment");

    let r = bench.apply_approved();
    assert!(r.success, "{:?}", r.error);
    assert!(warnings(&r).contains(&"mapping_unverified"), "{:?}", warnings(&r));
    assert_eq!(bench.writes_to_the_setting(), vec!["2EDE01001122B74455"]);
    assert_eq!(bench.record(), AFTER);
    // The probe wrote to an identifier the module does not have, and that is
    // the only other request that was not a read.
    assert_eq!(bench.changing_requests(), vec!["2EF1FE00", "2EDE01001122B74455"]);
}

// ------------------------------------------- what is said about a write afterwards

/// The module says yes and changes nothing.
#[test]
fn a_write_the_module_accepted_and_ignored_is_not_applied() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);
    for ecu in bench.emulator.lock().unwrap().vehicle.ecus.iter_mut() {
        if ecu.response_id == 0x7A8 {
            ecu.config_write = ConfigWriteBehaviour::AcceptButIgnore;
        }
    }
    let r = bench.apply_approved();

    assert!(r.success);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["verified"], false);
    assert_eq!(data["changed"], false);
    assert_eq!(data["outcome"], "not_applied");
    assert!(warnings(&r).contains(&"write_not_verified"));
    assert!(!bench.says_it_was_written());
    assert_eq!(bench.recorded_writes()[0].2, "not_applied");
}

/// The record ends up as something that is neither what was there nor what
/// was written. That is a change, and it is not the one that was asked for.
#[test]
fn a_record_that_became_something_else_is_changed_and_not_verified() {
    let elsewhere = vec![0x00, 0x11, 0x22, 0xFF, 0x44, 0x55];
    let mut bench = Bench::ready(VERIFIED, Wire::ReadBackReturns(elsewhere.clone()));
    let r = bench.apply_approved();

    assert!(r.success);
    let data = r.data.as_ref().unwrap();
    assert_eq!(data["verified"], false);
    assert_eq!(data["changed"], true, "the record is not what it was, and the result must say so");
    assert_eq!(data["outcome"], "changed_unexpectedly");
    assert_eq!(data["after"], "001122ff4455");
    assert!(warnings(&r).contains(&"write_not_verified"));
    assert!(!warnings(&r).contains(&"cycle_the_key_to_see_it"));
    assert!(!bench.says_it_was_written(), "recorded as written and read back as asked");
    assert_eq!(bench.record(), elsewhere);
}

/// A record that comes back shorter than the one written is not a match.
#[test]
fn a_short_read_back_is_not_verified() {
    let mut bench = Bench::ready(VERIFIED, Wire::ReadBackReturns(vec![0x00, 0x11, 0x22, 0xB7]));
    let r = bench.apply_approved();

    assert!(r.success);
    assert_eq!(r.data.as_ref().unwrap()["verified"], false);
    assert_eq!(r.data.as_ref().unwrap()["outcome"], "changed_unexpectedly");
    assert!(!bench.says_it_was_written());
}

/// The module accepts the write, and the read that would check it gets no
/// answer. The setting changed. Nothing here can know that, and the result
/// must say that it does not know, not that nothing happened.
#[test]
fn a_write_that_was_accepted_and_could_not_be_read_back_is_unknown() {
    let mut bench = Bench::ready(VERIFIED, Wire::ReadBackSilent);
    let r = bench.apply_approved();

    assert!(!r.success, "an unverifiable write must not read as a success");
    assert!(r.data.is_none());
    let error = r.error.as_ref().unwrap();
    assert!(error.message.contains("accepted the write"), "{}", error.message);
    assert!(error.message.contains("not known"), "{}", error.message);
    let details = error.details.as_ref().unwrap();
    assert_eq!(details["outcome"], "unknown");
    assert_eq!(details["write_reply"], "accepted");
    assert_eq!(details["bytes_offered"], "001122b74455");

    assert_eq!(bench.record(), AFTER, "the write landed");
    assert!(!bench.says_it_was_written());
    assert_eq!(
        bench.recorded_writes(),
        vec![(String::from("accepted"), None, String::from("unknown"))],
        "the trail says it was accepted and never read back"
    );
}

/// The right identifier, from the wrong module, is not this module's record.
#[test]
fn a_read_back_answered_by_another_module_verifies_nothing() {
    let mut bench = Bench::ready(VERIFIED, Wire::ReadBackFromAnotherModule);
    let r = bench.apply_approved();

    assert!(!r.success);
    assert_eq!(r.error.as_ref().unwrap().details.as_ref().unwrap()["outcome"], "unknown");
    assert!(!bench.says_it_was_written());
}

/// The module says "working on it" and the adapter stops listening, or the
/// answer is lost outright. Both used to be reported as the module not
/// accepting the write, over a record that had changed.
#[test]
fn a_write_whose_answer_never_arrived_is_settled_by_reading_the_record() {
    for (wire, reply) in
        [(Wire::WriteAnsweredPendingOnly, "pending"), (Wire::WriteReplyLost, "no_answer")]
    {
        let mut bench = Bench::ready(VERIFIED, wire);
        let r = bench.apply_approved();

        assert!(r.success, "{reply}: {:?}", r.error);
        let data = r.data.as_ref().unwrap();
        assert_eq!(data["verified"], true, "{reply}");
        assert_eq!(data["write_reply"], reply);
        assert!(warnings(&r).contains(&"write_reply_not_heard"), "{reply}: {:?}", warnings(&r));
        assert_eq!(bench.record(), AFTER, "{reply}");
        assert_eq!(
            bench.recorded_writes(),
            vec![(
                String::from(reply),
                Some(String::from("001122b74455")),
                String::from("verified")
            )]
        );
    }
}

/// And when the write really did not arrive, that is what is said, because
/// the record was read and is unchanged.
#[test]
fn a_write_that_never_arrived_is_a_failure_and_says_nothing_was_written() {
    let mut bench = Bench::ready(VERIFIED, Wire::WriteLost);
    let r = bench.apply_approved();

    assert!(!r.success);
    let error = r.error.as_ref().unwrap();
    assert!(error.message.contains("nothing was written"), "{}", error.message);
    assert_eq!(error.details.as_ref().unwrap()["outcome"], "not_applied");
    assert_eq!(bench.record(), BEFORE);
    assert!(!bench.says_it_was_written());
    assert_eq!(bench.recorded_writes()[0].2, "not_applied");
}

/// A refusal is the module's own answer, and is neither a write nor a doubt.
#[test]
fn a_refused_write_is_a_failure_and_is_recorded_as_refused() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);
    for ecu in bench.emulator.lock().unwrap().vehicle.ecus.iter_mut() {
        if ecu.response_id == 0x7A8 {
            ecu.config_write = ConfigWriteBehaviour::Refuse;
        }
    }
    let r = bench.apply_approved();

    assert!(!r.success);
    assert_eq!(r.error.as_ref().unwrap().details.as_ref().unwrap()["outcome"], "refused");
    assert_eq!(bench.record(), BEFORE);
    assert_eq!(
        bench.recorded_writes(),
        vec![(String::from("refused"), None, String::from("refused"))]
    );
}

// ------------------------------------------------------------- clearing codes

/// A cleared code is recorded at the level clearing has, which is L1.
#[test]
fn clearing_codes_is_recorded_at_its_own_level_allowed_or_not() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);

    let refused = bench.service.clear_dtcs(None, USER, None);
    assert_eq!(code(&refused), Some(ErrorCode::ConfirmationRequired));
    assert!(bench.changing_requests().is_empty());

    // The preconditions are read from the vehicle, never assumed.
    bench.service.read_live_data("ECU_7E8", &["vehicle_speed".into(), "engine_rpm".into()], USER);
    let cleared = bench.service.clear_dtcs(None, USER, common::approve_clear(None));
    assert!(cleared.success, "{:?}", cleared.error);
    assert_eq!(bench.changing_requests(), vec!["04"]);

    let decisions: Vec<_> = bench
        .events()
        .into_iter()
        .filter_map(|k| match k {
            EventKind::SafetyDecision { operation, level, allowed, confirmed, .. }
                if operation == capabilities::CLEAR_DTCS =>
            {
                Some((level, allowed, confirmed))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        decisions,
        vec![(Some(PermissionLevel::L1), false, false), (Some(PermissionLevel::L1), true, true)]
    );
}

/// An approval to clear one module does not clear them all, or another one.
#[test]
fn an_approval_to_clear_one_module_clears_only_that() {
    let mut bench = Bench::ready(VERIFIED, Wire::Honest);
    for (asked, approved) in
        [(None, Some("ECU_7E8")), (Some("ECU_7E8"), None), (Some("ECU_7E8"), Some("ECU_7EA"))]
    {
        let r = bench.service.clear_dtcs(asked, USER, common::approve_clear(approved));
        assert_eq!(code(&r), Some(ErrorCode::ConfirmationRequired), "{asked:?} under {approved:?}");
    }
    assert!(bench.changing_requests().is_empty(), "{:?}", bench.changing_requests());
    assert!(!bench.emulator.lock().unwrap().vehicle.dtcs_cleared);
}
