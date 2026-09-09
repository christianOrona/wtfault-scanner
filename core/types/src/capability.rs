//! Adapter capability model (handoff §4) and connection state.
//!
//! The point of this module is the sentence in §4: *"Design the adapter layer so
//! its limitations produce capability flags rather than app-wide assumptions."*
//! A cheap ELM327 clone that cannot do 29-bit CAN produces
//! `can29_bit: false`, not a global "this app does not support 29-bit CAN".

use serde::{Deserialize, Serialize};

/// Physical link between the PC and the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// USB CDC / FTDI style serial device.
    Usb,
    /// Bluetooth Classic SPP. On Windows this surfaces as an outgoing COM port.
    Bluetooth,
    /// Bluetooth Low Energy GATT adapter.
    BluetoothLe,
    /// Wi-Fi (typically a TCP socket to 192.168.0.10:35000).
    Wifi,
    /// In-process deterministic simulator. Never a real vehicle.
    Simulated,
    /// Playback of a previously recorded adapter transcript.
    Replay,
}

impl TransportKind {
    /// True when bytes are actually leaving the computer toward a vehicle.
    pub fn is_physical(&self) -> bool {
        matches!(
            self,
            TransportKind::Usb
                | TransportKind::Bluetooth
                | TransportKind::BluetoothLe
                | TransportKind::Wifi
        )
    }
}

/// OBD-II lower-layer protocol, matching the ELM327 `ATSP` numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObdProtocol {
    /// Protocol not yet determined (`ATSP0` still searching).
    Unknown,
    /// SAE J1850 PWM, 41.6 kbaud.
    J1850Pwm,
    /// SAE J1850 VPW, 10.4 kbaud.
    J1850Vpw,
    /// ISO 9141-2.
    Iso9141_2,
    /// ISO 14230-4 KWP, 5 baud init.
    Iso14230KwpSlow,
    /// ISO 14230-4 KWP, fast init.
    Iso14230KwpFast,
    /// ISO 15765-4 CAN, 11-bit identifiers, 500 kbaud.
    Iso15765Can11_500,
    /// ISO 15765-4 CAN, 29-bit identifiers, 500 kbaud.
    Iso15765Can29_500,
    /// ISO 15765-4 CAN, 11-bit identifiers, 250 kbaud.
    Iso15765Can11_250,
    /// ISO 15765-4 CAN, 29-bit identifiers, 250 kbaud.
    Iso15765Can29_250,
}

impl ObdProtocol {
    /// Decode the digit reported by `ATDPN` / used by `ATSP`.
    pub fn from_elm_id(id: u8) -> ObdProtocol {
        match id {
            1 => ObdProtocol::J1850Pwm,
            2 => ObdProtocol::J1850Vpw,
            3 => ObdProtocol::Iso9141_2,
            4 => ObdProtocol::Iso14230KwpSlow,
            5 => ObdProtocol::Iso14230KwpFast,
            6 => ObdProtocol::Iso15765Can11_500,
            7 => ObdProtocol::Iso15765Can29_500,
            8 => ObdProtocol::Iso15765Can11_250,
            9 => ObdProtocol::Iso15765Can29_250,
            _ => ObdProtocol::Unknown,
        }
    }

    /// The `ATSP` digit for this protocol, if it has one.
    pub fn elm_id(&self) -> Option<u8> {
        Some(match self {
            ObdProtocol::Unknown => return None,
            ObdProtocol::J1850Pwm => 1,
            ObdProtocol::J1850Vpw => 2,
            ObdProtocol::Iso9141_2 => 3,
            ObdProtocol::Iso14230KwpSlow => 4,
            ObdProtocol::Iso14230KwpFast => 5,
            ObdProtocol::Iso15765Can11_500 => 6,
            ObdProtocol::Iso15765Can29_500 => 7,
            ObdProtocol::Iso15765Can11_250 => 8,
            ObdProtocol::Iso15765Can29_250 => 9,
        })
    }

    /// True for the ISO 15765 (CAN) family, where ISO-TP framing applies.
    pub fn is_can(&self) -> bool {
        matches!(
            self,
            ObdProtocol::Iso15765Can11_500
                | ObdProtocol::Iso15765Can29_500
                | ObdProtocol::Iso15765Can11_250
                | ObdProtocol::Iso15765Can29_250
        )
    }

    /// True when this CAN variant uses 29-bit identifiers.
    pub fn is_29_bit(&self) -> bool {
        matches!(self, ObdProtocol::Iso15765Can29_500 | ObdProtocol::Iso15765Can29_250)
    }

    /// Human-readable protocol name (safe to log; not a UI string).
    pub fn label(&self) -> &'static str {
        match self {
            ObdProtocol::Unknown => "unknown",
            ObdProtocol::J1850Pwm => "SAE J1850 PWM",
            ObdProtocol::J1850Vpw => "SAE J1850 VPW",
            ObdProtocol::Iso9141_2 => "ISO 9141-2",
            ObdProtocol::Iso14230KwpSlow => "ISO 14230-4 KWP (5 baud init)",
            ObdProtocol::Iso14230KwpFast => "ISO 14230-4 KWP (fast init)",
            ObdProtocol::Iso15765Can11_500 => "ISO 15765-4 CAN 11/500",
            ObdProtocol::Iso15765Can29_500 => "ISO 15765-4 CAN 29/500",
            ObdProtocol::Iso15765Can11_250 => "ISO 15765-4 CAN 11/250",
            ObdProtocol::Iso15765Can29_250 => "ISO 15765-4 CAN 29/250",
        }
    }
}

