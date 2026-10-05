//! An ELM327 that answers in text.
//!
//! This is what makes the simulator worth having. It does not stub out the
//! adapter layer — it speaks the same ASCII protocol a real ELM327 speaks, so
//! `Elm327Adapter` runs unmodified against it: the same `ATZ`/`ATE0`/`ATH1`
//! handshake, the same `SEARCHING...`, the same `NO DATA`, the same raw ISO-TP
//! frames printed one per line. A bug in the adapter's parsing is therefore a
//! bug the simulator can catch.
//!
//! [`AdapterPersonality`] lets the emulator behave like a genuine device or
//! like the cheap clone actually sitting in the truck, so the capability
//! caveats the adapter records can be tested rather than assumed.

use crate::vehicle::VirtualVehicle;
use aim_protocols::{CanId, IsoTpFrame, IsoTpSender};
use std::collections::VecDeque;

/// How the emulated adapter behaves and identifies itself.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterPersonality {
    /// What `ATI` and the `ATZ` reset print.
    pub banner: String,
    /// What `AT@1` prints. `None` means it answers `?`, like most clones.
    pub description: Option<String>,
    /// The voltage `ATRV` reports.
    pub voltage: f64,
    /// AT commands this device does not implement, answered with `?`.
    pub unsupported: Vec<String>,
    /// STN firmware identifier, if present. None means the adapter is not
    /// STN-compatible.
    pub stn_firmware: Option<String>,
}

impl AdapterPersonality {
    /// A genuine ELM327 v1.5: answers everything this project uses.
    pub fn genuine_v1_5() -> Self {
        AdapterPersonality {
            banner: String::from("ELM327 v1.5"),
            description: Some(String::from("OBDII to RS232 Interpreter")),
            voltage: 14.1,
            unsupported: Vec::new(),
            stn_firmware: None,
        }
    }

    /// The cheap Bluetooth clone: claims v2.1, has no device description, and
    /// does not implement adaptive timing. Exactly the sort of device the
    /// handoff says must produce capability flags rather than assumptions.
    pub fn cheap_clone_v2_1() -> Self {
        AdapterPersonality {
            banner: String::from("ELM327 v2.1"),
            description: None,
            voltage: 14.1,
            unsupported: vec![String::from("AT@1"), String::from("ATAT1")],
            stn_firmware: None,
        }
    }

    /// An OBDLink MX+ adapter with STN firmware.
    pub fn obdlink_mx() -> Self {
        AdapterPersonality {
            banner: String::from("ELM327 v1.5"),
            description: Some(String::from("OBDLink MX+")),
            voltage: 14.1,
            unsupported: Vec::new(),
            stn_firmware: Some(String::from("STN2255 v5.10.3")),
        }
    }
}

/// A fault the emulator injects into the next command, once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedFault {
    /// Answer `BUFFER FULL`.
    BufferFull,
    /// Answer `STOPPED`.
    Stopped,
    /// Answer nothing at all, so the caller's read times out.
    Silence,
    /// Answer `?` regardless of the command.
    NotUnderstood,
    /// Answer `CAN ERROR`.
    BusError,
}

/// The emulated adapter.
#[derive(Debug, Clone)]
pub struct ElmEmulator {
    /// The vehicle behind the adapter.
    pub vehicle: VirtualVehicle,
    /// Device identity and quirks.
    pub personality: AdapterPersonality,
    echo: bool,
    headers: bool,
    spaces: bool,
    linefeeds: bool,
    /// `None` until a protocol has been established.
    protocol: Option<u8>,
    /// Which protocol `ATSP` selected; 0 means automatic.
    requested_protocol: u8,
    /// The request header: 11 bits, or all 29 once `ATCP` and a six-digit
    /// `ATSH` (or an eight-digit one) have set it.
    header: u32,
    /// The 29-bit priority byte `ATCP` sets, used by a six-digit `ATSH`.
    priority: u8,
    faults: VecDeque<InjectedFault>,
    /// Every command received, for transcript recording and assertions.
    pub log: Vec<String>,
    /// Whether we're currently on the second CAN bus.
    on_secondary_bus: bool,
    /// Baud rate of the second bus, if active.
    secondary_baud: u32,
}

