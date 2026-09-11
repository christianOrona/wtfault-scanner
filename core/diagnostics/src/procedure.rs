//! Putting the vehicle into a state, and measuring it while it is there.
//!
//! # What this changes
//!
//! Everything else in this application reads whatever is happening. Some
//! diagnoses need the vehicle in a state only the person sitting in it can
//! produce — a cold start, a steady 2500 rpm, the engine off but the ignition
//! on. Without that, the app can only report what it happened to catch.
//!
//! A procedure asks for the state, watches for it, and records what was
//! measured *while the condition held* — with the condition itself in the
//! flight recorder, because "oil pressure was 42 psi" and "oil pressure was 42
//! psi at a steady 2500 rpm with the engine at temperature" are different
//! findings and only the second is evidence.
//!
//! # The safety rule, which is structural rather than advisory
//!
//! A procedure declares what it needs through [`Precondition`], and a
//! precondition that cannot be met safely by one person with a laptop is
//! refused by construction rather than by a warning somebody may not read.
//! [`Precondition::RoadSpeed`] exists so that a procedure needing it can be
//! *described* — and [`Procedure::safe_for_one_person`] is false, so it will
//! not run. The app says a second person or a rolling road is required and
//! stops there.
//!
//! Nothing here ever asks somebody to read a screen while driving.
//!
//! # Why measurement is separate from instruction
//!
//! The person is told what to do; the vehicle is asked whether it is done.
//! A procedure never takes somebody's word for the state — "I'm holding 2500"
//! is a claim, and engine speed is a reading. Where the vehicle cannot report
//! the condition, the procedure says so and records it as stated rather than
//! measured.

use aim_types::Timestamp;
use serde::{Deserialize, Serialize};

/// A condition the vehicle has to be in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Precondition {
    /// Engine speed within a band, in rpm.
    EngineSpeed {
        /// Inclusive lower bound.
        min: f64,
        /// Inclusive upper bound.
        max: f64,
    },
    /// Coolant temperature at or above a threshold, in °C.
    CoolantAtLeast {
        /// Inclusive lower bound.
        min: f64,
    },
    /// Coolant temperature at or below a threshold, in °C. A cold start.
    CoolantAtMost {
        /// Inclusive upper bound.
        max: f64,
    },
    /// The engine turning at all, or not.
    EngineRunning {
        /// Whether it must be running.
        running: bool,
    },
    /// The vehicle stationary.
    Stationary,
    /// The vehicle at a road speed. **Never runnable by one person.**
    RoadSpeed {
        /// Inclusive lower bound, km/h.
        min: f64,
        /// Inclusive upper bound, km/h.
        max: f64,
    },
}

