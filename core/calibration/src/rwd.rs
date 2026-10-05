//! The header of a Honda `.rwd` update package, read and nothing more.
//!
//! # What this rests on
//!
//! Honda does not publish this format. Everything here follows one public
//! description of it, the `rwd-xray` project's README and reader, which calls
//! itself a work in progress. That description gives the *shape* of the
//! header for two layouts and says nothing about what most of the values in
//! it *mean*. So this module reads the shape and stops there:
//!
//! - It says which layout a file is, by its first byte.
//! - For the two layouts whose header is described, it reads the header's
//!   groups and keeps the values that are plain text.
//! - It does not say that a value is the calibration the file contains, the
//!   one it replaces, or a part number. The description does not say, and a
//!   guess here would become an "exact match" somewhere else.
//!
//! No real RWD file has been through this code. It is tested against files
//! built to the described shape.
//!
//! # What it never does
//!
//! The software in the package follows the header and is encoded. This module
//! does not read it. The public description says one header group holds the
//! key the software is encoded with; that group's values are counted and not
//! read out, because this crate has no use for a key.

use serde::{Deserialize, Serialize};

/// The most header groups read before the header is taken to be something
/// other than what was described.
const MAX_GROUPS: usize = 64;
/// The most values read in one group.
const MAX_VALUES: usize = 4096;
/// The longest single header line read in the delimited layout.
const MAX_LINE: usize = 1024;

/// Which of the described layouts a file is, by its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// First byte `5A` (`Z`). Header: counted groups of length-prefixed values.
    Z,
    /// First byte `31` (`1`). Header: groups of lines, each group opened and
    /// closed by its own one-byte tag.
    One,
    /// First byte `58` (`X`). Known to exist; its header is not described.
    X,
    /// First byte `59` (`Y`). Known to exist; its header is not described.
    Y,
    /// First byte `30` (`0`). Known to exist; its header is not described.
    Zero,
    /// One byte then `0D 0A`, as every described file begins, with a first
    /// byte that is none of the five described.
    Other,
}

impl Layout {
    fn of(indicator: u8) -> Layout {
        match indicator {
            0x5A => Layout::Z,
            0x31 => Layout::One,
            0x58 => Layout::X,
            0x59 => Layout::Y,
            0x30 => Layout::Zero,
            _ => Layout::Other,
        }
    }
}

/// One group of header values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderGroup {
    /// Which group: its position (`0`, `1`, ...) in the counted layout, its
    /// tag character in the delimited one.
    pub tag: String,
    /// The values in it that are plain text, as written.
    pub texts: Vec<String>,
    /// How many values it holds that are not shown: bytes that are not text,
    /// and every value of the group described as holding the encoding key.
    pub not_shown: usize,
}

/// What an RWD package's header holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RwdHeader {
    /// The file's first byte, as two hex digits.
    pub indicator: String,
    /// The layout that byte names.
    pub layout: Layout,
    /// True when the header's shape is described for this layout and it was
    /// read through to its end. False leaves `groups` as far as reading got,
    /// and nothing should be concluded from them.
    pub headers_read: bool,
    /// The groups, in file order.
    pub groups: Vec<HeaderGroup>,
    /// How many bytes the signature and header take, when read to the end.
    /// The encoded software starts after them and is not read.
    pub header_bytes: Option<u64>,
    /// Why the header was not read through, when it was not.
    pub problem: Option<String>,
}

impl RwdHeader {
    /// Every text value in the header, in file order.
    pub fn texts(&self) -> Vec<&str> {
        self.groups.iter().flat_map(|g| g.texts.iter().map(String::as_str)).collect()
    }
}

/// True when bytes begin the way every described RWD file does: one indicator
/// byte, then `0D 0A`.
pub fn has_signature(bytes: &[u8]) -> bool {
    matches!(bytes, [_, 0x0D, 0x0A, ..])
}

