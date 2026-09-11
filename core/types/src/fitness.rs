//! How much to trust what this adapter tells you.
//!
//! # Why this is a type and not a sentence
//!
//! The driver already measures throughput and adapts request sizing to it, and
//! already records timeouts and refusals. What it did with all of that was
//! write prose caveats — good prose, and unreadable by anything that needed to
//! make a decision. The agent could not see it at all.
//!
//! That matters because the right diagnostic strategy depends on it. An agent
//! working through a marginal adapter should read fewer signals, allow longer
//! budgets, and say plainly that a clean scan means less than it would on good
//! hardware. None of that is possible against a paragraph.
//!
//! # The number that is not a grade
//!
//! A failure rate over four requests is not a measurement, and the most
//! dangerous thing this module could do is dress one up as a verdict. So the
//! grade is [`FitnessGrade::Unmeasured`] until enough has happened to mean
//! something, and "we have not seen enough to say" is a first-class answer
//! rather than an optimistic default.
//!
//! # What it is not
//!
//! Not a judgement about the vehicle. Every rate here is a property of the link
//! between this computer and the adapter, and of the adapter's own behaviour. A
//! vehicle answering `NO DATA` to a parameter it does not implement is a
//! correct answer from a healthy adapter, which is why [`AdapterHealth::no_data`]
//! is deliberately absent from the error rate.

use crate::{AdapterCapabilities, AdapterHealth};
use serde::{Deserialize, Serialize};

/// How many requests before a rate is worth calling a grade.
///
/// Twenty is small, and it is the point at which one bad request stops moving
/// the rate by more than a few percent. Below it the honest answer is that
/// nothing has been established.
pub const MINIMUM_SAMPLE: u64 = 20;

/// Above this share of requests failing, a scan's silences stop being evidence.
const UNRELIABLE_FAILURE_RATE: f64 = 0.25;

/// Above this, the link works but is costing real time and hiding real data.
const MARGINAL_FAILURE_RATE: f64 = 0.08;

/// Above this, it is worth mentioning without changing strategy for it.
const WORKABLE_FAILURE_RATE: f64 = 0.02;

/// How much confidence the link itself has earned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitnessGrade {
    /// Not enough requests yet to say anything. The starting state, and never
    /// a euphemism for bad.
    Unmeasured,
    /// Almost everything asked for came back.
    Good,
    /// Occasional failures, none of them changing what a result means.
    Workable,
    /// Failing often enough that a silence might be the adapter rather than
    /// the vehicle.
    Marginal,
    /// Failing so often that an absence of findings establishes nothing.
    Unreliable,
}

impl FitnessGrade {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            FitnessGrade::Unmeasured => "unmeasured",
            FitnessGrade::Good => "good",
            FitnessGrade::Workable => "workable",
            FitnessGrade::Marginal => "marginal",
            FitnessGrade::Unreliable => "unreliable",
        }
    }

    /// What it means, in language meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            FitnessGrade::Unmeasured => {
                "Not enough has been asked of this adapter yet to say how well it is working."
            }
            FitnessGrade::Good => {
                "Nearly everything asked for is coming back. A module that stays silent through \
                 this link is a module that is genuinely not answering."
            }
            FitnessGrade::Workable => {
                "The occasional request fails. Not often enough to change what a result means, \
                 and worth knowing about."
            }
            FitnessGrade::Marginal => {
                "Requests are failing often enough that a silence may be this link rather than \
                 the vehicle. Read less, allow longer, and treat a clean result as weaker \
                 evidence than it looks."
            }
            FitnessGrade::Unreliable => {
                "So many requests are failing that finding nothing establishes nothing. Fix the \
                 connection before drawing conclusions from anything it reports."
            }
        }
    }

    /// Whether an absence of findings can be read as an absence of faults.
    ///
    /// The question this whole type exists to answer. A clean scan on a good
    /// link is evidence; the same scan on a failing one is the link's silence
    /// wearing the vehicle's clothes.
    pub fn silence_is_evidence(&self) -> bool {
        matches!(self, FitnessGrade::Good | FitnessGrade::Workable)
    }
}

