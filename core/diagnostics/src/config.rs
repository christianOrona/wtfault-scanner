//! Proposing a vehicle configuration change, and everything that has to be
//! true before one could happen.
//!
//! # The shape of the request is the security model
//!
//! A [`ChangeRequest`] carries a **feature id** and a **desired value**. That is
//! all it can carry. There is no field for a module address, a configuration
//! block, a byte offset, a bit mask, or a raw command, so a language model
//! driving this API cannot express "write 0x30 to byte 2 of block 740-01" even
//! if it wanted to. Where the bits live comes from a [`FeatureDef`] in a data
//! file, and a feature id that is not in the catalogue is not a bad write — it
//! is not a write at all.
//!
//! This is deliberately structural rather than procedural. Validation you can
//! forget to call is not a control; a type with nowhere to put the dangerous
//! value is.
//!
//! # Preview is not apply
//!
//! [`plan_change`] evaluates every check and reports what *would* happen. It
//! touches nothing. That is what the interface shows a person before asking
//! them to approve anything, and it is also what makes the whole chain testable
//! without a vehicle.
//!
//! # What stops a write, and what does not
//!
//! Writes are implemented and enabled. `config.write_feature` is L2, which
//! [`aim_safety::MAX_ENABLED_LEVEL`] permits, behind a typed confirmation that
//! must name a human — the agent is registered with read-only tools and cannot
//! reach it.
//!
//! What remains between a person and a change is deliberately two different
//! kinds of obstacle, and the interface must never blur them:
//!
//! 1. **Evidence.** A feature needs a mapping that has been verified against a
//!    real vehicle. Every mapping in the shipped catalogue is `null`, because
//!    this project has measured none. That is a gap, it is fillable, and a
//!    profile file fills it without rebuilding the app.
//! 2. **Policy.** Anything above [`aim_safety::MAX_ENABLED_RISK`] is refused
//!    permanently — the braking, steering and throttle path, immobilisers and
//!    keys, firmware. No amount of evidence changes that answer.
//!
//! "Nobody has measured this yet" and "this tool will never do that" are
//! different sentences, and a person is owed the right one.

use aim_decoders::{FeatureDef, FeatureSupport};
use aim_types::{AdapterCapabilities, RiskClass, Vehicle};
use serde::{Deserialize, Serialize};

/// What a caller may ask for.
///
/// See the module docs: the absence of fields here is the point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRequest {
    /// Feature id, which must exist in the catalogue.
    pub feature_id: String,
    /// What it should be set to.
    pub desired: DesiredValue,
}

/// The value a feature should take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredValue {
    /// Turn the feature on.
    On,
    /// Turn the feature off.
    Off,
}

/// One check, and whether it passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// Stable identifier for the check.
    pub id: String,
    /// What it establishes, in plain language.
    pub question: String,
    /// Whether it passed.
    pub passed: bool,
    /// Why not, when it did not.
    pub detail: Option<String>,
    /// Whether failing this check is permanent for this build, as opposed to
    /// something the user could fix by changing adapter, key position or data.
    pub blocking_by_design: bool,
}

impl Check {
    fn pass(id: &str, question: &str) -> Self {
        Check {
            id: id.into(),
            question: question.into(),
            passed: true,
            detail: None,
            blocking_by_design: false,
        }
    }

    /// A check that passed but whose reason a person still needs to read.
    ///
    /// A pass is not always the end of the story: the first write on a vehicle
    /// passes on evidence that permits an attempt, not evidence that it works,
    /// and hiding that distinction behind a green tick would be the whole
    /// problem.
    fn pass_with_detail(id: &str, question: &str, detail: &str) -> Self {
        Check {
            id: id.into(),
            question: question.into(),
            passed: true,
            detail: Some(detail.into()),
            blocking_by_design: false,
        }
    }

    fn fail(id: &str, question: &str, detail: impl Into<String>) -> Self {
        Check {
            id: id.into(),
            question: question.into(),
            passed: false,
            detail: Some(detail.into()),
            blocking_by_design: false,
        }
    }

