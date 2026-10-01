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

/// What kind of engine a vehicle has, as far as a measurement cares.
///
/// Read from what the engine says it burns (service 01 PID 0x51), never
/// inferred from the make or the VIN. Some signals exist only on one kind:
/// fuel trims are a correction around a petrol engine's stoichiometric target,
/// and a diesel does not run to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Petrol, alcohol and gas engines: spark ignition, run to a
    /// stoichiometric mixture, so they report fuel trims.
    SparkIgnition,
    /// Compression ignition. Runs lean of stoichiometric, so has no fuel trims.
    Diesel,
    /// No combustion engine at all.
    Electric,
}

impl Engine {
    /// The engine a PID 0x51 fuel type describes.
    ///
    /// `None` for "Not available", for anything unrecognised, and for the
    /// fuel types that name both an electric motor and a combustion engine
    /// without saying which is running: guessing there would be choosing
    /// which measurements a vehicle gets.
    pub fn from_fuel_type(fuel_type: &str) -> Option<Engine> {
        match fuel_type.trim().to_ascii_lowercase().as_str() {
            "gasoline"
            | "methanol"
            | "ethanol"
            | "lpg"
            | "cng"
            | "propane"
            | "bifuel running gasoline"
            | "bifuel running methanol"
            | "bifuel running ethanol"
            | "bifuel running lpg"
            | "bifuel running cng"
            | "bifuel running propane"
            | "hybrid gasoline"
            | "hybrid ethanol" => Some(Engine::SparkIgnition),
            "diesel" | "hybrid diesel" | "bifuel running diesel" => Some(Engine::Diesel),
            "electric" | "bifuel running electricity" | "hybrid electric" => Some(Engine::Electric),
            _ => None,
        }
    }

    /// How a person would say it.
    pub fn describe(&self) -> &'static str {
        match self {
            Engine::SparkIgnition => "a petrol (spark-ignition) engine",
            Engine::Diesel => "a diesel",
            Engine::Electric => "an electric vehicle",
        }
    }

    /// Whether this kind of engine can produce `signal` at all.
    ///
    /// Only the cases this build knows to be structural are listed. Anything
    /// else is assumed possible, and the vehicle gets to say otherwise.
    pub fn has_signal(&self, signal: &str) -> bool {
        let fuel_trim =
            signal.starts_with("short_fuel_trim") || signal.starts_with("long_fuel_trim");
        !(fuel_trim && *self != Engine::SparkIgnition)
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
    /// The engines this procedure means anything on. Empty means any.
    ///
    /// Checked against what the engine reports before anybody is asked to do
    /// anything. `warm_idle` exists for fuel trims, and on a diesel it once
    /// walked somebody to a warm engine, measured none, and reported success.
    #[serde(default)]
    pub engines: Vec<Engine>,
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

    /// Whether this means anything on `engine`.
    pub fn applies_to(&self, engine: Engine) -> bool {
        self.engines.is_empty() || self.engines.contains(&engine)
    }

    /// Why it does not apply to `engine`, when it does not.
    pub fn why_not_on(&self, engine: Engine) -> Option<String> {
        (!self.applies_to(engine)).then(|| {
            let fits: Vec<&str> = self.engines.iter().map(Engine::describe).collect();
            format!(
                "This engine reports that it is {}, and this procedure only means something on {}. \
                 Nothing was asked of you and nothing was measured. {}",
                engine.describe(),
                fits.join(" or "),
                self.purpose
            )
        })
    }

    /// What it measures that `engine` cannot produce. Not a fault, and not
    /// counted against the measurement being complete.
    pub fn not_on(&self, engine: Engine) -> Vec<String> {
        self.measure.iter().filter(|s| !engine.has_signal(s)).cloned().collect()
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
                    String::from("short_fuel_trim_b1"),
                    String::from("long_fuel_trim_b1"),
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
                // Any engine that turns. Its fuel trims only exist on petrol,
                // and a diesel is measured on the rest.
                engines: vec![Engine::SparkIgnition, Engine::Diesel],
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
                    String::from("short_fuel_trim_b1"),
                    String::from("long_fuel_trim_b1"),
                    String::from("intake_air_temp"),
                    String::from("maf_rate"),
                ],
                hold_seconds: 15,
                safety_notes: vec![String::from(
                    "Handbrake on. Do this outdoors or with extraction.",
                )],
                // It exists for fuel trims, which only a petrol engine has.
                engines: vec![Engine::SparkIgnition],
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
                engines: Vec::new(),
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
    /// Means nothing on this engine, so nothing was asked or measured.
    DoesNotApply,
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
            engines: vec![],
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

    /// The fuel types PID 0x51 can report, sorted into engines. Read from the
    /// strings the decoder produces, so a renamed value fails here.
    #[test]
    fn a_fuel_type_names_an_engine_or_admits_it_cannot() {
        assert_eq!(Engine::from_fuel_type("Diesel"), Some(Engine::Diesel));
        assert_eq!(Engine::from_fuel_type("Gasoline"), Some(Engine::SparkIgnition));
        assert_eq!(Engine::from_fuel_type("Hybrid gasoline"), Some(Engine::SparkIgnition));
        assert_eq!(Engine::from_fuel_type("Bifuel running diesel"), Some(Engine::Diesel));
        assert_eq!(Engine::from_fuel_type("Electric"), Some(Engine::Electric));
        assert_eq!(Engine::from_fuel_type("Not available"), None);
        // Both at once, and nothing says which is running.
        assert_eq!(Engine::from_fuel_type("Hybrid running electric and combustion engine"), None);
    }

    /// Every value the shipped decoder can produce for PID 0x51 is either an
    /// engine or one of the deliberate unknowns. A value added to the profile
    /// without a decision here fails rather than quietly running everything.
    #[test]
    fn every_fuel_type_the_decoder_knows_has_been_decided() {
        let deliberately_unknown = [
            "Not available",
            "Bifuel running electric and combustion engine",
            "Hybrid running electric and combustion engine",
            "Hybrid Regenerative",
        ];
        let decoders = aim_decoders::DecoderSet::generic_obd().expect("generic decoders load");
        let mut undecided = Vec::new();
        for byte in 0..=0xFFu8 {
            let Ok(values) = decoders.pids.decode(0x01, 0x51, &[byte], aim_types::now()) else {
                continue;
            };
            for value in values {
                if let aim_types::Value::Text(text) = &value.value {
                    if !text.starts_with("unmapped value")
                        && Engine::from_fuel_type(text).is_none()
                        && !deliberately_unknown.contains(&text.as_str())
                    {
                        undecided.push(format!("{byte}: {text}"));
                    }
                }
            }
        }
        assert!(undecided.is_empty(), "fuel types with no engine decided: {undecided:?}");
    }

    /// The case that started this: `warm_idle` on a diesel is not run at all.
    #[test]
    fn warm_idle_does_not_apply_to_a_diesel() {
        let warm_idle = Procedure::by_id("warm_idle").unwrap();
        assert!(warm_idle.applies_to(Engine::SparkIgnition));
        assert!(!warm_idle.applies_to(Engine::Diesel));
        let why = warm_idle.why_not_on(Engine::Diesel).unwrap();
        assert!(why.contains("diesel"), "{why}");
        assert!(why.contains("nothing was measured"), "{why}");
    }

    /// A diesel can still hold 2500 rpm; it is measured on what it has, and
    /// the fuel trims it cannot produce are named rather than counted missing.
    #[test]
    fn a_diesel_holding_2500_is_not_asked_for_fuel_trims() {
        let steady = Procedure::by_id("steady_rpm_2500").unwrap();
        assert!(steady.applies_to(Engine::Diesel));
        assert_eq!(steady.not_on(Engine::Diesel), ["short_fuel_trim_b1", "long_fuel_trim_b1"]);
        assert!(steady.not_on(Engine::SparkIgnition).is_empty());
        assert!(!steady.applies_to(Engine::Electric), "nothing to hold at 2500 rpm");
    }

    /// A procedure with no declaration applies everywhere.
    #[test]
    fn key_on_engine_off_applies_to_every_engine() {
        let p = Procedure::by_id("key_on_engine_off").unwrap();
        for engine in [Engine::SparkIgnition, Engine::Diesel, Engine::Electric] {
            assert!(p.applies_to(engine));
            assert!(p.why_not_on(engine).is_none());
        }
    }
}

