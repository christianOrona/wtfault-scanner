//! Telling a usable Bluetooth serial port from the one that only looks usable.
//!
//! # The problem this exists for
//!
//! Pairing a Bluetooth SPP device on Windows creates **two** COM ports. One is
//! outgoing and talks to the device. The other is an incoming listening socket
//! for something connecting to this machine, and nothing an OBD adapter does
//! will ever come out of it.
//!
//! Both appear in a port list. Both open without error. Writing to the wrong
//! one times out, which looks exactly like a broken adapter — and a person who
//! does not know two ports exist has no way to tell, because the enumeration
//! crate reports both as `Unknown` with no name, vendor or product.
//!
//! # How they are told apart
//!
//! The registry records each port under its Bluetooth device address:
//!
//! ```text
//! ...BTHENUM\{00001101-...}_LOCALMFG&0000\9&7e9089f&0&000000000000_00000012  → COM3
//! ...BTHENUM\{00001101-...}_LOCALMFG&005d\9&7e9089f&0&001DA5F3AC92_C00000000 → COM4
//! ```
//!
//! `00001101` is the Serial Port Profile UUID, so a key under it is an SPP
//! port. The address before the underscore is the remote device: a real MAC for
//! the outgoing port, and **all zeros** for the incoming one, because it has no
//! particular remote device — it is waiting for anyone.
//!
//! That is the whole test, and it is a fact about how Windows names things
//! rather than a guess about what a port might be.

use std::collections::BTreeMap;

/// What a Bluetooth SPP COM port is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SppRole {
    /// Talks to the paired device. The one you want.
    Outgoing,
    /// Listens for inbound connections. Opens, then every write times out.
    Incoming,
}

/// Map every Bluetooth SPP COM port on this machine to its role.
///
/// Returns an empty map on anything that is not Windows, and on any failure -
/// this enriches a port list and must never be the reason one cannot be shown.
#[cfg(windows)]
pub fn spp_roles() -> BTreeMap<String, SppRole> {
    use std::process::Command;

    let out = match Command::new("reg")
        .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Enum\BTHENUM", "/s", "/v", "PortName"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return BTreeMap::new(),
    };
    parse_reg_output(&String::from_utf8_lossy(&out))
}

/// Nothing to enrich anywhere else: the two-port arrangement is a Windows
/// Bluetooth stack behaviour, not a Bluetooth one.
#[cfg(not(windows))]
pub fn spp_roles() -> BTreeMap<String, SppRole> {
    BTreeMap::new()
}

/// Parse `reg query` output into port name to role.
///
/// Split out so the parsing is testable without a registry, which matters
/// because the interesting input is a shape nobody has on a build machine.
pub(crate) fn parse_reg_output(text: &str) -> BTreeMap<String, SppRole> {
    /// Serial Port Profile. A key under any other UUID is not a COM port.
    const SPP_UUID: &str = "00001101";

    let mut roles = BTreeMap::new();
    let mut current: Option<SppRole> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HKEY_") {
            current = if trimmed.to_ascii_lowercase().contains(SPP_UUID) {
                Some(role_from_key(trimmed))
            } else {
                None
            };
            continue;
        }
        // "    PortName    REG_SZ    COM4"
        if let Some(role) = current {
            if let Some(rest) = trimmed.strip_prefix("PortName") {
                if let Some(name) = rest.split_whitespace().next_back() {
                    if name.to_ascii_uppercase().starts_with("COM") {
                        roles.insert(name.to_ascii_uppercase(), role);
                    }
                }
            }
        }
    }
    roles
}

/// An all-zero device address means the port is waiting for anyone, which is
/// the incoming one.
fn role_from_key(key: &str) -> SppRole {
    let has_real_address = key
        .rsplit('\\')
        .find(|segment| segment.contains('&') || segment.contains('_'))
        .and_then(|segment| segment.split('&').next_back())
        .and_then(|tail| tail.split('_').next())
        .is_some_and(|addr| {
            addr.len() >= 12
                && addr.chars().any(|c| c != '0')
                && addr.chars().all(|c| c.is_ascii_hexdigit())
        });
    if has_real_address {
        SppRole::Outgoing
    } else {
        SppRole::Incoming
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact output this was written against, from a machine with a paired
    /// OBD adapter. COM3 is the trap; COM4 is the adapter.
    const REAL_OUTPUT: &str = r"
HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Enum\BTHENUM\{00001101-0000-1000-8000-00805f9b34fb}_LOCALMFG&0000\9&7e9089f&0&000000000000_00000012\Device Parameters
    PortName    REG_SZ    COM3

HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Enum\BTHENUM\{00001101-0000-1000-8000-00805f9b34fb}_LOCALMFG&005d\9&7e9089f&0&001DA5F3AC92_C00000000\Device Parameters
    PortName    REG_SZ    COM4

End of search: 2 match(es) found.
";

    #[test]
    fn the_all_zero_address_is_the_incoming_port() {
        let roles = parse_reg_output(REAL_OUTPUT);
        assert_eq!(roles.get("COM3"), Some(&SppRole::Incoming));
        assert_eq!(roles.get("COM4"), Some(&SppRole::Outgoing));
        assert_eq!(roles.len(), 2);
    }

    /// A key under a different profile UUID is not a serial port and must not
    /// be claimed as one.
    #[test]
    fn only_serial_port_profile_keys_count() {
        let audio = r"
HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Enum\BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_LOCALMFG&0002\9&x&0&001DA5F3AC92_C00000000\Device Parameters
    PortName    REG_SZ    COM9
";
        assert!(parse_reg_output(audio).is_empty());
    }

    #[test]
    fn nonsense_yields_nothing_rather_than_a_guess() {
        assert!(parse_reg_output("").is_empty());
        assert!(parse_reg_output("ERROR: The system was unable to find the key").is_empty());
    }
}