    fn refuse(id: &str, question: &str, detail: impl Into<String>) -> Self {
        Check {
            id: id.into(),
            question: question.into(),
            passed: false,
            detail: Some(detail.into()),
            blocking_by_design: true,
        }
    }
}

/// Whether the module owning this feature has been shown to accept writes.
///
/// Compares the mapping's module address against the gates measured open this
/// session. Address comparison is deliberate: a gate measured on one module
/// says nothing about another, and a body module accepting writes is not a
/// reason to believe the engine controller will.
fn gate_measured_open(f: &FeatureDef, ctx: &ChangeContext<'_>) -> bool {
    let Some(target) = f.mapping.as_ref().and_then(|m| m.as_data_identifier()) else {
        return false;
    };
    ctx.write_gate_open_modules.iter().any(|m| m.eq_ignore_ascii_case(&target.module))
}

/// The result of evaluating a proposed change without performing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangePlan {
    /// The feature this concerns.
    pub feature_id: String,
    /// Its human name, when it exists in the catalogue.
    pub feature_name: Option<String>,
    /// What kind of thing it touches.
    pub risk: Option<RiskClass>,
    /// What was asked for.
    pub desired: DesiredValue,
    /// Every check, in the order a person should read them.
    pub checks: Vec<Check>,
    /// Whether every check passed.
    pub can_apply: bool,
    /// Why not, summarised for the interface.
    pub blocked_reason: Option<String>,
    /// Which modules would be written, from the catalogue and never from the
    /// caller.
    pub modules: Vec<String>,
    /// Where the mapping came from, so a wrong one is traceable.
    pub mapping_source: Option<String>,
}

/// Everything the checks need to know about the current situation.
///
/// No `Default`: `max_level` has no safe default value. Defaulting it to L0
/// would make every plan report "this build refuses writes" even in a build
/// that permits them, and defaulting it to L2 would do the reverse. Callers
/// state it, which means the shipped value comes from
/// [`aim_safety::MAX_ENABLED_LEVEL`] rather than from a derive.
#[derive(Debug, Clone)]
pub struct ChangeContext<'a> {
    /// The identified vehicle, when one has been read.
    pub vehicle: Option<&'a Vehicle>,
    /// What the adapter was observed to be capable of.
    pub adapter: Option<&'a AdapterCapabilities>,
    /// Module keys discovered on the bus.
    pub modules_present: &'a [String],
    /// Whether any adapter reply this session arrived incomplete.
    ///
    /// A truncated read is the signature of an adapter mishandling multi-frame
    /// flow control. Configuration writes are multi-frame. An adapter that has
    /// already demonstrated it drops frames is not one to write a door module
    /// with, and this is measured rather than assumed.
    pub saw_truncated_response: bool,
    /// Control module voltage, when it has been read this session.
    pub battery_voltage: Option<f64>,
    /// The highest permission level this build will execute.
    pub max_level: aim_types::PermissionLevel,
    /// Module request addresses whose write gate was measured open this
    /// session, by asking them to write to an identifier they had just
    /// reported as absent.
    ///
    /// This exists to resolve a real deadlock. A mapping is only writable once
    /// somebody has established that writing works, and establishing that
    /// requires writing — so the first change on any vehicle could never
    /// happen. The way out is not to relax the rule but to notice that it
    /// conflates two different facts:
    ///
    /// * **The gate is open**: this module accepts `WriteDataByIdentifier` in
    ///   the session available, rather than refusing it for security or
    ///   session. Measurable without writing anything, and that is what this
    ///   field records.
    /// * **The write works**: this identifier, these bits, on this vehicle,
    ///   changed and stayed changed. Only a read-back establishes that.
    ///
    /// An open gate permits an *attempt*. It never counts as verification, and
    /// a change made on this basis is reported as a first attempt rather than
    /// as a known-good operation.
    pub write_gate_open_modules: &'a [String],
}

