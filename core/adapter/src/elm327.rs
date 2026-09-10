//! ELM327-class adapter.
//!
//! # Why this is more than "write string, read string"
//!
//! The ELM327 is a text terminal in front of a CAN controller. Everything it
//! can tell you arrives as ASCII, including its failures, and a cheap clone
//! lies about its own version. This module turns that into structured state:
//!
//! * an explicit initialization sequence where each command's rejection is
//!   recorded as a capability caveat rather than ignored,
//! * an identification step that reports what the device *demonstrated*, not
//!   what its banner claims,
//! * per-ECU ISO-TP reassembly of the frames the adapter prints,
//! * a connection state machine with bounded reconnection.
//!
//! # Headers and spaces
//!
//! The adapter is configured with **headers on** (`ATH1`) and, by default,
//! **spaces on** (`ATS1`). Headers are what make multi-ECU responses
//! separable: without them a broadcast request answered by three modules is an
//! unattributable pile of hex. Spaces make byte boundaries unambiguous;
//! [`super::EcuMessage`] assembly still accepts unspaced replies (a clone that
//! ignores `ATS1`) because the negotiated protocol fixes the header width, but
//! it never *guesses* a boundary it cannot derive.
//!
//! # Flow control
//!
//! With `ATCAF1` (the power-on default) the ELM327 transmits ISO-TP flow
//! control frames itself. We still run every received frame through
//! [`aim_protocols::IsoTpReceiver`] because the adapter prints the raw
//! segmented frames — but a [`aim_protocols::ReceiveOutcome::SendFlowControl`]
//! is informational here, not something this layer transmits.

