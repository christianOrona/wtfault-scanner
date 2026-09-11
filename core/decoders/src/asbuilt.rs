//! Ford "as built" data: the factory configuration of every module on one
//! vehicle, keyed to its VIN.
//!
//! # What this is
//!
//! Ford records how each vehicle was configured when it was built and publishes
//! it per-VIN. The file is XML and its useful content is a list of lines:
//!
//! ```text
//! <DATA LABEL="726-15-01"><CODE>0101</CODE><CODE>0101</CODE><CODE>0148</CODE></DATA>
//!        module ─┘ │  └─ line
//!                 block
//! ```
//!
//! Concatenate the codes and the **last byte is a checksum**; everything before
//! it is data. Lines of the same block join in order.
//!
//! # Why it matters here
//!
//! Two reasons, and the second is the one that changes what this app can do.
//!
//! First, it covers the whole vehicle. Measured on a 2019 F-250: the file held
//! 29 modules while only six were awake on the bus. Configuration for a module
//! that is asleep, or on a bus this adapter cannot reach, is in the file
//! regardless.
//!
//! Second, **a block corresponds to a data identifier**, which makes every
//! published as-built modification executable. Community documentation for
//! Ford configuration is written in as-built terms - "set 726-15-01 byte 4" -
//! and this app can only send UDS. The correspondence bridges them:
//!
//! ```text
//! as-built block N  ↔  data identifier 0xDE00 + (N - 1)
//! ```
//!
//! That is measured rather than assumed. On the truck above, every block
//! checked matched the live read of the corresponding identifier byte for byte,
//! including `726-09` against `DE08`, which decodes to the ASCII `1FT7W2` of the
//! vehicle's own VIN and could not match by coincidence.
//!
//! # What it is not
//!
//! It is a snapshot of how a vehicle **left the factory**, not how it is now.
//! Anything an owner or a dealer has changed since is not in it, which is
//! exactly what makes comparing it against a live read worth doing: the
//! difference is the history.
//!
//! It is also **specific to one VIN and contains that VIN**. A file belongs to
//! the vehicle it describes and should not be treated as a template for another
//! one. What generalises is the *labelling* - which block and byte holds which
//! feature - not the values.

use std::collections::BTreeMap;

/// One vehicle's factory configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AsBuiltData {
    /// The VIN the file was issued for.
    pub vin: Option<String>,
    /// Module address (as written, e.g. `726`) to its blocks.
    pub modules: BTreeMap<String, BTreeMap<u16, Block>>,
}

/// One configuration block: the data bytes of its lines, joined in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    /// Block number, as in `726-15`.
    pub number: u16,
    /// Data bytes, checksums removed, lines concatenated in order.
    pub bytes: Vec<u8>,
    /// The checksum byte each line carried, in line order.
    ///
    /// Kept rather than discarded: a write back to a module has to carry a
    /// correct checksum, and the algorithm is not something this build should
    /// assume. Holding the originals means a later change can be checked
    /// against what the factory wrote.
    pub line_checksums: Vec<u8>,
}

impl AsBuiltData {
    /// The data identifier a block corresponds to.
    ///
    /// Blocks are numbered from one and identifiers run from `0xDE00`. Measured
    /// on a 2019 F-250 across several blocks, including one holding the VIN in
    /// ASCII, where a coincidental match is not credible.
    ///
    /// Returns `None` for block zero, which does not occur in a real file and
    /// would otherwise underflow into an unrelated identifier.
    pub fn did_for_block(block: u16) -> Option<u16> {
        if block == 0 {
            return None;
        }
        0xDE00u16.checked_add(block - 1)
    }

    /// The block a data identifier corresponds to, the other way round.
    pub fn block_for_did(did: u16) -> Option<u16> {
        did.checked_sub(0xDE00).map(|offset| offset + 1)
    }

    /// Parse an as-built XML document.
    ///
    /// Deliberately tolerant of the surrounding structure: the file groups
    /// modules under element names that vary by vehicle, and none of that
    /// grouping is needed. What matters is the labelled lines, which have one
    /// shape.
    pub fn parse(text: &str) -> Result<AsBuiltData, String> {
        let vin = between(text, "<VIN>", "</VIN>").map(|v| v.trim().to_ascii_uppercase());
        let mut modules: BTreeMap<String, BTreeMap<u16, Block>> = BTreeMap::new();
        let mut lines_seen = 0usize;

        for chunk in text.split("<DATA ").skip(1) {
            let Some(label) = between(chunk, "LABEL=\"", "\"") else { continue };
            let Some((module, block, _line)) = split_label(label) else { continue };

            // Codes are hex, and the final byte across all of them is a
            // checksum rather than content.
            let mut hex = String::new();
            for code in chunk.split("<CODE>").skip(1) {
                if let Some(value) = code.split('<').next() {
                    hex.push_str(value.trim());
                }
            }
            let Some(mut bytes) = hex_bytes(&hex) else { continue };
            let Some(checksum) = bytes.pop() else { continue };

            lines_seen += 1;
            let entry = modules
                .entry(module)
                .or_default()
                .entry(block)
                .or_insert_with(|| Block { number: block, ..Block::default() });
            entry.bytes.extend(bytes);
            entry.line_checksums.push(checksum);
        }

        if lines_seen == 0 {
            return Err(String::from(
                "no as-built lines found: expected elements labelled like \"726-15-01\"",
            ));
        }
        Ok(AsBuiltData { vin, modules })
    }

