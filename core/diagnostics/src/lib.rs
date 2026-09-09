//! `aim-diagnostics` — the deterministic diagnostic core.
//!
//! This crate is where the layers meet. It owns an [`aim_adapter`] adapter, an
//! [`aim_decoders`] decoder set, an [`aim_safety`] gate and an [`aim_session`]
//! store, and turns them into the operations the product actually performs:
//! identify the vehicle, scan modules, read codes, read live data.
//!
//! Nothing above it — not the API, not the future agent runtime — is allowed
//! its own path to the vehicle. That single choke point is what makes the
//! safety gate meaningful and the flight recorder complete.
//!
//! ```text
//!   API / future agent runtime
//!             │  typed calls, ToolResult back
//!             ▼
//!    DiagnosticService  ──record──▶  SessionStore (flight recorder)
//!      │        │
//!      │        └──authorize──▶ SafetyGate
//!      ▼
//!   DiagnosticAdapter ──▶ Transport ──▶ vehicle or simulator
//! ```

#![warn(missing_docs)]

pub mod capture;
pub mod config;
pub mod recorder;
pub mod service;

pub use config::{plan_change, ChangeContext, ChangePlan, ChangeRequest, Check, DesiredValue};
pub use recorder::SessionRecorder;
pub use service::{capabilities, DiagnosticService, DtcReport};