impl ElmEmulator {
    /// Build an emulator in its power-on state.
    pub fn new(vehicle: VirtualVehicle, personality: AdapterPersonality) -> Self {
        ElmEmulator {
            vehicle,
            personality,
            echo: true,
            headers: false,
            spaces: true,
            linefeeds: true,
            protocol: None,
            requested_protocol: 0,
            header: 0x7DF,
            priority: 0x18,
            faults: VecDeque::new(),
            log: Vec::new(),
            on_secondary_bus: false,
            secondary_baud: 0,
        }
    }

    /// Queue a fault to be injected into the next command.
    pub fn inject(&mut self, fault: InjectedFault) {
        self.faults.push_back(fault);
    }

    /// Whether response headers are currently enabled.
    pub fn headers_enabled(&self) -> bool {
        self.headers
    }

    /// Handle one command line and produce the bytes the adapter would send
    /// back, prompt included. An empty result means the device stayed silent.
    pub fn handle_line(&mut self, line: &str) -> String {
        let raw = line.trim().to_string();
        self.log.push(raw.clone());

        let mut out = String::new();
        if self.echo {
            out.push_str(&raw);
            out.push('\r');
        }

        if let Some(fault) = self.faults.pop_front() {
            return match fault {
                InjectedFault::Silence => String::new(),
                InjectedFault::BufferFull => self.finish(out, &["BUFFER FULL"]),
                InjectedFault::Stopped => self.finish(out, &["STOPPED"]),
                InjectedFault::NotUnderstood => self.finish(out, &["?"]),
                InjectedFault::BusError => self.finish(out, &["CAN ERROR"]),
            };
        }

        let command: String =
            raw.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();

        if command.is_empty() {
            return self.finish(out, &[]);
        }

        // Handle ST commands first
        if command.starts_with("ST") {
            let lines = self.handle_st(&command);
            return self.finish(out, &lines.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        }

        if let Some(rest) = command.strip_prefix("AT") {
            let lines = self.handle_at(rest, &command);
            return self.finish(out, &lines.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        }
        let lines = self.handle_obd(&command);
        self.finish(out, &lines.iter().map(|s| s.as_str()).collect::<Vec<_>>())
    }

    fn finish(&self, mut out: String, lines: &[&str]) -> String {
        let terminator = if self.linefeeds { "\r\n" } else { "\r" };
        for l in lines {
            out.push_str(l);
            out.push_str(terminator);
        }
        out.push('\r');
        out.push('>');
        out
    }

    fn handle_at(&mut self, rest: &str, full: &str) -> Vec<String> {
        if self.personality.unsupported.iter().any(|u| u.eq_ignore_ascii_case(full)) {
            return vec![String::from("?")];
        }

        // Commands with a payload are matched by prefix; bare toggles exactly.
        if let Some(v) = rest.strip_prefix("SH") {
            // Three digits are an 11-bit header; six are the low 24 bits of a
            // 29-bit one, under the priority `ATCP` set; eight are all of it.
            let parsed = u32::from_str_radix(v, 16).ok().and_then(|h| match v.len() {
                3 => Some(h),
                6 => Some(u32::from(self.priority) << 24 | h),
                8 => Some(h),
                _ => None,
            });
            return match parsed {
                Some(h) => {
                    self.header = h;
                    vec![String::from("OK")]
                }
                None => vec![String::from("?")],
            };
        }
        if let Some(v) = rest.strip_prefix("CP") {
            return match u8::from_str_radix(v, 16) {
                Ok(p) if v.len() == 2 => {
                    self.priority = p;
                    vec![String::from("OK")]
                }
                _ => vec![String::from("?")],
            };
        }
        if let Some(v) = rest.strip_prefix("SP") {
            let digit = v.trim_start_matches(['A', 'a']);
            return match u8::from_str_radix(digit, 16) {
                Ok(p) if p <= 9 => {
                    self.requested_protocol = p;
                    // Selecting a protocol drops any established connection,
                    // so the next request searches again.
                    self.protocol = if p == 0 { None } else { Some(p) };
                    // Reset secondary bus state when changing protocols
                    self.on_secondary_bus = false;
                    vec![String::from("OK")]
                }
                _ => vec![String::from("?")],
            };
        }
        if rest.starts_with("ST") || rest.starts_with("AT") || rest.starts_with("CAF") {
            return vec![String::from("OK")];
        }

        match rest {
            "Z" | "WS" => {
                // Full reset: every setting returns to its power-on value.
                self.echo = true;
                self.headers = false;
                self.spaces = true;
                self.linefeeds = true;
                self.protocol = None;
                self.requested_protocol = 0;
                self.header = 0x7DF;
                self.priority = 0x18;
                self.on_secondary_bus = false;
                vec![self.personality.banner.clone()]
            }
            "I" => vec![self.personality.banner.clone()],
            "@1" => match &self.personality.description {
                Some(d) => vec![d.clone()],
                None => vec![String::from("?")],
            },
            "E0" => {
                self.echo = false;
                vec![String::from("OK")]
            }
            "E1" => {
                self.echo = true;
                vec![String::from("OK")]
            }
            "L0" => {
                self.linefeeds = false;
                vec![String::from("OK")]
            }
            "L1" => {
                self.linefeeds = true;
                vec![String::from("OK")]
            }
            "S0" => {
                self.spaces = false;
                vec![String::from("OK")]
            }
            "S1" => {
                self.spaces = true;
                vec![String::from("OK")]
            }
            "H0" => {
                self.headers = false;
                vec![String::from("OK")]
            }
            "H1" => {
                self.headers = true;
                vec![String::from("OK")]
            }
            "RV" => vec![format!("{:.1}V", self.personality.voltage)],
            "DPN" => match self.protocol {
                Some(p) if self.requested_protocol == 0 => vec![format!("A{p:X}")],
                Some(p) => vec![format!("{p:X}")],
                None => vec![String::from("A0")],
            },
            "DP" => match self.protocol {
                Some(p) => vec![format!("AUTO, {}", protocol_label(p))],
                None => vec![String::from("AUTO")],
            },
            "PC" => {
                // Closes the protocol; it does not choose another. The next
                // request reopens the same channel, so an adapter on the
                // secondary one is still there. Measured on an OBDLink MX+
                // (2026-10-04): `3E00` answered `CAN ERROR` on the secondary
                // channel, `ATPC` answered OK, and `3E00` answered `CAN ERROR`
                // again, 298 times.
                self.protocol = None;
                vec![String::from("OK")]
            }
            "MA" => {
                // ATMA - return traffic on secondary bus at 500kbps, on a
                // vehicle that has one. With nothing on those pins there is
                // nothing to hear at any rate.
                if self.on_secondary_bus
                    && self.secondary_baud == 500000
                    && !self.vehicle.secondary_ecus.is_empty()
                {
                    vec![
                        String::from("3B3 40 00 00 00 00 00 00 00"),
                        String::from("42C 00 00 02 00 00 00 00 00"),
                    ]
                } else {
                    vec![]
                }
            }
            _ => vec![String::from("?")],
        }
    }

    fn handle_st(&mut self, command: &str) -> Vec<String> {
        if self.personality.stn_firmware.is_none() {
            return vec![String::from("?")];
        }

        match command {
            "STI" => vec![self.personality.stn_firmware.clone().unwrap()],
            "STDI" => vec![String::from("OBDLink MX+ r1.2")],
            "STP53" => {
                self.on_secondary_bus = true;
                vec![String::from("OK")]
            }
            cmd if cmd.starts_with("STPBR") => {
                // Parse baud rate from command
                if let Ok(baud) = cmd[5..].parse::<u32>() {
                    self.secondary_baud = baud;
                    vec![String::from("OK")]
                } else {
                    vec![String::from("?")]
                }
            }
            _ => {
                // Other ST commands return OK
                vec![String::from("OK")]
            }
        }
    }

    fn handle_obd(&mut self, command: &str) -> Vec<String> {
        if !command.chars().all(|c| c.is_ascii_hexdigit()) {
            return vec![String::from("?")];
        }
        // A trailing odd digit is the optional "expected responses" hint. We
        // accept it and ignore it, as a real adapter effectively does once the
        // responses have arrived.
        let hex = if command.len() % 2 == 1 { &command[..command.len() - 1] } else { command };
        if hex.is_empty() {
            return vec![String::from("?")];
        }
        let request: Vec<u8> = (0..hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();

        let mut lines = Vec::new();
        // The secondary channel is a protocol that was chosen, not searched
        // for, so closing it and asking again finds it as it was left.
        let searching = self.protocol.is_none() && !self.on_secondary_bus;
        if searching {
            // A real search tries the non-CAN protocols first and takes several
            // seconds, longer than any discovery probe waits. Measured on a 2019
            // F-250 (2026-09-28): after `ATSP0`, every `3E00` probe was cut off
            // before the search reached CAN. Only a legislated OBD request
            // (services 01-0A) is given the time to finish one.
            if !(0x01..=0x0A).contains(&request[0]) {
                return Vec::new();
            }
            lines.push(String::from("SEARCHING..."));
        }

        // A protocol of the wrong width reaches nothing: a 29-bit vehicle does
        // not answer 11-bit frames, and the other way round. While searching,
        // the adapter tries each with its own broadcast header, so the vehicle
        // is found on its own width.
        let wants_29_bit = matches!(self.protocol, Some(7 | 9));
        let wants_11_bit = matches!(self.protocol, Some(6 | 8));
        // And a K-line vehicle answers no CAN protocol, nor a CAN vehicle a
        // K-line or J1850 one.
        let wants_can = matches!(self.protocol, Some(6..=9));
        let wants_legacy = matches!(self.protocol, Some(1..=5));
        if (self.vehicle.k_line && wants_can)
            || (!self.vehicle.k_line && wants_legacy)
            || (!self.vehicle.k_line && self.vehicle.extended && wants_11_bit)
            || (!self.vehicle.k_line && !self.vehicle.extended && wants_29_bit)
        {
            lines.push(String::from("NO DATA"));
            return lines;
        }
        // Until a header is set, the adapter uses the protocol's own default,
        // which on the K-line is the functional `68 6A F1`.
        let header = if self.vehicle.k_line && (searching || self.header == 0x7DF) {
            0x0068_6AF1
        } else if searching && self.vehicle.extended && self.header == 0x7DF {
            0x18DB_33F1
        } else {
            self.header
        };

        let replies = if self.on_secondary_bus {
            // Nothing is wired to those pins, so nothing acknowledges the
            // frame and the adapter reports that it could not send it.
            // Measured on a 2023 Honda Odyssey (2026-10-04): silence when
            // listening at 500, 250 and 125 kbit/s, then `CAN ERROR` to every
            // request, whatever address it was sent to.
            if self.vehicle.secondary_ecus.is_empty() {
                return vec![String::from("CAN ERROR")];
            }
            // Check if we're on the secondary bus with a supported baud rate
            if self.secondary_baud == 500000 {
                self.vehicle.handle_secondary(header, &request)
            } else {
                // Other baud rates return NO DATA (as per requirements)
                Vec::new()
            }
        } else {
            self.vehicle.handle(header, &request)
        };

        if replies.is_empty() {
            return if self.protocol.is_none() {
                // Nothing has ever answered, so no protocol was established.
                // This is what a real adapter says with the key off.
                vec![String::from("UNABLE TO CONNECT")]
            } else {
                lines.push(String::from("NO DATA"));
                lines
            };
        }

        // The first successful exchange is what establishes the protocol.
        if self.protocol.is_none() {
            let found = if self.vehicle.k_line {
                3
            } else if self.vehicle.extended {
                7
            } else {
                6
            };
            self.protocol =
                Some(if self.requested_protocol == 0 { found } else { self.requested_protocol });
        }

        if self.vehicle.k_line {
            for reply in replies {
                for frame in k_line_frames(&reply.payload) {
                    // Priority 48, target 6B (the tester), then the source.
                    let mut bytes = vec![0x48, 0x6B, reply.response_id as u8];
                    bytes.extend_from_slice(&frame);
                    let checksum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
                    bytes.push(checksum);
                    lines.push(self.format_bytes(&bytes));
                }
            }
            return lines;
        }

        for reply in replies {
            let id = if reply.response_id > 0x7FF {
                CanId::Extended(reply.response_id)
            } else {
                CanId::Standard(reply.response_id as u16)
            };
            for frame in segment(&reply.payload) {
                let data = frame.encode(8, 0x00);
                lines.push(self.format_frame(&id, &data));
            }
        }
        lines
    }

    /// A K-line frame as printed: with headers off, the header and checksum
    /// are not shown.
    fn format_bytes(&self, bytes: &[u8]) -> String {
        let shown = if self.headers { bytes } else { &bytes[3..bytes.len() - 1] };
        let sep = if self.spaces { " " } else { "" };
        shown.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(sep)
    }

    fn format_frame(&self, id: &CanId, data: &[u8]) -> String {
        let mut s = String::new();
        if self.headers {
            match id {
                // With spaces on, a 29-bit header prints as four bytes.
                CanId::Extended(v) if self.spaces => {
                    for b in v.to_be_bytes() {
                        s.push_str(&format!("{b:02X} "));
                    }
                }
                _ => {
                    s.push_str(&id.to_hex());
                    if self.spaces {
                        s.push(' ');
                    }
                }
            }
        }
        for (i, b) in data.iter().enumerate() {
            if self.spaces && i > 0 {
                s.push(' ');
            }
            s.push_str(&format!("{b:02X}"));
        }
        s
    }
}

/// A reply as the frames a K-line module sends, each without its header.
///
/// The simulator's modules answer in the CAN form, and SAE J1979 gives the
/// pre-CAN one:
/// - fault codes three to a frame, the service byte repeated, no count byte;
/// - vehicle information that carries a count of data items on CAN (a VIN,
///   calibration IDs) in numbered frames of four bytes, the VIN padded to 20;
/// - anything else in one frame.
fn k_line_frames(payload: &[u8]) -> Vec<Vec<u8>> {
    let service = payload[0];
    if matches!(service, 0x43 | 0x47 | 0x4A) {
        let codes = payload.get(2..).unwrap_or(&[]);
        let mut frames: Vec<Vec<u8>> = codes
            .chunks(6)
            .map(|c| {
                let mut f = vec![service];
                f.extend_from_slice(c);
                f.resize(7, 0x00);
                f
            })
            .collect();
        if frames.is_empty() {
            frames.push(vec![service, 0, 0, 0, 0, 0, 0]);
        }
        return frames;
    }
    if service == 0x49 && payload.len() > 3 && matches!(payload[1], 0x02 | 0x04 | 0x06 | 0x0A) {
        let pid = payload[1];
        // Past the count of data items, which the pre-CAN form does not send.
        let mut data = payload[3..].to_vec();
        if pid == 0x02 {
            let mut padded = vec![0x00; 20usize.saturating_sub(data.len())];
            padded.extend_from_slice(&data);
            data = padded;
        }
        return data
            .chunks(4)
            .enumerate()
            .map(|(i, c)| {
                let mut f = vec![0x49, pid, (i + 1) as u8];
                f.extend_from_slice(c);
                f.resize(7, 0x00);
                f
            })
            .collect();
    }
    vec![payload.to_vec()]
}

/// Split a payload into the ISO-TP frames an adapter would print.
///
/// The emulator is the sender, so it uses the tested [`IsoTpSender`] and grants
/// itself the flow control a receiver would send: block size 0, no separation
/// time, which is what a scan tool asks for.
fn segment(payload: &[u8]) -> Vec<IsoTpFrame> {
    let mut sender = match IsoTpSender::new(payload.to_vec(), 8) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut frames = Vec::new();
    if let Ok(Some(first)) = sender.next_frame() {
        let single = sender.is_complete();
        frames.push(first);
        if !single {
            let _ = sender.apply_flow_control(&IsoTpFrame::FlowControl {
                status: aim_protocols::isotp::FlowStatus::ContinueToSend,
                block_size: 0,
                st_min: 0,
            });
            while let Ok(Some(f)) = sender.next_frame() {
                frames.push(f);
            }
        }
    }
    frames
}

fn protocol_label(p: u8) -> &'static str {
    aim_types::ObdProtocol::from_elm_id(p).label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioId;

    fn emu(id: ScenarioId) -> ElmEmulator {
        ElmEmulator::new(VirtualVehicle::f250_2019(id), AdapterPersonality::genuine_v1_5())
    }

    /// A K-line vehicle is found by the search as protocol 3, and prints each
    /// frame with its three header bytes and a checksum that is the sum of the
    /// bytes before it. A VIN comes as five numbered frames.
    #[test]
    fn a_k_line_vehicle_prints_framed_checksummed_replies() {
        let mut e = ElmEmulator::new(
            VirtualVehicle::toyota_2004(ScenarioId::Healthy),
            AdapterPersonality::genuine_v1_5(),
        );
        for cmd in ["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATSP0"] {
            e.handle_line(cmd);
        }
        let bytes = |line: &str| -> Vec<u8> {
            line.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
        };
        let checks = |b: &[u8]| {
            let (body, ck) = b.split_at(b.len() - 1);
            body.iter().fold(0u8, |a, x| a.wrapping_add(*x)) == ck[0]
        };

        let out = e.handle_line("0100");
        let frames: Vec<&str> =
            out.split(['\r', '\n']).filter(|l| l.starts_with("48 6B")).collect();
        assert_eq!(frames.len(), 2, "{out:?}");
        let engine = bytes(frames[0]);
        assert_eq!(&engine[..5], &[0x48, 0x6B, 0x10, 0x41, 0x00]);
        assert_eq!(engine.len(), 3 + 6 + 1);
        assert!(checks(&engine), "{engine:02X?}");
        assert!(e.handle_line("ATDPN").contains('3'));

        let vin = e.handle_line("0902");
        let vin: Vec<Vec<u8>> =
            vin.split(['\r', '\n']).filter(|l| l.starts_with("48 6B 10")).map(bytes).collect();
        assert_eq!(vin.len(), 5);
        assert!(vin.iter().all(|f| checks(f) && f[3] == 0x49 && f[4] == 0x02));
        assert_eq!(vin.iter().map(|f| f[5]).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    }

    /// Run the handshake the real adapter runs, returning the emulator ready
    /// for OBD requests.
    fn initialized(id: ScenarioId) -> ElmEmulator {
        let mut e = emu(id);
        for cmd in ["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATAT1", "ATSP0"] {
            e.handle_line(cmd);
        }
        e
    }

    #[test]
    fn every_reply_ends_with_a_prompt() {
        let mut e = emu(ScenarioId::Healthy);
        for cmd in ["ATZ", "ATE0", "ATI", "0100", "NONSENSE"] {
            assert!(e.handle_line(cmd).ends_with('>'), "{cmd} did not end with a prompt");
        }
    }

    #[test]
    fn echo_is_on_at_power_up_and_ate0_turns_it_off() {
        let mut e = emu(ScenarioId::Healthy);
        assert!(e.handle_line("ATI").contains("ATI"));
        e.handle_line("ATE0");
        let r = e.handle_line("ATI");
        assert!(!r.contains("ATI\r"), "echo should be off: {r:?}");
        assert!(r.contains("ELM327 v1.5"));
    }

    #[test]
    fn unknown_at_commands_answer_with_a_question_mark() {
        let mut e = emu(ScenarioId::Healthy);
        e.handle_line("ATE0");
        assert!(e.handle_line("ATXYZ").contains('?'));
    }

    #[test]
    fn a_cheap_clone_refuses_the_commands_it_does_not_implement() {
        let mut e = ElmEmulator::new(
            VirtualVehicle::f250_2019(ScenarioId::Healthy),
            AdapterPersonality::cheap_clone_v2_1(),
        );
        e.handle_line("ATE0");
        assert!(e.handle_line("ATI").contains("v2.1"));
        assert!(e.handle_line("AT@1").contains('?'));
        assert!(e.handle_line("ATAT1").contains('?'));
        // But the commands it does implement still work.
        assert!(e.handle_line("ATH1").contains("OK"));
    }

    #[test]
    fn the_first_request_searches_and_later_ones_do_not() {
        let mut e = initialized(ScenarioId::Healthy);
        let first = e.handle_line("0100");
        assert!(first.contains("SEARCHING..."));
        let second = e.handle_line("0100");
        assert!(!second.contains("SEARCHING..."));
    }

    #[test]
    fn headers_appear_only_when_ath1_was_sent() {
        let mut e = emu(ScenarioId::Healthy);
        e.handle_line("ATE0");
        e.handle_line("ATSP0");
        let headerless = e.handle_line("010C");
        assert!(!headerless.contains("7E8"));
        e.handle_line("ATH1");
        let with_headers = e.handle_line("010C");
        assert!(with_headers.contains("7E8"));
    }

    #[test]
    fn spaces_follow_the_ats_setting() {
        let mut e = initialized(ScenarioId::Healthy);
        assert!(e.handle_line("010C").contains("7E8 "));
        e.handle_line("ATS0");
        let unspaced = e.handle_line("010C");
        assert!(!unspaced.contains("7E8 "));
        assert!(unspaced.contains("7E8"));
    }

    #[test]
    fn a_multi_frame_vin_is_printed_as_separate_iso_tp_frames() {
        let mut e = initialized(ScenarioId::Healthy);
        e.handle_line("0100");
        let reply = e.handle_line("0902");
        // Linefeeds are off after ATL0, so the reply is separated by bare
        // carriage returns and `str::lines` would see it as one line.
        let data: Vec<&str> =
            reply.split(['\r', '\n']).map(|l| l.trim()).filter(|l| l.starts_with("7E8")).collect();
        assert_eq!(data.len(), 3, "VIN needs a first frame and two more: {reply:?}");
        assert!(data[0].contains("10 14"), "first frame declares 20 bytes");
        assert!(data[1].contains("21"));
        assert!(data[2].contains("22"));
    }

    #[test]
    fn nothing_answering_is_no_data_once_a_protocol_exists() {
        let mut e = initialized(ScenarioId::Healthy);
        e.handle_line("0100");
        assert!(e.handle_line("01FE").contains("NO DATA"));
    }

    #[test]
    fn a_silent_vehicle_reports_unable_to_connect() {
        let mut e = initialized(ScenarioId::BusSilent);
        let r = e.handle_line("0100");
        assert!(r.contains("UNABLE TO CONNECT"), "{r:?}");
        assert!(e.handle_line("ATDPN").contains("A0"));
    }

    #[test]
    fn atdpn_reports_the_protocol_only_after_one_is_established() {
        let mut e = initialized(ScenarioId::Healthy);
        assert!(e.handle_line("ATDPN").contains("A0"));
        e.handle_line("0100");
        assert!(e.handle_line("ATDPN").contains("A6"));
    }

    #[test]
    fn atrv_reports_the_configured_voltage() {
        let mut e = initialized(ScenarioId::Healthy);
        assert!(e.handle_line("ATRV").contains("14.1V"));
    }

    #[test]
    fn atsh_selects_which_module_is_addressed() {
        let mut e = initialized(ScenarioId::Healthy);
        e.handle_line("0100");
        e.handle_line("ATSH7E2");
        let reply = e.handle_line("010C");
        assert!(reply.contains("7EA"));
        assert!(!reply.contains("7E8"));
    }

    #[test]
    fn injected_faults_surface_as_the_banners_a_real_device_prints() {
        let mut e = initialized(ScenarioId::Healthy);
        e.handle_line("0100");
        e.inject(InjectedFault::BufferFull);
        assert!(e.handle_line("0100").contains("BUFFER FULL"));
        e.inject(InjectedFault::Stopped);
        assert!(e.handle_line("0100").contains("STOPPED"));
        e.inject(InjectedFault::BusError);
        assert!(e.handle_line("0100").contains("CAN ERROR"));
        e.inject(InjectedFault::Silence);
        assert_eq!(e.handle_line("0100"), "");
        // The queue is one-shot: the next command behaves normally.
        assert!(e.handle_line("0100").contains("7E8"));
    }

    #[test]
    fn atz_returns_every_setting_to_its_power_on_value() {
        let mut e = initialized(ScenarioId::Healthy);
        e.handle_line("0100");
        assert!(e.headers_enabled());
        e.handle_line("ATZ");
        assert!(!e.headers_enabled());
        // Echo is back on, so the reply repeats the command.
        assert!(e.handle_line("ATI").starts_with("ATI"));
    }
}