/// The voltage below which no module write is attempted.
///
/// Not a guess at any particular vehicle's tolerance — a floor. Module writes
/// that lose power partway through are the classic way to produce a control
/// unit that no longer responds, and a battery already sagging with the key on
/// is the condition under which that happens.
pub const MIN_WRITE_VOLTAGE: f64 = 12.4;

/// Evaluate a proposed change. Touches nothing.
///
/// `feature` is `None` when the id is not in the catalogue, which is itself a
/// failed check rather than an error: the interface should say "this build does
/// not know that feature" rather than throw.
pub fn plan_change(
    request: &ChangeRequest,
    feature: Option<&FeatureDef>,
    ctx: &ChangeContext<'_>,
) -> ChangePlan {
    let mut checks = Vec::new();

    let Some(f) = feature else {
        checks.push(Check::fail(
            "feature_known",
            "Is this a feature this build knows about?",
            "No feature with this id exists in the catalogue. Nothing can be \
             changed that is not described in a data file first.",
        ));
        return finish(request, None, checks);
    };
    checks.push(Check::pass("feature_known", "Is this a feature this build knows about?"));

    // 1. Risk class. Checked before anything else, because for some classes the
    //    answer is no regardless of how good the rest of the situation is.
    if !f.risk.writable_in_principle_for(f) {
        checks.push(Check::refuse(
            "risk_permitted",
            "Is this the kind of change this product will ever make?",
            format!(
                "This is a {} change, and this tool does not make those. That is a \
                 deliberate decision about what it is for and not a missing feature: \
                 the braking, steering and throttle path, immobilisers and keys, and \
                 anything that rewrites firmware are permanently out of scope. A \
                 verified mapping would not change it.",
                f.risk.label()
            ),
        ));
    } else {
        checks.push(Check::pass(
            "risk_permitted",
            "Is this the kind of change this product will ever make?",
        ));
    }

    // 2. Do we know where the bits are?
    match f.support() {
        FeatureSupport::DescribedOnly => checks.push(Check::fail(
            "mapping_known",
            "Do we know where this setting lives?",
            "This build knows the feature exists but not which configuration \
             bits hold it. That mapping has to be measured on a real vehicle \
             and supplied as a profile file before anything can be read or \
             changed.",
        )),
        // A mapping measured on this vehicle, whose owning module has been
        // shown to accept writes, is the first-write case. It is allowed to be
        // attempted and is never called verified: the read-back decides.
        FeatureSupport::ReadOnly
            if f.verification == aim_types::VerificationStatus::Verified
                && gate_measured_open(f, ctx) =>
        {
            checks.push(Check::pass_with_detail(
                "mapping_known",
                "Do we know where this setting lives, and that it can be changed?",
                "Where this setting lives was measured on this vehicle, and the module that \
                 owns it has been shown to accept writes in the session available - it \
                 refused a write to a non-existent identifier for the identifier rather than \
                 for security. Nobody has yet written *this* setting, so this is a first \
                 attempt: it will be read back afterwards, and only what the module reports \
                 then counts as having happened.",
            ));
        }
        FeatureSupport::ReadOnly => checks.push(Check::fail(
            "mapping_known",
            "Do we know where this setting lives, and that it can be changed?",
            if f.verification == aim_types::VerificationStatus::Verified {
                "Where this setting lives has been verified, so it can be read. Whether it \
                 can be *changed* this way has not been: a module that will show you a \
                 record does not necessarily accept a write to it, and some want an \
                 extended session or security access first. Probe the module's write gate \
                 first - it establishes that without writing anything."
                    .to_string()
            } else {
                "A mapping exists but has not been verified against a real vehicle. It can \
                 be used to read the current setting and never to change it."
                    .to_string()
            },
        )),
        FeatureSupport::Writable => {
            checks.push(Check::pass("mapping_known", "Do we know where this setting lives?"))
        }
    }

    // 3. Is the owning module actually on this vehicle?
    let module_present = f.modules.is_empty()
        || f.modules.iter().any(|m| ctx.modules_present.iter().any(|p| p.contains(m.as_str())));
    if module_present {
        checks.push(Check::pass(
            "module_present",
            "Is the module that owns this setting responding?",
        ));
    } else {
        checks.push(Check::fail(
            "module_present",
            "Is the module that owns this setting responding?",
            format!(
                "None of {} answered on this bus. On many vehicles these sit on \
                 a second CAN bus that most adapters cannot reach.",
                f.modules.join(", ")
            ),
        ));
    }

    // 4. Can the adapter actually do it?
    let can_transmit = ctx.adapter.map(|a| a.supports_transmit).unwrap_or(false);
    if can_transmit {
        checks.push(Check::pass("adapter_can_write", "Can the adapter send, not just listen?"));
    } else {
        checks.push(Check::fail(
            "adapter_can_write",
            "Can the adapter send, not just listen?",
            "This adapter has not been observed transmitting.",
        ));
    }

    // A configuration record does not fit in one frame, so an adapter that
    // cannot segment a request cannot perform any write at all - however sound
    // the mapping and however willing the module.
    //
    // Measured rather than assumed, and checked here rather than discovered
    // halfway through a write: on a 2019 F-250 a clone answered `?` to a
    // 13-byte write in 11 ms, the vehicle never saw it, and the failure was
    // reported as the module refusing. Somebody spent an evening in a truck
    // finding that out.
    match ctx.adapter.map(|a| a.supports_long_messages) {
        Some(true) => checks.push(Check::pass(
            "adapter_can_send_a_whole_record",
            "Can the adapter send a request longer than one frame?",
        )),
        Some(false) => checks.push(Check::fail(
            "adapter_can_send_a_whole_record",
            "Can the adapter send a request longer than one frame?",
            "This adapter refused an eight-byte read without sending it, so it cannot transmit \
             a configuration record either - those are all longer than one frame. Reading is \
             unaffected. An STN-based adapter (OBDLink EX or MX+) does this properly, and \
             nothing about the vehicle or the mapping needs to change.",
        )),
        None => checks.push(Check::fail(
            "adapter_can_send_a_whole_record",
            "Can the adapter send a request longer than one frame?",
            "Not established yet. This is measured once a protocol has been negotiated, so \
             connect to the vehicle first.",
        )),
    }

    let needs_second_bus = f.requires.iter().any(|r| r == "ms_can");
    let has_second_bus = ctx.adapter.map(|a| a.multiple_can_buses).unwrap_or(false);
    if !needs_second_bus || has_second_bus {
        checks.push(Check::pass(
            "adapter_reaches_module",
            "Can the adapter reach the bus this module is on?",
        ));
    } else {
        checks.push(Check::fail(
            "adapter_reaches_module",
            "Can the adapter reach the bus this module is on?",
            "This setting lives on a second CAN bus. A plain ELM327 is wired to \
             the main bus only; a switchable adapter is required.",
        ));
    }

    // 5. Has this adapter already shown it drops frames?
    if ctx.saw_truncated_response {
        checks.push(Check::fail(
            "adapter_reliable",
            "Has every reply from this adapter arrived intact?",
            "At least one reply this session arrived incomplete. Configuration \
             writes span several frames, and an adapter that has already \
             dropped one is not one to write a module with.",
        ));
    } else {
        checks.push(Check::pass(
            "adapter_reliable",
            "Has every reply from this adapter arrived intact?",
        ));
    }

    // 6. Enough electricity to finish.
    match ctx.battery_voltage {
        Some(v) if v >= MIN_WRITE_VOLTAGE => checks.push(Check::pass(
            "battery_healthy",
            "Is there enough battery voltage to finish a write?",
        )),
        Some(v) => checks.push(Check::fail(
            "battery_healthy",
            "Is there enough battery voltage to finish a write?",
            format!(
                "{v:.2} V, below the {MIN_WRITE_VOLTAGE} V floor. A write that \
                 loses power partway through is how a module stops responding \
                 for good. Put a charger on it first."
            ),
        )),
        None => checks.push(Check::fail(
            "battery_healthy",
            "Is there enough battery voltage to finish a write?",
            "Battery voltage has not been read this session.",
        )),
    }

    // 7. The level gate. Last, because it is the one that is true regardless of
    //    the vehicle in front of you, and reading it last means the person sees
    //    everything else they would still need to fix.
    if ctx.max_level >= aim_types::PermissionLevel::L2 {
        checks.push(Check::pass(
            "build_permits_writes",
            "Does this build perform configuration writes at all?",
        ));
    } else {
        checks.push(Check::refuse(
            "build_permits_writes",
            "Does this build perform configuration writes at all?",
            "Configuration writing is compiled off. Everything above is \
             evaluated so you can see what would happen, and nothing is sent.",
        ));
    }

    finish(request, Some(f), checks)
}