    /// How many configuration lines were read.
    pub fn line_count(&self) -> usize {
        self.modules.values().flat_map(|b| b.values()).map(|b| b.line_checksums.len()).sum()
    }

    /// The blocks recorded for one module.
    pub fn module(&self, address: &str) -> Option<&BTreeMap<u16, Block>> {
        self.modules.get(&address.to_ascii_uppercase())
    }
}

/// Split `726-15-01` into module, block and line.
fn split_label(label: &str) -> Option<(String, u16, u16)> {
    let mut parts = label.split('-');
    let module = parts.next()?.trim().to_ascii_uppercase();
    let block = parts.next()?.trim().parse::<u16>().ok()?;
    let line = parts.next()?.trim().parse::<u16>().ok()?;
    if module.is_empty() || !module.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((module, block, line))
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(close)?;
    Some(&rest[..end])
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() || s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines quoted verbatim from the as-built file of a 2019 F-250, chosen
    /// because their live counterparts were read from the same truck.
    const REAL: &str = r#"<AS_BUILT_DATA><VEHICLE><VIN>1FT7W2BT7KEF78036</VIN>
<BCE_MODULE>
<DATA LABEL="726-01-01"><CODE>0001</CODE><CODE>0101</CODE><CODE>0032</CODE></DATA>
<DATA LABEL="726-01-02"><CODE>0A00</CODE><CODE>0101</CODE><CODE>003C</CODE></DATA>
<DATA LABEL="726-03-01"><CODE>0132</CODE><CODE /><CODE /></DATA>
<DATA LABEL="726-09-01"><CODE>3146</CODE><CODE>5437</CODE><CODE>5790</CODE></DATA>
<DATA LABEL="726-09-02"><CODE>326A</CODE><CODE /><CODE /></DATA>
<DATA LABEL="726-15-01"><CODE>0101</CODE><CODE>0101</CODE><CODE>0148</CODE></DATA>
<DATA LABEL="726-15-02"><CODE>0000</CODE><CODE>0000</CODE><CODE>0044</CODE></DATA>
</BCE_MODULE></VEHICLE></AS_BUILT_DATA>"#;

    #[test]
    fn the_real_file_shape_parses() {
        let data = AsBuiltData::parse(REAL).unwrap();
        assert_eq!(data.vin.as_deref(), Some("1FT7W2BT7KEF78036"));
        assert_eq!(data.line_count(), 7);
        assert_eq!(data.module("726").unwrap().len(), 4);
    }

    /// The correspondence this whole module exists for, checked against a live
    /// read of the same truck. `DE0E` read back
    /// `[1,1,1,1,1,0,0,0,0,0]`, and block 15 is where that lives.
    #[test]
    fn a_block_matches_the_identifier_it_was_read_from() {
        let data = AsBuiltData::parse(REAL).unwrap();
        let block15 = &data.module("726").unwrap()[&15];
        assert_eq!(block15.bytes, vec![1, 1, 1, 1, 1, 0, 0, 0, 0, 0]);
        assert_eq!(AsBuiltData::did_for_block(15), Some(0xDE0E));
        assert_eq!(AsBuiltData::block_for_did(0xDE0E), Some(15));
    }

    /// The one that cannot be coincidence: block 9 decodes to the ASCII of the
    /// vehicle's own VIN, and matched the live read of `DE08` byte for byte.
    #[test]
    fn the_vin_block_decodes_to_the_vin() {
        let data = AsBuiltData::parse(REAL).unwrap();
        let block9 = &data.module("726").unwrap()[&9];
        assert_eq!(block9.bytes, b"1FT7W2");
        assert_eq!(AsBuiltData::did_for_block(9), Some(0xDE08));
    }

    /// Checksums are removed from the data and kept separately: a write has to
    /// carry a correct one, and the algorithm is not something to assume.
    #[test]
    fn the_last_byte_of_each_line_is_a_checksum_and_is_kept() {
        let data = AsBuiltData::parse(REAL).unwrap();
        let module = data.module("726").unwrap();
        assert_eq!(module[&1].bytes, vec![0, 1, 1, 1, 0, 10, 0, 1, 1, 0]);
        assert_eq!(module[&1].line_checksums, vec![0x32, 0x3C]);
        // A single-line block with one data byte, which the real file contains.
        assert_eq!(module[&3].bytes, vec![1]);
        assert_eq!(module[&3].line_checksums, vec![0x32]);
    }

    #[test]
    fn a_file_with_no_lines_is_an_error_rather_than_an_empty_success() {
        assert!(AsBuiltData::parse("<AS_BUILT_DATA></AS_BUILT_DATA>").is_err());
        assert!(AsBuiltData::parse("").is_err());
    }

    /// Block zero does not occur and must not underflow into an unrelated
    /// identifier.
    #[test]
    fn block_zero_has_no_identifier() {
        assert_eq!(AsBuiltData::did_for_block(0), None);
    }
}