/// Declared capabilities of a connected adapter — handoff §4, verbatim fields.
///
/// Everything here is *observed or conservatively assumed*, never aspirational.
/// `Elm327Adapter` fills this in from the identification handshake; unknown
/// facts stay `false` so that callers fail closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    /// Physical link kind.
    pub transport: TransportKind,
    /// The device answered an ELM327 identification handshake.
    pub elm327_compatible: bool,
    /// 11-bit CAN identifiers usable.
    pub can_11_bit: bool,
    /// 29-bit CAN identifiers usable.
    pub can_29_bit: bool,
    /// ISO-TP segmentation available (the ELM327 does this internally).
    pub iso_tp: bool,
    /// More than one CAN bus reachable (HS-CAN/MS-CAN switching).
    pub multiple_can_buses: bool,
    /// SAE J2534 pass-through device.
    pub j2534: bool,
    /// The adapter can transmit arbitrary frames, not only request/response.
    pub supports_transmit: bool,
    /// Multi-frame messages longer than a single CAN frame are handled.
    pub supports_long_messages: bool,
    /// Conservative estimate of sustainable requests per second.
    pub max_reliable_throughput: f64,
    /// Vendor string as reported or inferred.
    pub vendor: String,
    /// Model string as reported or inferred.
    pub model: String,
    /// Firmware / identification banner, when the device reported one.
    pub firmware: Option<String>,
    /// Capability caveats. A cheap clone accumulates these instead of silently
    /// pretending to be a genuine ELM327 v1.5.
    pub caveats: Vec<String>,
}

/// How long different kinds of request are worth waiting for on this adapter.
///
/// Derived from what the adapter was *observed* to sustain, never from a
/// constant. The app used to carry `75 ms per PID` in the UI, measured on one
/// ELM327 clone, which quietly became the ceiling for every adapter that would
/// ever be plugged in - including the faster ones this design exists to
/// support. An STN-based or J2534 device is several times quicker and should
/// not be throttled by a number learned from a $10 cable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestBudget {
    /// Waiting for a module that may simply not exist. Short on purpose: a
    /// discovery sweep spends most of its time being ignored.
    pub probe: std::time::Duration,
    /// Reading something a module has to look up, such as its fault memory.
    pub read: std::time::Duration,
    /// Round-trip cost of one live-data parameter, used to work out how many
    /// fit inside a sampling interval.
    pub per_signal: std::time::Duration,
}

impl AdapterCapabilities {
    /// Timing budgets implied by this adapter's observed throughput.
    ///
    /// One measured number drives all three, so an adapter that turns out to be
    /// fast gets the benefit everywhere at once rather than in whichever call
    /// site somebody remembered to update.
    pub fn discovery_budget(&self) -> RequestBudget {
        // Fall back to something an ELM327 clone manages when nothing has been
        // measured yet. A default is not a limit: it is replaced the moment the
        // adapter reports what it actually did.
        let per_second =
            if self.max_reliable_throughput > 0.1 { self.max_reliable_throughput } else { 14.0 };
        let one = std::time::Duration::from_secs_f64(1.0 / per_second);
        RequestBudget {
            // Silence is the common answer during discovery, so this only has
            // to be long enough for a module that *is* there to speak.
            probe: one.max(std::time::Duration::from_millis(40)),
            // Fault memory can take a module real time to assemble.
            read: (one * 12).max(std::time::Duration::from_millis(600)),
            per_signal: one,
        }
    }
}

impl AdapterCapabilities {
    /// The safest possible starting point: nothing is assumed to work.
    pub fn unknown(transport: TransportKind) -> Self {
        AdapterCapabilities {
            transport,
            elm327_compatible: false,
            can_11_bit: false,
            can_29_bit: false,
            iso_tp: false,
            multiple_can_buses: false,
            j2534: false,
            supports_transmit: false,
            supports_long_messages: false,
            max_reliable_throughput: 0.0,
            vendor: String::from("unknown"),
            model: String::from("unknown"),
            firmware: None,
            caveats: Vec::new(),
        }
    }

    /// Record a capability caveat (deduplicated).
    pub fn add_caveat(&mut self, caveat: impl Into<String>) {
        let c = caveat.into();
        if !self.caveats.contains(&c) {
            self.caveats.push(c);
        }
    }
}

