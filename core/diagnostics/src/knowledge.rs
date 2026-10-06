//! What this application has established about a particular vehicle.
//!
//! # The gap this fills
//!
//! Everything else in this project records what a vehicle *said*. The session
//! log holds every adapter exchange, the capture store holds configuration
//! bytes, the as-built table holds the factory record. All of it is evidence,
//! and none of it is a conclusion.
//!
//! Two sessions on a 2019 F-250 established that a documented mirror-fold
//! mapping does nothing on it, that its second bus runs at 500 kbit/s rather
//! than the 125 the code had assumed, and that both door modules accept writes
//! without a security handshake. Every one of those cost real time on a real
//! vehicle. None of them survived the process exiting, so the next session
//! either rediscovered them or — worse — did not, and repeated a failed
//! experiment on somebody's truck.
//!
//! # Negative findings are the expensive ones
//!
//! "This does not work here" is worth as much as "this does", and costs more to
//! learn, because nothing suggests it in advance. A store that kept only
//! successes would quietly invite the same failed experiment every week. So
//! [`FindingOutcome::RuledOut`] is a first-class result and is rendered first.
//!
//! # Written for a model to read quickly
//!
//! The rendering below is the whole point. A model reading this is paying for
//! every token of it on every turn, so:
//!
//! * one line per finding, no prose paragraphs;
//! * ruled-out first, because the most common failure is confidently
//!   suggesting a thing that has already been disproved on this vehicle;
//! * a hard cap, oldest dropped, so a truck with a long history cannot crowd
//!   out the rest of the prompt;
//! * and a count of what was dropped, so the model knows the list is partial
//!   rather than complete.
//!
//! Keyed by VIN throughout. A garage with six vehicles keeps six sets, and
//! nothing leaks between them.

use aim_session::{Finding, FindingOutcome};

/// How this application marks a finding it measured itself, in this session.
pub const MEASURED_THIS_SESSION: &str = "measured_this_session";

/// How it marks one it measured in an earlier session.
pub const MEASURED_EARLIER: &str = "measured_earlier";

/// A finding somebody recorded by hand: what they say is so, and no more.
pub const USER_STATEMENT: &str = "user_statement";

/// Whether `authority` is one this application keeps for its own measurements.
///
/// Nothing that arrives from outside may carry one. "Measured" has to mean
/// that code which took a measurement wrote the finding, or a typed sentence
/// and a reading off the vehicle are the same thing to everything downstream.
pub fn is_measurement_authority(authority: &str) -> bool {
    let authority = authority.trim();
    authority.eq_ignore_ascii_case(MEASURED_THIS_SESSION)
        || authority.eq_ignore_ascii_case(MEASURED_EARLIER)
}

/// How many findings reach a prompt.
///
/// Enough to carry a vehicle's real history, bounded so that it cannot crowd
/// out the diagnosis the person actually asked for. Findings are dropped
/// oldest-first and the reader is told how many were dropped.
pub const MAX_IN_PROMPT: usize = 24;

