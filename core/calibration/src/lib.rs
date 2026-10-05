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
//! result. Files come from a folder on the person's own disk.
//!
//! # The pieces
//!
//! - [`identity`]: what a module said about its software, each identifier with
//!   where it came from and the bytes it was read out of, and each identifier
//!   it was asked for and did not give.
//! - [`artifact`]: a calibration file and what is claimed about it, each claim
//!   with the standing of whoever made it.
//! - [`format`]: what kind of file it is, from its name and from its content.
//! - [`cache`]: files kept by their SHA-256, so the same file from two places
//!   is one file.
//! - [`source`]: where files are looked for.
//! - [`validate`]: whether a file is intact and consistent with itself.
//! - [`resolve`]: whether a file is this module's calibration: exact, partial,
//!   no, or unknown, with every comparison shown.
//!
//! # The rule
//!
//! A file whose name looks right is not the right file. `resolve` says *exact*
//! only when the module's own calibration identification equals one a source
//! has declared for the file, and nothing either side knows disagrees. A name,
//! a model year or a string found inside the file can make a match *partial*.
//! They can never make it exact.

#![warn(missing_docs)]

pub mod artifact;
pub mod cache;
pub mod format;
pub mod identity;
pub mod resolve;
pub mod source;
pub mod validate;

pub use artifact::{ArtifactRecord, Basis, Claim, Claims, SourceRef};
pub use cache::{Cache, Incoming};
pub use format::ArtifactFormat;
pub use identity::{CalibrationIdentity, Field, Identified, IdentitySource, Unanswered};
pub use resolve::{
    evaluate, resolve, Check, Evaluated, MatchReport, MatchStatus, Outcome, Resolution,
    SourceReport, Verdict,
};
pub use source::{CalibrationSource, Candidate, DirectorySource, SourceInfo};
pub use validate::{validate, Validation, ValidationStatus};

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