impl Precondition {
    /// The signal that establishes this, when the vehicle can report one.
    ///
    /// `None` means the condition cannot be measured and would have to be
    /// taken on somebody's word — which a procedure records as *stated*, never
    /// as observed.
    pub fn signal(&self) -> Option<&'static str> {
        match self {
            Precondition::EngineSpeed { .. } => Some("engine_rpm"),
            Precondition::EngineRunning { .. } => Some("engine_rpm"),
            Precondition::CoolantAtLeast { .. } | Precondition::CoolantAtMost { .. } => {
                Some("coolant_temp")
            }
            Precondition::Stationary | Precondition::RoadSpeed { .. } => Some("vehicle_speed"),
        }
    }

    /// Whether a reading satisfies this condition.
    pub fn satisfied_by(&self, value: f64) -> bool {
        match self {
            Precondition::EngineSpeed { min, max } => value >= *min && value <= *max,
            Precondition::CoolantAtLeast { min } => value >= *min,
            Precondition::CoolantAtMost { max } => value <= *max,
            // A stopped engine reads zero; anything above idle-cranking is
            // running. The threshold is low on purpose — this distinguishes
            // turning from not turning, not idle from load.
            Precondition::EngineRunning { running } => (value > 200.0) == *running,
            Precondition::Stationary => value <= 1.0,
            Precondition::RoadSpeed { min, max } => value >= *min && value <= *max,
        }
    }

    /// What to tell the person to do.
    pub fn instruction(&self) -> String {
        match self {
            Precondition::EngineSpeed { min, max } => format!(
                "Hold the engine steady between {min:.0} and {max:.0} rpm. Use the accelerator \
                 with the vehicle stationary and in neutral or park."
            ),
            Precondition::CoolantAtLeast { min } => format!(
                "Let the engine reach at least {min:.0} °C. That is normal running temperature \
                 — a few minutes of idling, or a short drive."
            ),
            Precondition::CoolantAtMost { max } => format!(
                "The engine needs to be at or below {max:.0} °C, which in practice means a cold \
                 start: leave it overnight, or several hours."
            ),
            Precondition::EngineRunning { running: true } => String::from("Start the engine."),
            Precondition::EngineRunning { running: false } => {
                String::from("Turn the engine off, leaving the ignition on.")
            }
            Precondition::Stationary => {
                String::from("Bring the vehicle to a complete stop and leave it stopped.")
            }
            Precondition::RoadSpeed { min, max } => format!(
                "The vehicle has to be moving between {min:.0} and {max:.0} km/h. This app will \
                 not walk you through that: it needs somebody else driving while you watch the \
                 screen, or a rolling road."
            ),
        }
    }

    /// Whether one person with a laptop can do this safely.
    ///
    /// Road speed cannot. Everything else here is done stationary with the
    /// handbrake on.
    pub fn safe_for_one_person(&self) -> bool {
        !matches!(self, Precondition::RoadSpeed { .. })
    }
}

/// A test the app can walk somebody through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    /// Stable id.
    pub id: String,
    /// Short name.
    pub name: String,
    /// What it establishes, and why it is worth doing.
    pub purpose: String,
    /// The state the vehicle has to be in.
    pub conditions: Vec<Precondition>,
    /// What to read while the conditions hold.
    pub measure: Vec<String>,
    /// How long the conditions must hold before the reading counts, in seconds.
    ///
    /// A single sample the instant a needle crosses a threshold is noise. The
    /// window is what turns it into a measurement.
    pub hold_seconds: u64,
    /// Anything a person must know before starting.
    pub safety_notes: Vec<String>,
}

impl Procedure {
    /// Whether this can be run by one person with a laptop.
    ///
    /// Checked before a procedure is offered, not after it has begun. A
    /// procedure that needs road speed is described and refused — the person
    /// is told what it would take rather than led into doing it alone.
    pub fn safe_for_one_person(&self) -> bool {
        self.conditions.iter().all(Precondition::safe_for_one_person)
    }

    /// Why it cannot be run alone, when it cannot.
    pub fn why_not_alone(&self) -> Option<String> {
        (!self.safe_for_one_person()).then(|| {
            String::from(
                "This needs the vehicle moving. Doing it alone would mean reading a screen while \
                 driving, which this app will not walk anybody through. It needs a second person \
                 driving, or a rolling road.",
            )
        })
    }

