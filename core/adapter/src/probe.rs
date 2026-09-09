//! "Is there an ELM327 on this port?"
//!
//! On Windows a paired Bluetooth ELM327 shows up as an outgoing COM port with
//! no indication of what is on the other end, and the laptop usually has
//! several other COM ports that are not adapters. The UI therefore needs to
//! offer the user a list and, ideally, say which entry actually answered.
//!
//! Probing is deliberately cheap and read-only: reset, ask for the
//! identification banner, close. It never initializes the vehicle protocol and
//! never puts a frame on the bus, so probing the wrong port is harmless.

use crate::response;
use aim_transport::Transport;
use aim_types::{AimResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What a probe found on a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identification {
    /// The transport that was probed, e.g. `COM5`.
    pub descriptor: String,
    /// Whether anything answered at all.
    pub responded: bool,
    /// The identification banner, verbatim.
    pub banner: Option<String>,
    /// Device description from `AT@1`, when the device answers it.
    pub description: Option<String>,
    /// Whether the banner identifies an ELM327-compatible device.
    pub elm327_compatible: bool,
    /// Round-trip time of the identification exchange.
    pub elapsed_ms: u64,
    /// Line speed the device answered at, when the port is a real serial one
    /// and a speed had to be found. `None` for Bluetooth virtual ports, which
    /// ignore baud, and for non-serial transports.
    pub baud_rate: Option<u32>,
}

impl Identification {
    /// A probe where nothing answered.
    pub fn silent(descriptor: impl Into<String>, elapsed_ms: u64) -> Self {
        Identification {
            descriptor: descriptor.into(),
            responded: false,
            banner: None,
            description: None,
            elm327_compatible: false,
            elapsed_ms,
            baud_rate: None,
        }
    }
}

/// Probe any transport. Works against the simulator, which is what makes this
/// path testable without hardware.
///
/// The transport is opened if it is not already, and is left in the state it
/// was found in.
pub fn identify_transport(
    transport: &mut dyn Transport,
    timeout: Duration,
) -> AimResult<Identification> {
    let descriptor = transport.descriptor();
    let was_open = transport.is_open();
    if !was_open {
        transport.open()?;
    }
    let result = identify_open_transport(transport, &descriptor, timeout);
    if !was_open {
        let _ = transport.close();
    }
    result
}

/// Terminate whatever half-finished line the device is holding.
///
/// `flush_input` empties *our* buffer. It cannot empty the adapter's, and that
/// is the one that causes trouble. Two things leave junk in it: another program
/// that stopped mid-command, and — the case that actually bit here — a baud
/// sweep, where every wrong-speed attempt delivers a burst of garbage bytes
/// that the device dutifully queues as the beginning of a command line.
///
/// The next real command is then appended to that garbage, and the whole line
/// comes back as `?` — "I did not understand". Which is indistinguishable, to a
/// caller that only checks for success, from an adapter that is not there.
///
/// A bare carriage return ends the pending line and gets it answered and out of
/// the way. Whatever comes back is discarded on purpose; the only thing that
/// matters is that the device's buffer is empty afterwards.
fn clear_pending_line(transport: &mut dyn Transport, timeout: Duration) {
    let _ = transport.flush_input();
    let _ = transport.write_all(b"\r");
    // Short: this is housekeeping, not a question. A device that says nothing
    // to a bare CR has nothing pending, which is exactly what we wanted.
    let brief = timeout.min(Duration::from_millis(300));
    let _ = aim_transport::read_until(transport, b'>', brief);
    let _ = transport.flush_input();
}

fn identify_open_transport(
    transport: &mut dyn Transport,
    descriptor: &str,
    timeout: Duration,
) -> AimResult<Identification> {
    let started = std::time::Instant::now();

    clear_pending_line(transport, timeout);

    // ATZ first: a device mid-command, or one left with echo on by another
    // program, answers ATI unusably until it has been reset.
    let mut reset = exchange(transport, "ATZ", timeout)?;

    // "I did not understand" is not silence. It is a device that is present,
    // listening, and answering — which is far more than a wrong baud rate ever
    // produces — that has been handed a corrupted line. Clear it properly and
    // ask once more before concluding anything.
    if matches!(reset.class, response::ResponseClass::NotUnderstood) {
        clear_pending_line(transport, timeout);
        reset = exchange(transport, "ATZ", timeout)?;
    }

    if !reset.class.is_success() {
        return Ok(Identification::silent(descriptor, started.elapsed().as_millis() as u64));
    }

    // Echo off, so the banner is not confused with our own command.
    let _ = exchange(transport, "ATE0", timeout)?;

    let ident = exchange(transport, "ATI", timeout)?;
    let banner = if ident.class.is_success() && !ident.lines.is_empty() {
        Some(ident.lines.join(" ").trim().to_string())
    } else {
        // Some devices print their banner only in response to ATZ.
        let b = reset.lines.join(" ").trim().to_string();
        if b.is_empty() {
            None
        } else {
            Some(b)
        }
    };

    let elm327_compatible = banner
        .as_deref()
        .map(|b| {
            let u = b.to_ascii_uppercase();
            u.contains("ELM327") || u.contains("OBDII")
        })
        .unwrap_or(false);

    let description = if elm327_compatible {
        let d = exchange(transport, "AT@1", timeout)?;
        if d.class.is_success() && !d.lines.is_empty() {
            Some(d.lines.join(" ").trim().to_string())
        } else {
            None
        }
    } else {
        None
    };

    Ok(Identification {
        descriptor: descriptor.to_string(),
        responded: banner.is_some(),
        banner,
        description,
        elm327_compatible,
        elapsed_ms: started.elapsed().as_millis() as u64,
        baud_rate: None,
    })
}

fn exchange(
    transport: &mut dyn Transport,
    command: &str,
    timeout: Duration,
) -> AimResult<response::AdapterResponse> {
    transport.flush_input()?;
    let started = std::time::Instant::now();
    transport.write_all(format!("{command}\r").as_bytes())?;
    let (bytes, terminated) = aim_transport::read_until(transport, b'>', timeout)?;
    Ok(response::parse(
        command,
        &String::from_utf8_lossy(&bytes),
        terminated,
        started.elapsed().as_millis() as u64,
    ))
}

/// Probe one named serial port (a Windows Bluetooth COM port, `/dev/rfcomm0`,
/// `/dev/tty.*`).
///
/// Only compiled when the `serial` feature is on, so the workspace still
/// builds and tests where libudev is unavailable.
#[cfg(feature = "serial")]
pub fn probe_port(port: &str, timeout: Duration) -> AimResult<Identification> {
    match find_baud(port, timeout) {
        Ok((id, _)) => Ok(id),
        // A port that cannot even be opened is a normal outcome when scanning
        // every COM port on a laptop, not an exception worth propagating.
        Err(e) if e.code == ErrorCode::TransportOpenFailed => {
            Err(e.with_details(serde_json::json!({ "port": port })))
        }
        Err(e) => Err(e),
    }
}

/// Find the line speed a serial adapter is actually talking at.
///
/// Nothing in the serial API can ask a device what speed it is set to. At the
/// wrong speed the port opens cleanly, every write succeeds, and the replies
/// come back as noise or not at all — which is indistinguishable from an
/// adapter that is not there. So the speeds are tried in turn and the one that
/// produces a real answer is the one that is used.
///
/// This only matters for wired cables. A Bluetooth virtual COM port ignores
/// baud completely, which is why a Bluetooth adapter works at whatever value
/// happens to be set and a USB cable does not.
///
/// Returns the identification and the speed it was obtained at. When nothing
/// answers at any speed, the last attempt's silent result is returned with a
/// `baud_rate` of `None` — the port exists, and nothing on it spoke.
#[cfg(feature = "serial")]
pub fn find_baud(port: &str, timeout: Duration) -> AimResult<(Identification, Option<u32>)> {
    use aim_transport::serial::{SerialConfig, SerialTransport, CANDIDATE_BAUD_RATES};

    let kind = aim_transport::list_ports()
        .into_iter()
        .find(|p| p.name == port)
        .map(|p| p.kind.transport_kind())
        .unwrap_or(aim_types::TransportKind::Usb);

    // A Bluetooth virtual port ignores baud, so sweeping one would be six
    // identical attempts at the same silence. One try, at the full timeout.
    if kind == aim_types::TransportKind::Bluetooth {
        let mut t = SerialTransport::new(SerialConfig::bluetooth(port));
        return identify_transport(&mut t, timeout).map(|id| (id, None));
    }

    // A wrong speed produces silence or garbage, and waiting the full timeout
    // for each one would make a six-way sweep unpleasant. The real device gets
    // the full timeout once it has been found.
    let per_try = timeout.min(Duration::from_millis(1_200));

    let mut last: Option<Identification> = None;
    let mut open_error: Option<aim_types::AimError> = None;

    for baud in CANDIDATE_BAUD_RATES {
        let config = SerialConfig::for_port(port, kind).with_baud(baud);
        let mut t = SerialTransport::new(config);
        // A wrong-speed attempt is not harmless. The bytes still arrive at the
        // device, as noise, and sit in its buffer waiting to corrupt the next
        // command — which is why the sweep used to walk straight past the one
        // baud that worked. `identify_open_transport` clears that before it
        // asks anything, and this pause gives the device's UART time to
        // resynchronise after the speed changes underneath it.
        std::thread::sleep(Duration::from_millis(120));
        match identify_transport(&mut t, per_try) {
            Ok(id) if id.responded => {
                return Ok((Identification { baud_rate: Some(baud), ..id }, Some(baud)));
            }
            Ok(id) => last = Some(id),
            // The port being unopenable is the same at every speed, so there is
            // no point trying the rest.
            Err(e) if e.code == ErrorCode::TransportOpenFailed => {
                open_error = Some(e);
                break;
            }
            // A write failure on one speed is worth remembering but not fatal:
            // a stale Bluetooth port fails this way and a real cable may not.
            Err(e) => open_error = Some(e),
        }
    }

    match last {
        Some(id) => Ok((id, None)),
        None => Err(open_error.unwrap_or_else(|| {
            aim_types::AimError::new(
                ErrorCode::TransportOpenFailed,
                format!("could not open {port} at any supported line speed"),
            )
        })),
    }
}

/// Probe every enumerable serial port and report what each one is.
///
/// Ports that fail to open are reported with their error rather than dropped —
/// "COM3 is in use by another program" is exactly what a user needs to be told.
#[cfg(feature = "serial")]
pub fn probe_ports(timeout: Duration) -> Vec<(aim_transport::PortInfo, AimResult<Identification>)> {
    aim_transport::list_ports()
        .into_iter()
        .map(|info| {
            let result = probe_port(&info.name, timeout);
            (info, result)
        })
        .collect()
}

/// Stub for builds without the `serial` feature: enumeration is unavailable,
/// and saying so is better than returning an empty list that looks like "no
/// adapters found".
#[cfg(not(feature = "serial"))]
pub fn probe_port(port: &str, _timeout: Duration) -> AimResult<Identification> {
    Err(aim_types::AimError::new(
        ErrorCode::TransportUnsupported,
        format!("serial support is not compiled into this build; cannot probe {port}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_transport::LoopbackTransport;
    use aim_transport::TransportStats;

    #[test]
    fn a_genuine_banner_identifies_as_elm327() {
        let mut t = LoopbackTransport::new(vec![
            ("ATZ", vec!["ELM327 v1.5"]),
            ("ATE0", vec!["OK"]),
            ("ATI", vec!["ELM327 v1.5"]),
            ("AT@1", vec!["OBDII to RS232 Interpreter"]),
        ]);
        let id = identify_transport(&mut t, Duration::from_millis(200)).unwrap();
        assert!(id.responded);
        assert!(id.elm327_compatible);
        assert_eq!(id.banner.as_deref(), Some("ELM327 v1.5"));
        assert_eq!(id.description.as_deref(), Some("OBDII to RS232 Interpreter"));
    }

    #[test]
    fn a_clone_that_refuses_at1_is_still_identified() {
        let mut t = LoopbackTransport::new(vec![
            ("ATZ", vec!["ELM327 v2.1"]),
            ("ATE0", vec!["OK"]),
            ("ATI", vec!["ELM327 v2.1"]),
            ("AT@1", vec!["?"]),
        ]);
        let id = identify_transport(&mut t, Duration::from_millis(200)).unwrap();
        assert!(id.elm327_compatible);
        assert_eq!(id.description, None);
    }

    #[test]
    fn some_other_serial_device_is_not_reported_as_an_adapter() {
        let mut t = LoopbackTransport::new(vec![
            ("ATZ", vec!["MODEM READY"]),
            ("ATE0", vec!["OK"]),
            ("ATI", vec!["Generic USB Widget 3000"]),
        ]);
        let id = identify_transport(&mut t, Duration::from_millis(200)).unwrap();
        assert!(id.responded);
        assert!(!id.elm327_compatible);
        assert_eq!(id.description, None);
    }

    #[test]
    fn probing_leaves_a_closed_transport_closed() {
        let mut t = LoopbackTransport::new(vec![
            ("ATZ", vec!["ELM327 v1.5"]),
            ("ATE0", vec!["OK"]),
            ("ATI", vec!["ELM327 v1.5"]),
            ("AT@1", vec!["?"]),
        ]);
        assert!(!t.is_open());
        identify_transport(&mut t, Duration::from_millis(200)).unwrap();
        assert!(!t.is_open());
    }

    /// An ELM327 with a byte or two of junk already in its line buffer.
    ///
    /// Models the real failure this fixed. A baud sweep delivers garbage to the
    /// device at every wrong speed; the device queues it as the start of a
    /// command line. The next real command is appended to that junk, and the
    /// whole line is answered `?`. Our own `flush_input` cannot help — the junk
    /// is in the adapter, not in us.
    ///
    /// Only a bare carriage return clears it, which is exactly what the probe
    /// now sends before asking anything.
    struct PoisonedElm {
        open: bool,
        /// True until a bare CR has terminated the junk line.
        poisoned: bool,
        pending: Vec<u8>,
        stats: TransportStats,
    }

    impl PoisonedElm {
        fn new() -> Self {
            PoisonedElm {
                open: false,
                poisoned: true,
                pending: Vec::new(),
                stats: TransportStats::default(),
            }
        }
    }

    impl Transport for PoisonedElm {
        fn kind(&self) -> aim_types::TransportKind {
            aim_types::TransportKind::Usb
        }
        fn descriptor(&self) -> String {
            "fake:poisoned".into()
        }
        fn open(&mut self) -> AimResult<()> {
            self.open = true;
            Ok(())
        }
        fn close(&mut self) -> AimResult<()> {
            self.open = false;
            Ok(())
        }
        fn is_open(&self) -> bool {
            self.open
        }
        fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
            let s = String::from_utf8_lossy(data);
            let cmd = s.trim_end_matches('\r').to_ascii_uppercase();
            let reply = if self.poisoned {
                // Whatever was asked, it landed on the end of the junk line.
                // A bare CR is what ends that line and clears the condition.
                if cmd.is_empty() {
                    self.poisoned = false;
                }
                "?\r\r>".to_string()
            } else {
                match cmd.as_str() {
                    "" => ">".to_string(),
                    "ATZ" => "\r\rELM327 v1.5\r\r>".to_string(),
                    "ATE0" => "OK\r\r>".to_string(),
                    "ATI" => "ELM327 v1.5\r\r>".to_string(),
                    "AT@1" => "OBDII to RS232 Interpreter\r\r>".to_string(),
                    _ => "?\r\r>".to_string(),
                }
            };
            self.pending.extend_from_slice(reply.as_bytes());
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8], _: Duration) -> AimResult<usize> {
            let n = self.pending.len().min(buf.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            Ok(n)
        }
        fn flush_input(&mut self) -> AimResult<()> {
            // Empties our buffer. Deliberately does NOT clear `poisoned`: that
            // is the whole point of the bug.
            self.pending.clear();
            Ok(())
        }
        fn stats(&self) -> TransportStats {
            self.stats
        }
    }

    #[test]
    fn an_adapter_with_junk_in_its_buffer_is_found_not_written_off() {
        // The bug, in one assertion. A real FTDI cable at 500000 baud reported
        // "silent" because the sweep's wrong-speed attempts had left noise in
        // the device's line buffer, so its answer to ATZ was `?`. A `?` means
        // present, listening and answering - which no wrong baud rate ever
        // produces - and it must never be read as absence.
        let mut t = PoisonedElm::new();
        let id = identify_transport(&mut t, Duration::from_millis(200)).unwrap();
        assert!(id.responded, "a confused adapter is still an adapter");
        assert!(id.elm327_compatible);
        assert_eq!(id.banner.as_deref(), Some("ELM327 v1.5"));
    }

    #[test]
    fn a_genuinely_silent_port_is_still_reported_silent() {
        // The other half: the retry must not turn silence into a false
        // positive. A port with nothing on it answers nothing, twice.
        struct Mute;
        impl Transport for Mute {
            fn kind(&self) -> aim_types::TransportKind {
                aim_types::TransportKind::Usb
            }
            fn descriptor(&self) -> String {
                "fake:mute".into()
            }
            fn open(&mut self) -> AimResult<()> {
                Ok(())
            }
            fn close(&mut self) -> AimResult<()> {
                Ok(())
            }
            fn is_open(&self) -> bool {
                true
            }
            fn write_all(&mut self, _: &[u8]) -> AimResult<()> {
                Ok(())
            }
            fn read(&mut self, _: &mut [u8], _: Duration) -> AimResult<usize> {
                Ok(0)
            }
            fn flush_input(&mut self) -> AimResult<()> {
                Ok(())
            }
            fn stats(&self) -> TransportStats {
                TransportStats::default()
            }
        }
        let id = identify_transport(&mut Mute, Duration::from_millis(80)).unwrap();
        assert!(!id.responded);
        assert!(id.banner.is_none());
    }
}