use crate::{
    response, AdapterResponse, DiagnosticAdapter, EcuMessage, NullObserver, ObserverRef,
    RequestTarget, ResponseClass,
};
use aim_protocols::{CanFrame, CanId, IsoTpFrame, IsoTpReceiver, ObdRequest, ReceiveOutcome};
use aim_transport::{read_until, Transport};
use aim_types::{
    AdapterCapabilities, AdapterHealth, AimError, AimResult, ConnectionState, ErrorCode,
    ObdProtocol,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tunables for [`Elm327Adapter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elm327Config {
    /// Deadline for `ATZ`, which resets the device and takes the longest.
    pub reset_timeout: Duration,
    /// Deadline for ordinary `AT` configuration commands.
    pub at_timeout: Duration,
    /// Deadline for an OBD request. Protocol search on the first request can
    /// legitimately take several seconds.
    pub request_timeout: Duration,
    /// Force a specific protocol (`ATSP<n>`) instead of auto-detect (`ATSP0`).
    pub force_protocol: Option<ObdProtocol>,
    /// Ask the adapter for spaces between bytes. Recommended; a device that
    /// refuses gets a caveat and unspaced parsing.
    pub spaces: bool,
    /// Enable adaptive timing (`ATAT1`).
    pub adaptive_timing: bool,
    /// How many times to rebuild the link after it drops mid-session.
    pub max_reconnect_attempts: u32,
}

impl Default for Elm327Config {
    fn default() -> Self {
        Elm327Config {
            reset_timeout: Duration::from_secs(8),
            at_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(6),
            force_protocol: None,
            spaces: true,
            adaptive_timing: true,
            max_reconnect_attempts: 2,
        }
    }
}

impl Elm327Config {
    /// Short deadlines for the simulator and tests, where nothing is slow.
    pub fn fast() -> Self {
        Elm327Config {
            reset_timeout: Duration::from_millis(500),
            at_timeout: Duration::from_millis(300),
            request_timeout: Duration::from_millis(500),
            ..Default::default()
        }
    }
}

/// An ELM327-compatible adapter driving one [`Transport`].
pub struct Elm327Adapter {
    transport: Box<dyn Transport>,
    config: Elm327Config,
    observer: ObserverRef,
    state: ConnectionState,
    caps: AdapterCapabilities,
    protocol: ObdProtocol,
    /// False when the device refused `ATH1`; assembly then cannot attribute
    /// responses to modules and says so.
    headers_enabled: bool,
    spaces_enabled: bool,
    current_header: Option<String>,
    /// Which vehicle bus the adapter is currently switched to.
    current_bus: crate::VehicleBus,
    /// A protocol known to have worked on this vehicle before, tried first.
    ///
    /// Set by the caller from session history. Never treated as established -
    /// it only changes the order the sweep tries things in.
    preferred_protocol: Option<ObdProtocol>,
    /// How long the adapter is currently told to wait for a vehicle, in ms.
    ///
    /// Written to the device with `ATST`. Learned rather than fixed: `NO DATA`
    /// is the most common reply from a vehicle that is in fact present, and a
    /// window that is too short turns a slow module into an absent one.
    response_window_ms: u32,
    /// The shortest window at which a request has actually succeeded.
    ///
    /// The window eases back toward this after a success, so one slow module
    /// does not permanently slow down every request afterwards. `None` until
    /// something has worked.
    proven_window_ms: Option<u32>,
    requests: u64,
    responses: u64,
    timeouts: u64,
    no_data: u64,
    adapter_errors: u64,
    latency_total_ms: u64,
    latency_samples: u64,
    /// Set when the device answered `STI`, proving STN-series firmware rather
    /// than a clone that borrowed the name.
    stn_confirmed: bool,
    battery_voltage: Option<f64>,
}

impl Elm327Adapter {
    /// Build an adapter over `transport`. Nothing is opened until
    /// [`DiagnosticAdapter::connect`].
    pub fn new(transport: Box<dyn Transport>, config: Elm327Config) -> Self {
        let caps = AdapterCapabilities::unknown(transport.kind());
        Elm327Adapter {
            transport,
            config,
            observer: Arc::new(NullObserver),
            state: ConnectionState::Disconnected,
            caps,
            protocol: ObdProtocol::Unknown,
            headers_enabled: false,
            spaces_enabled: true,
            current_header: None,
            current_bus: crate::VehicleBus::HighSpeed,
            preferred_protocol: None,
            response_window_ms: RESPONSE_WINDOW_DEFAULT_MS,
            proven_window_ms: None,
            requests: 0,
            responses: 0,
            timeouts: 0,
            no_data: 0,
            adapter_errors: 0,
            latency_total_ms: 0,
            latency_samples: 0,
            stn_confirmed: false,
            battery_voltage: None,
        }
    }

    /// Attach the flight recorder.
    pub fn with_observer(mut self, observer: ObserverRef) -> Self {
        self.observer = observer;
        self
    }

    /// Replace the observer on an existing adapter.
    pub fn set_observer(&mut self, observer: ObserverRef) {
        self.observer = observer;
    }

    fn set_state(&mut self, next: ConnectionState) {
        if self.state != next {
            let prev = std::mem::replace(&mut self.state, next);
            self.observer.on_state_change(&prev, &self.state);
        }
    }

    /// Write one command and read until the `>` prompt.
    ///
    /// Classification failures (`NO DATA`, `?`, `UNABLE TO CONNECT`) come back
    /// as `Ok` with a failing [`ResponseClass`] — they are answers, and the
    /// caller decides what they mean. Only transport faults are `Err`.
    fn send_raw(&mut self, command: &str, timeout: Duration) -> AimResult<AdapterResponse> {
        if !self.transport.is_open() {
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                format!("cannot send {command:?}: transport is closed"),
            ));
        }
        self.observer.on_request(command);
        self.requests += 1;

        // A late reply to the previous command must never be read as this
        // command's answer.
        if let Err(e) = self.transport.flush_input() {
            self.observer.on_failure(Some(command), &e);
            return Err(e);
        }

        let started = Instant::now();
        let mut line = String::with_capacity(command.len() + 1);
        line.push_str(command);
        line.push('\r');
        if let Err(e) = self.transport.write_all(line.as_bytes()) {
            self.observer.on_failure(Some(command), &e);
            return Err(e);
        }

        let (bytes, terminated) = match read_until(self.transport.as_mut(), b'>', timeout) {
            Ok(v) => v,
            Err(e) => {
                self.observer.on_failure(Some(command), &e);
                return Err(e);
            }
        };
        let elapsed = started.elapsed();
        let raw = String::from_utf8_lossy(&bytes);
        let parsed = response::parse(command, &raw, terminated, elapsed.as_millis() as u64);

        self.latency_total_ms += parsed.elapsed_ms;
        self.latency_samples += 1;
        match parsed.class {
            c if c.is_success() => self.responses += 1,
            ResponseClass::NoData => self.no_data += 1,
            ResponseClass::Timeout => self.timeouts += 1,
            _ => self.adapter_errors += 1,
        }
        self.observer.on_response(&parsed);
        Ok(parsed)
    }

    /// Send a configuration command, treating a rejection as a recorded caveat
    /// rather than a fatal error. Returns whether the device accepted it.
    fn configure(&mut self, command: &str, why: &str) -> AimResult<bool> {
        let r = self.send_raw(command, self.config.at_timeout)?;
        if r.class.is_success() {
            Ok(true)
        } else {
            self.caps
                .add_caveat(format!("device rejected {command} ({why}) with {}", r.class.as_str()));
            Ok(false)
        }
    }

    /// Handoff §17 item 4: the initialization half of the connection state
    /// machine.
    fn initialize(&mut self) -> AimResult<()> {
        // ATZ: full reset. The banner it prints is the first evidence of what
        // this device is, so a silent ATZ is fatal — we will not talk to a
        // device that never answered.
        let reset = self.send_raw("ATZ", self.config.reset_timeout)?;
        if !reset.class.is_success() {
            return Err(AimError::new(
                ErrorCode::AdapterInitFailed,
                format!("ATZ was answered with {}", reset.class.as_str()),
            )
            .with_details(serde_json::json!({
                "command": "ATZ",
                "classification": reset.class.as_str(),
                "lines": reset.lines,
            })));
        }
        let banner = reset.lines.join(" ");

        // ATE0: echo off. Everything downstream assumes replies are not
        // prefixed with the command, so this one is also mandatory.
        let echo = self.send_raw("ATE0", self.config.at_timeout)?;
        if !echo.class.is_success() {
            return Err(AimError::new(
                ErrorCode::AdapterInitFailed,
                format!("ATE0 was answered with {}", echo.class.as_str()),
            ));
        }

        self.configure("ATL0", "linefeeds off")?;
        let spaces_cmd = if self.config.spaces { "ATS1" } else { "ATS0" };
        let spaces_ok = self.configure(spaces_cmd, "byte spacing")?;
        self.spaces_enabled = if spaces_ok { self.config.spaces } else { true };

        // Headers are what make per-module attribution possible.
        self.headers_enabled = self.configure("ATH1", "response headers on")?;
        if !self.headers_enabled {
            self.caps.add_caveat(
                "responses cannot be attributed to individual modules: device refused ATH1",
            );
        }

        if self.config.adaptive_timing {
            self.configure("ATAT1", "adaptive timing")?;
        }

        match self.config.force_protocol.and_then(|p| p.elm_id()) {
            Some(id) => {
                self.configure(&format!("ATSP{id}"), "forced protocol")?;
            }
            None => {
                self.configure("ATSP0", "automatic protocol detection")?;
            }
        }

        self.caps.firmware = if banner.is_empty() { None } else { Some(banner) };
        self.current_header = None;
        Ok(())
    }

    /// Identification: work out what this device actually is.
    fn identify(&mut self) -> AimResult<()> {
        let ident = self.send_raw("ATI", self.config.at_timeout)?;
        let banner = ident.lines.join(" ");
        if !ident.class.is_success() || banner.is_empty() {
            return Err(AimError::new(
                ErrorCode::AdapterNotIdentified,
                "device did not answer ATI with an identification banner",
            )
            .with_details(serde_json::json!({ "lines": ident.lines })));
        }

        let upper = banner.to_ascii_uppercase();
        self.caps.elm327_compatible = upper.contains("ELM327") || upper.contains("OBDII");
        if !self.caps.elm327_compatible {
            return Err(AimError::new(
                ErrorCode::AdapterNotIdentified,
                format!("{banner:?} is not an ELM327-compatible identification banner"),
            )
            .with_details(serde_json::json!({ "banner": banner })));
        }
        self.caps.model = banner.trim().to_string();
        self.caps.firmware = Some(banner.clone());

        // AT@1 is the device-description command. Genuine and better clones
        // answer it; the cheapest ones return '?'. Either way it is evidence.
        let desc = self.send_raw("AT@1", self.config.at_timeout)?;
        if desc.class.is_success() && !desc.lines.is_empty() {
            self.caps.vendor = desc.lines.join(" ").trim().to_string();
        } else {
            self.caps.vendor = String::from("unknown");
            self.caps.add_caveat(
                "device did not answer AT@1 (device description); vendor is unidentified",
            );
        }

        // Better silicon, recognised and used.
        //
        // "ELM327-compatible" spans a very wide range of hardware. At the
        // bottom is a counterfeit chip that drops multi-frame responses; at the
        // top is an STN-series device (OBDLink and similar) that is a different
        // class of thing — several times the throughput, real multi-frame
        // handling, and on the dual-bus models a second CAN bus that reaches
        // the modules a plain ELM327 cannot see at all.
        //
        // This is checked and recorded, never assumed. Everything downstream —
        // how many live signals fit in an interval, how long a discovery sweep
        // waits per address — is derived from observed throughput, so an
        // adapter that reports itself as better is *tested* as better and earns
        // the benefit automatically. Nothing here hard-codes a device.
        let vendor_upper = self.caps.vendor.to_ascii_uppercase();
        let stn = upper.contains("STN")
            || vendor_upper.contains("STN")
            || vendor_upper.contains("OBDLINK")
            || vendor_upper.contains("SCANTOOL");
        if stn {
            // ST is the STN command prefix. A device that answers `STI` with a
            // version string is genuinely STN firmware rather than a clone that
            // merely borrowed the name for its product page.
            let sti = self.send_raw("STI", self.config.at_timeout)?;
            if sti.class.is_success() && !sti.lines.is_empty() {
                self.caps.add_caveat(format!(
                    "STN-series firmware confirmed ({}); multi-frame handling and throughput \
                     are materially better than a generic ELM327 clone",
                    sti.lines.join(" ").trim()
                ));
                self.caps.supports_long_messages = true;
                self.stn_confirmed = true;

                // Ask, rather than assume, whether this is a dual-bus device.
                // Reaching the second bus is what makes door, body and chassis
                // modules addressable on vehicles that put them there, and it
                // is the single capability that most changes what the product
                // can do.
                let buses = self.send_raw("STPX", self.config.at_timeout)?;
                if buses.class.is_success() {
                    self.caps.multiple_can_buses = true;
                }
            } else {
                self.caps.add_caveat(
                    "the banner mentions STN but the device did not answer STI, so it is \
                     treated as a generic ELM327-compatible device",
                );
            }
        }

        // Version claims from ELM327-class hardware are not trustworthy: clone
        // firmware routinely reports v2.1 on v1.5-era silicon. Record the claim
        // and the doubt; never act on the version number.
        if upper.contains("V2.") {
            self.caps.add_caveat(
                "banner claims ELM327 v2.x; inexpensive clones commonly report v2.1 while \
                 implementing v1.5 behaviour, so treat unsupported-command replies as expected",
            );
        }

        // Segmentation is performed by the adapter itself on every ELM327-class
        // device, and long messages follow from it.
        self.caps.iso_tp = true;
        self.caps.supports_long_messages = true;

        // The ELM327 command set covers both 11- and 29-bit CAN. That is
        // inferred from the identification, not measured, and is labelled so.
        self.caps.can_11_bit = true;
        self.caps.can_29_bit = true;
        self.caps.add_caveat(
            "11/29-bit CAN support is inferred from the ELM327 command set, not measured on this \
             vehicle",
        );

        // No ELM327-class device is a J2534 pass-through, whatever else it can
        // do. This one really is true for all of them.
        self.caps.j2534 = false;

        // Whether a second CAN bus is reachable is measured, not assumed. It
        // used to be hardcoded false here - which also silently overwrote the
        // `true` the STN branch above had just established by asking the device.
        //
        // Two things have to hold, and only one of them is software:
        //
        //   * The adapter must be able to speak the second bus's bit rate.
        //     Ford's MS-CAN is 125 kbit/s, reachable on any ELM327 that
        //     implements the programmable protocol B commands (`ATPB`), and
        //     natively on STN hardware.
        //   * The cable must physically reach pins 3 and 11. Nothing on the
        //     wire can report this, because a cable that is not connected to
        //     those pins is indistinguishable from a bus with nothing on it.
        //
        // The second is a hardware fact and is reported as one, rather than as
        // something the app has not got round to.
        // Only STN hardware, which said so when asked, is recorded as able to
        // reach a second bus.
        //
        // There used to be an `ATPB` probe here for everything else. It was
        // wrong twice over. It wrote programmable protocol parameters into a
        // device on every single connect, to establish a capability nothing in
        // this app uses yet - the second bus is detected and never selected.
        // And it proved less than it appeared to: an ELM327 answering `OK` to a
        // parameter write has said the command parsed, not that a second bus
        // exists or that the cable reaches pins 3 and 11.
        //
        // So the question is left open rather than answered badly. When
        // something actually selects a second bus, it can probe then, on a
        // vehicle, where the answer means something.
        if self.caps.multiple_can_buses {
            self.caps.add_caveat(
                "this adapter reports being able to reach a second CAN bus (125 kbit/s). Whether \
                 anything answers there also depends on the cable being wired to pins 3 and 11, \
                 which no adapter can report",
            );
        } else {
            self.caps.add_caveat(
                "second CAN bus not established by software: this device did not identify as one \
                 that can switch buses on command. That is not the same as being unable to reach \
                 one - some cables carry a physical HS-CAN/MS-CAN switch, which no adapter can \
                 report and no software can detect or move. If yours has one, the bus you are on \
                 is whichever way the switch is set. Nothing in this build selects a second bus \
                 yet, so it costs you nothing today",
            );
        }

        if let Ok(v) = self.read_voltage_inner() {
            self.battery_voltage = Some(v);
            // `ATRV` is the adapter's own guess from an internal divider, not a
            // vehicle measurement, and on clones that divider is frequently
            // uncalibrated. Measured on a real truck: a cable reported 28.3 V
            // on a healthy 14 V system — almost exactly double — and the number
            // then travelled all the way into a report as if the vehicle had
            // said it.
            //
            // Nothing here is corrected, because there is no honest correction
            // to apply. What changes is that an implausible reading is labelled
            // implausible, so everything downstream can decline to treat it as
            // a fact about the vehicle. PID 42 from the engine module is the
            // real measurement and is preferred wherever it exists.
            if !(PLAUSIBLE_SYSTEM_VOLTS).contains(&v) {
                self.caps.add_caveat(format!(
                    "this adapter reports {v:.1} V, which is not a plausible vehicle system \
                     voltage: its internal measurement is uncalibrated and should not be \
                     trusted. Read control module voltage from the engine module instead."
                ));
            }
        }

        Ok(())
    }

    /// Try each OBD-II protocol in turn until one answers.
    ///
    /// Only used when `ATSP0` has already failed. The order is deliberate:
    /// CAN first, because every vehicle sold since 2008 uses it and those
    /// attempts fail fast, then the legacy protocols, which are slower to give
    /// up — `ATSP4` performs a five-baud initialisation that takes seconds on
    /// its own. Trying the cheap possibilities first keeps the common case
    /// quick and the hopeless case bounded.
    ///
    /// Returns the successful `0100` response, leaving the adapter set to the
    /// protocol that produced it.
    fn sweep_protocols(&mut self) -> AimResult<Option<AdapterResponse>> {
        // 6,7: CAN 500k (11- and 29-bit). 8,9: CAN 250k. 1,2: J1850.
        // 3: ISO 9141-2. 5: KWP fast. 4: KWP slow, last because of its init.
        const ORDER: [u8; 9] = [6, 7, 8, 9, 3, 5, 1, 2, 4];

        // A protocol that worked on this vehicle before goes first. Measured on
        // a real truck: a failing sweep spent eleven seconds trying all nine on
        // a vehicle already known to be CAN, including two 2.8-second bus-init
        // attempts on protocols it had no reason to try.
        //
        // Remembered is not verified. If it does not answer, the full sweep
        // runs exactly as before rather than concluding the vehicle is silent.
        let order: Vec<u8> = match self.preferred_protocol.and_then(|p| p.elm_id()) {
            Some(first) => std::iter::once(first)
                .chain(ORDER.iter().copied().filter(|&id| id != first))
                .collect(),
            None => ORDER.to_vec(),
        };

        let probe = ObdRequest::current_data(0x00);
        for id in order {
            // A failure to even set the protocol is a broken adapter, not a
            // wrong guess, so it stops the sweep rather than being skipped.
            self.configure(&format!("ATSP{id}"), "protocol sweep")?;
            // Each candidate is asked in its own addressing scheme. Carrying
            // one protocol's header into the next is what makes a sweep report
            // that a vehicle answered nothing when it would have answered.
            self.apply_functional_header(ObdProtocol::from_elm_id(id))?;

            let r = self.send_raw(&probe.to_elm_command(), self.config.request_timeout)?;
            // Only actual data proves a protocol. `is_success()` also admits
            // Ok and Info, and an adapter says plenty of non-error things on a
            // protocol the vehicle does not speak - init banners, prompts,
            // anything unrecognised. Accepting those picks the first protocol
            // that fails politely instead of the one the vehicle answers on.
            if matches!(r.class, ResponseClass::Data) {
                let found = ObdProtocol::from_elm_id(id);
                self.caps.add_caveat(format!(
                    "automatic protocol detection (ATSP0) failed; {} was found by trying each \
                     protocol in turn. Cheap ELM327 clones are unreliable at auto-detection.",
                    found.label()
                ));
                tracing::info!(protocol = %found.label(), "protocol sweep succeeded");
                return Ok(Some(r));
            }
            tracing::debug!(
                atsp = id,
                class = r.class.as_str(),
                lines = ?r.lines,
                "protocol did not answer"
            );
        }

        // Leave the adapter as we found it so a later retry starts clean.
        self.configure("ATSP0", "restore automatic protocol detection")?;
        Ok(None)
    }

    /// Ask the vehicle a question every OBD-II vehicle must answer, and learn
    /// the protocol from the result.
    ///
    /// Returns `Ok(true)` when the vehicle answered. A silent bus is *not* an
    /// error here: the adapter is fine and the caller is told so through
    /// [`ConnectionState::Degraded`].
    fn negotiate_protocol(&mut self) -> AimResult<bool> {
        let probe = ObdRequest::current_data(0x00);
        self.set_header(&RequestTarget::Functional)?;
        let r = self.send_raw(&probe.to_elm_command(), self.config.request_timeout)?;
        tracing::debug!(class = r.class.as_str(), lines = ?r.lines, "ATSP0 auto-detect probe");

        // Automatic detection failing is not the same as the vehicle being
        // silent, and a cheap clone is bad enough at `ATSP0` that treating it
        // as the final word gives up on cars that work perfectly.
        //
        // Observed on a real vehicle: the adapter initialised cleanly, `ATRV`
        // read 14.0 V (so the port was live and the ignition on), and every
        // request still came back `..UNABLE TO CONNECT` — the dots being the
        // ELM327's own protocol search. The same adapter had worked on another
        // vehicle minutes earlier.
        //
        // So when auto-detect fails, ask for each protocol explicitly. This is
        // what a human does with a terminal, and it is the difference between
        // "this app does not work on my car" and a five-second delay.
        let r = if r.class.is_success() {
            r
        } else {
            // Before spending nine protocol attempts on a socket that cannot
            // answer, ask whether it is powered at all. An unpowered socket and
            // a powered one with a silent bus produce identical symptoms —
            // every protocol failing — and they are completely different
            // problems. Getting this backwards once sent a user looking for a
            // blown fuse on a vehicle that was working perfectly.
            if let Some(v) = self.battery_voltage {
                if v < SOCKET_POWERED_VOLTS {
                    self.caps.add_caveat(format!(
                        "the diagnostic socket is reading {v:.1} V, so it is not powered. No \
                         protocol can answer through an unpowered socket, and none were tried. \
                         Check the ignition is on and the socket's fuse before anything else."
                    ));
                    return Ok(false);
                }
            }
            match self.sweep_protocols()? {
                Some(answer) => answer,
                // Everything failed at the default window. Before concluding
                // the vehicle is silent, give it the longest window we allow
                // and ask once more on automatic detection. A vehicle that is
                // merely slow answers here, and the cost of being wrong is one
                // request rather than a person being told their car is dead.
                None if self.widen_to_ceiling() => {
                    self.configure("ATSP0", "retry auto-detect with the widest window")?;
                    let retry =
                        self.send_raw(&probe.to_elm_command(), self.config.request_timeout)?;
                    if retry.class == ResponseClass::Data {
                        self.caps.add_caveat(format!(
                            "this vehicle answered only after the response window was widened to \
                             {} ms. It is slower to reply than most, which is worth knowing if \
                             readings seem intermittent",
                            self.response_window_ms
                        ));
                        self.record_window_success();
                        retry
                    } else {
                        self.no_answer_caveat(r.class.as_str());
                        return Ok(false);
                    }
                }
                None => {
                    self.no_answer_caveat(r.class.as_str());
                    return Ok(false);
                }
            }
        };

        if !r.class.is_success() {
            self.caps.add_caveat(format!(
                "vehicle did not answer service 01 PID 00 during connect ({})",
                r.class.as_str()
            ));
            return Ok(false);
        }

        // The vehicle answered, which means this adapter demonstrably put a
        // request on the bus. That is an observation, so it becomes a
        // capability rather than an assumption.
        self.caps.supports_transmit = true;

        // It also proves the response window is long enough for this vehicle,
        // which is what stops every later `NO DATA` being retried to re-learn
        // something already established.
        self.record_window_success();

        let dpn = self.send_raw("ATDPN", self.config.at_timeout)?;
        self.protocol = dpn
            .lines
            .first()
            .and_then(|l| parse_protocol_number(l))
            .unwrap_or(ObdProtocol::Unknown);
        if self.protocol == ObdProtocol::Unknown {
            self.caps.add_caveat("adapter did not report a usable protocol number from ATDPN");
        }
        if !self.protocol.is_can() && self.protocol != ObdProtocol::Unknown {
            self.caps.add_caveat(format!(
                "{}: multi-line reassembly on non-CAN protocols is not validated by this project",
                self.protocol.label()
            ));
        }

        // Now that the protocol is known, put its broadcast header in place
        // while still connecting. Leaving it until the first request would make
        // that one request carry an extra command, and the failure of an `ATSH`
        // would then surface as a failure of whatever the caller happened to
        // ask for first.
        self.apply_functional_header(self.protocol)?;

        // Throughput is measured, not declared: the round trips we just made
        // are the only honest evidence available at connect time.
        let mean = self.mean_latency_ms();
        // The ceiling follows the hardware, not the product.
        let ceiling = if self.stn_confirmed { THROUGHPUT_CEILING_STN } else { THROUGHPUT_CEILING };
        self.caps.max_reliable_throughput = if self.latency_samples == 0 {
            0.0
        } else if mean <= 0.0 {
            // Every round trip completed inside one millisecond, so the clock
            // cannot resolve a rate. Report the ceiling as a floor estimate
            // and say why, rather than reporting a measured zero.
            self.caps.add_caveat(
                "throughput is a floor estimate: round trips completed faster than the \
                 millisecond clock could measure",
            );
            ceiling
        } else {
            (1000.0 / mean).clamp(0.5, ceiling)
        };
        Ok(true)
    }

    fn read_voltage_inner(&mut self) -> AimResult<f64> {
        let r = self.send_raw("ATRV", self.config.at_timeout)?;
        if let Some(e) = r.error() {
            return Err(e);
        }
        r.lines.iter().find_map(|l| parse_voltage(l)).ok_or_else(|| {
            AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("ATRV returned {:?}, which is not a voltage", r.lines),
            )
        })
    }

    fn mean_latency_ms(&self) -> f64 {
        if self.latency_samples == 0 {
            0.0
        } else {
            self.latency_total_ms as f64 / self.latency_samples as f64
        }
    }

    /// Set the broadcast header belonging to `protocol`, whatever the adapter
    /// is currently negotiated to.
    ///
    /// Used by the sweep, where the protocol under test is not yet the
    /// connection's protocol.
    /// Write a request header to the adapter, in the form that adapter accepts.
    ///
    /// A 29-bit header is four bytes, and the ELM327 does not take all four in
    /// one command: the leading priority byte goes to `ATCP` and the remaining
    /// three to `ATSH`. The one-command form is documented for v1.3 and later,
    /// but that is a claim about firmware and this is a market full of clones —
    /// measured on a device whose banner reads `ELM327 v1.5`, `ATSH18DB33F1`
    /// came back as an error while the split form worked. Splitting always is
    /// correct on every version, so there is nothing to detect and nothing to
    /// fall back from.
    ///
    /// Returns whether the adapter accepted it.
    fn send_header(&mut self, header: &str) -> AimResult<bool> {
        if header.len() == 8 {
            let (priority, rest) = header.split_at(2);
            let cp = self.send_with_recovery(&format!("ATCP{priority}"), self.config.at_timeout)?;
            if !cp.class.is_success() {
                return Ok(false);
            }
            let sh = self.send_with_recovery(&format!("ATSH{rest}"), self.config.at_timeout)?;
            return Ok(sh.class.is_success());
        }
        let r = self.send_with_recovery(&format!("ATSH{header}"), self.config.at_timeout)?;
        Ok(r.class.is_success())
    }

    fn apply_functional_header(&mut self, protocol: ObdProtocol) -> AimResult<()> {
        let Some(header) = protocol.functional_header() else {
            return Ok(());
        };
        if self.current_header.as_deref() == Some(header) {
            return Ok(());
        }
        if self.send_header(header)? {
            self.current_header = Some(String::from(header));
        } else {
            // An adapter that refuses a header for a protocol it has just been
            // set to is telling us it cannot speak that protocol. That is a
            // reason to move on to the next candidate, not to abandon the
            // sweep, so the header is forgotten rather than the error raised.
            self.current_header = None;
        }
        Ok(())
    }

    /// Point the adapter at a broadcast or a single module.
    ///
    /// A functional (broadcast) header is a property of the *protocol*, not of
    /// the request, so it is looked up from the negotiated protocol rather than
    /// hardcoded. Forcing the 11-bit `7DF` onto a 29-bit vehicle sends a
    /// malformed request that no module answers.
    fn set_header(&mut self, target: &RequestTarget) -> AimResult<()> {
        let header = match target {
            RequestTarget::Functional => match self.protocol.functional_header() {
                Some(h) => String::from(h),
                // No protocol established yet, so there is no right header to
                // set. The adapter's own per-protocol default is correct and a
                // guess is not, so leave it alone.
                None => return Ok(()),
            },
            RequestTarget::Physical(h) => h.clone(),
        };
        if self.current_header.as_deref() == Some(header.as_str()) {
            return Ok(());
        }
        if !self.send_header(&header)? {
            return Err(AimError::new(
                ErrorCode::AdapterRejectedCommand,
                format!("adapter refused to set request header {header}"),
            )
            .with_capabilities(self.caps.clone()));
        }
        self.current_header = Some(header);
        Ok(())
    }

    fn ensure_usable(&self) -> AimResult<()> {
        if self.state.is_usable() {
            Ok(())
        } else {
            Err(AimError::new(
                ErrorCode::NoActiveSession,
                format!("adapter is {} and cannot service requests", self.state.name()),
            )
            .with_capabilities(self.caps.clone()))
        }
    }

    /// Rebuild the link after it dropped. Bounded by
    /// [`Elm327Config::max_reconnect_attempts`]; each attempt is announced.
    fn reconnect(&mut self) -> AimResult<()> {
        for attempt in 1..=self.config.max_reconnect_attempts {
            self.set_state(ConnectionState::Reconnecting { attempt });
            let _ = self.transport.close();
            if self.transport.open().is_err() {
                continue;
            }
            self.current_header = None;
            if self.initialize().is_err() {
                continue;
            }
            match self.negotiate_protocol() {
                Ok(true) => {
                    self.set_state(ConnectionState::Ready);
                    return Ok(());
                }
                Ok(false) => {
                    self.set_state(ConnectionState::Degraded {
                        reason: String::from("reconnected but the vehicle is not answering"),
                    });
                    return Ok(());
                }
                Err(_) => continue,
            }
        }
        let err = AimError::new(
            ErrorCode::TransportDisconnected,
            format!(
                "link lost and {} reconnection attempts failed",
                self.config.max_reconnect_attempts
            ),
        );
        self.set_state(ConnectionState::Failed { code: err.code, detail: err.message.clone() });
        Err(err)
    }

    /// Send, and on a link-level fault rebuild the link once and retry.
    fn send_with_recovery(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> AimResult<AdapterResponse> {
        let reply = match self.send_raw(command, timeout) {
            Ok(r) => r,
            Err(e) if is_link_fault(&e) => {
                self.reconnect()?;
                self.send_raw(command, timeout)?
            }
            Err(e) => return Err(e),
        };

        // Adapter commands are answered by the adapter itself and never by a
        // vehicle, so the response window has nothing to do with them. They can
        // still hit a glitch that is worth recovering from, which is why this
        // is a guard on the timing arms rather than an early return.
        let vehicle_traffic = !is_at_command(command);

        match reply.class {
            ResponseClass::Data if vehicle_traffic => {
                self.record_window_success();
                Ok(reply)
            }
            // `NO DATA` is the most common reply from a vehicle that is in fact
            // present. It can mean nothing is there; it can equally mean we
            // stopped listening too early. Asking once more with a longer
            // window is what separates the two.
            ResponseClass::NoData | ResponseClass::Timeout if vehicle_traffic => {
                // Only while the window is still unproven. Once something on
                // this vehicle has answered at the current window, the window
                // is known to be long enough for this vehicle, and a later
                // `NO DATA` means there is nothing there - retrying it would
                // double the cost of every unsupported PID to re-learn a fact
                // already established.
                //
                // Not during a discovery sweep either. Silence at 200 of 240
                // addresses is the expected result there, and retrying each one
                // would double the sweep while dragging the window to its
                // ceiling. The caller's own budget says which case this is: a
                // probe budget is far too small to hold a widened window, a
                // read budget comfortably holds one.
                let widened = Duration::from_millis(u64::from(self.widened_window_ms()));
                if self.proven_window_ms.is_some()
                    || widened >= timeout
                    || !self.widen_response_window()
                {
                    return Ok(reply);
                }
                let again = self.send_raw(command, timeout)?;
                if again.class == ResponseClass::Data {
                    tracing::debug!(
                        window_ms = self.response_window_ms,
                        command,
                        "answered only after the response window was widened"
                    );
                    self.record_window_success();
                }
                Ok(again)
            }
            // The adapter lost its footing rather than the vehicle answering.
            // These are recoverable, and losing an address in the middle of a
            // 255-address sweep to a transient buffer overflow is a worse
            // outcome than one extra request.
            ResponseClass::BufferFull | ResponseClass::Stopped => {
                if self.recover_adapter("ATWS", command) {
                    return self.send_raw(command, timeout);
                }
                Ok(reply)
            }
            // The protocol itself came unstuck. Closing and reopening it is
            // what AndrOBD does here and it is the difference between a scan
            // that survives a glitch and one that ends on it.
            ResponseClass::BusError => {
                if self.recover_adapter("ATPC", command) {
                    return self.send_raw(command, timeout);
                }
                Ok(reply)
            }
            _ => Ok(reply),
        }
    }

    /// Put the adapter back in a usable state after a recoverable glitch.
    ///
    /// `ATWS` is a warm start: it clears the device without the full reset and
    /// reconfiguration `ATZ` costs, which matters because `ATZ` would discard
    /// the echo, header and spacing settings this session depends on.
    /// `ATPC` closes the current protocol so the next request reopens it.
    ///
    /// Returns whether a retry is worth making. Bounded to one attempt per
    /// request, and recorded rather than hidden: a scan that quietly retried
    /// its way to a clean result would be worse than one that reported the
    /// glitch, so the recovery is always announced to the flight recorder by
    /// virtue of going through `send_raw`.
    fn recover_adapter(&mut self, command: &str, failed: &str) -> bool {
        // A bus error in reply to an `AT` command is the adapter describing the
        // bus, not the adapter failing, so closing the protocol and asking
        // again would say nothing new. A buffer overflow or an interrupted
        // exchange is a genuine glitch whatever provoked it, including a header
        // command — and losing a whole request because a transient fault landed
        // on its `ATSH` is exactly the fragility this exists to remove.
        if command == "ATPC" && is_at_command(failed) {
            return false;
        }
        tracing::debug!(recovery = command, after = failed, "recovering the adapter");
        let recovered =
            self.send_raw(command, self.config.at_timeout).map(|r| r.class.is_success());
        match recovered {
            Ok(true) => {
                // A warm start really does return the device to its power-on
                // settings: echo back on, headers off, spaces off. Retrying
                // without restoring them produces a reply nothing downstream
                // can parse, which is a worse failure than the one being
                // recovered from.
                if command == "ATWS" {
                    self.restore_working_configuration();
                }
                true
            }
            _ => false,
        }
    }

    /// Re-apply the settings a reset clears, after a warm start.
    ///
    /// Only the ones everything downstream depends on. Failures are recorded
    /// in the observer by `send_raw` and otherwise ignored: this runs while
    /// already recovering from a fault, and a second failure here should leave
    /// the original error to be reported rather than replace it.
    fn restore_working_configuration(&mut self) {
        let _ = self.send_raw("ATE0", self.config.at_timeout);
        let _ = self.send_raw("ATL0", self.config.at_timeout);
        let spaces = if self.spaces_enabled { "ATS1" } else { "ATS0" };
        let _ = self.send_raw(spaces, self.config.at_timeout);
        if self.headers_enabled {
            let _ = self.send_raw("ATH1", self.config.at_timeout);
        }
        // The protocol survives a warm start but the request header does not.
        self.current_header = None;
        let _ = self.apply_response_window();
    }

    /// Record that nothing answered, saying what was ruled out along the way.
    ///
    /// The voltage is included so that "powered, and still nothing answered" is
    /// stated rather than left for somebody to work out — the two failures look
    /// identical from outside and have nothing else in common.
    fn no_answer_caveat(&mut self, class: &str) {
        let power = match self.battery_voltage {
            Some(v) => format!(
                ". The socket is powered ({v:.1} V by the adapter's own uncalibrated \
                 measurement), so this is not a power problem"
            ),
            None => String::from(
                ". This adapter could not report socket voltage, so whether the socket is \
                 powered was not established",
            ),
        };
        self.caps.add_caveat(format!(
            "vehicle did not answer service 01 PID 00 during connect ({class}), and no protocol \
             from ATSP1 to ATSP9 answered either, including a final attempt with the response \
             window widened to {} ms{power}",
            self.response_window_ms
        ));
    }

    /// Open the response window as far as it goes, for one last attempt.
    ///
    /// Returns false when it was already there, so the caller does not repeat
    /// an attempt that has effectively already been made.
    fn widen_to_ceiling(&mut self) -> bool {
        if self.response_window_ms >= RESPONSE_WINDOW_MAX_MS {
            return false;
        }
        self.response_window_ms = RESPONSE_WINDOW_MAX_MS;
        self.apply_response_window()
    }

    /// What the window would become if widened. Does not change anything.
    fn widened_window_ms(&self) -> u32 {
        (self.response_window_ms * 2).min(RESPONSE_WINDOW_MAX_MS)
    }

    /// Give the vehicle longer to answer. Returns whether anything moved.
    fn widen_response_window(&mut self) -> bool {
        let next = self.widened_window_ms();
        if next == self.response_window_ms {
            return false;
        }
        self.response_window_ms = next;
        self.apply_response_window()
    }

    /// Record that the current window worked, and ease back toward the
    /// shortest one that has.
    ///
    /// Without the easing, one slow module would leave every later request
    /// waiting for a delay only that module needed.
    fn record_window_success(&mut self) {
        let proven = self
            .proven_window_ms
            .map_or(self.response_window_ms, |p| p.min(self.response_window_ms));
        self.proven_window_ms = Some(proven);
        if self.response_window_ms > proven {
            // A quarter of the gap at a time rather than straight back down:
            // snapping to the proven value on every success would oscillate
            // against a module that is only sometimes slow.
            let gap = self.response_window_ms - proven;
            let next = (self.response_window_ms - gap.div_ceil(4)).max(proven);
            if next != self.response_window_ms {
                self.response_window_ms = next;
                self.apply_response_window();
            }
        }
    }

    /// Write the current window to the device with `ATST`.
    ///
    /// `ATST` counts in units of 4 ms, so the value sent is the window divided
    /// by four. Returns whether the device accepted it; a device that refuses
    /// keeps whatever window it had, and the learned value simply stops being
    /// applied rather than the request failing.
    fn apply_response_window(&mut self) -> bool {
        let counts = (self.response_window_ms / ATST_RESOLUTION_MS).clamp(1, 255);
        match self.send_raw(&format!("ATST{counts:02X}"), self.config.at_timeout) {
            Ok(r) => r.class.is_success(),
            Err(_) => false,
        }
    }

    /// Turn adapter reply lines into one message per responding ECU.
    fn assemble(&self, lines: &[String]) -> AimResult<Vec<EcuMessage>> {
        if !self.headers_enabled {
            return assemble_headerless(lines);
        }
        if self.protocol.is_can() || self.protocol == ObdProtocol::Unknown {
            assemble_can(lines, self.spaces_enabled, self.protocol)
        } else {
            Ok(assemble_non_can(lines))
        }
    }
}

impl DiagnosticAdapter for Elm327Adapter {
    fn descriptor(&self) -> String {
        self.transport.descriptor()
    }

    fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.caps.clone()
    }

    fn set_observer(&mut self, observer: ObserverRef) {
        self.observer = observer;
    }

    fn health(&self) -> AdapterHealth {
        AdapterHealth {
            state: self.state.clone(),
            requests: self.requests,
            responses: self.responses,
            timeouts: self.timeouts,
            no_data: self.no_data,
            adapter_errors: self.adapter_errors,
            mean_latency_ms: self.mean_latency_ms(),
            battery_voltage: self.battery_voltage,
            protocol: self.protocol,
        }
    }

    fn protocol(&self) -> ObdProtocol {
        self.protocol
    }

    fn connect(&mut self) -> AimResult<()> {
        self.set_state(ConnectionState::Connecting);
        self.caps = AdapterCapabilities::unknown(self.transport.kind());
        self.caps.baud = self.transport.baud();

        if let Err(e) = self.transport.open() {
            self.set_state(ConnectionState::Failed { code: e.code, detail: e.message.clone() });
            self.observer.on_failure(None, &e);
            return Err(e);
        }

        self.set_state(ConnectionState::Initializing);
        if let Err(e) = self.initialize() {
            self.set_state(ConnectionState::Failed { code: e.code, detail: e.message.clone() });
            self.observer.on_failure(None, &e);
            return Err(e);
        }

        self.set_state(ConnectionState::Identifying);
        if let Err(e) = self.identify() {
            self.set_state(ConnectionState::Failed { code: e.code, detail: e.message.clone() });
            self.observer.on_failure(None, &e);
            return Err(e);
        }

        // The adapter is now known-good. Whether a *vehicle* is present is a
        // separate question, and the answer is a state, not an exception: a
        // garage laptop connected to an adapter on the bench is a real and
        // supported situation.
        let answering = match self.negotiate_protocol() {
            Ok(v) => v,
            Err(e) => {
                self.set_state(ConnectionState::Failed { code: e.code, detail: e.message.clone() });
                self.observer.on_failure(None, &e);
                return Err(e);
            }
        };

        self.observer.on_identified(&self.caps);
        if answering {
            self.set_state(ConnectionState::Ready);
        } else {
            self.set_state(ConnectionState::Degraded {
                reason: String::from(
                    "adapter identified but the vehicle did not answer service 01 PID 00",
                ),
            });
        }
        Ok(())
    }

    fn disconnect(&mut self) -> AimResult<()> {
        if self.transport.is_open() {
            // Best effort: tell the adapter to drop the bus politely. A device
            // that refuses is not worth failing a disconnect over.
            let _ = self.send_raw("ATPC", self.config.at_timeout);
            // And reset it, so whatever opens this device next finds it in its
            // power-on state rather than wearing this session's configuration -
            // headers on, a protocol pinned by the sweep, programmable
            // parameters changed. That "whatever" includes a phone app, another
            // scan tool, or this app after a reconnect, none of which asked to
            // inherit our settings.
            let _ = self.send_raw("ATZ", self.config.reset_timeout);
        }
        let r = self.transport.close();
        self.current_header = None;
        self.protocol = ObdProtocol::Unknown;
        self.set_state(ConnectionState::Disconnected);
        r
    }

    fn read_battery_voltage(&mut self) -> AimResult<f64> {
        self.ensure_usable()?;
        let v = self.read_voltage_inner()?;
        self.battery_voltage = Some(v);
        Ok(v)
    }

    fn request(
        &mut self,
        request: &ObdRequest,
        target: &RequestTarget,
    ) -> AimResult<Vec<EcuMessage>> {
        self.ensure_usable()?;
        self.set_header(target)?;
        let command = request.to_elm_command();
        let reply = self.send_with_recovery(&command, self.config.request_timeout)?;

        // ok_lines() converts NO DATA / ? / UNABLE TO CONNECT into structured
        // errors. Nothing is swallowed.
        let lines = reply.ok_lines().map_err(|e| e.with_capabilities(self.caps.clone()))?;

        let messages = self.assemble(lines)?;
        if messages.is_empty() {
            return Err(AimError::new(ErrorCode::NoData, format!("no ECU answered {command}"))
                .with_capabilities(self.caps.clone()));
        }
        Ok(messages)
    }

    fn request_pdu(
        &mut self,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: std::time::Duration,
    ) -> AimResult<Vec<EcuMessage>> {
        self.ensure_usable()?;
        self.set_header(target)?;
        let command: String = pdu.iter().map(|b| format!("{b:02X}")).collect();
        let reply = self.send_with_recovery(&command, timeout)?;

        // Deliberately does NOT go through `ok_lines()`.
        //
        // That helper turns NO DATA into an error, which is right for "read
        // this PID" and wrong for a discovery sweep, where silence at 200 of
        // 240 addresses is the expected result rather than 200 failures. The
        // caller gets an empty list and decides what it means.
        let lines = match reply.ok_lines() {
            Ok(lines) => lines,
            Err(_) => return Ok(Vec::new()),
        };
        Ok(self.assemble(lines).unwrap_or_default())
    }

    fn raw_command(&mut self, command: &str) -> AimResult<AdapterResponse> {
        self.send_with_recovery(command, self.config.request_timeout)
    }

    fn select_bus(&mut self, bus: crate::VehicleBus) -> AimResult<()> {
        if self.current_bus == bus {
            return Ok(());
        }
        if !self.caps.multiple_can_buses {
            return Err(AimError::new(
                ErrorCode::OperationNotAllowed,
                "this adapter did not accept the commands needed to change bus speed, so only \
                 the high-speed bus is reachable with it",
            ));
        }

        // Programmable protocol B, which is where a non-default bit rate lives
        // on ELM327-class hardware. The divisor is 500 / rate: 0x01 for the
        // 500 kbit/s bus, 0x04 for 125 kbit/s.
        //
        // `ATPB` takes two bytes: the first configures the protocol options
        // (0xC0 selects 11-bit ids with a variable data-length code), the
        // second is the divisor.
        let divisor = 500 / bus.kbits().max(1);
        let ok = self.configure(
            &format!("ATPB C0 {divisor:02X}"),
            "set the bus bit rate for a protocol change",
        )?;
        if !ok {
            return Err(AimError::new(
                ErrorCode::AdapterRejectedCommand,
                format!("the adapter refused to configure the {}", bus.label()),
            ));
        }
        if !self.configure("ATSPB", "switch to the reconfigured protocol")? {
            return Err(AimError::new(
                ErrorCode::AdapterRejectedCommand,
                "the adapter refused to switch to the reconfigured protocol",
            ));
        }

        // The header cache describes the old bus and is now meaningless.
        self.current_header = None;
        self.current_bus = bus;
        Ok(())
    }

    fn current_bus(&self) -> crate::VehicleBus {
        self.current_bus
    }

    fn prefer_protocol(&mut self, protocol: Option<ObdProtocol>) {
        self.set_preferred(protocol);
    }
}

/// Upper bound on the requests-per-second figure, for a generic ELM327-class
/// device. A faster measurement than this means the measurement, not the
/// adapter, is the limit.
///
/// Not a property of the product. STN-series hardware genuinely exceeds it, and
/// clamping such a device to a number learned from a cheap clone would make the
/// app slower on better hardware for no reason - see `THROUGHPUT_CEILING_STN`.
/// What a vehicle electrical system can actually sit at, key on or engine
/// running. Deliberately wide: 12 V systems idle near 14.5 V charging and can
/// dip to 11 V cranking, and 24 V commercial systems exist. A reading outside
/// even this range is the adapter being wrong, not the vehicle being unusual.
const PLAUSIBLE_SYSTEM_VOLTS: std::ops::RangeInclusive<f64> = 9.0..=32.0;

/// True for a command addressed to the adapter rather than to the vehicle.
///
/// `AT` is the ELM327's own prefix and `ST` is the STN extension set. Anything
/// else is hex going out on the bus.
fn is_at_command(command: &str) -> bool {
    let c = command.trim_start();
    c.len() >= 2 && (c[..2].eq_ignore_ascii_case("AT") || c[..2].eq_ignore_ascii_case("ST"))
}

/// Where the learned response window starts, in milliseconds.
///
/// The ELM327's own power-on default is roughly this, and it is enough for a
/// healthy module on CAN.
const RESPONSE_WINDOW_DEFAULT_MS: u32 = 200;

/// The longest the adapter is ever told to wait for one reply.
///
/// A real ceiling, not a suggestion. Widening until something answers would
/// turn every genuine `NO DATA` into a slow `NO DATA`, and a protocol that only
/// ever answers here is marginal rather than working.
const RESPONSE_WINDOW_MAX_MS: u32 = 1000;

/// `ATST` counts in units of 4 ms.
const ATST_RESOLUTION_MS: u32 = 4;

/// Below this, the diagnostic socket is not powered and no protocol can answer.
///
/// Deliberately far under any working system voltage rather than near it. This
/// is a floor test for "is there power here at all", not a battery-health
/// judgement: adapter voltage readings are uncalibrated — one cable reported
/// 28.3 V on a healthy 14 V system — so the only thing they can carry safely is
/// the difference between a live socket and a dead one. Six volts is the
/// threshold python-OBD settled on for the same purpose.
const SOCKET_POWERED_VOLTS: f64 = 6.0;

const THROUGHPUT_CEILING: f64 = 100.0;

/// The same bound for confirmed STN-series firmware.
///
/// A different class of hardware, not a faster version of the same one: real
/// multi-frame handling, hardware-timed request pipelining, and no need for the
/// text-terminal round trip that limits a generic ELM327. Clamping such a
/// device to the generic ceiling would make the app slower on better hardware
/// for no reason other than a constant.
const THROUGHPUT_CEILING_STN: f64 = 400.0;

/// True for errors that mean the link itself is gone rather than the request
/// being wrong.
fn is_link_fault(e: &AimError) -> bool {
    matches!(
        e.code,
        ErrorCode::TransportDisconnected | ErrorCode::TransportIo | ErrorCode::TransportOpenFailed
    )
}

/// Parse `ATDPN` output: `A6` (auto, protocol 6) or `6`.
pub fn parse_protocol_number(line: &str) -> Option<ObdProtocol> {
    let t = line.trim().trim_start_matches(['A', 'a']);
    let digit = t.chars().next()?;
    let id = digit.to_digit(16)? as u8;
    match ObdProtocol::from_elm_id(id) {
        ObdProtocol::Unknown => None,
        p => Some(p),
    }
}

/// Parse `ATRV` output: `12.6V`.
pub fn parse_voltage(line: &str) -> Option<f64> {
    let t = line.trim().trim_end_matches(['V', 'v']).trim();
    t.parse::<f64>().ok().filter(|v| (0.0..=60.0).contains(v))
}

/// Split one printed line into a header token and its data bytes.
///
/// Spaced lines are unambiguous. Unspaced ones are split using the header
/// width implied by the protocol — 3 hex digits for 11-bit, 8 for 29-bit. When
/// the protocol is not yet known, the width is inferred from the line length
/// (an 11-bit line has an odd number of hex digits, a 29-bit line an even one)
/// and a line that fits neither is rejected rather than guessed at.
fn split_header_line(
    line: &str,
    spaces: bool,
    protocol: ObdProtocol,
) -> AimResult<(String, Vec<u8>)> {
    let trimmed = line.trim();
    if spaces && trimmed.contains(' ') {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        // With spaces on, an 11-bit header arrives as a single three-digit
        // token (`7E8 06 41 ...`) but a 29-bit one arrives as four separate
        // byte tokens (`18 DA F1 10 06 41 ...`). Taking the first token either
        // way reads `18` as the whole identifier and rejects the line.
        //
        // With no protocol established, the width of the first token says
        // which it is: three hex digits can only be an 11-bit identifier.
        let header_tokens = if protocol.is_can() {
            if protocol.is_29_bit() {
                4
            } else {
                1
            }
        } else if parts.first().is_some_and(|t| t.len() == 3) {
            1
        } else {
            4
        };
        if parts.len() <= header_tokens {
            return Err(AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("adapter line {trimmed:?} is too short to contain a header and data"),
            ));
        }
        let header = parts[..header_tokens].concat();
        let mut data = Vec::new();
        for p in &parts[header_tokens..] {
            data.push(u8::from_str_radix(p, 16).map_err(|e| {
                AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("invalid byte {p:?} in adapter line {trimmed:?}: {e}"),
                )
            })?);
        }
        return Ok((header, data));
    }

    let hex: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("adapter line {trimmed:?} is not hexadecimal"),
        ));
    }
    // An 11-bit header prints as 3 hex digits and a 29-bit one as 8. When the
    // protocol is known it decides; otherwise parity does, because an odd
    // number of hex digits can only have come from a 3-digit header.
    const HEADER_11_BIT: usize = 3;
    const HEADER_29_BIT: usize = 8;
    let is_11_bit = if protocol.is_can() { !protocol.is_29_bit() } else { hex.len() % 2 == 1 };
    let header_width = if is_11_bit { HEADER_11_BIT } else { HEADER_29_BIT };
    if hex.len() <= header_width {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("adapter line {trimmed:?} is too short to contain a header and data"),
        ));
    }
    let (header, rest) = hex.split_at(header_width);
    if rest.len() % 2 != 0 {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("adapter line {trimmed:?} has a trailing half byte"),
        ));
    }
    let data = (0..rest.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&rest[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|e| {
            AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("invalid data in adapter line {trimmed:?}: {e}"),
            )
        })?;
    Ok((header.to_string(), data))
}

/// Reassemble CAN responses, one ISO-TP stream per source address.
pub fn assemble_can(
    lines: &[String],
    spaces: bool,
    protocol: ObdProtocol,
) -> AimResult<Vec<EcuMessage>> {
    // BTreeMap keeps output ordered by address, so a multi-module scan is
    // deterministic and its golden transcripts are stable.
    let mut receivers: BTreeMap<String, IsoTpReceiver> = BTreeMap::new();
    let mut raw: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut done: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    for line in lines {
        let (header, data) = split_header_line(line, spaces, protocol)?;
        // Validate the header is a real CAN id; this rejects stray text that
        // slipped past classification instead of treating it as a frame.
        let id = CanId::parse_hex(&header)?;
        let frame = CanFrame::new(id, data)?;
        let address = frame.id.to_hex();
        raw.entry(address.clone()).or_default().push(line.clone());

        let isotp = IsoTpFrame::parse(&frame.data)?;
        let rx = receivers.entry(address.clone()).or_insert_with(|| {
            // Block size 0 / STmin 0: the adapter owns flow control, so the
            // values we would advertise are never transmitted.
            IsoTpReceiver::new(0, 0)
        });
        match rx.feed(isotp)? {
            ReceiveOutcome::Complete(payload) => {
                done.insert(address, payload);
            }
            ReceiveOutcome::NeedMore | ReceiveOutcome::SendFlowControl(_) => {}
        }
    }

    // A module that started a segmented message and never finished it is a
    // truncated read, not a message. Report it rather than returning a partial
    // payload that would decode into a plausible-looking lie.
    for (address, rx) in &receivers {
        if rx.in_progress() && !done.contains_key(address) {
            return Err(AimError::new(
                ErrorCode::IsoTpError,
                format!("module {address} sent an incomplete multi-frame response"),
            )
            .with_details(serde_json::json!({
                "address": address,
                "lines": raw.get(address).cloned().unwrap_or_default(),
            })));
        }
    }

    Ok(done
        .into_iter()
        .map(|(address, payload)| EcuMessage {
            raw_lines: raw.get(&address).cloned().unwrap_or_default(),
            address,
            payload,
        })
        .collect())
}

/// Reassemble non-CAN (J1850 / ISO 9141 / KWP) responses.
///
/// These protocols print a three-byte header and the adapter has already
/// stripped the checksum. Consecutive lines from the same source are
/// concatenated. This project has **not** validated multi-line reassembly on
/// these protocols against real hardware, which is why
/// [`Elm327Adapter::negotiate_protocol`] records a caveat when one is
/// negotiated.
pub fn assemble_non_can(lines: &[String]) -> Vec<EcuMessage> {
    let mut out: Vec<EcuMessage> = Vec::new();
    for line in lines {
        let bytes: Vec<u8> =
            line.split_whitespace().filter_map(|p| u8::from_str_radix(p, 16).ok()).collect();
        if bytes.len() < 4 {
            continue;
        }
        let address = format!("{:02X}", bytes[2]);
        let payload = bytes[3..].to_vec();
        match out.last_mut() {
            Some(prev) if prev.address == address => {
                prev.payload.extend_from_slice(&payload);
                prev.raw_lines.push(line.clone());
            }
            _ => out.push(EcuMessage { address, payload, raw_lines: vec![line.clone()] }),
        }
    }
    out
}

/// Reassemble when headers are off, which the adapter only allows after it has
/// refused `ATH1`.
///
/// Without headers the response cannot be attributed to a module, so the
/// address is the literal string `unknown` — never a plausible-looking guess.
/// The ELM327 prints long headerless responses as a length line followed by
/// `n:`-prefixed rows, which is handled here.
pub fn assemble_headerless(lines: &[String]) -> AimResult<Vec<EcuMessage>> {
    let mut payload = Vec::new();
    let mut used = Vec::new();
    for line in lines {
        let body = match line.split_once(':') {
            Some((idx, rest)) if idx.trim().chars().all(|c| c.is_ascii_hexdigit()) => rest,
            _ => line.as_str(),
        };
        let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        if cleaned.is_empty() || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        // A bare 3-digit line is the total-length prefix of a segmented
        // headerless response, not data.
        if cleaned.len() == 3 && !line.contains(':') {
            continue;
        }
        if cleaned.len() % 2 != 0 {
            return Err(AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("headerless adapter line {line:?} has a trailing half byte"),
            ));
        }
        for i in (0..cleaned.len()).step_by(2) {
            payload.push(u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|e| {
                AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("invalid byte in {line:?}: {e}"),
                )
            })?);
        }
        used.push(line.clone());
    }
    if payload.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![EcuMessage { address: String::from("unknown"), payload, raw_lines: used }])
}

/// Release the port even when nobody called `disconnect`.
///
/// The transport already frees its handle when it drops - that is Rust doing
/// its job - so this is not about leaking the port within a process. It is
/// about the window being closed, or a panic unwinding, and the device being
/// left holding a protocol and a set of headers from a session that is over.
///
/// Deliberately silent and bounded. A drop that blocks on a slow adapter turns
/// closing the window into a hang, and a drop that panics while unwinding
/// aborts the process, so every failure here is swallowed on purpose.
impl Drop for Elm327Adapter {
    fn drop(&mut self) {
        if self.transport.is_open() {
            let _ = self.send_raw("ATPC", Duration::from_millis(200));
            let _ = self.transport.close();
        }
    }
}

impl Elm327Adapter {
    /// Internal setter; the trait method delegates here.
    ///
    /// Only reorders the sweep. A wrong hint costs one extra attempt; it
    /// cannot cause a protocol to be reported as working when it is not,
    /// because the sweep still requires actual data before accepting one.
    fn set_preferred(&mut self, protocol: Option<ObdProtocol>) {
        self.preferred_protocol = protocol.filter(|p| p.elm_id().is_some());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn protocol_number_parsing_handles_the_auto_prefix() {
        assert_eq!(parse_protocol_number("A6"), Some(ObdProtocol::Iso15765Can11_500));
        assert_eq!(parse_protocol_number("6"), Some(ObdProtocol::Iso15765Can11_500));
        assert_eq!(parse_protocol_number("A7"), Some(ObdProtocol::Iso15765Can29_500));
        assert_eq!(parse_protocol_number("A0"), None);
        assert_eq!(parse_protocol_number("nonsense"), None);
    }

    #[test]
    fn voltage_parsing_rejects_nonsense() {
        assert_eq!(parse_voltage("12.6V"), Some(12.6));
        assert_eq!(parse_voltage(" 14.1 "), Some(14.1));
        assert_eq!(parse_voltage("ELM327 v1.5"), None);
        assert_eq!(parse_voltage("999V"), None);
    }

    #[test]
    fn single_frame_can_response_assembles() {
        let msgs =
            assemble_can(&lines(&["7E8 04 41 0C 1A F8"]), true, ObdProtocol::Iso15765Can11_500)
                .unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].address, "7E8");
        assert_eq!(msgs[0].payload, vec![0x41, 0x0C, 0x1A, 0xF8]);
    }

    #[test]
    fn multi_frame_vin_response_reassembles_across_lines() {
        // Service 09 PID 02 for VIN 1FTBF2B69KEC00001, as an ELM327 prints it
        // with headers on: first frame declares 0x14 = 20 payload bytes.
        let msgs = assemble_can(
            &lines(&[
                "7E8 10 14 49 02 01 31 46 54",
                "7E8 21 42 46 32 42 36 39 4B",
                "7E8 22 45 43 30 30 30 30 31",
            ]),
            true,
            ObdProtocol::Iso15765Can11_500,
        )
        .unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload.len(), 20);
        assert_eq!(&msgs[0].payload[..3], &[0x49, 0x02, 0x01]);
        assert_eq!(String::from_utf8_lossy(&msgs[0].payload[3..]), "1FTBF2B69KEC00001");
        assert_eq!(msgs[0].raw_lines.len(), 3);
    }

    #[test]
    fn several_modules_answering_one_broadcast_produce_several_messages() {
        let msgs = assemble_can(
            &lines(&[
                "7E8 06 41 00 BE 3F A8 13",
                "7EA 06 41 00 80 00 00 01",
                "7EB 06 41 00 98 18 00 01",
            ]),
            true,
            ObdProtocol::Iso15765Can11_500,
        )
        .unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            msgs.iter().map(|m| m.address.as_str()).collect::<Vec<_>>(),
            vec!["7E8", "7EA", "7EB"]
        );
    }

    #[test]
    fn interleaved_multi_frame_streams_stay_separate() {
        // Two modules segmenting at the same time is exactly the case a
        // single shared reassembly buffer would corrupt.
        let msgs = assemble_can(
            &lines(&[
                "7E8 10 0A 49 02 01 41 41 41",
                "7EA 10 0A 49 02 01 42 42 42",
                "7E8 21 41 41 41 41 00 00",
                "7EA 21 42 42 42 42 00 00",
            ]),
            true,
            ObdProtocol::Iso15765Can11_500,
        )
        .unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(&msgs[0].payload[3..], b"AAAAAAA");
        assert_eq!(&msgs[1].payload[3..], b"BBBBBBB");
    }

    #[test]
    fn a_truncated_multi_frame_response_is_an_error_not_a_partial_payload() {
        let err = assemble_can(
            &lines(&["7E8 10 14 49 02 01 31 46 54", "7E8 21 42 46 32 42 36 39 4B"]),
            true,
            ObdProtocol::Iso15765Can11_500,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::IsoTpError);
    }

    #[test]
    fn unspaced_lines_are_split_by_protocol_header_width() {
        let msgs = assemble_can(&lines(&["7E804410C1AF8"]), false, ObdProtocol::Iso15765Can11_500)
            .unwrap();
        assert_eq!(msgs[0].address, "7E8");
        assert_eq!(msgs[0].payload, vec![0x41, 0x0C, 0x1A, 0xF8]);

        let msgs =
            assemble_can(&lines(&["18DAF11004410C1AF8"]), false, ObdProtocol::Iso15765Can29_500)
                .unwrap();
        assert_eq!(msgs[0].address, "18DAF110");
        assert_eq!(msgs[0].payload, vec![0x41, 0x0C, 0x1A, 0xF8]);
    }

    #[test]
    fn garbage_lines_are_rejected_rather_than_guessed() {
        assert!(assemble_can(&lines(&["NO DATA"]), true, ObdProtocol::Iso15765Can11_500).is_err());
        assert!(assemble_can(
            &lines(&["7E8 04 41 ZZ 1A F8"]),
            true,
            ObdProtocol::Iso15765Can11_500
        )
        .is_err());
    }

    #[test]
    fn headerless_assembly_refuses_to_invent_an_address() {
        let msgs = assemble_headerless(&lines(&["41 0C 1A F8"])).unwrap();
        assert_eq!(msgs[0].address, "unknown");
        assert_eq!(msgs[0].payload, vec![0x41, 0x0C, 0x1A, 0xF8]);
    }

    #[test]
    fn headerless_multi_line_format_drops_the_length_and_row_indices() {
        let msgs = assemble_headerless(&lines(&[
            "014",
            "0: 49 02 01 31 46 54",
            "1: 42 46 32 42 36 39 4B",
            "2: 45 43 30 30 30 30 31",
        ]))
        .unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(&msgs[0].payload[..3], &[0x49, 0x02, 0x01]);
        assert_eq!(String::from_utf8_lossy(&msgs[0].payload[3..]), "1FTBF2B69KEC00001");
    }

    #[test]
    fn non_can_lines_use_the_source_byte_as_the_address() {
        let msgs = assemble_non_can(&lines(&["48 6B 10 41 00 BE 3F A8 13"]));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].address, "10");
        assert_eq!(msgs[0].payload, vec![0x41, 0x00, 0xBE, 0x3F, 0xA8, 0x13]);
    }

    #[test]
    fn link_faults_are_distinguished_from_request_faults() {
        assert!(is_link_fault(&AimError::new(ErrorCode::TransportDisconnected, "x")));
        assert!(!is_link_fault(&AimError::no_data("x")));
        assert!(!is_link_fault(&AimError::new(ErrorCode::AdapterRejectedCommand, "x")));
    }
}