/// A header value as text, when it is text: printable ASCII once the padding
/// a fixed-width field carries is trimmed. Anything else is not shown.
fn text_of(data: &[u8]) -> Option<String> {
    let trimmed: &[u8] = {
        let pad = |b: &u8| matches!(*b, 0x00 | 0xFF | b' ' | b'\r' | b'\n' | b'\t');
        let start = data.iter().position(|b| !pad(b)).unwrap_or(data.len());
        let end = data.iter().rposition(|b| !pad(b)).map_or(start, |i| i + 1);
        &data[start..end]
    };
    if trimmed.is_empty() || !trimmed.iter().all(|b| (0x20..=0x7E).contains(b)) {
        return None;
    }
    Some(String::from_utf8_lossy(trimmed).into_owned())
}

/// Read an RWD package's header. `None` when the bytes do not begin as one.
///
/// Never fails and never reads past the header: a file that ends early, or
/// whose header is not shaped as described, comes back with `headers_read`
/// false and the reason.
pub fn read(payload: &[u8]) -> Option<RwdHeader> {
    if !has_signature(payload) {
        return None;
    }
    let layout = Layout::of(payload[0]);
    let mut header = RwdHeader {
        indicator: format!("{:02x}", payload[0]),
        layout,
        headers_read: false,
        groups: Vec::new(),
        header_bytes: None,
        problem: None,
    };
    let body = &payload[3..];
    let read = match layout {
        Layout::Z => counted_groups(body, &mut header.groups),
        Layout::One => delimited_groups(body, &mut header.groups),
        Layout::X | Layout::Y | Layout::Zero => {
            Err(String::from("no public description of this layout's header is known"))
        }
        Layout::Other => Err(String::from(
            "the first byte is not one of the five layouts that have been described",
        )),
    };
    match read {
        Ok(length) => {
            header.headers_read = true;
            header.header_bytes = Some(3 + length as u64);
        }
        Err(problem) => header.problem = Some(problem),
    }
    Some(header)
}

/// The `Z` layout: a run of groups, each a count byte followed by that many
/// values, each value a length byte followed by that many bytes. The run is
/// wrapped in empty groups and ends at the second one.
fn counted_groups(body: &[u8], groups: &mut Vec<HeaderGroup>) -> Result<usize, String> {
    let ended = || String::from("the file ends inside its header");
    let mut at = 0usize;
    let mut empty_groups = 0;
    while empty_groups < 2 {
        if groups.len() >= MAX_GROUPS {
            return Err(format!("more than {MAX_GROUPS} header groups: not the described shape"));
        }
        let count = *body.get(at).ok_or_else(ended)?;
        at += 1;
        if count == 0 {
            empty_groups += 1;
        }
        let mut group =
            HeaderGroup { tag: groups.len().to_string(), texts: Vec::new(), not_shown: 0 };
        for _ in 0..count {
            let length = usize::from(*body.get(at).ok_or_else(ended)?);
            at += 1;
            let data = body.get(at..at + length).ok_or_else(ended)?;
            at += length;
            match text_of(data) {
                Some(text) => group.texts.push(text),
                None => group.not_shown += 1,
            }
        }
        groups.push(group);
    }
    Ok(at)
}