#[cfg(test)]
mod catalogue_tests {
    use super::Procedure;

    /// Every signal a procedure claims to measure must actually exist.
    ///
    /// `warm_idle` shipped naming `short_term_fuel_trim_1` and
    /// `long_term_fuel_trim_1`. Neither is a signal id — the catalogue calls
    /// them `short_fuel_trim_b1` and `long_fuel_trim_b1` — so the procedure
    /// whose stated purpose is "fuel trims at a warm idle" measured no fuel
    /// trims at all. It reported success anyway, on a warm engine, on a real
    /// truck, having established nothing it exists to establish.
    ///
    /// Nothing caught it because nothing compared the two lists. This does.
    #[test]
    fn every_procedure_measures_signals_that_exist() {
        let decoders = aim_decoders::DecoderSet::generic_obd().expect("generic decoders load");
        let mut unknown = Vec::new();

        for procedure in Procedure::built_in() {
            for signal in &procedure.measure {
                if decoders.pids.address_of(signal).is_none() {
                    unknown.push(format!("{}: {signal}", procedure.id));
                }
            }
        }

        assert!(
            unknown.is_empty(),
            "these procedures name signals that do not exist, so they would run and measure \
             nothing: {unknown:?}"
        );
    }

    /// A procedure's preconditions are read from the vehicle too, and the same
    /// mistake would be just as quiet there — a condition on a signal that does
    /// not exist can never be met, so the procedure would sit at `waiting`
    /// forever with no explanation.
    #[test]
    fn every_precondition_watches_a_signal_that_exists() {
        let decoders = aim_decoders::DecoderSet::generic_obd().expect("generic decoders load");
        let mut unknown = Vec::new();

        for procedure in Procedure::built_in() {
            for condition in &procedure.conditions {
                if let Some(signal) = condition.signal() {
                    if decoders.pids.address_of(signal).is_none() {
                        unknown.push(format!("{}: {signal}", procedure.id));
                    }
                }
            }
        }

        assert!(
            unknown.is_empty(),
            "preconditions watching signals that do not exist: {unknown:?}"
        );
    }
}
