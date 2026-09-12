//! `aim-adapter` — the adapter abstraction and the ELM327 implementation.
//!
//! Handoff §4 asks for one thing above all else: *"Design the adapter layer so
//! its limitations produce capability flags rather than app-wide assumptions."*
//! That is why [`DiagnosticAdapter`] hands back an [`AdapterCapabilities`]
//! snapshot that the rest of the stack consults, and why nothing above this
//! layer is allowed to assume 29-bit CAN, multiple buses, or long messages.
//!
//! # What lives here
//!
//! * [`DiagnosticAdapter`] — the seam. A future J2534 or STN adapter implements
//!   this trait and nothing above it changes.
//! * [`Elm327Adapter`] — the implementation for ELM327-class devices, driving
//!   any [`aim_transport::Transport`]: a Windows Bluetooth COM port, a Linux
//!   `/dev/rfcomm0`, or the in-process simulator.
//! * [`AdapterObserver`] — the flight-recorder hook. Every command, every
//!   reply, every state change and every failure is announced here so the
//!   session store can persist it without the adapter knowing SQLite exists.
//! * [`probe`] — "is there an ELM327 on this port?" identification.
//!
//! # Requests are messages, not lines
//!
//! [`DiagnosticAdapter::request`] returns [`EcuMessage`]s: one reassembled
//! payload per responding ECU. The caller never sees adapter text, and a
//! functional (broadcast) request that four modules answer produces four
//! messages rather than a pile of interleaved lines.

#![warn(missing_docs)]

pub mod elm327;
pub mod probe;
pub mod response;

pub use elm327::{Elm327Adapter, Elm327Config};
pub use probe::{identify_transport, Identification};
pub use response::{parse, AdapterResponse, ResponseClass};

use aim_types::{
    AdapterCapabilities, AdapterHealth, AimError, AimResult, ConnectionState, ErrorCode,
    ObdProtocol,
};
use std::sync::Arc;

/// Which ECU a request is addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTarget {
    /// Broadcast to every emissions-related ECU.
    ///
    /// Carries no address of its own: the broadcast identifier belongs to the
    /// negotiated protocol (`7DF` on 11-bit CAN, `18DB33F1` on 29-bit, and
    /// different again on the K-line protocols), so the adapter looks it up
    /// rather than this enum naming one.
    Functional,
    /// A single ECU, addressed by its request identifier, e.g. `7E0`.
    ///
    /// The string is the adapter-level header, not a module name — the
    /// diagnostics layer maps names to addresses, not this one.
    Physical(String),
}

impl RequestTarget {
    /// Physical target derived from the *response* address an ECU used.
    ///
    /// A module that answered on `7E8` is addressed on `7E0`. Returns `None`
    /// when the response address has no conventional request counterpart, in
    /// which case the caller must keep using functional addressing rather than
    /// guessing.
    pub fn from_response_address(address: &str) -> Option<RequestTarget> {
        let id = aim_protocols::CanId::parse_hex(address).ok()?;
        id.obd_response_to_request().map(|req| RequestTarget::Physical(req.to_hex()))
    }
}

/// One fully reassembled response from one ECU.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EcuMessage {
    /// Source address as the adapter reported it, e.g. `7E8`.
    pub address: String,
    /// The reassembled diagnostic payload (service byte first). ISO-TP PCI
    /// bytes have been consumed and are not present.
    pub payload: Vec<u8>,
    /// The adapter lines this message was assembled from, kept verbatim for
    /// the flight recorder.
    pub raw_lines: Vec<String>,
}

impl EcuMessage {
    /// Lowercase hex of the payload, the canonical raw-evidence form.
    pub fn payload_hex(&self) -> String {
        aim_types::hex(&self.payload)
    }
}

/// Flight-recorder hook.
///
/// The adapter announces everything it does here. `aim-session` implements this
/// to write the append-only event log; tests implement it to assert on traffic.
/// Implementations must not block for long — they are called inline on the
/// diagnostic thread.
pub trait AdapterObserver: Send + Sync {
    /// A command is about to be written to the transport.
    fn on_request(&self, command: &str);

    /// A reply was received and classified. Called for failures too — an
    /// adapter error is a recorded observation, never a silent drop.
    fn on_response(&self, response: &AdapterResponse);

    /// A transport-level failure occurred.
    fn on_failure(&self, command: Option<&str>, error: &AimError);