    /// The procedures this build ships.
    ///
    /// Deliberately few and all stationary. Each one exists because it
    /// establishes something a passive read cannot.
    pub fn built_in() -> Vec<Procedure> {
        vec![
            Procedure {
                id: String::from("steady_rpm_2500"),
                name: String::from("Hold 2500 rpm"),
                purpose: String::from(
                    "Reads the sensors under load rather than at idle. Several faults — a lazy \
                     oxygen sensor, a failing alternator, low fuel pressure — look perfectly \
                     normal at idle and only appear when the engine is asked to work.",
                ),
                conditions: vec![
                    Precondition::Stationary,
                    Precondition::EngineSpeed { min: 2300.0, max: 2700.0 },
                ],
                measure: vec![
                    String::from("engine_rpm"),
                    String::from("coolant_temp"),
                    String::from("engine_load"),
                    String::from("short_term_fuel_trim_1"),
                    String::from("long_term_fuel_trim_1"),
                    String::from("control_module_voltage"),
                ],
                hold_seconds: 10,
                safety_notes: vec![
                    String::from(
                        "Handbrake on, gearbox in neutral or park, and nobody in front of the \
                         vehicle.",
                    ),
                    String::from(
                        "Do this outdoors or with extraction. A stationary engine held at 2500 \
                         rpm produces exhaust faster than a closed garage clears it.",
                    ),
                ],
            },
            Procedure {
                id: String::from("warm_idle"),
                name: String::from("Warm idle"),
                purpose: String::from(
                    "The baseline everything else is compared against. Fuel trims at a warm idle \
                     say whether the engine is running rich or lean once the sensors are in \
                     closed loop, which is not true when it is cold.",
                ),
                conditions: vec![
                    Precondition::Stationary,
                    Precondition::EngineRunning { running: true },
                    Precondition::CoolantAtLeast { min: 80.0 },
                ],
                measure: vec![
                    String::from("coolant_temp"),
                    String::from("short_term_fuel_trim_1"),
                    String::from("long_term_fuel_trim_1"),
                    String::from("intake_air_temp"),
                    String::from("maf_rate"),
                ],
                hold_seconds: 15,
                safety_notes: vec![String::from(
                    "Handbrake on. Do this outdoors or with extraction.",
                )],
            },
            Procedure {
                id: String::from("key_on_engine_off"),
                name: String::from("Key on, engine off"),
                purpose: String::from(
                    "What the electrical system looks like with everything awake and nothing \
                     charging. A battery that reads fine with the engine running can be flat \
                     here, and this is the state most configuration work needs anyway.",
                ),
                conditions: vec![
                    Precondition::Stationary,
                    Precondition::EngineRunning { running: false },
                ],
                measure: vec![String::from("control_module_voltage")],
                hold_seconds: 5,
                safety_notes: vec![String::from(
                    "Ignition on, engine not started. The dash will light up and that is \
                     expected.",
                )],
            },
        ]
    }

    /// One built-in procedure by id.
    pub fn by_id(id: &str) -> Option<Procedure> {
        Procedure::built_in().into_iter().find(|p| p.id == id)
    }
}

/// Where a procedure has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureState {
    /// Conditions are not met. The person is being asked to change something.
    Waiting,
    /// Conditions are met and the window is running.
    Holding,
    /// The window completed and readings were taken under it.
    Measured,
    /// The conditions broke before the window completed.
    Lost,
    /// Refused before it began.
    Refused,
}

/// One check of whether the vehicle is where it needs to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionCheck {
    /// Which condition.
    pub condition: Precondition,
    /// What the person is being asked to do about it.
    pub instruction: String,
    /// The signal consulted, when there is one.
    pub signal: Option<String>,
    /// What it read.
    pub value: Option<f64>,
    /// Whether the condition holds.
    pub met: bool,
    /// Why it could not be established, when it could not.
    pub unmeasurable: Option<String>,
}

/// What a procedure run produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureRun {
    /// The procedure.
    pub procedure_id: String,
    /// Where it got to.
    pub state: ProcedureState,
    /// Every condition and how it stood at the last check.
    pub conditions: Vec<ConditionCheck>,
    /// When the conditions were first all met.
    pub holding_since: Option<Timestamp>,
    /// How long they have held, in seconds.
    pub held_seconds: u64,
    /// How long they must hold.
    pub hold_seconds: u64,
    /// What to tell the person right now.
    pub next_step: String,
}

impl ProcedureRun {
    /// Whether every condition is currently satisfied.
    pub fn all_met(&self) -> bool {
        !self.conditions.is_empty() && self.conditions.iter().all(|c| c.met)
    }