fn finish(request: &ChangeRequest, f: Option<&FeatureDef>, checks: Vec<Check>) -> ChangePlan {
    let failed: Vec<&Check> = checks.iter().filter(|c| !c.passed).collect();
    let blocked_reason =
        failed.first().and_then(|c| c.detail.clone()).filter(|_| !failed.is_empty());
    ChangePlan {
        feature_id: request.feature_id.clone(),
        feature_name: f.map(|f| f.name.clone()),
        risk: f.map(|f| f.risk),
        desired: request.desired,
        can_apply: failed.is_empty(),
        blocked_reason,
        modules: f.map(|f| f.modules.clone()).unwrap_or_default(),
        mapping_source: f.and_then(|f| f.source.clone()),
        checks,
    }
}

/// Risk classes this product will never write, whatever the data says.
trait RiskVeto {
    fn writable_in_principle_for(&self, f: &FeatureDef) -> bool;
}

impl RiskVeto for RiskClass {
    fn writable_in_principle_for(&self, f: &FeatureDef) -> bool {
        // Both must agree. The feature's own helper covers security and
        // programming; safety-critical is vetoed here as well, because a
        // convenience-classed product has no business in the braking, steering
        // or restraint path even with a verified mapping in hand.
        f.writable_in_principle() && *self <= aim_safety::MAX_ENABLED_RISK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_decoders::{Applicability, Mapping};
    use aim_types::{PermissionLevel, VerificationStatus};

    pub(super) fn feature(
        risk: RiskClass,
        mapping: Option<Mapping>,
        v: VerificationStatus,
    ) -> FeatureDef {
        FeatureDef {
            id: "f".into(),
            name: "F".into(),
            easy: "e".into(),
            technical: "t".into(),
            risk,
            modules: vec!["DDM".into()],
            applies_to: Applicability::default(),
            requires: vec![],
            mapping,
            verification: v,
            // These tests say "verified" meaning "fully verified", from before
            // reading and writing had separate evidence. Writing needs its own
            // now, so a `Verified` fixture carries both - otherwise every write
            // test here would be asserting the read gate rather than the one it
            // was written to exercise.
            write_verification: (v == VerificationStatus::Verified).then(|| {
                aim_decoders::OperationEvidence {
                    verification: VerificationStatus::Verified,
                    verified_on_vehicles: 1,
                    source: Some("test".into()),
                    last_verified: None,
                    notes: None,
                }
            }),
            source: Some("test".into()),
            notes: None,
        }
    }

    pub(super) fn verified_mapping() -> Option<Mapping> {
        Some(Mapping::AsBuiltBits {
            block: "740-01".into(),
            byte: 2,
            mask: 0x30,
            on: 0x10,
            off: 0x00,
        })
    }

    pub(super) fn perfect_context(modules: &[String]) -> ChangeContext<'_> {
        ChangeContext {
            vehicle: None,
            adapter: None,
            modules_present: modules,
            saw_truncated_response: false,
            battery_voltage: Some(12.8),
            // A hypothetical build that permits writes, so the other checks are
            // the ones under test.
            max_level: PermissionLevel::L2,
            // Empty by default so the deadlock this resolves stays visible in
            // tests: a first write is refused unless the gate was measured.
            write_gate_open_modules: &[],
        }
    }

