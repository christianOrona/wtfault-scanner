//! What kind of file an artifact is.
//!
//! Two separate questions, kept separate: what its name says, and what its
//! content looks like. They are compared, and a file whose content cannot be
//! what its name says is not valid.
//!
//! These formats do not mean the same thing. An Intel HEX or S-record file is
//! addressed memory as text. A `.bin` is bytes with no description at all. A
//! Honda `.rwd` is that manufacturer's update package, and this crate does not
//! open it: it is recognised by name, hashed and kept whole, and nothing is
//! concluded from its insides. No parser for it ships here, and one is not
//! worked out by guessing.

use serde::{Deserialize, Serialize};

/// What an artifact's name says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    /// A Honda update package. Kept as it is; never opened.
    Rwd,
    /// Raw bytes with no container.
    Bin,
    /// A gzip-compressed file. What is inside is not asserted.
    Gz,
    /// Intel HEX.
    Hex,
    /// Motorola S-record.
    S19,
    /// Anything else. Kept, hashed and never treated as a calibration.
    Unknown,
}

impl ArtifactFormat {
    /// The formats this build knows.
    pub const SUPPORTED: [ArtifactFormat; 5] = [
        ArtifactFormat::Rwd,
        ArtifactFormat::Bin,
        ArtifactFormat::Gz,
        ArtifactFormat::Hex,
        ArtifactFormat::S19,
    ];

    /// The format a file name states, from its last extension.
    pub fn from_filename(name: &str) -> ArtifactFormat {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !name.contains('.') {
            return ArtifactFormat::Unknown;
        }
        match ext.as_str() {
            "rwd" => ArtifactFormat::Rwd,
            "bin" => ArtifactFormat::Bin,
            "gz" => ArtifactFormat::Gz,
            "hex" => ArtifactFormat::Hex,
            "s19" => ArtifactFormat::S19,
            _ => ArtifactFormat::Unknown,
        }
    }

    /// The extension files of this format are kept under.
    pub fn extension(self) -> &'static str {
        match self {
            ArtifactFormat::Rwd => "rwd",
            ArtifactFormat::Bin => "bin",
            ArtifactFormat::Gz => "gz",
            ArtifactFormat::Hex => "hex",
            ArtifactFormat::S19 => "s19",
            ArtifactFormat::Unknown => "dat",
        }
    }

    /// Whether this build treats the format as a calibration artifact.
    pub fn is_supported(self) -> bool {
        self != ArtifactFormat::Unknown
    }
}

/// What a file's content looks like, as far as can be told without
/// interpreting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    /// No bytes at all.
    Empty,
    /// Starts with the gzip signature, `1F 8B`.
    Gzip,
    /// Text in which every line is an Intel HEX record.
    IntelHex,
    /// Text in which every line is a Motorola S-record.
    SRecord,
    /// Anything else.
    Opaque,
}

/// Look at the content. Only the start is read for the text formats: this
/// says what the file looks like, not that every record in it is sound.
pub fn sniff(bytes: &[u8]) -> Content {
    if bytes.is_empty() {
        return Content::Empty;
    }
    if bytes.starts_with(&[0x1F, 0x8B]) {
        return Content::Gzip;
    }
    let head = &bytes[..bytes.len().min(4096)];
    let Ok(text) = std::str::from_utf8(head) else {
        return Content::Opaque;
    };
    // The last line may be cut by the 4096-byte window, so it is not judged.
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    if bytes.len() > head.len() {
        lines.pop();
    }
    if lines.is_empty() {
        return Content::Opaque;
    }
    let hex_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    if lines.iter().all(|l| l.len() >= 11 && l.starts_with(':') && hex_digits(&l[1..])) {
        return Content::IntelHex;
    }
    if lines.iter().all(|l| {
        l.len() >= 10
            && l.starts_with('S')
            && l.as_bytes()[1].is_ascii_digit()
            && hex_digits(&l[2..])
    }) {
        return Content::SRecord;
    }
    Content::Opaque
}

/// Whether content can be what the name says. `None` when the format has
/// nothing its content can be checked against, which is true of raw bytes and
/// of a package this crate does not open.
pub fn content_agrees(format: ArtifactFormat, content: Content) -> Option<bool> {
    match (format, content) {
        (_, Content::Empty) => Some(false),
        (ArtifactFormat::Gz, c) => Some(c == Content::Gzip),
        (ArtifactFormat::Hex, c) => Some(c == Content::IntelHex),
        (ArtifactFormat::S19, c) => Some(c == Content::SRecord),
        (ArtifactFormat::Bin | ArtifactFormat::Rwd | ArtifactFormat::Unknown, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_states_the_format_by_its_last_extension() {
        assert_eq!(ArtifactFormat::from_filename("37805-5MR-C120.rwd"), ArtifactFormat::Rwd);
        assert_eq!(ArtifactFormat::from_filename("37805-5MR-C120.rwd.gz"), ArtifactFormat::Gz);
        assert_eq!(ArtifactFormat::from_filename("TEST-CAL-001.BIN"), ArtifactFormat::Bin);
        assert_eq!(ArtifactFormat::from_filename("cal.hex"), ArtifactFormat::Hex);
        assert_eq!(ArtifactFormat::from_filename("cal.s19"), ArtifactFormat::S19);
        assert_eq!(ArtifactFormat::from_filename("notes.txt"), ArtifactFormat::Unknown);
        assert_eq!(ArtifactFormat::from_filename("bin"), ArtifactFormat::Unknown);
    }

    #[test]
    fn content_is_told_apart_without_being_interpreted() {
        assert_eq!(sniff(b""), Content::Empty);
        assert_eq!(sniff(&[0x1F, 0x8B, 0x08, 0x00]), Content::Gzip);
        assert_eq!(
            sniff(b":10010000214601360121470136007EFE09D2190140\n:00000001FF\n"),
            Content::IntelHex
        );
        assert_eq!(
            sniff(b"S00F000068656C6C6F202020202000003C\r\nS5030001FB\r\n"),
            Content::SRecord
        );
        assert_eq!(sniff(&[0x00, 0xFF, 0x12, 0x34]), Content::Opaque);
        assert_eq!(sniff(b"just some notes"), Content::Opaque);
    }

    #[test]
    fn a_name_the_content_cannot_be_is_caught_and_raw_bytes_are_not_judged() {
        assert_eq!(content_agrees(ArtifactFormat::Gz, Content::Opaque), Some(false));
        assert_eq!(content_agrees(ArtifactFormat::Gz, Content::Gzip), Some(true));
        assert_eq!(content_agrees(ArtifactFormat::Hex, Content::SRecord), Some(false));
        assert_eq!(content_agrees(ArtifactFormat::Bin, Content::Opaque), None);
        assert_eq!(content_agrees(ArtifactFormat::Rwd, Content::Opaque), None);
        assert_eq!(content_agrees(ArtifactFormat::Bin, Content::Empty), Some(false));
    }
}