    /// The first thing standing in the way, if anything is.
    ///
    /// One instruction at a time. A person given five things to change at once
    /// does none of them, and the conditions are ordered so the first is the
    /// one to do first.
    pub fn blocking(&self) -> Option<&ConditionCheck> {
        self.conditions.iter().find(|c| !c.met)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The safety rule, and it is structural: a procedure needing road speed
    /// is refused by construction rather than by a warning somebody may skip.
    #[test]
    fn a_procedure_needing_road_speed_cannot_be_run_alone() {
        let p = Procedure {
            id: "rolling".into(),
            name: "Rolling test".into(),
            purpose: "x".into(),
            conditions: vec![Precondition::RoadSpeed { min: 50.0, max: 70.0 }],
            measure: vec![],
            hold_seconds: 10,
            safety_notes: vec![],
        };
        assert!(!p.safe_for_one_person());
        let why = p.why_not_alone().expect("it says why");
        assert!(why.contains("second person") || why.contains("rolling road"), "{why}");
        assert!(why.contains("will not walk anybody through"), "{why}");
    }

    /// And everything shipped can be. All of it is done stationary.
    #[test]
    fn every_built_in_procedure_is_safe_for_one_person() {
        for p in Procedure::built_in() {
            assert!(p.safe_for_one_person(), "{} is not", p.id);
            assert!(p.why_not_alone().is_none());
            assert!(
                p.conditions.contains(&Precondition::Stationary),
                "{} does not require the vehicle stopped",
                p.id
            );
        }
    }

    /// Every shipped procedure warns about the things that actually hurt
    /// people: a vehicle that can move, and exhaust in a closed space.
    #[test]
    fn a_procedure_that_runs_the_engine_says_so() {
        for p in Procedure::built_in() {
            let running = p.conditions.iter().any(|c| {
                matches!(c, Precondition::EngineSpeed { .. })
                    || matches!(c, Precondition::EngineRunning { running: true })
                    || matches!(c, Precondition::CoolantAtLeast { .. })
            });
            if !running {
                continue;
            }
            let notes = p.safety_notes.join(" ").to_ascii_lowercase();
            assert!(notes.contains("handbrake"), "{}: {notes}", p.id);
            assert!(
                notes.contains("outdoors") || notes.contains("extraction"),
                "{}: exhaust in a closed garage: {notes}",
                p.id
            );
        }
    }

    /// Conditions are judged by reading the vehicle, never by asking.
    #[test]
    fn a_condition_is_settled_by_a_reading() {
        let hold = Precondition::EngineSpeed { min: 2300.0, max: 2700.0 };
        assert_eq!(hold.signal(), Some("engine_rpm"));
        assert!(hold.satisfied_by(2500.0));
        assert!(!hold.satisfied_by(1800.0));
        assert!(!hold.satisfied_by(3200.0));
    }

    /// Running and not running are distinguished by whether the engine is
    /// turning at all, not by whether it is at some particular speed.
    #[test]
    fn engine_running_distinguishes_turning_from_stopped() {
        let off = Precondition::EngineRunning { running: false };
        assert!(off.satisfied_by(0.0));
        assert!(!off.satisfied_by(750.0));

        let on = Precondition::EngineRunning { running: true };
        assert!(on.satisfied_by(750.0));
        assert!(!on.satisfied_by(0.0));
    }

    /// One instruction at a time. Somebody given five things to change at once
    /// does none of them.
    #[test]
    fn only_the_first_unmet_condition_is_asked_for() {
        let run = ProcedureRun {
            procedure_id: "x".into(),
            state: ProcedureState::Waiting,
            conditions: vec![
                ConditionCheck {
                    condition: Precondition::Stationary,
                    instruction: "stop".into(),
                    signal: Some("vehicle_speed".into()),
                    value: Some(0.0),
                    met: true,
                    unmeasurable: None,
                },
                ConditionCheck {
                    condition: Precondition::EngineSpeed { min: 2300.0, max: 2700.0 },
                    instruction: "hold 2500".into(),
                    signal: Some("engine_rpm".into()),
                    value: Some(800.0),
                    met: false,
                    unmeasurable: None,
                },
            ],
            holding_since: None,
            held_seconds: 0,
            hold_seconds: 10,
            next_step: "hold 2500".into(),
        };
        assert!(!run.all_met());
        assert_eq!(run.blocking().unwrap().instruction, "hold 2500");
    }

    /// The instruction for a cold start says what cold actually means, because
    /// "cold" to a person means "not hot" and to a thermostat means overnight.
    #[test]
    fn a_cold_start_says_how_long_that_takes() {
        let cold = Precondition::CoolantAtMost { max: 30.0 };
        let text = cold.instruction();
        assert!(text.contains("overnight") || text.contains("several hours"), "{text}");
    }
}
