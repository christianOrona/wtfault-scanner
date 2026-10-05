//! `aim-calibration` — which software a module is running, and whether a
//! file is that software.
//!
//! A control module says what it runs: a calibration identification, a
//! checksum over it, sometimes part numbers. A calibration file is a
//! manufacturer's release of that software. This crate answers one question
//! and refuses to round the answer up: **is this file the calibration this
//! module reports, and on what evidence?**
//!
//! # What it is not
//!
//! Nothing here can talk to a vehicle, and nothing here writes one. It does not
//! flash, program, patch, convert or tune, it holds no key algorithm, and it
//! never fetches anything from a network. The reading is done by
//! `aim-diagnostics` with requests that only read; this crate is given the
//! result. Files come from folders on the person's own disk.
//!
//! It also cannot say who made a file. Which calibration a file is described
//! as, and whether the manufacturer made it, are two questions, and this
//! crate answers only the first. See [`resolve::ManufacturerOrigin`].
//!
//! # The pieces
//!
//! - [`identity`]: what a module said about its software, each identifier with
//!   where it came from and the bytes it was read out of, and each identifier
//!   it was asked for and did not give, with which kind of "did not".
//! - [`artifact`]: a calibration file and what is claimed about it, each claim
//!   with who made it and on what basis.
//! - [`format`]: what kind of file it is, layer by layer: its packing, and
//!   what is inside the packing.
//! - [`rwd`]: the header of a Honda update package, read as far as the format
//!   has been described in public. The software in the package is not read.
//! - [`cache`]: files kept by their SHA-256, so the same file from two places
//!   is one file.
//! - [`source`]: where files are looked for.
//! - [`validate`]: whether a file is intact and consistent with itself.
//! - [`resolve`]: whether a file is this module's calibration: exact, partial,
//!   conflicting, no, or unknown, with every comparison shown.
//! - [`library`]: the files on this computer and the one way they are
//!   searched, which every caller goes through.
//!
//! # The rule
//!
//! A file whose name looks right is not the right file. `resolve` says *exact*
//! only when the module's own calibration identification equals one declared
//! for the file, and nothing either side knows disagrees. A name, a model year
//! or an identifier found in the file's header can make a match *partial*.
//! They can never make it exact. And an exact match is a statement about what
//! the file is described as: every result says whose description it rests on.

#![warn(missing_docs)]

pub mod artifact;
pub mod cache;
pub mod format;
pub mod identity;
pub mod library;
pub mod resolve;
pub mod rwd;
pub mod source;
pub mod validate;

pub use artifact::{ArtifactRecord, Basis, Claim, Claims, SourceKind, SourceRef};
pub use cache::{Cache, Incoming};
pub use format::{inspect, ArtifactFormat, Compression, Content, Inspection, Named, Understanding};
pub use identity::{
    Availability, CalibrationIdentity, Field, Identified, IdentitySource, NotGiven, Unanswered,
};
pub use library::{AddError, Added, Found, Library, Status};
pub use resolve::{
    evaluate, resolve, Check, Evaluated, ManufacturerOrigin, MatchReport, MatchStatus, Origin,
    Outcome, Resolution, SourceReport, SourceStatus, Verdict,
};
pub use source::{CalibrationSource, Candidate, DirectorySource, Offered, SourceError, SourceInfo};
pub use validate::{validate, validate_inspected, Validation, ValidationStatus};

/// SHA-256 of `bytes`, lowercase hex. The identity of an artifact.
pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// True for 64 hexadecimal digits.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
