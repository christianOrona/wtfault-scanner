//! What kind of file an artifact is.
//!
//! Three separate questions, kept separate: what its name says, how it is
//! packed, and what its content looks like once unpacked. They are compared,
//! and a file whose content cannot be what its name says is not valid.
//!
//! A name can state two layers. `37805-5MR-C120.rwd.gz` says "gzip" and,
//! inside it, "RWD": the packing and the thing packed are different facts and
//! are recorded as two.
//!
//! These formats do not mean the same thing. An Intel HEX or S-record file is
//! addressed memory as text. A `.bin` is bytes with no description at all. A
//! Honda `.rwd` is that manufacturer's update package: its header is read as
//! far as the format has been described in public (see [`crate::rwd`]), and
//! the software it carries is not opened at all.

use crate::rwd::{self, RwdHeader};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// The most a packed file may unpack to. A module's whole flash is a few
/// megabytes; a file that unpacks past this is not opened any further.
pub const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;

/// How an artifact's bytes are packed, apart from what they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    /// gzip.
    Gzip,
}

/// What an artifact is once any packing is taken off, as its name states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    /// A Honda update package.
    Rwd,
    /// Raw bytes with no container.
    Bin,
    /// Intel HEX.
    Hex,
    /// Motorola S-record.
    S19,
    /// Not named, or not a kind this build knows. A record written before
    /// packing was told apart from content said `gz` here.
    #[serde(alias = "gz")]
    Unknown,
}