/// Render what is known about a vehicle, for a model.
///
/// Empty string when nothing is known, so a caller can concatenate without
/// checking — a heading with nothing under it is worse than no heading.
pub fn for_prompt(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return String::new();
    }

    // Ruled-out first. The failure this is here to prevent is a model
    // recommending something this vehicle has already been shown not to do.
    let mut ordered: Vec<&Finding> = findings.iter().collect();
    ordered.sort_by_key(|f| match f.outcome {
        FindingOutcome::RuledOut => 0,
        FindingOutcome::Established => 1,
        FindingOutcome::Observed => 2,
    });

    let dropped = ordered.len().saturating_sub(MAX_IN_PROMPT);
    ordered.truncate(MAX_IN_PROMPT);

    let mut s = String::from(
        "\n# What has already been established about THIS vehicle\n\
         Measured on this vehicle by this application. Treat it as fact and do \
         not re-derive it. In particular, never propose something listed as \
         ruled out — it has been tried here and did not work.\n",
    );

    // A finding this application did not measure says so on its own line. The
    // heading above is true of the rest, and a sentence somebody typed must
    // not inherit it.
    let mut any_not_measured = false;
    for f in ordered {
        let mark = match f.outcome {
            FindingOutcome::RuledOut => "RULED OUT",
            FindingOutcome::Established => "ESTABLISHED",
            FindingOutcome::Observed => "OBSERVED",
        };
        if is_measurement_authority(&f.authority) {
            s.push_str(&format!("- [{mark}] {} ({})\n", f.claim, f.evidence));
        } else {
            any_not_measured = true;
            s.push_str(&format!(
                "- [{mark}, NOT MEASURED: {}] {} ({})\n",
                f.authority, f.claim, f.evidence
            ));
        }
    }
    if any_not_measured {
        s.push_str(
            "Lines marked NOT MEASURED were recorded by a person or taken from somewhere else, \
             not read off the vehicle by this application. Say whose word they are when you \
             rely on one.\n",
        );
    }

    if dropped > 0 {
        s.push_str(&format!(
            "- ...and {dropped} older findings not shown. This list is partial.\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(subject: &str, outcome: FindingOutcome, claim: &str) -> Finding {
        Finding {
            subject: subject.into(),
            outcome,
            claim: claim.into(),
            evidence: "measured".into(),
            authority: "measured_this_session".into(),
            observed_at: "2026-09-12T00:00:00Z".into(),
            session_id: None,
        }
    }

    /// A finding somebody typed in is not presented to a model as a
    /// measurement, whatever outcome it claims.
    #[test]
    fn a_finding_nobody_measured_says_so_in_the_prompt() {
        let mut typed = finding("a", FindingOutcome::Established, "the gate is open");
        typed.authority = USER_STATEMENT.into();
        let measured = finding("b", FindingOutcome::Established, "the bus runs at 500 kbit/s");

        let text = for_prompt(&[typed, measured.clone()]);
        assert!(text.contains("- [ESTABLISHED, NOT MEASURED: user_statement] the gate is open"));
        assert!(text.contains("- [ESTABLISHED] the bus runs at 500 kbit/s"));
        assert!(text.contains("Lines marked NOT MEASURED"));

        // Nothing is said about it when everything was measured.
        assert!(!for_prompt(&[measured]).contains("NOT MEASURED"));
    }

    #[test]
    fn only_this_applications_own_measurements_are_measurements() {
        for authority in ["measured_this_session", " Measured_This_Session ", "measured_earlier"] {
            assert!(is_measurement_authority(authority), "{authority:?}");
        }
        for authority in ["user_statement", "fetched_from_obdb", "", "measured", "a"] {
            assert!(!is_measurement_authority(authority), "{authority:?}");
        }
    }

    #[test]
    fn nothing_known_renders_nothing() {
        assert_eq!(for_prompt(&[]), "");
    }

    /// The one ordering that matters. A model that reads the established facts
    /// and runs out of attention before the ruled-out ones will confidently
    /// propose the thing that was already tried.
    #[test]
    fn ruled_out_findings_come_first() {
        let items = vec![
            finding("a", FindingOutcome::Observed, "the second bus runs at 500 kbit/s"),
            finding("b", FindingOutcome::Established, "both door modules accept writes"),
            finding("c", FindingOutcome::RuledOut, "DE02 bit 3 does not fold the mirrors"),
        ];
        let out = for_prompt(&items);
        let ruled = out.find("RULED OUT").expect("a ruled-out line");
        let established = out.find("ESTABLISHED").expect("an established line");
        let observed = out.find("OBSERVED").expect("an observed line");
        assert!(ruled < established, "{out}");
        assert!(established < observed, "{out}");
    }

    /// A vehicle with a long history must not crowd out the rest of the
    /// prompt, and the model has to be told the list is partial rather than
    /// silently reading a truncated one as complete.
    #[test]
    fn a_long_history_is_capped_and_says_so() {
        let items: Vec<Finding> = (0..MAX_IN_PROMPT + 7)
            .map(|i| finding(&format!("s{i}"), FindingOutcome::Observed, &format!("claim {i}")))
            .collect();
        let out = for_prompt(&items);
        assert_eq!(out.matches("[OBSERVED]").count(), MAX_IN_PROMPT);
        assert!(out.contains("7 older findings not shown"), "{out}");
        assert!(out.contains("partial"), "{out}");
    }

    /// Every line carries its evidence. A claim a model cannot attribute is a
    /// claim it will repeat without being able to say why.
    #[test]
    fn every_line_carries_its_evidence() {
        let mut f = finding("x", FindingOutcome::Established, "the gate is open");
        f.evidence = "probed with TesterPresent on 2026-09-12".into();
        let out = for_prompt(&[f]);
        assert!(out.contains("probed with TesterPresent"), "{out}");
    }
}