    pub(super) fn request() -> ChangeRequest {
        ChangeRequest { feature_id: "f".into(), desired: DesiredValue::On }
    }

    pub(super) fn check<'a>(plan: &'a ChangePlan, id: &str) -> &'a Check {
        plan.checks.iter().find(|c| c.id == id).expect(id)
    }

    #[test]
    fn an_unknown_feature_id_cannot_be_planned_at_all() {
        let modules = vec!["ECU_7E8".to_string()];
        let plan = plan_change(&request(), None, &perfect_context(&modules));
        assert!(!plan.can_apply);
        assert!(!check(&plan, "feature_known").passed);
        // And nothing else was even evaluated: there is nothing to evaluate.
        assert_eq!(plan.checks.len(), 1);
    }

    #[test]
    fn a_described_feature_with_no_mapping_is_blocked_on_the_mapping() {
        let f = feature(RiskClass::Convenience, None, VerificationStatus::Unverified);
        let modules = vec!["DDM_740".to_string()];
        let plan = plan_change(&request(), Some(&f), &perfect_context(&modules));
        assert!(!plan.can_apply);
        assert!(!check(&plan, "mapping_known").passed);
        assert!(check(&plan, "risk_permitted").passed);
    }

    #[test]
    fn an_unverified_mapping_reads_but_never_writes() {
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Unverified);
        let modules = vec!["DDM_740".to_string()];
        let plan = plan_change(&request(), Some(&f), &perfect_context(&modules));
        assert!(!check(&plan, "mapping_known").passed);
        assert!(check(&plan, "mapping_known")
            .detail
            .as_ref()
            .unwrap()
            .contains("never to change it"));
    }

    #[test]
    fn a_safety_critical_feature_is_refused_even_fully_verified() {
        // The case that matters most: everything is in place and the answer is
        // still no, permanently, and the plan says so rather than listing it as
        // a fixable prerequisite.
        let f =
            feature(RiskClass::SafetyCritical, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let plan = plan_change(&request(), Some(&f), &perfect_context(&modules));
        assert!(!plan.can_apply);
        let c = check(&plan, "risk_permitted");
        assert!(!c.passed);
        assert!(c.blocking_by_design, "must read as permanent, not as a to-do");
    }

    #[test]
    fn security_and_programming_are_refused_the_same_way() {
        for risk in [RiskClass::Security, RiskClass::Programming] {
            let f = feature(risk, verified_mapping(), VerificationStatus::Verified);
            let modules = vec!["DDM_740".to_string()];
            let plan = plan_change(&request(), Some(&f), &perfect_context(&modules));
            assert!(check(&plan, "risk_permitted").blocking_by_design, "{risk:?}");
        }
    }

    #[test]
    fn an_adapter_that_dropped_a_frame_this_session_is_not_written_with() {
        // Measured, not assumed. The truck's own service 09 reply arrived
        // truncated on a real cable during development; that is exactly the
        // condition this refuses on.
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.saw_truncated_response = true;
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!plan.can_apply);
        assert!(!check(&plan, "adapter_reliable").passed);
    }

    #[test]
    fn a_sagging_battery_blocks_the_write() {
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.battery_voltage = Some(11.9);
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!check(&plan, "battery_healthy").passed);
        assert!(check(&plan, "battery_healthy").detail.as_ref().unwrap().contains("charger"));
    }

    #[test]
    fn a_feature_on_a_second_bus_needs_an_adapter_that_reaches_it() {
        let mut f =
            feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        f.requires = vec!["ms_can".into()];
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        let caps = AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        ctx.adapter = Some(&caps);
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!check(&plan, "adapter_reaches_module").passed);
    }

    #[test]
    fn a_verified_convenience_change_is_now_allowed_end_to_end() {
        // The shipped configuration. Everything passes, and the answer is yes -
        // this is the case the whole seam was built for.
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.max_level = aim_safety::MAX_ENABLED_LEVEL;
        let mut caps = AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        caps.supports_transmit = true;
        caps.multiple_can_buses = true;
        caps.supports_long_messages = true;

        ctx.adapter = Some(&caps);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(
            plan.can_apply,
            "everything passes, so this must be applicable: {:?}",
            plan.checks.iter().filter(|c| !c.passed).collect::<Vec<_>>()
        );
        assert!(check(&plan, "build_permits_writes").passed);
        assert!(check(&plan, "risk_permitted").passed);
    }

    /// The risk ceiling refuses on consequence, and says so in those words.
    #[test]
    fn a_safety_critical_change_is_refused_as_policy_not_as_a_gap() {
        let f =
            feature(RiskClass::SafetyCritical, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.max_level = aim_safety::MAX_ENABLED_LEVEL;
        let mut caps = AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        caps.supports_transmit = true;
        caps.multiple_can_buses = true;
        caps.supports_long_messages = true;
        ctx.adapter = Some(&caps);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!plan.can_apply);
        let c = check(&plan, "risk_permitted");
        assert!(!c.passed);
        assert!(c.blocking_by_design);
        let detail = c.detail.clone().unwrap_or_default();
        assert!(
            detail.contains("deliberate decision") && detail.contains("not a missing feature"),
            "a policy refusal must not read as an unfinished feature: {detail}"
        );
    }

    #[test]
    fn an_unmapped_feature_still_cannot_be_written() {
        let f = feature(RiskClass::Convenience, None, VerificationStatus::Unverified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.max_level = aim_safety::MAX_ENABLED_LEVEL;
        let mut caps = AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        caps.supports_transmit = true;
        ctx.adapter = Some(&caps);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!plan.can_apply, "no mapping means no write, ever");
    }

    #[test]
    fn the_preview_reports_every_failure_at_once() {
        let f =
            feature(RiskClass::SafetyCritical, verified_mapping(), VerificationStatus::Verified);
        let modules = vec!["DDM_740".to_string()];
        let mut ctx = perfect_context(&modules);
        ctx.max_level = aim_safety::MAX_ENABLED_LEVEL;
        let mut caps = AdapterCapabilities::unknown(aim_types::TransportKind::Usb);
        caps.supports_transmit = true;
        caps.multiple_can_buses = true;
        caps.supports_long_messages = true;
        ctx.adapter = Some(&caps);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert_eq!(
            plan.checks.iter().filter(|c| !c.passed).count(),
            1,
            "only the build gate should fail: {:#?}",
            plan.checks
        );
    }

    #[test]
    fn the_plan_never_echoes_an_address_the_caller_supplied() {
        // There is nowhere in ChangeRequest to put one. This test exists to
        // fail loudly if a field is ever added.
        let json = serde_json::to_string(&request()).unwrap();
        assert_eq!(json, r#"{"feature_id":"f","desired":"on"}"#);
    }
}