    /// The connection state machine moved.
    fn on_state_change(&self, from: &ConnectionState, to: &ConnectionState);

    /// Identification finished and capabilities are known.
    fn on_identified(&self, capabilities: &AdapterCapabilities);
}

/// An observer that discards everything. Default when no recorder is attached.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullObserver;

impl AdapterObserver for NullObserver {
    fn on_request(&self, _command: &str) {}
    fn on_response(&self, _response: &AdapterResponse) {}
    fn on_failure(&self, _command: Option<&str>, _error: &AimError) {}
    fn on_state_change(&self, _from: &ConnectionState, _to: &ConnectionState) {}
    fn on_identified(&self, _capabilities: &AdapterCapabilities) {}
}

/// A shareable observer handle.
pub type ObserverRef = Arc<dyn AdapterObserver>;

/// The adapter seam.
///
/// Everything above this trait — diagnostics, tools, API — is written against
/// it, so adding an OBDLink or J2534 backend later is an additive change.
pub trait DiagnosticAdapter: Send {
    /// Stable identity of the underlying link, e.g. `COM5` or `sim:dpf_regen`.
    fn descriptor(&self) -> String;

    /// Current position in the connection state machine.
    fn state(&self) -> ConnectionState;

    /// Observed capabilities. Before [`DiagnosticAdapter::connect`] this is the
    /// all-false "nothing is assumed" snapshot.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Rolling link health for the UI.
    fn health(&self) -> AdapterHealth;

    /// Attach the flight recorder. Called once the session it records into
    /// exists, which is necessarily after the adapter has been constructed.
    fn set_observer(&mut self, observer: ObserverRef);

    /// Negotiated vehicle protocol, [`ObdProtocol::Unknown`] until known.
    fn protocol(&self) -> ObdProtocol;

    /// Open the link, initialize the adapter and identify it.
    fn connect(&mut self) -> AimResult<()>;

    /// Close the link. Idempotent.
    fn disconnect(&mut self) -> AimResult<()>;

    /// Control-module voltage as the adapter measures it, if it can.
    fn read_battery_voltage(&mut self) -> AimResult<f64>;

    /// Issue an OBD-II request and return one message per responding ECU.
    ///
    /// An empty vector is never returned: nothing answering is
    /// [`aim_types::ErrorCode::NoData`], which is an error the caller must see.
    fn request(
        &mut self,
        request: &aim_protocols::ObdRequest,
        target: &RequestTarget,
    ) -> AimResult<Vec<EcuMessage>>;

    /// Send an arbitrary service PDU and collect what answers.
    ///
    /// The protocol-neutral primitive. [`DiagnosticAdapter::request`] is the
    /// OBD-II-shaped convenience over the top of it; UDS goes through here,
    /// and so would anything else that is "put these service bytes on the bus
    /// and tell me who replied".
    ///
    /// Separate from [`DiagnosticAdapter::raw_command`], which sends adapter
    /// commands (`ATZ`, `ATSP0`) rather than vehicle traffic.
    ///
    /// `timeout` is the caller's budget, because the right value is not a
    /// property of the adapter: a discovery sweep across 240 addresses wants a
    /// short one and is happy with silence, while a module reading its whole
    /// fault memory may legitimately take seconds.
    fn request_pdu(
        &mut self,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: std::time::Duration,
    ) -> AimResult<Vec<EcuMessage>>;

    /// Send a raw adapter command. Escape hatch for diagnostics and probing;
    /// the safety gate is what decides whether a caller may reach it.
    fn raw_command(&mut self, command: &str) -> AimResult<AdapterResponse>;

    /// Move to a different vehicle bus.
    ///
    /// Vehicles run more than one CAN bus and the diagnostic connector exposes
    /// at least two of them: the high-speed bus everything standard lives on,
    /// and a slower one carrying body and comfort modules. Reaching the second
    /// is how a tool sees door, seat and lighting modules at all.
    ///
    /// The default implementation refuses, so an adapter that cannot do this
    /// says so rather than silently continuing to talk to the first bus and
    /// reporting that the modules are missing.
    fn select_bus(&mut self, _bus: VehicleBus) -> AimResult<()> {
        Err(AimError::new(
            ErrorCode::OperationNotAllowed,
            "this adapter cannot change which vehicle bus it is connected to",
        ))
    }

