//! In-process transport backed by the emulated adapter.
//!
//! Bytes written here are handed to [`ElmEmulator`] and its reply is queued for
//! reading, so the whole stack above — adapter, protocol, decoding, session
//! recording — runs exactly as it does against hardware. Nothing above this
//! type knows it is not talking to a truck.

use crate::elm::{AdapterPersonality, ElmEmulator};
use crate::scenario::ScenarioId;
use crate::vehicle::VirtualVehicle;
use aim_transport::{Transport, TransportStats};
use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A shared handle to the emulator, so a test or the API can inspect the
/// virtual vehicle while the adapter owns the transport.
pub type SharedEmulator = Arc<Mutex<ElmEmulator>>;

/// A [`Transport`] whose other end is the simulated vehicle.
pub struct SimulatedTransport {
    emulator: SharedEmulator,
    descriptor: String,
    open: bool,
    pending: VecDeque<u8>,
    line: Vec<u8>,
    stats: TransportStats,
    latency: Duration,
}

impl SimulatedTransport {
    /// A transport for `scenario` with a cheap-clone adapter personality —
    /// the default, because the cheap clone is what is actually plugged into
    /// the development truck and it is the harsher test.
    pub fn new(scenario: ScenarioId) -> Self {
        SimulatedTransport::with_personality(scenario, AdapterPersonality::cheap_clone_v2_1())
    }

    /// A transport for `scenario` with a specific adapter personality.
    pub fn with_personality(scenario: ScenarioId, personality: AdapterPersonality) -> Self {
        let emulator = ElmEmulator::new(VirtualVehicle::f250_2019(scenario), personality);
        SimulatedTransport::from_emulator(
            Arc::new(Mutex::new(emulator)),
            format!("sim:{}", scenario.as_str()),
        )
    }

    /// Wrap an emulator someone else configured.
    pub fn from_emulator(emulator: SharedEmulator, descriptor: impl Into<String>) -> Self {
        SimulatedTransport {
            emulator,
            descriptor: descriptor.into(),
            open: false,
            pending: VecDeque::new(),
            line: Vec::new(),
            stats: TransportStats::default(),
            latency: Duration::ZERO,
        }
    }

    /// Add a per-command delay, so live-data streaming looks believable
    /// instead of running as fast as the CPU allows.
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// The emulator handle, for inspecting the virtual vehicle.
    pub fn emulator(&self) -> SharedEmulator {
        Arc::clone(&self.emulator)
    }
}

impl Transport for SimulatedTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Simulated
    }

    fn descriptor(&self) -> String {
        self.descriptor.clone()
    }

    fn open(&mut self) -> AimResult<()> {
        if !self.open {
            self.open = true;
            self.stats.opens += 1;
            self.pending.clear();
            self.line.clear();
        }
        Ok(())
    }

    fn close(&mut self) -> AimResult<()> {
        self.open = false;
        self.pending.clear();
        self.line.clear();
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        if !self.open {
            self.stats.io_errors += 1;
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "simulated transport is closed",
            ));
        }
        self.stats.bytes_written += data.len() as u64;
        for b in data {
            match b {
                b'\r' | b'\n' => {
                    let command = String::from_utf8_lossy(&self.line).to_string();
                    self.line.clear();
                    if !self.latency.is_zero() {
                        std::thread::sleep(self.latency);
                    }
                    let reply = {
                        let mut e = self.emulator.lock().map_err(|_| {
                            AimError::new(
                                ErrorCode::Internal,
                                "simulator emulator lock was poisoned",
                            )
                        })?;
                        e.handle_line(&command)
                    };
                    self.pending.extend(reply.as_bytes());
                }
                other => self.line.push(*other),
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        if !self.open {
            self.stats.io_errors += 1;
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "simulated transport is closed",
            ));
        }
        if self.pending.is_empty() {
            // Nothing queued means the emulated device chose to stay silent.
            // Burn the caller's slice so a timeout takes real time and the
            // adapter's deadline logic is genuinely exercised.
            std::thread::sleep(timeout.min(Duration::from_millis(50)));
            self.stats.read_timeouts += 1;
            return Ok(0);
        }
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().unwrap_or(0);
        }
        self.stats.bytes_read += n as u64;
        Ok(n)
    }

    fn flush_input(&mut self) -> AimResult<()> {
        self.pending.clear();
        Ok(())
    }

    fn stats(&self) -> TransportStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_reply(t: &mut SimulatedTransport) -> String {
        let (bytes, terminated) =
            aim_transport::read_until(t, b'>', Duration::from_millis(200)).unwrap();
        assert!(terminated, "reply was not terminated by a prompt");
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[test]
    fn a_command_written_produces_a_reply_to_read() {
        let mut t = SimulatedTransport::new(ScenarioId::Healthy);
        t.open().unwrap();
        t.write_all(b"ATZ\r").unwrap();
        let reply = read_reply(&mut t);
        assert!(reply.contains("ELM327"));
        assert!(reply.ends_with('>'));
    }

    #[test]
    fn writing_to_a_closed_transport_is_an_error() {
        let mut t = SimulatedTransport::new(ScenarioId::Healthy);
        let e = t.write_all(b"ATZ\r").unwrap_err();
        assert_eq!(e.code, ErrorCode::TransportDisconnected);
    }

    #[test]
    fn a_silent_device_produces_a_read_timeout_rather_than_a_hang() {
        let mut t = SimulatedTransport::new(ScenarioId::Healthy);
        t.open().unwrap();
        t.emulator()
            .lock()
            .unwrap()
            .inject(crate::elm::InjectedFault::Silence);
        t.write_all(b"ATZ\r").unwrap();
        let (bytes, terminated) =
            aim_transport::read_until(&mut t, b'>', Duration::from_millis(120)).unwrap();
        assert!(!terminated);
        assert!(bytes.is_empty());
    }

    #[test]
    fn flush_discards_a_reply_nobody_read() {
        let mut t = SimulatedTransport::new(ScenarioId::Healthy);
        t.open().unwrap();
        t.write_all(b"ATZ\r").unwrap();
        t.flush_input().unwrap();
        let mut buf = [0u8; 32];
        assert_eq!(t.read(&mut buf, Duration::from_millis(10)).unwrap(), 0);
    }

    #[test]
    fn stats_count_traffic_and_opens() {
        let mut t = SimulatedTransport::new(ScenarioId::Healthy);
        t.open().unwrap();
        t.write_all(b"ATZ\r").unwrap();
        read_reply(&mut t);
        let s = t.stats();
        assert_eq!(s.opens, 1);
        assert_eq!(s.bytes_written, 4);
        assert!(s.bytes_read > 0);
    }

    #[test]
    fn the_descriptor_names_the_scenario_so_sessions_are_labelled_honestly() {
        let t = SimulatedTransport::new(ScenarioId::DpfRegen);
        assert_eq!(t.descriptor(), "sim:dpf-regen");
        assert_eq!(t.kind(), TransportKind::Simulated);
        assert!(!t.kind().is_physical());
    }
}