#[cfg(test)]
mod first_write {
    use super::tests::*;
    use super::*;
    use aim_decoders::Mapping;
    use aim_types::VerificationStatus;

    /// An executable mapping, which `verified_mapping` deliberately is not:
    /// an as-built mapping cannot be performed until somebody establishes
    /// which identifier its block corresponds to.
    fn did_mapping() -> Option<Mapping> {
        Some(Mapping::DataIdentifierBits {
            module: String::from("726"),
            did: 0xDE0E,
            byte: 4,
            mask: 0x01,
            on: 0x01,
            off: 0x00,
        })
    }

    /// The deadlock this resolves: a mapping is only writable once somebody
    /// established writing works, and establishing that requires writing. So
    /// the first change on any vehicle could never happen.
    #[test]
    fn a_measured_mapping_alone_is_still_not_enough_to_write() {
        let f = feature(RiskClass::Convenience, did_mapping(), VerificationStatus::Verified);
        let mut f = f;
        // Where it lives was measured; whether it can be changed was not.
        f.write_verification = None;

        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        ctx.write_gate_open_modules = &[];
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!plan.can_apply);
        assert!(!check(&plan, "mapping_known").passed);
    }

    /// With the gate measured open on the owning module, the attempt is
    /// permitted - and the check says out loud that it is an attempt.
    #[test]
    fn a_measured_gate_permits_a_first_attempt_and_says_so() {
        let mut f = feature(RiskClass::Convenience, did_mapping(), VerificationStatus::Verified);
        f.write_verification = None;

        let owning = f
            .mapping
            .as_ref()
            .and_then(|m| m.as_data_identifier())
            .map(|t| t.module)
            .expect("the fixture has an executable mapping");
        let gates = vec![owning];

        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        ctx.write_gate_open_modules = &gates;
        let plan = plan_change(&request(), Some(&f), &ctx);

        let c = check(&plan, "mapping_known");
        assert!(c.passed, "a measured gate should permit the attempt");
        let detail = c.detail.as_ref().expect("a pass that needs reading still carries its reason");
        assert!(detail.contains("first attempt"), "{detail}");
        assert!(detail.contains("read back"), "{detail}");
    }

    /// A gate measured on one module says nothing about another. A body module
    /// accepting writes is not a reason to believe the engine controller will.
    #[test]
    fn a_gate_measured_on_another_module_does_not_count() {
        let mut f = feature(RiskClass::Convenience, did_mapping(), VerificationStatus::Verified);
        f.write_verification = None;

        let gates = vec![String::from("7E0")];
        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        ctx.write_gate_open_modules = &gates;
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!check(&plan, "mapping_known").passed);
    }

    /// An open gate never rescues a mapping nobody measured.
    #[test]
    fn an_open_gate_does_not_rescue_an_undid_mapping() {
        let mut f = feature(RiskClass::Convenience, did_mapping(), VerificationStatus::Unverified);
        f.write_verification = None;

        let owning =
            f.mapping.as_ref().and_then(|m| m.as_data_identifier()).map(|t| t.module).unwrap();
        let gates = vec![owning];
        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        ctx.write_gate_open_modules = &gates;
        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(
            !check(&plan, "mapping_known").passed,
            "knowing a module takes writes is not knowing where the setting lives"
        );
    }
}