/// The `1` layout: a run of groups, each opened by a tag byte and `0D 0A`,
/// holding lines that end `0D 0A`, and closed by the tag and `0D 0A` again.
/// The run ends where a byte is not followed by `0D 0A`.
fn delimited_groups(body: &[u8], groups: &mut Vec<HeaderGroup>) -> Result<usize, String> {
    let ended = || String::from("the file ends inside its header");
    let mut at = 0usize;
    while let Some(&[tag, 0x0D, 0x0A]) = body.get(at..at + 3) {
        if groups.len() >= MAX_GROUPS {
            return Err(format!("more than {MAX_GROUPS} header groups: not the described shape"));
        }
        at += 3;
        // The group the public description reads the encoding key from. Its
        // values are counted and not read out.
        let holds_key = tag == b'&';
        let mut group = HeaderGroup {
            tag: text_of(&[tag]).unwrap_or_else(|| format!("0x{tag:02x}")),
            texts: Vec::new(),
            not_shown: 0,
        };
        loop {
            let rest = body.get(at..).ok_or_else(ended)?;
            let end = rest.iter().position(|b| *b == b'\n').ok_or_else(ended)?;
            if end > MAX_LINE {
                return Err(format!(
                    "a header line longer than {MAX_LINE} bytes: not the described shape"
                ));
            }
            let line = &rest[..=end];
            at += end + 1;
            if line == [tag, 0x0D, 0x0A] {
                break;
            }
            if group.texts.len() + group.not_shown >= MAX_VALUES {
                return Err(format!(
                    "more than {MAX_VALUES} values in one header group: not the described shape"
                ));
            }
            match text_of(line) {
                Some(text) if !holds_key => group.texts.push(text),
                _ => group.not_shown += 1,
            }
        }
        groups.push(group);
    }
    Ok(at)
}

/// Build files to the described shape, for tests here and in the crates that
/// use this one. The identifiers are made up and say so.
#[doc(hidden)]
pub mod fixture {
    /// A `Z`-layout package: an empty group, one group per slice of values,
    /// an empty group, then bytes standing in for the encoded software.
    pub fn z(groups: &[&[&[u8]]], software: &[u8]) -> Vec<u8> {
        let mut out = vec![0x5A, 0x0D, 0x0A, 0x00];
        for values in groups {
            out.push(values.len() as u8);
            for value in *values {
                out.push(value.len() as u8);
                out.extend_from_slice(value);
            }
        }
        out.push(0x00);
        out.extend_from_slice(software);
        out
    }

