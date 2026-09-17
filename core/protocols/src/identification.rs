//! Utilities for parsing UDS identification data from vehicle modules.
//! This module handles decoding of identification strings reported by vehicle
//! modules over UDS, converting them into structured identity information
//! and display names.

use std::collections::HashMap;

/// A parsed UDS module identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UdsIdentity {
    /// System name or engine type (0xF197)
    pub system_name: Option<String>,
    /// Manufacturer spare part number (0xF187)
    pub spare_part_number: Option<String>,
    /// System supplier identifier (0xF18A)
    pub system_supplier: Option<String>,
    /// Manufacturer ECU hardware number (0xF191)
    pub hardware_number: Option<String>,
    /// System supplier ECU software version (0xF195)
    pub software_version: Option<String>,
}

impl UdsIdentity {
    /// Create a UdsIdentity from a collection of DID responses.
    ///
    /// Each (DID, data) pair fills the corresponding field with `identification_text(data)`.
    /// DIDs not in the list above are ignored. If the same DID appears more than once,
    /// the first value that gives Some wins, and later ones never overwrite it.
    /// A response whose text is None leaves the field as it was.
    pub fn from_dids<'a, I>(responses: I) -> Self
    where
        I: IntoIterator<Item = (u16, &'a [u8])>,
    {
        let mut identity = UdsIdentity::default();
        let mut dids_seen = HashMap::new();

        // Map of DID to field name for easy lookup
        let did_field_map = [
            (0xF197, "system_name"),
            (0xF187, "spare_part_number"),
            (0xF18A, "system_supplier"),
            (0xF191, "hardware_number"),
            (0xF195, "software_version"),
        ];

        for (did, data) in responses {
            if let Some((_, field_name)) = did_field_map.iter().find(|(d, _)| *d == did) {
                // The first usable value for each DID wins; unusable ones don't claim it
                let text = identification_text(data);
                if text.is_some() && dids_seen.insert(did, ()).is_none() {
                    match *field_name {
                        "system_name" => identity.system_name = text,
                        "spare_part_number" => identity.spare_part_number = text,
                        "system_supplier" => identity.system_supplier = text,
                        "hardware_number" => identity.hardware_number = text,
                        "software_version" => identity.software_version = text,
                        _ => unreachable!(),
                    }
                }
            }
        }

        identity
    }

    /// Generate a display name for this module.
    ///
    /// Uses the system name when available, otherwise falls back to a generic label.
    pub fn label(&self, address: &str) -> String {
        match &self.system_name {
            Some(name) => name.clone(),
            None => format!("Module at {address}"),
        }
    }

    /// Check if this identity has a system name.
    ///
    /// Returns true only when `system_name` is Some.
    pub fn is_named(&self) -> bool {
        self.system_name.is_some()
    }
}

/// Parse identification text from UDS data.
///
/// Strip leading and trailing bytes that are 0x00, 0xFF, space, tab, CR (0x0D) or LF (0x0A).
/// If nothing is left, return None.
/// If any remaining byte is outside printable ASCII (0x20..=0x7E), return None.
/// Collapse every run of two or more spaces inside the text into a single space.
/// Return Some(the text).
pub fn identification_text(data: &[u8]) -> Option<String> {
    // Strip leading and trailing bytes that are 0x00, 0xFF, space, tab, CR (0x0D) or LF (0x0A)
    let mut start = 0;
    let mut end = data.len();

    // Find first non-skipped byte from the start
    while start < data.len()
        && (data[start] == 0x00
            || data[start] == 0xFF
            || data[start] == b' '
            || data[start] == b'\t'
            || data[start] == b'\r'
            || data[start] == b'\n')
    {
        start += 1;
    }

    // Find last non-skipped byte from the end
    while end > start
        && (data[end - 1] == 0x00
            || data[end - 1] == 0xFF
            || data[end - 1] == b' '
            || data[end - 1] == b'\t'
            || data[end - 1] == b'\r'
            || data[end - 1] == b'\n')
    {
        end -= 1;
    }

    // If nothing is left, return None
    if start >= end {
        return None;
    }

    // Check if any remaining byte is outside printable ASCII (0x20..=0x7E)
    for &b in &data[start..end] {
        if !(0x20..=0x7E).contains(&b) {
            return None;
        }
    }

    // Convert to string and collapse multiple spaces
    let s = String::from_utf8(data[start..end].to_vec()).ok()?;
    let collapsed = collapse_spaces(&s);

    Some(collapsed)
}

fn collapse_spaces(s: &str) -> String {
    let mut result = String::new();
    let chars = s.chars().peekable();
    let mut space_count = 0;

    for ch in chars {
        if ch == ' ' {
            space_count += 1;
        } else {
            if space_count > 0 {
                result.push(' ');
            }
            space_count = 0;
            result.push(ch);
        }
    }

    // Handle trailing spaces
    if space_count > 1 {
        result.push(' ');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identification_text() {
        assert_eq!(identification_text(b"ABS MODULE"), Some("ABS MODULE".to_string()));
        assert_eq!(identification_text(b"  PCM   \x00\x00\xFF\xFF"), Some("PCM".to_string()));
        assert_eq!(identification_text(b"BODY    CONTROL"), Some("BODY CONTROL".to_string()));
        assert_eq!(identification_text(b"\tIPC\r\n"), Some("IPC".to_string()));
        assert_eq!(identification_text(b""), None);
        assert_eq!(identification_text(b"\x00\x00\x00"), None);
        assert_eq!(identification_text(b"\xFF\xFF"), None);
        assert_eq!(identification_text(b"    "), None);
        assert_eq!(identification_text(b"PCM\x01"), None);
        assert_eq!(identification_text(b"\xC3\xA9CU"), None);
    }

    #[test]
    fn test_uds_identity() {
        // All five DIDs present
        let responses = vec![
            (0xF197, &b"ENGINE CONTROL  "[..]),
            (0xF187, &b"KC3A-12A650-XA"[..]),
            (0xF18A, &b"BOSCH"[..]),
            (0xF191, &b"KC3A-12B684-AA\x00"[..]),
            (0xF195, &b"1.4.2\xFF\xFF"[..]),
        ];
        let identity = UdsIdentity::from_dids(responses);
        assert_eq!(identity.system_name, Some("ENGINE CONTROL".to_string()));
        assert_eq!(identity.spare_part_number, Some("KC3A-12A650-XA".to_string()));
        assert_eq!(identity.system_supplier, Some("BOSCH".to_string()));
        assert_eq!(identity.hardware_number, Some("KC3A-12B684-AA".to_string()));
        assert_eq!(identity.software_version, Some("1.4.2".to_string()));
        assert!(identity.is_named());
        assert_eq!(identity.label("7E0"), "ENGINE CONTROL");

        // Only some DIDs present
        let responses = vec![(0xF187, &b"PART-1"[..]), (0xF1A0, &b"IGNORED"[..])];
        let identity = UdsIdentity::from_dids(responses);
        assert_eq!(identity.spare_part_number, Some("PART-1".to_string()));
        assert_eq!(identity.system_name, None);
        assert!(!identity.is_named());
        assert_eq!(identity.label("760"), "Module at 760");

        // Unusable name
        let responses = vec![(0xF197, &b"\xFF\xFF\xFF"[..])];
        let identity = UdsIdentity::from_dids(responses);
        assert_eq!(identity.system_name, None);
        assert!(!identity.is_named());
        assert_eq!(identity.label("7E8"), "Module at 7E8");

        // Duplicates
        let responses =
            vec![(0xF197, &b"\x00\x00"[..]), (0xF197, &b"FIRST"[..]), (0xF197, &b"SECOND"[..])];
        let identity = UdsIdentity::from_dids(responses);
        assert_eq!(identity.system_name, Some("FIRST".to_string()));

        // Empty iterator
        let identity = UdsIdentity::from_dids(Vec::<(u16, &[u8])>::new());
        assert_eq!(identity, UdsIdentity::default());
    }
}
