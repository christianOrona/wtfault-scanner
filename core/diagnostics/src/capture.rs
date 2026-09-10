//! Capturing a module's configuration, and working out what moved between two
//! captures.
//!
//! # Why this exists
//!
//! Every configurable feature in the shipped catalogue has `mapping: null`,
//! because this project has measured none. That is honest, and on its own it is
//! also a dead end: a person is told the app knows the feature exists and
//! cannot say where it lives, with no way forward.
//!
//! This is the way forward. The procedure is mechanical, and it is the same one
//! every community mapping has ever come from:
//!
//! 1. Capture the module's configuration.
//! 2. Change the setting once, with a tool already known to do it correctly.
//! 3. Capture again.
//! 4. The bits that moved are the mapping.
//!
//! # What this deliberately does not do
//!
//! It does not discover a mapping on its own. Step 2 needs a different tool,
//! and there is no honest way around that: the only way to learn which bit
//! means "fold the mirrors" without being told is to change bits and watch the
//! vehicle, and changing bits in a door module to see what happens is how a
//! door module stops working.
//!
//! So the app does the parts it can do exactly - read, record, compare and
//! propose - and is clear that the middle step is yours.

use aim_decoders::Mapping;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One module's configuration at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCapture {
    /// Diagnostic request address of the module this came from.
    ///
    /// A string, so a 29-bit module (`18DA10F1`) is expressible. It was a
    /// `u16`, which silently truncated one.
    pub module: String,
    /// Records by data identifier, exactly as the module returned them.
    pub records: BTreeMap<u16, Vec<u8>>,
    /// When it was taken, ISO 8601.
    pub taken_at: String,
    /// Free-text note, so "before" and "after" are still distinguishable by a
    /// person six weeks later.
    #[serde(default)]
    pub label: Option<String>,
}

/// One byte that differs between two captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteChange {
    /// The data identifier holding it.
    pub did: u16,
    /// Zero-based byte within that record.
    pub byte: u8,
    /// Value before.
    pub before: u8,
    /// Value after.
    pub after: u8,
    /// Bits that actually changed, as a mask.
    pub changed_mask: u8,
}

impl ByteChange {
    /// A mapping that would describe this change, ready to be recorded.
    ///
    /// `after_is_on` is taken from whoever flipped the switch, because the
    /// bytes do not say which state is the enabled one and guessing it writes
    /// the setting backwards on every vehicle that ever uses the mapping.
    pub fn as_mapping(&self, module: &str, after_is_on: bool) -> Mapping {
        let (on, off) = if after_is_on {
            (self.after & self.changed_mask, self.before & self.changed_mask)
        } else {
            (self.before & self.changed_mask, self.after & self.changed_mask)
        };
        Mapping::DataIdentifierBits {
            module: module.to_string(),
            did: self.did,
            byte: self.byte,
            mask: self.changed_mask,
            on,
            off,
        }
    }
}

/// What differs between two captures of the same module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDiff {
    /// Bytes that changed.
    pub changes: Vec<ByteChange>,
    /// Identifiers present in the earlier capture and not the later one, which
    /// usually means the two came from different modules or different vehicles.
    pub only_in_before: Vec<u16>,
    /// As above, the other way round.
    pub only_in_after: Vec<u16>,
    /// Identifiers whose record changed length. A length change is not a
    /// setting moving; it is a sign the two captures are not comparable.
    pub length_mismatches: Vec<u16>,
}

impl CaptureDiff {
    /// True when the two captures describe the same shape of configuration.
    ///
    /// A diff that is not comparable must not be turned into a mapping, however
    /// convincing the single changed byte in the middle of it looks.
    pub fn is_comparable(&self) -> bool {
        self.only_in_before.is_empty()
            && self.only_in_after.is_empty()
            && self.length_mismatches.is_empty()
    }

    /// True when exactly one byte moved, which is the only case a mapping can
    /// be proposed from without guessing.
    pub fn is_unambiguous(&self) -> bool {
        self.is_comparable() && self.changes.len() == 1
    }
}