/// What has been measured about this adapter, and what follows from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterFitness {
    /// How much confidence the link has earned.
    pub grade: FitnessGrade,
    /// That, in a sentence.
    pub summary: String,
    /// Requests issued this connection — the sample everything rests on.
    pub requests: u64,
    /// Share of requests that timed out, 0.0 to 1.0.
    pub timeout_rate: f64,
    /// Share the adapter itself refused or failed, 0.0 to 1.0.
    pub error_rate: f64,
    /// The two together, which is what the grade is drawn from.
    pub failure_rate: f64,
    /// Mean round-trip time in milliseconds.
    pub mean_latency_ms: f64,
    /// Requests per second this link sustains, when enough have been timed.
    pub requests_per_second: Option<f64>,
    /// Whether a configuration write could be sent at all.
    pub can_write: bool,
    /// Whether the second CAN bus is reachable.
    pub reaches_second_bus: bool,
    /// Whether an absence of findings means anything.
    pub silence_is_evidence: bool,
    /// What a caller should do differently because of all this.
    ///
    /// Empty on a good link. Present, specific and actionable otherwise —
    /// advice nobody can act on is decoration.
    pub advice: Vec<String>,
}

impl AdapterFitness {
    /// Assess a link from what it has actually done.
    pub fn assess(health: &AdapterHealth, caps: &AdapterCapabilities) -> AdapterFitness {
        let requests = health.requests;
        let rate = |n: u64| if requests == 0 { 0.0 } else { n as f64 / requests as f64 };

        // `no_data` is deliberately not a failure. A vehicle declining to
        // answer a parameter it does not implement is a correct answer, and
        // counting it here would grade a healthy adapter on a sparse vehicle as
        // broken.
        let timeout_rate = rate(health.timeouts);
        let error_rate = rate(health.adapter_errors);
        let failure_rate = rate(health.timeouts + health.adapter_errors);

        let grade = if requests < MINIMUM_SAMPLE {
            FitnessGrade::Unmeasured
        } else if failure_rate > UNRELIABLE_FAILURE_RATE {
            FitnessGrade::Unreliable
        } else if failure_rate > MARGINAL_FAILURE_RATE {
            FitnessGrade::Marginal
        } else if failure_rate > WORKABLE_FAILURE_RATE {
            FitnessGrade::Workable
        } else {
            FitnessGrade::Good
        };

        let requests_per_second = (health.mean_latency_ms > 0.0 && requests >= MINIMUM_SAMPLE)
            .then(|| 1000.0 / health.mean_latency_ms);

        let mut advice = Vec::new();
        match grade {
            FitnessGrade::Marginal | FitnessGrade::Unreliable => {
                advice.push(String::from(
                    "Read fewer signals at a time and allow longer per request: this link is \
                     losing enough of them that a silence is ambiguous.",
                ));
                advice.push(String::from(
                    "Do not report a clean scan as a clean vehicle. Say that the link was \
                     failing and that a fault could have been missed.",
                ));
            }
            FitnessGrade::Unmeasured => advice.push(String::from(
                "Nothing has been established about this link yet. Treat early results as \
                 provisional until more has been asked of it.",
            )),
            _ => {}
        }
        if !caps.supports_long_messages {
            advice.push(String::from(
                "This adapter cannot send a request longer than one CAN frame, so no \
                 configuration change is possible with it. Reading is unaffected.",
            ));
        }
        if !caps.multiple_can_buses {
            advice.push(String::from(
                "Only one CAN bus is reachable. Modules on a second bus will not answer, and \
                 their absence is not evidence that the vehicle lacks them.",
            ));
        }

        AdapterFitness {
            summary: Self::summarise(grade, requests, failure_rate),
            grade,
            requests,
            timeout_rate,
            error_rate,
            failure_rate,
            mean_latency_ms: health.mean_latency_ms,
            requests_per_second,
            can_write: caps.supports_long_messages && caps.supports_transmit,
            reaches_second_bus: caps.multiple_can_buses,
            silence_is_evidence: grade.silence_is_evidence(),
            advice,
        }
    }

