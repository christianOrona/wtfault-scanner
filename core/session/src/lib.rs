//! `aim-session` — local diagnostic history and the Flight Recorder.
//!
//! Handoff §12 defines the data model and §9 asks for a signature feature: an
//! immutable-ish chronological trace of every session that can be replayed and
//! exported. Both live here.
//!
//! This crate is **storage only**. It does not know what an adapter is, and
//! nothing in it can talk to a vehicle. `aim-diagnostics` owns the recorder
//! that turns adapter activity into the events written here, which keeps the
//! dependency pointing one way and keeps this crate testable with nothing but
//! a temp file.
//!
//! # The append-only rule
//!
//! `session_events` has database triggers that reject `UPDATE` and `DELETE`.
//! A flight recorder that can be edited after the fact is not evidence, and
//! enforcing that in SQL rather than in Rust means it holds even for someone
//! poking at the file with the `sqlite3` CLI.

#![warn(missing_docs)]

pub mod schema;
pub mod store;

pub use schema::{latest_version, Migration, MIGRATIONS};
pub use store::{measurement_from, SessionStore, SessionSummary};
