//! `aim-api` — the localhost diagnostic API.
//!
//! Binds to 127.0.0.1 only. This process can talk to a vehicle, so it is not
//! something to expose on a network: there is no authentication because there
//! is no remote caller, and that is a deliberate pairing rather than an
//! omission. The Tauri shell and any future mobile client reach it through the
//! same `/api/v1` contract documented in `docs/API.md`.
//!
//! The server is a library so that integration tests can start it in-process on
//! an ephemeral port and drive it over a real socket, which is what
//! `tests/end_to_end.rs` does. The binary in `main.rs` is a thin wrapper around
//! [`router`].

#![warn(missing_docs)]

pub mod agent;
pub mod agent_routes;
pub mod error;
pub mod routes;
pub mod state;
pub mod support;
pub mod update;
pub mod ws;

pub use error::{ApiError, ApiResult};
pub use routes::router;
pub use state::{AppState, ConnectRequest, PersonalityChoice, ServerConfig, TransportChoice};
