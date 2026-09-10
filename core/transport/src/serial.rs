//! Real serial transport — the hardware path.
//!
//! On **Windows** a Bluetooth-paired ELM327 exposes an *outgoing* COM port; a
//! Bluetooth adapter is therefore driven through this exact type with
//! `port = "COM5"`. On Linux the equivalent is an rfcomm binding
//! (`/dev/rfcomm0`), on macOS `/dev/tty.OBDII-SPPDev`. Baud rate is meaningless
//! for a Bluetooth virtual port but harmless to set, so one code path serves
//! all three.
//!
//! Compiled only with the `serial` feature (on by default) because the
//! underlying crate needs libudev on Linux.

use crate::{Transport, TransportStats};
use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use std::time::Duration;

/// Serial port settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SerialConfig {
    /// OS port name: `COM5`, `/dev/rfcomm0`, `/dev/tty.OBDII-SPPDev`.
    pub port: String,
    /// Baud rate. 38400 is the ELM327 default; clones often want 9600 or 115200.
    /// Ignored by Bluetooth virtual COM ports.
    pub baud_rate: u32,
    /// Per-read timeout handed to the OS driver.
    pub read_timeout: Duration,
    /// Declared link kind, so capabilities record Bluetooth vs USB honestly.
    pub transport_kind: TransportKind,
}

impl SerialConfig {
    /// Defaults for a Bluetooth-paired ELM327 on an outgoing COM port.
    pub fn bluetooth(port: impl Into<String>) -> Self {
        SerialConfig {
            port: port.into(),
            baud_rate: 38_400,
            read_timeout: Duration::from_millis(100),
            transport_kind: TransportKind::Bluetooth,
        }
    }

    /// Defaults for a wired USB adapter.
    pub fn usb(port: impl Into<String>) -> Self {
        SerialConfig {
            port: port.into(),
            baud_rate: 115_200,
            read_timeout: Duration::from_millis(100),
            transport_kind: TransportKind::Usb,
        }
    }

    /// Defaults for a port whose attachment type the OS has already reported.
    ///
    /// Keeps the recorded transport kind honest — capabilities and timing
    /// expectations both depend on Bluetooth versus wired — without the caller
    /// having to remember which constructor implies which kind.
    pub fn for_port(port: impl Into<String>, kind: TransportKind) -> Self {
        let port = port.into();
        match kind {
            TransportKind::Bluetooth | TransportKind::BluetoothLe => SerialConfig::bluetooth(port),
            _ => SerialConfig { transport_kind: kind, ..SerialConfig::usb(port) },
        }
    }

    /// The same settings at a different line speed.
    ///
    /// A Bluetooth virtual COM port ignores baud entirely, which is why the
    /// Bluetooth adapter worked with whatever was set. A wired USB cable does
    /// not: at the wrong speed the port opens, writes succeed, and nothing ever
    /// comes back. There is no way to ask a serial port what speed the device
    /// on the far end is using, so the only honest answer is to try.
    pub fn with_baud(mut self, baud: u32) -> Self {
        self.baud_rate = baud;
        self
    }
}

/// Line speeds worth trying on an unknown ELM327-family cable, most likely
/// first.
///
/// 38400 is the speed the ELM327 datasheet specifies. Clones vary widely and
/// several popular USB cables ship at 115200 or 500000, so a fixed guess locks
/// out hardware that is working perfectly.
pub const CANDIDATE_BAUD_RATES: [u32; 6] = [38_400, 115_200, 9_600, 500_000, 230_400, 57_600];

/// A serial-port transport (also the Bluetooth-SPP path).
pub struct SerialTransport {
    config: SerialConfig,
    port: Option<Box<dyn serialport::SerialPort>>,
    stats: TransportStats,
}

impl SerialTransport {
    /// Create a closed transport for `config`. No I/O happens until [`Transport::open`].
    pub fn new(config: SerialConfig) -> Self {
        SerialTransport { config, port: None, stats: TransportStats::default() }
    }

    /// The configuration in use.
    pub fn config(&self) -> &SerialConfig {
        &self.config
    }
}

impl Transport for SerialTransport {
    fn kind(&self) -> TransportKind {
        self.config.transport_kind
    }

    fn descriptor(&self) -> String {
        self.config.port.clone()
    }

    fn baud(&self) -> Option<u32> {
        (self.config.transport_kind != TransportKind::Bluetooth).then_some(self.config.baud_rate)
    }

    fn open(&mut self) -> AimResult<()> {
        if self.port.is_some() {
            return Ok(());
        }
        let port = serialport::new(&self.config.port, self.config.baud_rate)
            .timeout(self.config.read_timeout)
            .data_bits(serialport::DataBits::Eight)
            .stop_bits(serialport::StopBits::One)
            .parity(serialport::Parity::None)
            .flow_control(serialport::FlowControl::None)
            .open()
            .map_err(|e| map_serial_error(&self.config.port, e))?;
        self.port = Some(port);
        self.stats.opens += 1;
        tracing::info!(port = %self.config.port, baud = self.config.baud_rate, "serial port opened");
        Ok(())
    }