    /// Bit rate found on the secondary bus, in kbit/s, once a probe has
    /// established one.
    ///
    /// `None` when nothing has been established, which is not the same as the
    /// bus being absent.
    fn secondary_bus_kbits(&self) -> Option<u32> {
        None
    }

    /// Which bus the adapter is currently on.
    fn current_bus(&self) -> VehicleBus {
        VehicleBus::Primary
    }

    /// Hint which protocol worked here last time, so it is tried first.
    ///
    /// A hint and nothing more: an adapter that acts on it must still prove the
    /// protocol works before accepting it. The default ignores it, which is
    /// correct for any adapter that does not negotiate a protocol at all.
    fn prefer_protocol(&mut self, _protocol: Option<ObdProtocol>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A broadcast header belongs to the protocol, not to the request.
    ///
    /// Measured on a 29-bit vehicle: forcing the 11-bit `7DF` produced
    /// `NO DATA` on every CAN protocol, while the protocol's own header got two
    /// modules answering `0100`. `RequestTarget` deliberately no longer carries
    /// a header for this reason — there is nowhere left to hardcode `7DF`.
    #[test]
    fn a_broadcast_header_comes_from_the_protocol_not_the_target() {
        use aim_types::ObdProtocol;
        assert_eq!(ObdProtocol::Iso15765Can11_500.functional_header(), Some("7DF"));
        assert_eq!(ObdProtocol::Iso15765Can29_500.functional_header(), Some("18DB33F1"));
        assert_eq!(ObdProtocol::Iso15765Can29_250.functional_header(), Some("18DB33F1"));
        assert_eq!(ObdProtocol::Iso9141_2.functional_header(), Some("686AF1"));
        assert_eq!(ObdProtocol::J1850Vpw.functional_header(), Some("616AF1"));
        // With no protocol established there is no correct header, and the
        // adapter's own default beats a guess.
        assert_eq!(ObdProtocol::Unknown.functional_header(), None);
    }

    #[test]
    fn response_addresses_map_back_to_request_addresses() {
        assert_eq!(
            RequestTarget::from_response_address("7E8"),
            Some(RequestTarget::Physical("7E0".into()))
        );
        assert_eq!(
            RequestTarget::from_response_address("7EA"),
            Some(RequestTarget::Physical("7E2".into()))
        );
        // Not a conventional OBD response id: no guess is made.
        assert_eq!(RequestTarget::from_response_address("123"), None);
        assert_eq!(RequestTarget::from_response_address("nonsense"), None);
    }

    #[test]
    fn ecu_message_exposes_payload_hex() {
        let m = EcuMessage {
            address: "7E8".into(),
            payload: vec![0x41, 0x0C, 0x1A, 0xF8],
            raw_lines: vec!["7E8 04 41 0C 1A F8".into()],
        };
        assert_eq!(m.payload_hex(), "410c1af8");
    }
}

/// Which vehicle bus an adapter is talking to.
///
/// Named by the pins rather than by a speed, because the speed is not a
/// property of the bus this enum can know.
///
/// This used to be `HighSpeed` and `MediumSpeed`, with 500 and 125 kbit/s
/// written into the type. That was wrong on the first vehicle it met: a 2019
/// F-250's second bus runs at **500 kbit/s**, and a sweep at the assumed 125
/// produced nothing but `CAN ERROR` — measured on 2026-09-11, where 500 kbit/s
/// on the same pins carried 239 frames in five seconds and 29 modules answered.
///
/// Ford called the slow one MS-CAN and the newer fast one HS-CAN2, and a type
/// that encodes either number is a type that is wrong on half the fleet. So the
/// bus says which pins, and the rate is discovered from the vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VehicleBus {
    /// Pins 6 and 14. Powertrain and everything the emissions standard
    /// requires. Every OBD-II tool reaches this one, always at 500 kbit/s for
    /// CAN vehicles.
    Primary,
    /// Pins 3 and 11. Body, comfort and instrument modules — most of a vehicle,
    /// and nearly everything configurable. Reaching it needs an adapter wired
    /// to those pins, which no adapter can report about itself.
    ///
    /// The rate varies by manufacturer and model year and is discovered, not
    /// assumed.
    Secondary,
}

/// Bit rates a secondary bus is worth trying, fastest first.
///
/// Fastest first because it is the one modern vehicles use, and because a
/// wrong rate costs a probe rather than a wrong answer.
pub const SECONDARY_BUS_RATES: [u32; 3] = [500, 250, 125];

