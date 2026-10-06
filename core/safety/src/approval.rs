//! Proof that a person approved one specific operation.
//!
//! A confirmation used to be a string. Whatever arrived in that field was
//! taken as a person's word unless it happened to be spelled `agent:` or
//! `system`, which made the confirmation requirement a spelling test: any
//! caller that could put a string in the request could satisfy it.
//!
//! So an approval is a value now, and there is one way to get one:
//!
//! 1. Something a person is looking at asks the [`ApprovalDesk`] for a
//!    [`Challenge`], naming the operation and exactly what it would act on.
//! 2. The person types the phrase the challenge asks for.
//! 3. The desk checks the phrase and hands back a [`HumanApproval`] for that
//!    operation on that subject, once.
//!
//! [`HumanApproval`] has no public constructor, cannot be cloned and cannot be
//! read out of JSON. The [`crate::SafetyGate`] accepts one only from the desk it
//! was built to trust, only for the operation and subject it names, and only
//! while it is fresh.
//!
//! # What this does and does not establish
//!
//! It establishes that whoever asked went through the desk: read a challenge,
//! answered it, and presented the answer for the thing the challenge was
//! about. The tool registry a model speaks through holds no desk and has no
//! field an approval could arrive in, so a model cannot get one by saying
//! anything.
//!
//! It does not establish that a human being pressed the keys. A program
//! running as the same user, talking to the same local API, can read a
//! challenge and answer it. That program can also open the serial port itself,
//! so it is inside the boundary this application has, not outside it.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a person has to answer a challenge.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(600);

/// How long an approval can be spent for after it is given.
///
/// Longer than any single operation takes, because an approved change may wait
/// behind a scan that already holds the adapter. The vehicle's own conditions
/// are measured again at the moment of the write whatever this says.
pub const APPROVAL_LIFETIME: Duration = Duration::from_secs(300);

/// Wrong answers a challenge survives before it is withdrawn.
const MAX_WRONG_ANSWERS: u8 = 5;

/// Challenges held at once. The oldest goes when another is asked for.
const MAX_PENDING: usize = 64;

/// Which desk issued an approval. Never leaves this crate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeskId(u128);

/// What a person is shown, and has to answer, before something changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Challenge {
    /// Names this challenge when it is answered.
    pub id: String,
    /// The word the person has to type.
    pub phrase: String,
    /// What typing it approves, in the desk's words rather than the caller's.
    pub statement: String,
    /// Seconds until this challenge is withdrawn.
    pub expires_in_seconds: u64,
}

/// A person's approval of one operation on one subject.
///
/// Made only by [`ApprovalDesk::redeem`]. Spent by handing it to the gate.
pub struct HumanApproval {
    desk: DeskId,
    operation: String,
    subject: String,
    statement: String,
    expires_at: Instant,
}

impl HumanApproval {
    /// The capability this approves.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// What it approves that capability acting on.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// What the person was told they were approving. Recorded in the audit
    /// trail in place of anything the caller says about itself.
    pub fn statement(&self) -> &str {
        &self.statement
    }

    pub(crate) fn desk(&self) -> DeskId {
        self.desk
    }

    pub(crate) fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

impl std::fmt::Debug for HumanApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HumanApproval")
            .field("operation", &self.operation)
            .field("subject", &self.subject)
            .field("statement", &self.statement)
            .finish_non_exhaustive()
    }
}

struct Pending {
    /// Capabilities this challenge still covers. Each is spent once.
    operations: Vec<String>,
    subject: String,
    phrase: String,
    statement: String,
    issued_at: Instant,
    wrong_answers: u8,
}

/// Issues challenges and turns a correct answer into a [`HumanApproval`].
///
/// Held by whatever faces the person. Nothing that faces a model is given one.
pub struct ApprovalDesk {
    id: DeskId,
    pending: Mutex<HashMap<String, Pending>>,
    challenge_lifetime: Duration,
    approval_lifetime: Duration,
}

impl Default for ApprovalDesk {
    fn default() -> Self {
        ApprovalDesk::new()
    }
}

impl std::fmt::Debug for ApprovalDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalDesk").finish_non_exhaustive()
    }
}

impl ApprovalDesk {
    /// A desk with the shipped lifetimes.
    pub fn new() -> Self {
        ApprovalDesk::with_lifetimes(CHALLENGE_LIFETIME, APPROVAL_LIFETIME)
    }

    /// A desk with its own lifetimes, so expiry can be tested without waiting
    /// ten minutes for it.
    pub fn with_lifetimes(challenge: Duration, approval: Duration) -> Self {
        ApprovalDesk {
            id: DeskId(uuid::Uuid::new_v4().as_u128()),
            pending: Mutex::new(HashMap::new()),
            challenge_lifetime: challenge,
            approval_lifetime: approval,
        }
    }

