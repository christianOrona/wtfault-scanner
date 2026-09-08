//! `aim-transport` — bytes in, bytes out.
//!
//! Handoff §5 puts transport at the bottom of the stack with exactly one job:
//! *"Connect/disconnect, bytes in/out, timeouts, reconnection, adapter health."*
//! It knows nothing about ELM327 commands, CAN, or PIDs.
//!
//! # Implementations
//!
//! | Type | Where it runs |
//! |------|---------------|
//! | [`SerialTransport`] | Real hardware. On Windows a paired Bluetooth-SPP ELM327 is an outgoing COM port, so Bluetooth *is* serial here. Linux `/dev/rfcomm0`, macOS `/dev/tty.*`. |
//! | [`LoopbackTransport`] | Tests: a scripted request→response table. |
//! | `SimulatedTransport` (in `aim-simulator`) | The virtual vehicle. |
//!
//! The trait is deliberately **blocking**. Serial ports are blocking, ELM327
//! exchanges are strictly request/response, and the whole diagnostic core runs
//! on one dedicated thread; async here would buy nothing and cost clarity.

#![warn(missing_docs)]

pub mod loopback;
pub mod ports;
#[cfg(feature = "serial")]
pub mod serial;

pub use loopback::LoopbackTransport;
pub use ports::{list_ports, PortInfo, PortKind};
#[cfg(feature = "serial")]
pub use serial::{SerialConfig, SerialTransport};

use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use std::time::{Duration, Instant};

/// Byte-level link to an adapter.
///
/// Implementations must be `Send` because the diagnostic core owns its
/// transport on a dedicated worker thread.
pub trait Transport: Send {
    /// What kind of physical link this is.
    fn kind(&self) -> TransportKind;

    /// Stable human-readable identity: `COM5`, `/dev/rfcomm0`, `sim:dpf_regen`.
    fn descriptor(&self) -> String;

    /// Open the link. Idempotent: opening an open transport is a no-op.
    fn open(&mut self) -> AimResult<()>;

    /// Close the link. Idempotent.
    fn close(&mut self) -> AimResult<()>;

    /// Whether the link is currently open.
    fn is_open(&self) -> bool;

    /// Write every byte, or fail.
    fn write_all(&mut self, data: &[u8]) -> AimResult<()>;

    /// Read whatever is available, blocking at most `timeout`.
    ///
    /// Returns `Ok(0)` when the timeout elapsed with nothing available — a
    /// silent adapter is not an error at this layer, it is an observation the
    /// caller turns into [`ErrorCode::TransportTimeout`] if it cares.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize>;

    /// Discard any buffered input. Called before each command so a late
    /// response to a previous command cannot be mistaken for this one's.
    fn flush_input(&mut self) -> AimResult<()>;

    /// Counters for adapter-health reporting.
    fn stats(&self) -> TransportStats;
}

/// Byte and error counters maintained by every transport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransportStats {
    /// Bytes written since open.
    pub bytes_written: u64,
    /// Bytes read since open.
    pub bytes_read: u64,
    /// Reads that returned nothing within their deadline.
    pub read_timeouts: u64,
    /// I/O errors observed.
    pub io_errors: u64,
    /// Number of times the link has been opened.
    pub opens: u64,
}

/// Read from `transport` until `terminator` is seen or `timeout` elapses.
///
/// This is the primitive every text-protocol adapter needs (the ELM327
/// terminates every reply with the `>` prompt). Bytes read before the deadline
/// are always returned, even on timeout, so a partial reply can be inspected
/// and reported rather than silently dropped.
///
/// Returns `(bytes, terminated)` where `terminated` is false on timeout.
pub fn read_until(
    transport: &mut dyn Transport,
    terminator: u8,
    timeout: Duration,
) -> AimResult<(Vec<u8>, bool)> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::with_capacity(128);
    let mut chunk = [0u8; 256];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok((out, false));
        }
        // Poll in slices so a long overall deadline still notices a closed port.
        let slice = remaining.min(Duration::from_millis(50));
        let n = transport.read(&mut chunk, slice)?;
        if n > 0 {
            out.extend_from_slice(&chunk[..n]);
            if chunk[..n].contains(&terminator) {
                return Ok((out, true));
            }
        }
    }
}

/// Convenience: a transport-level timeout error naming the operation.
pub fn timeout_error(op: &str, waited: Duration) -> AimError {
    AimError::new(
        ErrorCode::TransportTimeout,
        format!("{op} timed out after {} ms", waited.as_millis()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_until_returns_partial_data_on_timeout() {
        let mut t = LoopbackTransport::new(vec![("PING", vec!["PONG"])]);
        t.open().unwrap();
        t.write_all(b"PING\r").unwrap();
        // '#' never appears, so this must time out but still hand back bytes.
        let (bytes, terminated) = read_until(&mut t, b'#', Duration::from_millis(60)).unwrap();
        assert!(!terminated);
        assert!(String::from_utf8_lossy(&bytes).contains("PONG"));
    }

    #[test]
    fn read_until_stops_at_the_terminator() {
        let mut t = LoopbackTransport::new(vec![("PING", vec!["PONG"])]);
        t.open().unwrap();
        t.write_all(b"PING\r").unwrap();
        let (bytes, terminated) = read_until(&mut t, b'>', Duration::from_millis(200)).unwrap();
        assert!(terminated);
        assert!(bytes.ends_with(b">"));
    }
}
