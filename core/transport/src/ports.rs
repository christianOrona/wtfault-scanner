//! Serial port discovery.
//!
//! The UI needs a list to show the user. On Windows a paired Bluetooth ELM327
//! appears as an *outgoing* COM port (e.g. `COM5`), which is why Bluetooth
//! discovery here is serial-port discovery: enumerate ports, classify them, and
//! flag the ones that plausibly belong to an OBD adapter.
//!
//! Classification is a *hint only*. Nothing downstream trusts it — the adapter
//! is only ever confirmed by an actual `ATZ`/`ATI` handshake.

use serde::{Deserialize, Serialize};

/// How a port is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    /// USB CDC/FTDI device.
    Usb,
    /// Bluetooth Classic SPP virtual COM port.
    Bluetooth,
    /// On-board / PCI serial hardware.
    Native,
    /// Enumerated but unclassifiable.
    Unknown,
}

/// One discovered serial port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortInfo {
    /// OS port name: `COM5`, `/dev/rfcomm0`, `/dev/tty.OBDII-SPPDev`.
    pub name: String,
    /// Attachment type.
    pub kind: PortKind,
    /// USB vendor id, when known.
    pub vid: Option<u16>,
    /// USB product id, when known.
    pub pid: Option<u16>,
    /// Serial number string, when reported.
    pub serial_number: Option<String>,
    /// Manufacturer string, when reported.
    pub manufacturer: Option<String>,
    /// Product string, when reported.
    pub product: Option<String>,
    /// Heuristic: this port *might* be an OBD adapter. Confirmed only by a
    /// successful `ATZ`/`ATI` probe, never by this flag.
    pub likely_obd_adapter: bool,
}

impl PortKind {
    /// How this attachment should be recorded in adapter capabilities.
    ///
    /// A port the OS could not classify is reported as `Usb` rather than
    /// guessed at as Bluetooth: the practical difference is that a wired link
    /// has a real line speed to get right, and treating an unknown port as
    /// wired is the assumption that fails safely.
    pub fn transport_kind(&self) -> aim_types::TransportKind {
        match self {
            PortKind::Bluetooth => aim_types::TransportKind::Bluetooth,
            PortKind::Usb | PortKind::Native | PortKind::Unknown => aim_types::TransportKind::Usb,
        }
    }
}

impl PortInfo {
    /// Apply the naming heuristic used to set `likely_obd_adapter`.
    ///
    /// Public because the heuristic is part of what the UI shows the user when
    /// it asks them to pick a port, and because it is only a hint: a port is
    /// confirmed to be an adapter by a successful probe, never by its name.
    pub fn classify(mut self) -> Self {
        let haystack = format!(
            "{} {} {}",
            self.name.to_ascii_lowercase(),
            self.manufacturer.as_deref().unwrap_or("").to_ascii_lowercase(),
            self.product.as_deref().unwrap_or("").to_ascii_lowercase()
        );
        const HINTS: [&str; 8] =
            ["obd", "elm", "obdii", "obd2", "vgate", "vlink", "stn", "scantool"];
        self.likely_obd_adapter =
            HINTS.iter().any(|h| haystack.contains(h)) || self.kind == PortKind::Bluetooth;
        self
    }
}

/// Enumerate serial ports visible to the OS.
///
/// Returns an empty list when the `serial` feature is disabled (for example in
/// a CI container with no libudev). Callers must treat "no ports" as a normal
/// state, not an error.
#[cfg(feature = "serial")]
pub fn list_ports() -> Vec<PortInfo> {
    let ports = match serialport::available_ports() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "serial port enumeration failed");
            return Vec::new();
        }
    };
    ports
        .into_iter()
        .map(|p| {
            let (kind, vid, pid, serial_number, manufacturer, product) = match p.port_type {
                serialport::SerialPortType::UsbPort(info) => (
                    PortKind::Usb,
                    Some(info.vid),
                    Some(info.pid),
                    info.serial_number,
                    info.manufacturer,
                    info.product,
                ),
                serialport::SerialPortType::BluetoothPort => {
                    (PortKind::Bluetooth, None, None, None, None, None)
                }
                serialport::SerialPortType::PciPort => {
                    (PortKind::Native, None, None, None, None, None)
                }
                serialport::SerialPortType::Unknown => {
                    (PortKind::Unknown, None, None, None, None, None)
                }
            };
            PortInfo {
                name: p.port_name,
                kind,
                vid,
                pid,
                serial_number,
                manufacturer,
                product,
                likely_obd_adapter: false,
            }
            .classify()
        })
        .collect()
}

/// Enumerate serial ports — stub used when the `serial` feature is off.
#[cfg(not(feature = "serial"))]
pub fn list_ports() -> Vec<PortInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, kind: PortKind, product: Option<&str>) -> PortInfo {
        PortInfo {
            name: name.into(),
            kind,
            vid: None,
            pid: None,
            serial_number: None,
            manufacturer: None,
            product: product.map(String::from),
            likely_obd_adapter: false,
        }
        .classify()
    }

    #[test]
    fn bluetooth_ports_are_always_candidates() {
        // On Windows this is the paired ELM327's outgoing COM port.
        assert!(port("COM5", PortKind::Bluetooth, None).likely_obd_adapter);
    }

    #[test]
    fn product_strings_are_matched_case_insensitively() {
        assert!(port("COM3", PortKind::Usb, Some("OBDLink EX")).likely_obd_adapter);
        assert!(port("/dev/ttyUSB0", PortKind::Usb, Some("ELM327 v1.5")).likely_obd_adapter);
        assert!(port("/dev/tty.OBDII-SPPDev", PortKind::Unknown, None).likely_obd_adapter);
    }

    #[test]
    fn ordinary_serial_hardware_is_not_flagged() {
        assert!(!port("/dev/ttyS0", PortKind::Native, None).likely_obd_adapter);
        assert!(!port("COM1", PortKind::Usb, Some("USB Serial Device")).likely_obd_adapter);
    }

    #[test]
    fn enumeration_never_panics() {
        let _ = list_ports();
    }

    #[test]
    fn a_wired_port_is_never_mistaken_for_a_bluetooth_one() {
        // Only Bluetooth ports may be treated as Bluetooth. Everything else is
        // wired, and a wired link has a line speed that has to be got right —
        // so an unclassifiable port is treated as wired, which is the
        // assumption that fails safely.
        assert_eq!(PortKind::Bluetooth.transport_kind(), aim_types::TransportKind::Bluetooth);
        for k in [PortKind::Usb, PortKind::Native, PortKind::Unknown] {
            assert_eq!(k.transport_kind(), aim_types::TransportKind::Usb, "{k:?}");
        }
    }
}