    pub(crate) fn id(&self) -> DeskId {
        self.id
    }

    /// Ask a person to approve `operations` acting on `subject`.
    ///
    /// More than one operation is for a change that takes two steps under one
    /// approval: asking a module whether it accepts writes, then writing. They
    /// are steps, in the order given: each is covered once, and taking a later
    /// one gives up the ones before it.
    ///
    /// `statement` is what the person is told, and what the audit trail will
    /// say they approved.
    pub fn issue(
        &self,
        operations: &[&str],
        subject: &str,
        phrase: &str,
        statement: &str,
    ) -> Challenge {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|_, p| now.duration_since(p.issued_at) < self.challenge_lifetime);
            while pending.len() >= MAX_PENDING {
                let Some(oldest) =
                    pending.iter().min_by_key(|(_, p)| p.issued_at).map(|(k, _)| k.clone())
                else {
                    break;
                };
                pending.remove(&oldest);
            }
            pending.insert(
                id.clone(),
                Pending {
                    operations: operations.iter().map(|o| o.to_string()).collect(),
                    subject: subject.to_string(),
                    phrase: phrase.to_string(),
                    statement: statement.to_string(),
                    issued_at: now,
                    wrong_answers: 0,
                },
            );
        }
        Challenge {
            id,
            phrase: phrase.to_string(),
            statement: statement.to_string(),
            expires_in_seconds: self.challenge_lifetime.as_secs(),
        }
    }

    /// Answer a challenge, for one operation on one subject.
    ///
    /// Fails closed on everything: a challenge that was never issued or has
    /// been used, one that has expired, one issued for a different operation or
    /// a different subject, and a phrase that is not the one asked for.
    pub fn redeem(
        &self,
        id: &str,
        typed: &str,
        operation: &str,
        subject: &str,
    ) -> AimResult<HumanApproval> {
        let mut pending = self.pending.lock().map_err(|_| {
            AimError::new(ErrorCode::Internal, "the approval desk lock was poisoned")
        })?;

        let Some(entry) = pending.get_mut(id) else {
            return Err(refused(
                "approval_unknown",
                "that approval does not exist or has already been used. Ask again and confirm it.",
            ));
        };
        if entry.issued_at.elapsed() >= self.challenge_lifetime {
            pending.remove(id);
            return Err(refused(
                "approval_expired",
                "that approval was asked for too long ago. Ask again and confirm it.",
            ));
        }
        if entry.subject != subject || !entry.operations.iter().any(|o| o == operation) {
            return Err(refused(
                "approval_for_something_else",
                "that approval was given for something else, and does not cover this.",
            ));
        }
        if !typed.trim().eq_ignore_ascii_case(entry.phrase.trim()) {
            entry.wrong_answers += 1;
            if entry.wrong_answers >= MAX_WRONG_ANSWERS {
                pending.remove(id);
            }
            return Err(refused(
                "approval_phrase_wrong",
                "the confirmation typed is not the one that was asked for. Nothing was done.",
            ));
        }

        // The operations are steps, in order. Taking one uses it up and every
        // step before it, so the last step closes the approval and nothing
        // earlier is left lying around to be spent afterwards.
        if let Some(step) = entry.operations.iter().position(|o| o == operation) {
            entry.operations.drain(..=step);
        }
        let statement = entry.statement.clone();
        if entry.operations.is_empty() {
            pending.remove(id);
        }
        Ok(HumanApproval {
            desk: self.id,
            operation: operation.to_string(),
            subject: subject.to_string(),
            statement,
            expires_at: Instant::now() + self.approval_lifetime,
        })
    }
}