    /// One sentence carrying the grade and the evidence behind it.
    ///
    /// The sample size is in the sentence on purpose: "good, over 14 requests"
    /// and "good, over 1,400" are different claims, and a reader who cannot see
    /// which one they have been given will assume the second.
    fn summarise(grade: FitnessGrade, requests: u64, failure_rate: f64) -> String {
        if requests < MINIMUM_SAMPLE {
            return format!(
                "Not established yet — {requests} request(s) so far, and a rate over that few \
                 says nothing."
            );
        }
        format!("{} {:.1}% of {requests} requests failed.", grade.explain(), failure_rate * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionState, TransportKind};

    fn health(requests: u64, timeouts: u64, errors: u64) -> AdapterHealth {
        let mut h = AdapterHealth::new(ConnectionState::Ready);
        h.requests = requests;
        h.responses = requests - timeouts - errors;
        h.timeouts = timeouts;
        h.adapter_errors = errors;
        h.mean_latency_ms = 100.0;
        h
    }

    fn caps(long: bool, buses: bool) -> AdapterCapabilities {
        let mut c = AdapterCapabilities::unknown(TransportKind::Usb);
        c.supports_transmit = true;
        c.supports_long_messages = long;
        c.multiple_can_buses = buses;
        c
    }

    /// The most dangerous thing this could do is turn four requests into a
    /// verdict. Below the sample floor the answer is that nothing is known.
    #[test]
    fn a_handful_of_requests_is_not_a_grade() {
        let f = AdapterFitness::assess(&health(4, 4, 0), &caps(true, true));
        assert_eq!(f.grade, FitnessGrade::Unmeasured);
        assert!(!f.silence_is_evidence, "and an unmeasured link proves nothing either");
        assert!(f.summary.contains("says nothing"), "{}", f.summary);
    }

    /// A working link, and the thing that follows from it: a module that stays
    /// quiet through this is genuinely quiet.
    #[test]
    fn a_clean_link_makes_silence_mean_something() {
        let f = AdapterFitness::assess(&health(200, 1, 0), &caps(true, true));
        assert_eq!(f.grade, FitnessGrade::Good);
        assert!(f.silence_is_evidence);
        assert!(f.advice.is_empty(), "nothing to do differently: {:?}", f.advice);
        assert_eq!(f.requests_per_second, Some(10.0));
    }

    /// And a failing one, where the advice has to be specific enough to act on.
    #[test]
    fn a_failing_link_says_what_to_do_about_it() {
        let f = AdapterFitness::assess(&health(200, 60, 10), &caps(true, true));
        assert_eq!(f.grade, FitnessGrade::Unreliable);
        assert!(!f.silence_is_evidence);
        assert!(
            f.advice.iter().any(|a| a.contains("clean scan")),
            "the conclusion that must not be drawn: {:?}",
            f.advice
        );
    }

    /// `NO DATA` is a vehicle correctly declining to answer a parameter it does
    /// not implement. Counting it as a failure would grade a healthy adapter on
    /// a sparse vehicle as broken.
    #[test]
    fn a_vehicle_saying_no_data_is_not_the_adapter_failing() {
        let mut h = health(200, 0, 0);
        h.no_data = 150;
        let f = AdapterFitness::assess(&h, &caps(true, true));
        assert_eq!(f.grade, FitnessGrade::Good);
        assert_eq!(f.failure_rate, 0.0);
    }

    /// What today's two adapters actually looked like, so the thresholds are
    /// anchored to hardware rather than to taste.
    #[test]
    fn the_two_adapters_this_was_written_against_grade_differently() {
        // The clone: refused every long request outright, single bus.
        let clone = AdapterFitness::assess(&health(200, 6, 4), &caps(false, false));
        assert!(!clone.can_write, "it could not send a write at all");
        assert!(clone.advice.iter().any(|a| a.contains("configuration change")));
        assert!(clone.advice.iter().any(|a| a.contains("second bus")));

        // The MX+: writes work, and its absence of findings is worth something.
        let mx = AdapterFitness::assess(&health(200, 1, 0), &caps(true, false));
        assert!(mx.can_write);
        assert!(mx.silence_is_evidence);
    }

    /// The sample size travels with the grade. "Good over 20 requests" and
    /// "good over 2000" are different claims and a reader who cannot see which
    /// they were given will assume the stronger one.
    #[test]
    fn the_summary_carries_the_sample_it_rests_on() {
        let f = AdapterFitness::assess(&health(37, 0, 0), &caps(true, true));
        assert!(f.summary.contains("37"), "{}", f.summary);
    }
}