/// Adapter connection state machine (handoff §17 item 4).
///
/// ```text
///   Disconnected ──connect()──▶ Connecting ──port open──▶ Initializing
///        ▲                          │                          │
///        │                          │ failure                  │ AT sequence ok
///        │                          ▼                          ▼
///        └───────── disconnect() ── Failed ◀── giveup ── Identifying
///        │                                                     │
///        │                                                     ▼
///        └──────────── disconnect() ────────────────────────  Ready
///                                                              │
///                            Reconnecting ◀── link lost ───────┤
///                                 │                            │
///                                 └── recovered ──▶ Ready      ▼
///                                                           Degraded
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    /// No transport open.
    Disconnected,
    /// Opening the transport (COM port, socket, simulator).
    Connecting,
    /// Transport open, running the adapter initialization sequence.
    Initializing,
    /// Adapter initialized, negotiating the vehicle protocol.
    Identifying,
    /// Adapter initialized and talking to the vehicle.
    Ready,
    /// Still usable but with degraded expectations (timeouts, partial reads).
    Degraded {
        /// Why the connection is degraded.
        reason: String,
    },
    /// The link dropped and recovery is being attempted.
    Reconnecting {
        /// Which recovery attempt this is, 1-based.
        attempt: u32,
    },
    /// Connection attempt failed and was abandoned.
    Failed {
        /// Machine-readable failure code.
        code: crate::ErrorCode,
        /// Detail for logs and the flight recorder.
        detail: String,
    },
}

impl ConnectionState {
    /// True when diagnostic requests may be issued.
    pub fn is_usable(&self) -> bool {
        matches!(self, ConnectionState::Ready | ConnectionState::Degraded { .. })
    }

    /// Short stable discriminant, used in the event log and API payloads.
    pub fn name(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "disconnected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Initializing => "initializing",
            ConnectionState::Identifying => "identifying",
            ConnectionState::Ready => "ready",
            ConnectionState::Degraded { .. } => "degraded",
            ConnectionState::Reconnecting { .. } => "reconnecting",
            ConnectionState::Failed { .. } => "failed",
        }
    }
}

/// Rolling health of the adapter link, exposed to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterHealth {
    /// Current state machine position.
    pub state: ConnectionState,
    /// Total adapter commands issued during this connection.
    pub requests: u64,
    /// Commands that produced a usable response.
    pub responses: u64,
    /// Commands that timed out.
    pub timeouts: u64,
    /// Commands that produced `NO DATA`.
    pub no_data: u64,
    /// Commands the adapter rejected with `?` or an error banner.
    pub adapter_errors: u64,
    /// Mean round-trip time over the last window, milliseconds.
    pub mean_latency_ms: f64,
    /// Last observed control-module voltage, if `ATRV` was read.
    pub battery_voltage: Option<f64>,
    /// Negotiated vehicle protocol.
    pub protocol: ObdProtocol,
}

impl AdapterHealth {
    /// A zeroed health record in the given state.
    pub fn new(state: ConnectionState) -> Self {
        AdapterHealth {
            state,
            requests: 0,
            responses: 0,
            timeouts: 0,
            no_data: 0,
            adapter_errors: 0,
            mean_latency_ms: 0.0,
            battery_voltage: None,
            protocol: ObdProtocol::Unknown,
        }
    }

    /// Fraction of requests that produced a usable response, 0.0 when idle.
    pub fn success_rate(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.responses as f64 / self.requests as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elm_protocol_ids_round_trip() {
        for id in 1..=9u8 {
            let p = ObdProtocol::from_elm_id(id);
            assert_ne!(p, ObdProtocol::Unknown, "id {id}");
            assert_eq!(p.elm_id(), Some(id));
        }
        assert_eq!(ObdProtocol::from_elm_id(0), ObdProtocol::Unknown);
        assert_eq!(ObdProtocol::from_elm_id(0xA), ObdProtocol::Unknown);
        assert_eq!(ObdProtocol::Unknown.elm_id(), None);
    }

    #[test]
    fn can_classification() {
        assert!(ObdProtocol::Iso15765Can11_500.is_can());
        assert!(!ObdProtocol::Iso15765Can11_500.is_29_bit());
        assert!(ObdProtocol::Iso15765Can29_250.is_29_bit());
        assert!(!ObdProtocol::Iso9141_2.is_can());
    }

    #[test]
    fn unknown_capabilities_assume_nothing() {
        let c = AdapterCapabilities::unknown(TransportKind::Bluetooth);
        assert!(!c.elm327_compatible);
        assert!(!c.supports_transmit);
        assert_eq!(c.max_reliable_throughput, 0.0);
    }

    #[test]
    fn caveats_are_deduplicated() {
        let mut c = AdapterCapabilities::unknown(TransportKind::Bluetooth);
        c.add_caveat("clone firmware");
        c.add_caveat("clone firmware");
        assert_eq!(c.caveats.len(), 1);
    }

    #[test]
    fn only_ready_and_degraded_are_usable() {
        assert!(ConnectionState::Ready.is_usable());
        assert!(ConnectionState::Degraded { reason: "slow".into() }.is_usable());
        assert!(!ConnectionState::Connecting.is_usable());
        assert!(!ConnectionState::Disconnected.is_usable());
    }

    #[test]
    fn simulated_transport_is_not_physical() {
        assert!(!TransportKind::Simulated.is_physical());
        assert!(!TransportKind::Replay.is_physical());
        assert!(TransportKind::Bluetooth.is_physical());
    }
}