    /// A `1`-layout package: each group as its tag and its lines, then bytes
    /// standing in for the encoded software.
    pub fn one(groups: &[(u8, &[&str])], software: &[u8]) -> Vec<u8> {
        let mut out = vec![0x31, 0x0D, 0x0A];
        for (tag, lines) in groups {
            out.extend_from_slice(&[*tag, 0x0D, 0x0A]);
            for line in *lines {
                out.extend_from_slice(line.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(&[*tag, 0x0D, 0x0A]);
        }
        out.extend_from_slice(software);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes standing in for encoded software. It does not start with a byte
    /// followed by `0D 0A`, as nothing after a real header is described to.
    const SOFTWARE: &[u8] = &[0x9C, 0x41, 0x07, 0xE2, 0x55, 0x00, 0x0D, 0x0A, 0x13];

    #[test]
    fn anything_that_does_not_begin_as_an_rwd_is_not_one() {
        assert!(read(b"").is_none());
        assert!(read(b"Z\r").is_none());
        assert!(read(b"ZZ\r\n").is_none());
        assert!(read(&[0x1F, 0x8B, 0x08]).is_none());
        assert!(has_signature(b"Z\r\n"));
        assert!(!has_signature(b"Z\n\r"));
    }

    #[test]
    fn the_counted_layout_gives_its_text_and_counts_what_is_not_text() {
        let file = fixture::z(
            &[&[b"TEST-MOD-A010\x00\x00", b"TEST-MOD-A020\x00\x00"], &[&[0x01, 0x02, 0x03]]],
            SOFTWARE,
        );
        let header = read(&file).unwrap();

        assert_eq!(header.layout, Layout::Z);
        assert_eq!(header.indicator, "5a");
        assert!(header.headers_read, "{:?}", header.problem);
        assert_eq!(header.texts(), vec!["TEST-MOD-A010", "TEST-MOD-A020"]);
        // Wrapped in two empty groups, as described.
        assert_eq!(header.groups.len(), 4);
        assert_eq!(header.groups[2].not_shown, 1);
        assert!(header.groups[2].texts.is_empty());
        // The header ends where the software begins, and nothing past it was read.
        assert_eq!(header.header_bytes, Some((file.len() - SOFTWARE.len()) as u64));
    }

    #[test]
    fn the_delimited_layout_gives_its_lines_and_never_the_key_group() {
        let file = fixture::one(
            &[(b'$', &["TEST-MOD-A010", "TEST-MOD-A020"]), (b'&', &["0a1b2c"])],
            SOFTWARE,
        );
        let header = read(&file).unwrap();

        assert_eq!(header.layout, Layout::One);
        assert!(header.headers_read, "{:?}", header.problem);
        assert_eq!(header.texts(), vec!["TEST-MOD-A010", "TEST-MOD-A020"]);
        let key = header.groups.iter().find(|g| g.tag == "&").unwrap();
        assert!(key.texts.is_empty(), "the key group is counted, not read out");
        assert_eq!(key.not_shown, 1);
        assert!(!serde_json::to_string(&header).unwrap().contains("0a1b2c"));
        assert_eq!(header.header_bytes, Some((file.len() - SOFTWARE.len()) as u64));
    }

    #[test]
    fn a_header_that_ends_early_is_not_read_and_says_so() {
        let whole = fixture::z(&[&[b"TEST-MOD-A010"]], SOFTWARE);
        // Cut inside the one value.
        let header = read(&whole[..10]).unwrap();
        assert!(!header.headers_read);
        assert!(header.problem.as_deref().is_some_and(|p| p.contains("ends inside")));
        assert_eq!(header.header_bytes, None);

        let whole = fixture::one(&[(b'$', &["TEST-MOD-A010"])], SOFTWARE);
        // The group is opened and never closed.
        let cut = &whole[..whole.len() - SOFTWARE.len() - 3];
        let header = read(cut).unwrap();
        assert!(!header.headers_read);
        assert!(header.problem.is_some());
    }

    #[test]
    fn a_layout_nobody_has_described_is_named_and_not_guessed_at() {
        for (first, layout) in [(b'X', Layout::X), (b'Y', Layout::Y), (b'0', Layout::Zero)] {
            let header = read(&[first, 0x0D, 0x0A, 0x01, 0x04, b'T', b'E', b'S', b'T']).unwrap();
            assert_eq!(header.layout, layout);
            assert!(!header.headers_read);
            assert!(header.groups.is_empty());
            assert!(header.problem.as_deref().is_some_and(|p| p.contains("no public description")));
        }
        let other = read(b"Q\r\nanything").unwrap();
        assert_eq!(other.layout, Layout::Other);
        assert!(!other.headers_read);
    }

    #[test]
    fn bytes_shaped_like_nothing_described_stop_the_reading_instead_of_running_on() {
        // Every byte a count of 255 with no values behind it.
        let mut junk = vec![0x5A, 0x0D, 0x0A];
        junk.extend_from_slice(&[0xFF; 8]);
        let header = read(&junk).unwrap();
        assert!(!header.headers_read);

        // A delimited group whose line never ends within reason.
        let mut long = vec![0x31, 0x0D, 0x0A, b'$', 0x0D, 0x0A];
        long.extend_from_slice(&[b'A'; MAX_LINE + 10]);
        long.extend_from_slice(b"\r\n$\r\n");
        let header = read(&long).unwrap();
        assert!(!header.headers_read);
        assert!(header.problem.as_deref().is_some_and(|p| p.contains("longer than")));
    }

    #[test]
    fn text_is_printable_ascii_with_its_padding_trimmed() {
        assert_eq!(text_of(b"TEST-MOD-A010\x00\x00").as_deref(), Some("TEST-MOD-A010"));
        assert_eq!(text_of(b"  spaced \r\n").as_deref(), Some("spaced"));
        assert_eq!(text_of(&[0x01, 0x02, 0x03]), None);
        assert_eq!(text_of(&[0x00, 0x00]), None);
        assert_eq!(text_of(b""), None);
    }
}