/// Compare two captures.
///
/// Order matters and is not inferred: `before` is the earlier one. Nothing here
/// decides which state is "on" - that is something the person who flipped the
/// switch knows and the bytes do not say.
pub fn diff(before: &ConfigCapture, after: &ConfigCapture) -> CaptureDiff {
    let mut changes = Vec::new();
    let mut length_mismatches = Vec::new();

    for (did, b) in &before.records {
        let Some(a) = after.records.get(did) else { continue };
        if a.len() != b.len() {
            length_mismatches.push(*did);
            continue;
        }
        for (i, (bv, av)) in b.iter().zip(a.iter()).enumerate() {
            if bv != av {
                changes.push(ByteChange {
                    did: *did,
                    byte: i as u8,
                    before: *bv,
                    after: *av,
                    changed_mask: bv ^ av,
                });
            }
        }
    }

    let only_in_before =
        before.records.keys().filter(|d| !after.records.contains_key(d)).copied().collect();
    let only_in_after =
        after.records.keys().filter(|d| !before.records.contains_key(d)).copied().collect();

    CaptureDiff { changes, only_in_before, only_in_after, length_mismatches }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(module: &str, records: &[(u16, &[u8])]) -> ConfigCapture {
        ConfigCapture {
            module: module.to_string(),
            records: records.iter().map(|(d, r)| (*d, r.to_vec())).collect(),
            taken_at: "2026-09-09T00:00:00Z".into(),
            label: None,
        }
    }

    #[test]
    fn one_bit_moving_is_found_and_isolated_to_that_bit() {
        let before = capture("726", &[(0xDE01, &[0x00, 0b0000_0000])]);
        let after = capture("726", &[(0xDE01, &[0x00, 0b0000_0100])]);

        let d = diff(&before, &after);
        assert!(d.is_unambiguous());
        assert_eq!(d.changes.len(), 1);
        let c = &d.changes[0];
        assert_eq!(c.byte, 1);
        // Only the bit that moved, not the whole byte: a mapping claiming the
        // byte would also claim settings nobody looked at.
        assert_eq!(c.changed_mask, 0b0000_0100);
    }

    #[test]
    fn the_proposed_mapping_carries_only_the_bits_that_moved() {
        let before = capture("726", &[(0xDE01, &[0xFF, 0b1010_0000])]);
        let after = capture("726", &[(0xDE01, &[0xFF, 0b1010_0100])]);

        let d = diff(&before, &after);
        match d.changes[0].as_mapping("726", true) {
            Mapping::DataIdentifierBits { module, did, byte, mask, on, off } => {
                assert_eq!((module.as_str(), did, byte), ("726", 0xDE01, 1));
                assert_eq!(mask, 0b0000_0100);
                assert_eq!(on, 0b0000_0100, "after was the enabled state");
                assert_eq!(off, 0b0000_0000);
            }
            other => panic!("wrong mapping kind: {other:?}"),
        }
    }

    /// The direction is the caller's to state. Getting it backwards writes the
    /// setting the wrong way round on every vehicle that ever uses the mapping.
    #[test]
    fn the_enabled_state_is_stated_rather_than_inferred() {
        let before = capture("726", &[(0xDE01, &[0b0000_0100])]);
        let after = capture("726", &[(0xDE01, &[0b0000_0000])]);
        let d = diff(&before, &after);

        match d.changes[0].as_mapping("726", false) {
            Mapping::DataIdentifierBits { on, off, .. } => {
                assert_eq!(on, 0b0000_0100, "before was the enabled state");
                assert_eq!(off, 0b0000_0000);
            }
            other => panic!("wrong mapping kind: {other:?}"),
        }
    }

    #[test]
    fn several_bytes_moving_is_reported_but_not_unambiguous() {
        let before = capture("726", &[(0xDE01, &[0x00, 0x00])]);
        let after = capture("726", &[(0xDE01, &[0x01, 0x02])]);
        let d = diff(&before, &after);
        assert!(d.is_comparable());
        assert!(!d.is_unambiguous(), "two bytes moved; a mapping would be a guess");
        assert_eq!(d.changes.len(), 2);
    }

    /// Two captures of different shapes are not two states of one thing.
    #[test]
    fn a_length_change_makes_the_captures_incomparable() {
        let before = capture("726", &[(0xDE01, &[0x00, 0x00])]);
        let after = capture("726", &[(0xDE01, &[0x00, 0x00, 0x00])]);
        let d = diff(&before, &after);
        assert!(!d.is_comparable());
        assert_eq!(d.length_mismatches, vec![0xDE01]);
        assert!(d.changes.is_empty(), "no byte-level claim from incomparable records");
    }

    #[test]
    fn identifiers_present_in_only_one_capture_are_reported() {
        let before = capture("726", &[(0xDE01, &[0x00]), (0xDE02, &[0x00])]);
        let after = capture("726", &[(0xDE01, &[0x00]), (0xDE03, &[0x00])]);
        let d = diff(&before, &after);
        assert!(!d.is_comparable());
        assert_eq!(d.only_in_before, vec![0xDE02]);
        assert_eq!(d.only_in_after, vec![0xDE03]);
    }

    #[test]
    fn identical_captures_produce_no_changes() {
        let c = capture("726", &[(0xDE01, &[0x12, 0x34])]);
        let d = diff(&c, &c);
        assert!(d.is_comparable());
        assert!(d.changes.is_empty());
        assert!(!d.is_unambiguous(), "nothing moved, so nothing can be proposed");
    }
}