impl ArtifactFormat {
    /// The extensions this build reads a name by, packing included.
    pub const EXTENSIONS: [&'static str; 5] = ["rwd", "bin", "gz", "hex", "s19"];

    /// The format a name states by its last extension. For a name that may
    /// also state packing, use [`Named::from_filename`].
    pub fn from_filename(name: &str) -> ArtifactFormat {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !name.contains('.') {
            return ArtifactFormat::Unknown;
        }
        match ext.as_str() {
            "rwd" => ArtifactFormat::Rwd,
            "bin" => ArtifactFormat::Bin,
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

/// What a file's name says it is, layer by layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    /// The packing the name states, when it states one.
    pub compression: Option<Compression>,
    /// What the name says is inside the packing, or the file itself when it
    /// is not packed.
    pub payload: ArtifactFormat,
}

impl Named {
    /// Read both layers off a name: `x.rwd.gz` is gzip holding RWD, `x.gz` is
    /// gzip holding something the name does not say, `x.rwd` is RWD.
    pub fn from_filename(name: &str) -> Named {
        let lower = name.to_ascii_lowercase();
        match lower.strip_suffix(".gz") {
            Some(inner) if !inner.is_empty() => Named {
                compression: Some(Compression::Gzip),
                payload: ArtifactFormat::from_filename(inner),
            },
            _ => Named { compression: None, payload: ArtifactFormat::from_filename(&lower) },
        }
    }

    /// Whether this build treats a file so named as a calibration artifact.
    /// A gzip whose name does not say what it holds is one: what it holds is
    /// looked at, not assumed.
    pub fn is_supported(self) -> bool {
        self.compression.is_some() || self.payload.is_supported()
    }

    /// The extension a file so named is kept under: `rwd.gz`, `gz`, `bin`.
    pub fn extension(self) -> String {
        match (self.compression, self.payload) {
            (Some(Compression::Gzip), ArtifactFormat::Unknown) => String::from("gz"),
            (Some(Compression::Gzip), payload) => format!("{}.gz", payload.extension()),
            (None, payload) => payload.extension().to_string(),
        }
    }

    /// The layers in words, outermost first.
    pub fn describe(self) -> String {
        let payload = match self.payload {
            ArtifactFormat::Rwd => "a Honda RWD package",
            ArtifactFormat::Bin => "raw bytes",
            ArtifactFormat::Hex => "Intel HEX",
            ArtifactFormat::S19 => "Motorola S-records",
            ArtifactFormat::Unknown => "something its name does not say",
        };
        match self.compression {
            Some(Compression::Gzip) => format!("gzip holding {payload}"),
            None => payload.to_string(),
        }
    }
}

/// What content looks like, as far as can be told without interpreting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    /// No bytes at all.
    Empty,
    /// Starts with the gzip signature, `1F 8B`.
    Gzip,
    /// Starts the way an RWD package does: one indicator byte, then `0D 0A`.
    Rwd,
    /// Text in which every line is an Intel HEX record.
    IntelHex,
    /// Text in which every line is a Motorola S-record.
    SRecord,
    /// Anything else.
    Opaque,
}

/// Look at content. Only the start is read for the text formats: this says
/// what the bytes look like, not that every record in them is sound.
pub fn sniff(bytes: &[u8]) -> Content {
    if bytes.is_empty() {
        return Content::Empty;
    }
    if bytes.starts_with(&[0x1F, 0x8B]) {
        return Content::Gzip;
    }
    if rwd::has_signature(bytes) {
        return Content::Rwd;
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

/// Whether unpacked content can be what the name says is inside. `None` when
/// the format has nothing its content can be checked against, which is true
/// of raw bytes and of a packing whose name does not say what it holds.
pub fn content_agrees(payload: ArtifactFormat, content: Content) -> Option<bool> {
    match (payload, content) {
        (_, Content::Empty) => Some(false),
        (ArtifactFormat::Rwd, c) => Some(c == Content::Rwd),
        (ArtifactFormat::Hex, c) => Some(c == Content::IntelHex),
        (ArtifactFormat::S19, c) => Some(c == Content::SRecord),
        (ArtifactFormat::Bin | ArtifactFormat::Unknown, _) => None,
    }
}

/// Unpack gzip, and stop rather than fill memory.
///
/// A file that is cut short, fails its own checksum, or unpacks past
/// [`MAX_UNPACKED_BYTES`] is an error said in words. Nothing is written
/// anywhere: the unpacked bytes exist only to be looked at.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut unpacked = Vec::new();
    let mut limited = flate2::read::GzDecoder::new(bytes).take(MAX_UNPACKED_BYTES + 1);
    limited.read_to_end(&mut unpacked).map_err(|e| format!("it does not unpack as gzip: {e}"))?;
    if unpacked.len() as u64 > MAX_UNPACKED_BYTES {
        return Err(format!(
            "it unpacks to more than {MAX_UNPACKED_BYTES} bytes, which no calibration this build \
             expects does, so it was not unpacked further"
        ));
    }
    Ok(unpacked)
}

/// How much of the software an artifact carries is understood.
///
/// There is no value here for "understood". Nothing in this crate decodes,
/// decrypts or interprets the software inside a calibration file, and the
/// type says so rather than leaving it to a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Understanding {
    /// The software inside was reached and is not interpreted: it is bytes.
    #[serde(rename = "PAYLOAD_OPAQUE")]
    Opaque,
    /// The software inside was not reached: the packing did not open.
    #[serde(rename = "PAYLOAD_NOT_REACHED")]
    NotReached,
}

/// What looking inside an artifact found.
///
/// Everything here is read out of the file's own bytes and nothing is
/// concluded beyond it: how it is packed, what the thing inside looks like,
/// and, for an RWD package, the values in its header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspection {
    /// How the file is packed, from its content and not its name.
    pub compression: Option<Compression>,
    /// Why the packing did not open, when it did not.
    pub unpack_problem: Option<String>,
    /// SHA-256 of what is inside the packing. `None` for a file that is not
    /// packed, where the artifact's own hash already is that, and for one
    /// that did not unpack.
    pub payload_sha256: Option<String>,
    /// The size of what is inside the packing, under the same conditions.
    pub payload_size: Option<u64>,
    /// What the innermost layer reached looks like.
    pub content: Content,
    /// The RWD header, when the innermost layer is an RWD package.
    pub rwd: Option<RwdHeader>,
    /// How much of the software inside is understood.
    pub software: Understanding,
}

impl Inspection {
    /// Every piece of text found in the file's own header, when a header was
    /// read to its end. A header that stopped part-way gives nothing: a
    /// reading that went wrong is not half-trusted.
    pub fn header_texts(&self) -> Vec<&str> {
        match &self.rwd {
            Some(header) if header.headers_read => header.texts(),
            _ => Vec::new(),
        }
    }
}

/// Look inside an artifact: take off one layer of gzip when there is one, and
/// read an RWD header when that is what is underneath.
pub fn inspect(bytes: &[u8]) -> Inspection {
    let outer = sniff(bytes);
    if outer != Content::Gzip {
        return Inspection {
            compression: None,
            unpack_problem: None,
            payload_sha256: None,
            payload_size: None,
            content: outer,
            rwd: rwd::read(bytes),
            software: Understanding::Opaque,
        };
    }
    match gunzip(bytes) {
        Ok(payload) => {
            let content = sniff(&payload);
            // One layer is taken off. A gzip inside a gzip is reported as
            // what it is and not chased.
            let nested = content == Content::Gzip;
            Inspection {
                compression: Some(Compression::Gzip),
                unpack_problem: nested.then(|| {
                    String::from("what is inside is gzip again; only one layer is opened")
                }),
                payload_sha256: Some(crate::sha256_hex(&payload)),
                payload_size: Some(payload.len() as u64),
                content,
                rwd: rwd::read(&payload),
                software: if nested { Understanding::NotReached } else { Understanding::Opaque },
            }
        }
        Err(problem) => Inspection {
            compression: Some(Compression::Gzip),
            unpack_problem: Some(problem),
            payload_sha256: None,
            payload_size: None,
            content: Content::Gzip,
            rwd: None,
            software: Understanding::NotReached,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// gzip `bytes`, as a test's stand-in for a packed file.
    pub(crate) fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn a_name_states_packing_and_content_as_two_layers() {
        let packed = Named::from_filename("37805-5MR-C120.rwd.gz");
        assert_eq!(packed.compression, Some(Compression::Gzip));
        assert_eq!(packed.payload, ArtifactFormat::Rwd);
        assert_eq!(packed.extension(), "rwd.gz");
        assert_eq!(packed.describe(), "gzip holding a Honda RWD package");

        let plain = Named::from_filename("37805-5MR-C120.RWD");
        assert_eq!((plain.compression, plain.payload), (None, ArtifactFormat::Rwd));

        // A gzip that does not say what it holds is still looked at.
        let unnamed = Named::from_filename("download.gz");
        assert_eq!(unnamed.compression, Some(Compression::Gzip));
        assert_eq!(unnamed.payload, ArtifactFormat::Unknown);
        assert!(unnamed.is_supported());
        assert_eq!(unnamed.extension(), "gz");

        assert_eq!(Named::from_filename("TEST-CAL-001.BIN").payload, ArtifactFormat::Bin);
        assert_eq!(Named::from_filename("cal.hex").payload, ArtifactFormat::Hex);
        assert_eq!(Named::from_filename("cal.s19").payload, ArtifactFormat::S19);
        assert!(!Named::from_filename("notes.txt").is_supported());
        assert!(!Named::from_filename("bin").is_supported());
        assert!(!Named::from_filename(".gz").is_supported());
    }

    #[test]
    fn a_record_written_before_layers_still_reads() {
        let old: ArtifactFormat = serde_json::from_str(r#""gz""#).unwrap();
        assert_eq!(old, ArtifactFormat::Unknown);
    }

    #[test]
    fn content_is_told_apart_without_being_interpreted() {
        assert_eq!(sniff(b""), Content::Empty);
        assert_eq!(sniff(&[0x1F, 0x8B, 0x08, 0x00]), Content::Gzip);
        assert_eq!(sniff(b"Z\r\n\x00\x00"), Content::Rwd);
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
        assert_eq!(content_agrees(ArtifactFormat::Rwd, Content::Opaque), Some(false));
        assert_eq!(content_agrees(ArtifactFormat::Rwd, Content::Rwd), Some(true));
        assert_eq!(content_agrees(ArtifactFormat::Hex, Content::SRecord), Some(false));
        assert_eq!(content_agrees(ArtifactFormat::Bin, Content::Opaque), None);
        assert_eq!(content_agrees(ArtifactFormat::Unknown, Content::Rwd), None);
        assert_eq!(content_agrees(ArtifactFormat::Bin, Content::Empty), Some(false));
    }

    #[test]
    fn a_packed_file_is_unpacked_to_be_looked_at_and_both_hashes_are_kept_apart() {
        let payload = b"Z\r\n\x00\x00opaque";
        let packed = gz(payload);
        let seen = inspect(&packed);

        assert_eq!(seen.compression, Some(Compression::Gzip));
        assert_eq!(seen.content, Content::Rwd);
        assert_eq!(seen.payload_sha256.as_deref(), Some(crate::sha256_hex(payload).as_str()));
        assert_ne!(seen.payload_sha256.as_deref(), Some(crate::sha256_hex(&packed).as_str()));
        assert_eq!(seen.payload_size, Some(payload.len() as u64));
        assert!(seen.rwd.is_some());
        assert_eq!(seen.software, Understanding::Opaque);
        assert_eq!(
            serde_json::to_value(seen.software).unwrap(),
            serde_json::json!("PAYLOAD_OPAQUE")
        );

        // The same thing not packed: nothing to unpack, so no second hash.
        let bare = inspect(payload);
        assert_eq!(bare.compression, None);
        assert_eq!(bare.payload_sha256, None);
        assert_eq!(bare.content, Content::Rwd);
    }

    #[test]
    fn a_gzip_that_does_not_open_is_said_to_be_one_and_nothing_inside_is_claimed() {
        let mut cut = gz(b"Z\r\n\x00\x00 some payload that is long enough to be cut short");
        cut.truncate(cut.len() - 12);
        let seen = inspect(&cut);
        assert_eq!(seen.compression, Some(Compression::Gzip));
        assert!(seen.unpack_problem.as_deref().is_some_and(|p| p.contains("does not unpack")));
        assert_eq!(seen.software, Understanding::NotReached);
        assert!(seen.rwd.is_none() && seen.payload_sha256.is_none());

        // Not gzip at all behind the signature.
        let fake = inspect(&[0x1F, 0x8B, 0x00, 0x01, 0x02, 0x03]);
        assert!(fake.unpack_problem.is_some());

        // gzip inside gzip: one layer, and it says so.
        let twice = inspect(&gz(&gz(b"Z\r\n\x00\x00")));
        assert_eq!(twice.content, Content::Gzip);
        assert_eq!(twice.software, Understanding::NotReached);
        assert!(twice.rwd.is_none());
    }

    #[test]
    fn unpacking_stops_at_the_limit() {
        // Zeros pack to almost nothing: a few kilobytes that unpack past the
        // limit must be refused, not held in memory.
        let big = vec![0u8; (MAX_UNPACKED_BYTES + 1024) as usize];
        let packed = gz(&big);
        drop(big);
        assert!(packed.len() < 1024 * 1024);
        let problem = gunzip(&packed).unwrap_err();
        assert!(problem.contains("more than"), "{problem}");
    }
}