fn refused(reason: &str, message: &str) -> AimError {
    AimError::new(ErrorCode::ConfirmationRequired, message)
        .with_details(serde_json::json!({ "reason": reason }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(e: &AimError) -> String {
        e.details.as_ref().and_then(|d| d["reason"].as_str()).unwrap_or_default().to_string()
    }

    #[test]
    fn the_right_answer_is_an_approval_for_what_was_asked() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(&["config.write_feature"], "feature:x=on", "CHANGE", "turn x on");
        let a = desk.redeem(&c.id, " change ", "config.write_feature", "feature:x=on").unwrap();
        assert_eq!(a.operation(), "config.write_feature");
        assert_eq!(a.subject(), "feature:x=on");
        assert_eq!(a.statement(), "turn x on");
        assert!(a.is_fresh());
    }

    #[test]
    fn an_approval_is_spent_once() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        assert!(desk.redeem(&c.id, "CLEAR", "obd2.clear_dtcs", "clear:*").is_ok());
        let again = desk.redeem(&c.id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap_err();
        assert_eq!(reason(&again), "approval_unknown");
    }

    #[test]
    fn one_approval_can_cover_two_steps_and_each_only_once() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(
            &["config.probe_write_gate", "config.write_feature"],
            "feature:x=on",
            "CHANGE",
            "turn x on",
        );
        assert!(desk.redeem(&c.id, "CHANGE", "config.probe_write_gate", "feature:x=on").is_ok());
        let twice =
            desk.redeem(&c.id, "CHANGE", "config.probe_write_gate", "feature:x=on").unwrap_err();
        assert_eq!(reason(&twice), "approval_for_something_else");
        assert!(desk.redeem(&c.id, "CHANGE", "config.write_feature", "feature:x=on").is_ok());
        let spent =
            desk.redeem(&c.id, "CHANGE", "config.write_feature", "feature:x=on").unwrap_err();
        assert_eq!(reason(&spent), "approval_unknown");
    }

    /// Once the change has been made, the approval is finished. The step that
    /// came before it is not still there to be taken.
    #[test]
    fn the_last_step_closes_the_approval() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(
            &["config.probe_write_gate", "config.write_feature"],
            "feature:x=on",
            "CHANGE",
            "turn x on",
        );
        assert!(desk.redeem(&c.id, "CHANGE", "config.write_feature", "feature:x=on").is_ok());
        for operation in ["config.probe_write_gate", "config.write_feature"] {
            let e = desk.redeem(&c.id, "CHANGE", operation, "feature:x=on").unwrap_err();
            assert_eq!(reason(&e), "approval_unknown", "{operation}");
        }
    }

    #[test]
    fn an_approval_for_one_thing_does_not_cover_another() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(&["config.write_feature"], "feature:x=on", "CHANGE", "turn x on");
        for (operation, subject) in [
            ("config.write_feature", "feature:x=off"),
            ("config.write_feature", "feature:y=on"),
            ("obd2.clear_dtcs", "feature:x=on"),
        ] {
            let e = desk.redeem(&c.id, "CHANGE", operation, subject).unwrap_err();
            assert_eq!(reason(&e), "approval_for_something_else", "{operation} {subject}");
        }
        // Asking for the wrong thing did not use it up.
        assert!(desk.redeem(&c.id, "CHANGE", "config.write_feature", "feature:x=on").is_ok());
    }

    #[test]
    fn malformed_answers_are_refused() {
        let desk = ApprovalDesk::new();
        let c = desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        for id in ["", "user", "owner", "human", "00000000000000000000000000000000", "../x"] {
            let e = desk.redeem(id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfirmationRequired);
            assert_eq!(reason(&e), "approval_unknown", "{id:?}");
        }
        for typed in ["", "yes", "owner", "CLEAR IT", "CHANGE"] {
            let e = desk.redeem(&c.id, typed, "obd2.clear_dtcs", "clear:*").unwrap_err();
            assert_eq!(reason(&e), "approval_phrase_wrong", "{typed:?}");
        }
        // Five wrong answers withdrew it. The right one is too late.
        let e = desk.redeem(&c.id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap_err();
        assert_eq!(reason(&e), "approval_unknown");
    }

    #[test]
    fn a_challenge_nobody_answered_in_time_is_withdrawn() {
        let desk = ApprovalDesk::with_lifetimes(Duration::ZERO, APPROVAL_LIFETIME);
        let c = desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        let e = desk.redeem(&c.id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap_err();
        assert_eq!(reason(&e), "approval_expired");
    }

    #[test]
    fn an_approval_does_not_stay_fresh() {
        let desk = ApprovalDesk::with_lifetimes(CHALLENGE_LIFETIME, Duration::ZERO);
        let c = desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        let a = desk.redeem(&c.id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap();
        assert!(!a.is_fresh());
    }

    #[test]
    fn the_desk_does_not_fill_up() {
        let desk = ApprovalDesk::new();
        let first = desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        for _ in 0..MAX_PENDING {
            desk.issue(&["obd2.clear_dtcs"], "clear:*", "CLEAR", "clear every module");
        }
        assert!(desk.pending.lock().unwrap().len() <= MAX_PENDING);
        let e = desk.redeem(&first.id, "CLEAR", "obd2.clear_dtcs", "clear:*").unwrap_err();
        assert_eq!(reason(&e), "approval_unknown", "the oldest made room");
    }
}
