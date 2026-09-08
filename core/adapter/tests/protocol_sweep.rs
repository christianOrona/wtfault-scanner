//! Recovering a vehicle that `ATSP0` cannot find.
//!
//! Observed on a real minivan with a working cheap clone: the adapter
//! initialised cleanly, `ATRV` read 14.0 V so the port was live and the
//! ignition on, and every request still came back `..UNABLE TO CONNECT` — the
//! dots being the ELM327's own protocol search giving up. The same adapter had
//! read a truck minutes earlier.
//!
//! Auto-detection failing is not the same as the vehicle being silent, so the
//! driver now asks for each protocol explicitly before concluding anything.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config};
use aim_transport::{Transport, TransportStats};
use aim_types::{AimResult, ObdProtocol, TransportKind};
use std::time::Duration;

/// An ELM327 that only answers on one specific protocol, and never on `ATSP0`.
///
/// This is the real failure being reproduced: the adapter is healthy and
/// talkative, the bus is live, and only automatic detection is broken.
struct PickyElm {
    /// The `ATSP` digit this pretend vehicle actually speaks.
    speaks: u8,
    /// What `ATSP` last selected. 0 is automatic.
    selected: u8,
    pending: Vec<u8>,
    open: bool,
    stats: TransportStats,
}

impl PickyElm {
    fn new(speaks: u8) -> Self {
        PickyElm {
            speaks,
            selected: 0,
            pending: Vec::new(),
            open: false,
            stats: TransportStats::default(),
        }
    }

    fn reply(&mut self, command: &str) -> String {
        let c = command.trim().to_ascii_uppercase();
        if let Some(rest) = c.strip_prefix("ATSP") {
            self.selected = u8::from_str_radix(rest, 16).unwrap_or(0);
            return "OK".into();
        }
        if c == "ATZ" || c == "ATI" {
            return "ELM327 v2.1".into();
        }
        if c == "ATRV" {
            // Live port, ignition on. The whole point: nothing is unplugged.
            return "14.0V".into();
        }
        if c == "ATDPN" {
            return format!("{:X}", self.selected);
        }
        if c.starts_with("AT") {
            return "OK".into();
        }
        // A real request. Only answered once the right protocol is selected.
        if self.selected == self.speaks {
            if c.starts_with("0100") {
                return "7E8 06 41 00 BE 3F A8 13".into();
            }
            return "NO DATA".into();
        }
        "..UNABLE TO CONNECT".into()
    }
}

impl Transport for PickyElm {
    fn kind(&self) -> TransportKind {
        TransportKind::Simulated
    }
    fn descriptor(&self) -> String {
        "picky-elm".into()
    }
    fn open(&mut self) -> AimResult<()> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) -> AimResult<()> {
        self.open = false;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        let command = String::from_utf8_lossy(data).replace(['\r', '\n'], "");
        if !command.is_empty() {
            let reply = self.reply(&command);
            self.pending.extend_from_slice(format!("{reply}\r\r>").as_bytes());
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], _timeout: Duration) -> AimResult<usize> {
        let n = self.pending.len().min(buf.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
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

fn connect_to(speaks: u8) -> Elm327Adapter {
    let mut adapter =
        Elm327Adapter::new(Box::new(PickyElm::new(speaks)), Elm327Config::fast());
    adapter.connect().expect("connect should not error");
    adapter
}

#[test]
fn a_vehicle_auto_detect_cannot_find_is_still_reached() {
    // ISO 9141-2: the classic case a cheap clone's auto-search fumbles.
    let adapter = connect_to(3);

    assert_eq!(
        adapter.protocol(),
        ObdProtocol::Iso9141_2,
        "the sweep should have found the protocol auto-detect missed"
    );
    assert!(
        adapter.state().is_usable(),
        "the vehicle answered, so the link is usable rather than degraded"
    );
}

#[test]
fn every_protocol_in_the_sweep_is_reachable() {
    // Whatever the vehicle speaks, the sweep has to arrive at it - otherwise
    // the order is quietly wrong for some cars.
    for id in [1u8, 2, 3, 4, 5, 6, 7, 8, 9] {
        let adapter = connect_to(id);
        assert_eq!(
            adapter.protocol(),
            ObdProtocol::from_elm_id(id),
            "a vehicle speaking ATSP{id} was not reached"
        );
    }
}

#[test]
fn finding_a_protocol_the_hard_way_is_recorded_as_a_caveat() {
    // The user is owed the reason their scan took longer than usual, and the
    // fact that their adapter is unreliable at the thing it claims to do.
    let adapter = connect_to(3);
    let caveats = adapter.capabilities().caveats;
    assert!(
        caveats.iter().any(|c| c.contains("ATSP0") && c.contains("auto")),
        "the sweep should say why it was needed. Caveats: {caveats:?}"
    );
}

#[test]
fn a_genuinely_silent_vehicle_still_reports_degraded() {
    // The sweep must not turn "this car is not answering" into a false
    // positive. 0xFF is not a protocol any `ATSP` selects, so nothing this
    // pretend vehicle is asked will ever be answered — the silent-bus case.
    // (Not 0: that is `ATSP0` itself, and the fake would answer on the very
    // first automatic attempt.)
    let adapter = connect_to(0xFF);
    // `is_usable()` is deliberately true for a degraded link — the adapter
    // works even when the vehicle does not — so the state itself is what has
    // to be checked here.
    assert!(
        matches!(adapter.state(), aim_types::ConnectionState::Degraded { .. }),
        "nothing answered, so the link must be degraded, not ready. Got {:?}",
        adapter.state()
    );
    let caveats = adapter.capabilities().caveats;
    assert!(
        caveats.iter().any(|c| c.contains("ATSP1 to ATSP9")),
        "the user should be told every protocol was tried. Caveats: {caveats:?}"
    );
}
