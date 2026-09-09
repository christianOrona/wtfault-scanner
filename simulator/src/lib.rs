//! `aim-simulator` — a deterministic virtual vehicle that speaks ELM327.
//!
//! Handoff §17 item 3: *"Implement fake adapter + virtual vehicle before deep
//! hardware work."* This crate is that, and it is the reason every layer above
//! it can be tested on a laptop with no truck attached.
//!
//! # Layers
//!
//! | Module | Role |
//! |--------|------|
//! | [`state`] | Physics: rpm, temperatures, pressures, and their SAE encodings. |
//! | [`scenario`] | Injectable behaviours: healthy, particulate-filter regeneration, silent bus. |
//! | [`vehicle`] | Virtual ECUs answering OBD-II services at standard addresses. |
//! | [`elm`] | An ELM327 that answers in text, quirks and all. |
//! | [`transport`] | [`SimulatedTransport`], the in-process link. |
//! | [`replay`] | Golden-transcript playback from files. |
//!
//! # The rule this crate obeys
//!
//! Everything simulated is generic SAE J1979 at standard ISO 15765-4
//! addresses. No Ford-specific module address, PID or service is modelled,
//! because none has been validated. A simulator that invented them would
//! manufacture confidence in code that has never met a truck.

#![warn(missing_docs)]

pub mod elm;
pub mod replay;
pub mod scenario;
pub mod state;
pub mod transport;
pub mod vehicle;

pub use elm::{AdapterPersonality, ElmEmulator, InjectedFault};
pub use replay::{Exchange, ReplayMode, ReplayTransport, Transcript};
pub use scenario::{Scenario, ScenarioId, SimDtc, SimMonitorTest};
pub use state::VehicleState;
pub use transport::{SharedEmulator, SimulatedTransport};
pub use vehicle::{
    ConfigWriteBehaviour, EcuReply, TimeSource, VirtualEcu, VirtualVehicle, SIMULATED_VIN,
};