impl VehicleBus {
    /// Bit rate in kbit/s, where it is a property of the bus rather than of the
    /// vehicle.
    ///
    /// `None` for the secondary bus: that is the whole point of this type. Ask
    /// the adapter what it found instead.
    pub fn kbits(&self) -> Option<u32> {
        match self {
            VehicleBus::Primary => Some(500),
            VehicleBus::Secondary => None,
        }
    }

    /// Human label.
    pub fn label(&self) -> &'static str {
        match self {
            VehicleBus::Primary => "primary bus (pins 6 and 14)",
            VehicleBus::Secondary => "secondary bus (pins 3 and 11)",
        }
    }

    /// Prefix distinguishing a module key on this bus from one at the same
    /// address on another.
    ///
    /// Two buses can each have a module answering at `7E8`. They are different
    /// modules, and a key that recorded only the address would file the second
    /// one on top of the first. The high-speed prefix is what module keys have
    /// always been, so existing stored sessions keep matching.
    pub fn key_prefix(&self) -> &'static str {
        match self {
            VehicleBus::Primary => "ECU",
            VehicleBus::Secondary => "BUS2",
        }
    }

    /// Which bus a stored module key came from.
    ///
    /// The key is the record of it, so nothing has to be migrated into the
    /// session database for a session recorded before there were two buses.
    pub fn of_module_key(key: &str) -> VehicleBus {
        match key.starts_with(&format!("{}_", VehicleBus::Secondary.key_prefix())) {
            true => VehicleBus::Secondary,
            false => VehicleBus::Primary,
        }
    }

    /// Whether a module on this bus can be expected to answer OBD-II.
    ///
    /// Service 01 is an emissions obligation, and it applies to the modules on
    /// the legislated bus. Body, comfort and instrument modules are not
    /// emissions modules: they implement UDS and nothing else. Measured on a
    /// 2019 F-250 on 2026-09-11, the OBD-II broadcast on the secondary bus
    /// returned NO DATA while 29 modules there answered UDS TesterPresent.
    ///
    /// This is what stops the live-data screen offering a door module a list of
    /// engine sensors it was never going to produce.
    pub fn answers_obd2(&self) -> bool {
        matches!(self, VehicleBus::Primary)
    }
}

#[cfg(test)]
mod bus_tests {
    use super::VehicleBus;

    /// Two buses, one address, two modules.
    ///
    /// A body module on the secondary bus can answer at the same address as a
    /// powertrain module on the primary one — measured on a 2019 F-250, where
    /// both buses have modules in the `7xx` range. They are unrelated, and a
    /// key built from the address alone would file the second on top of the
    /// first: the scan would report finding fewer modules the more buses it
    /// swept.
    #[test]
    fn a_module_key_distinguishes_the_bus_it_answered_on() {
        let primary = format!("{}_{}", VehicleBus::Primary.key_prefix(), "7E8");
        let secondary = format!("{}_{}", VehicleBus::Secondary.key_prefix(), "7E8");
        assert_ne!(primary, secondary);
        // The primary form is what module keys have always been, so sessions
        // recorded before there was a second bus still match.
        assert_eq!(primary, "ECU_7E8");
    }

    /// The secondary bus does not get to claim a speed.
    ///
    /// It had one written into it — 125 kbit/s — and the first vehicle this met
    /// runs its second bus at 500. A type that answers this question without
    /// asking the vehicle is a type that is confidently wrong.
    #[test]
    fn only_the_primary_bus_has_a_rate_worth_asserting() {
        assert_eq!(VehicleBus::Primary.kbits(), Some(500));
        assert_eq!(
            VehicleBus::Secondary.kbits(),
            None,
            "the secondary bus rate is discovered from the vehicle, never assumed"
        );
    }

    /// Every rate worth trying divides the programmable base cleanly, because a
    /// fractional divisor would silently configure the wrong speed.
    #[test]
    fn every_candidate_rate_divides_the_programmable_base() {
        for kbits in super::SECONDARY_BUS_RATES {
            assert_ne!(kbits, 0, "a zero bit rate would divide by zero");
            assert_eq!(500 % kbits, 0, "{kbits} kbit/s needs a fractional divisor");
        }
        // Fastest first: modern vehicles use it, and the F-250 that prompted
        // this would have been found on the first attempt.
        assert_eq!(super::SECONDARY_BUS_RATES[0], 500);
    }
}