    fn close(&mut self) -> AimResult<()> {
        self.port = None;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.port.is_some()
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        let port = self.port.as_mut().ok_or_else(|| {
            AimError::new(ErrorCode::TransportDisconnected, "serial port is not open")
        })?;
        use std::io::Write as _;
        match port.write_all(data).and_then(|_| port.flush()) {
            Ok(()) => {
                self.stats.bytes_written += data.len() as u64;
                Ok(())
            }
            Err(e) => {
                self.stats.io_errors += 1;
                Err(AimError::from(e))
            }
        }
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        let port = self.port.as_mut().ok_or_else(|| {
            AimError::new(ErrorCode::TransportDisconnected, "serial port is not open")
        })?;
        // The driver timeout is per-read; align it with the caller's deadline.
        if let Err(e) = port.set_timeout(timeout) {
            self.stats.io_errors += 1;
            return Err(AimError::new(ErrorCode::TransportIo, e.to_string()));
        }
        use std::io::Read as _;
        match port.read(buf) {
            Ok(n) => {
                self.stats.bytes_read += n as u64;
                Ok(n)
            }
            // A quiet port is an observation, not a failure — the adapter layer
            // decides when silence has gone on too long.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                self.stats.read_timeouts += 1;
                Ok(0)
            }
            Err(e) => {
                self.stats.io_errors += 1;
                Err(AimError::from(e))
            }
        }
    }

    fn flush_input(&mut self) -> AimResult<()> {
        let port = self.port.as_mut().ok_or_else(|| {
            AimError::new(ErrorCode::TransportDisconnected, "serial port is not open")
        })?;
        port.clear(serialport::ClearBuffer::Input)
            .map_err(|e| AimError::new(ErrorCode::TransportIo, e.to_string()))
    }

    fn stats(&self) -> TransportStats {
        self.stats
    }
}

fn map_serial_error(port: &str, e: serialport::Error) -> AimError {
    let code = match e.kind() {
        serialport::ErrorKind::NoDevice => ErrorCode::TransportNotFound,
        serialport::ErrorKind::InvalidInput => ErrorCode::BadRequest,
        serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
            ErrorCode::TransportOpenFailed
        }
        _ => ErrorCode::TransportOpenFailed,
    };
    AimError::new(code, format!("could not open {port}: {e}"))
        .with_details(serde_json::json!({ "port": port }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_defaults_match_elm327_expectations() {
        let c = SerialConfig::bluetooth("COM5");
        assert_eq!(c.baud_rate, 38_400);
        assert_eq!(c.transport_kind, TransportKind::Bluetooth);
        assert_eq!(c.port, "COM5");
    }

    #[test]
    fn a_ports_own_attachment_type_is_what_gets_recorded() {
        // The bug this pins: every serial port was opened with the Bluetooth
        // defaults, so a wired USB cable was both mislabelled and driven at the
        // wrong line speed. Over Bluetooth that is invisible, because a virtual
        // COM port ignores baud entirely — which is exactly why it survived
        // being tested against a Bluetooth adapter.
        let bt = SerialConfig::for_port("COM5", TransportKind::Bluetooth);
        assert_eq!(bt.transport_kind, TransportKind::Bluetooth);

        let usb = SerialConfig::for_port("COM5", TransportKind::Usb);
        assert_eq!(usb.transport_kind, TransportKind::Usb);
        assert_eq!(usb.baud_rate, 115_200);
    }

    #[test]
    fn the_baud_sweep_covers_the_speeds_elm327_clones_actually_ship_at() {
        // Order is the claim being pinned: the datasheet speed first, then the
        // one most clones use. A device answering at any of these is found.
        assert_eq!(CANDIDATE_BAUD_RATES[0], 38_400, "the ELM327 standard");
        assert!(CANDIDATE_BAUD_RATES.contains(&115_200));
        assert!(CANDIDATE_BAUD_RATES.contains(&500_000));
        let mut seen = CANDIDATE_BAUD_RATES.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CANDIDATE_BAUD_RATES.len(), "no wasted attempts");
    }

    #[test]
    fn overriding_the_speed_leaves_everything_else_alone() {
        let c = SerialConfig::bluetooth("COM5").with_baud(500_000);
        assert_eq!(c.baud_rate, 500_000);
        assert_eq!(c.transport_kind, TransportKind::Bluetooth);
        assert_eq!(c.port, "COM5");
    }

    #[test]
    fn a_new_transport_is_closed_and_does_no_io() {
        let t = SerialTransport::new(SerialConfig::bluetooth("COM5"));
        assert!(!t.is_open());
        assert_eq!(t.descriptor(), "COM5");
        assert_eq!(t.kind(), TransportKind::Bluetooth);
        assert_eq!(t.stats(), TransportStats::default());
    }

    #[test]
    fn opening_a_nonexistent_port_reports_a_structured_error() {
        let mut t = SerialTransport::new(SerialConfig::usb("/dev/definitely-not-a-port"));
        let err = t.open().unwrap_err();
        assert!(matches!(err.code, ErrorCode::TransportNotFound | ErrorCode::TransportOpenFailed));
        assert_eq!(err.details.unwrap()["port"], "/dev/definitely-not-a-port");
    }

    #[test]
    fn operations_on_a_closed_port_are_disconnect_errors() {
        let mut t = SerialTransport::new(SerialConfig::usb("/dev/null-port"));
        assert_eq!(t.write_all(b"ATZ\r").unwrap_err().code, ErrorCode::TransportDisconnected);
        let mut buf = [0u8; 8];
        assert_eq!(
            t.read(&mut buf, Duration::from_millis(1)).unwrap_err().code,
            ErrorCode::TransportDisconnected
        );
    }
}