#[cfg(test)]
mod long_messages {
    use super::tests::*;
    use super::*;
    use aim_types::{AdapterCapabilities, TransportKind, VerificationStatus};

    fn caps(long: bool) -> AdapterCapabilities {
        let mut c = AdapterCapabilities::unknown(TransportKind::Usb);
        c.supports_transmit = true;
        c.multiple_can_buses = true;
        c.supports_long_messages = long;
        c
    }

    /// A configuration record does not fit in one frame, so an adapter that
    /// cannot segment cannot write however sound the mapping is. Measured on a
    /// 2019 F-250, where a clone answered `?` in 11 ms and the vehicle never
    /// saw the write - and the app blamed the module.
    #[test]
    fn an_adapter_that_cannot_segment_cannot_write() {
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        let c = caps(false);
        ctx.adapter = Some(&c);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(!plan.can_apply);
        let check = check(&plan, "adapter_can_send_a_whole_record");
        assert!(!check.passed);
        // It must point at the adapter rather than leaving somebody to suspect
        // their vehicle or their mapping.
        let detail = check.detail.as_ref().unwrap();
        assert!(detail.contains("Reading is unaffected"), "{detail}");
        assert!(detail.contains("OBDLink"), "{detail}");
    }

    #[test]
    fn an_adapter_that_can_segment_passes_the_check() {
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        let c = caps(true);
        ctx.adapter = Some(&c);

        let plan = plan_change(&request(), Some(&f), &ctx);
        assert!(check(&plan, "adapter_can_send_a_whole_record").passed);
    }

    /// Not measured yet is its own answer. Reporting "cannot" before anything
    /// has been tried would send somebody shopping for hardware they may
    /// already own.
    #[test]
    fn unmeasured_is_reported_as_unmeasured() {
        let f = feature(RiskClass::Convenience, verified_mapping(), VerificationStatus::Verified);
        let modules: Vec<String> = Vec::new();
        let mut ctx = perfect_context(&modules);
        ctx.adapter = None;

        let plan = plan_change(&request(), Some(&f), &ctx);
        let detail =
            check(&plan, "adapter_can_send_a_whole_record").detail.as_ref().unwrap().clone();
        assert!(detail.contains("Not established yet"), "{detail}");
    }
}
