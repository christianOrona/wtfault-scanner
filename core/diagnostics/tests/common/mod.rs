//! Standing in for the person at the interface.
//!
//! A change to a vehicle needs a person's approval, and an approval is not a
//! string a test can pass: it comes from an [`ApprovalDesk`] the gate was
//! built to trust, for one operation on one subject. In the application that
//! desk belongs to the API's own routes. Here it belongs to the test, which is
//! the person.

#![allow(dead_code)]

use aim_diagnostics::{approvals, capabilities, DesiredValue};
use aim_safety::{ApprovalDesk, HumanApproval, SafetyGate};
use std::sync::OnceLock;

/// The desk every gate built by [`gate`] trusts.
pub fn desk() -> &'static ApprovalDesk {
    static DESK: OnceLock<ApprovalDesk> = OnceLock::new();
    DESK.get_or_init(ApprovalDesk::new)
}

/// The shipped gate, trusting this test's desk.
pub fn gate() -> SafetyGate {
    SafetyGate::phase1().trusting(desk())
}

/// A person's approval of `operation` on `subject`, asked for and given.
pub fn approve(operation: &str, subject: &str) -> Option<HumanApproval> {
    let challenge = desk().issue(&[operation], subject, "YES", "approved in a test");
    Some(desk().redeem(&challenge.id, "YES", operation, subject).expect("the right answer"))
}

/// Approval to change one setting to one value.
pub fn approve_change(feature_id: &str, desired: DesiredValue) -> Option<HumanApproval> {
    approve(capabilities::WRITE_FEATURE, &approvals::change(feature_id, desired))
}

/// Approval to ask a setting's module whether it accepts writes, as the first
/// half of changing that setting.
pub fn approve_gate_for_change(feature_id: &str, desired: DesiredValue) -> Option<HumanApproval> {
    approve(capabilities::PROBE_WRITE_GATE, &approvals::change(feature_id, desired))
}

/// Approval to ask one module whether it accepts writes.
pub fn approve_probe(module_key: &str) -> Option<HumanApproval> {
    approve(capabilities::PROBE_WRITE_GATE, &approvals::probe(module_key))
}

/// Approval to clear one module's codes, or every module's.
pub fn approve_clear(module_key: Option<&str>) -> Option<HumanApproval> {
    approve(capabilities::CLEAR_DTCS, &approvals::clear(module_key))
}
