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
}

impl AdapterPersonality {
    /// A genuine ELM327 v1.5: answers everything this project uses.
    pub fn genuine_v1_5() -> Self {
        AdapterPersonality {
            banner: String::from("ELM327 v1.5"),
            description: Some(String::from("OBDII to RS232 Interpreter")),
            voltage: 14.1,
            unsupported: Vec::new(),
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
    header: u16,
    faults: VecDeque<InjectedFault>,
    /// Every command received, for transcript recording and assertions.
    pub log: Vec<String>,
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
            faults: VecDeque::new(),
            log: Vec::new(),
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
            return match u16::from_str_radix(v, 16) {
                Ok(h) => {
                    self.header = h;
                    vec![String::from("OK")]
                }
                Err(_) => vec![String::from("?")],
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
                self.protocol = None;
                vec![String::from("OK")]
            }
            _ => vec![String::from("?")],
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
        let searching = self.protocol.is_none();
        if searching {
            lines.push(String::from("SEARCHING..."));
        }

        let replies = self.vehicle.handle(self.header, &request);

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
            self.protocol =
                Some(if self.requested_protocol == 0 { 6 } else { self.requested_protocol });
        }

        for reply in replies {
            let id = CanId::Standard(reply.response_id);
            for frame in segment(&reply.payload) {
                let data = frame.encode(8, 0x00);
                lines.push(self.format_frame(&id, &data));
            }
        }
        lines
    }

    fn format_frame(&self, id: &CanId, data: &[u8]) -> String {
        let mut s = String::new();
        if self.headers {
            s.push_str(&id.to_hex());
            if self.spaces {
                s.push(' ');
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
