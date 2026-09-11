//! What the driver does when an ELM327 answers `STOPPED`.
//!
//! # The measurement behind this
//!
//! A real session on 2026-09-09 (`ses_53b92dab`) recorded **611** `STOPPED`
//! responses. Every one of them answered an `ATSH<address>` during a
//! full-vehicle address sweep, and every one came back inside 50 ms — which is
//! what "systematic rather than incidental" looks like in the data.
//!
//! On an ELM327, `STOPPED` means a character arrived while the device was
//! still busy: it abandoned what it was doing, discarded the command, and
//! printed a prompt. The cause was on our side — the sweep sent the next
//! header before the previous probe's listening window had closed.
//!
//! # Why it is a plain re-send and not a warm start
//!
//! The device is already at the prompt by the time it has said `STOPPED`.
//! Resetting it first clears echo, headers and spacing, all of which then have
//! to be put back, so one interrupted command becomes six round trips. At 611
//! occurrences over Bluetooth that is minutes of somebody's evening spent
//! resetting a device that was never broken.
//!
//! `BUFFER FULL` is the one that genuinely needs the reset, and it keeps it.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config, RequestTarget};
use aim_simulator::{InjectedFault, ScenarioId, SimulatedTransport};
use std::time::Duration;

fn connected() -> (Elm327Adapter, aim_simulator::SharedEmulator) {
    let transport = SimulatedTransport::new(ScenarioId::Healthy);
    let emulator = transport.emulator();
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());
    adapter.connect().expect("connect");
    (adapter, emulator)
}

/// Commands the emulator was asked for, since a point in the log.
fn sent_since(emulator: &aim_simulator::SharedEmulator, from: usize) -> Vec<String> {
    emulator.lock().unwrap().log[from..].to_vec()
}

fn log_len(emulator: &aim_simulator::SharedEmulator) -> usize {
    emulator.lock().unwrap().log.len()
}

/// The interrupted command is simply sent again, and the answer arrives.
#[test]
fn an_interrupted_command_is_re_sent_rather_than_lost() {
    let (mut adapter, emulator) = connected();
    let mark = log_len(&emulator);

    emulator.lock().unwrap().inject(InjectedFault::Stopped);
    let replies = adapter
        .request_pdu(&[0x01, 0x0C], &RequestTarget::Functional, Duration::from_millis(1500))
        .expect("the retry should produce the answer the first attempt never got");
    assert!(!replies.is_empty(), "a re-sent request must come back with data");

    let sent = sent_since(&emulator, mark);
    let asks: Vec<&String> = sent.iter().filter(|c| c.starts_with("010C")).collect();
    assert_eq!(asks.len(), 2, "asked once, interrupted, asked again: {sent:?}");
}

/// And nothing is reset to achieve it. This is the whole point of the change:
/// the device said `STOPPED` *and a prompt*, so it is ready, and putting it
/// through a warm start plus four restoration commands would be six round
/// trips to fix a device that was never broken.
#[test]
fn nothing_is_warm_started_to_recover_from_it() {
    let (mut adapter, emulator) = connected();
    let mark = log_len(&emulator);

    emulator.lock().unwrap().inject(InjectedFault::Stopped);
    let _ =
        adapter.request_pdu(&[0x01, 0x0C], &RequestTarget::Functional, Duration::from_millis(1500));

    let sent = sent_since(&emulator, mark);
    assert!(!sent.iter().any(|c| c == "ATWS"), "no warm start: {sent:?}");
    assert!(!sent.iter().any(|c| c == "ATZ"), "and certainly no full reset: {sent:?}");
}

/// `BUFFER FULL` is a different fault and keeps the reset. The device is
/// holding a partial reply it will otherwise keep trying to deliver, and
/// clearing that is what a warm start is for.
#[test]
fn a_buffer_overflow_still_gets_the_reset_it_needs() {
    let (mut adapter, emulator) = connected();
    let mark = log_len(&emulator);

    emulator.lock().unwrap().inject(InjectedFault::BufferFull);
    let _ =
        adapter.request_pdu(&[0x01, 0x0C], &RequestTarget::Functional, Duration::from_millis(1500));

    let sent = sent_since(&emulator, mark);
    assert!(sent.iter().any(|c| c == "ATWS"), "a buffer overflow is cleared by a reset: {sent:?}");
}

/// The case from the session data: an interruption landing on the header
/// command in the middle of an address sweep. The address must still get
/// probed — silently skipping it would report a module as absent when nobody
/// ever asked it anything, which is the failure this whole application exists
/// to avoid.
#[test]
fn an_interruption_on_a_header_does_not_silently_skip_the_address() {
    let (mut adapter, emulator) = connected();
    let mark = log_len(&emulator);

    emulator.lock().unwrap().inject(InjectedFault::Stopped);
    let _ = adapter.request_pdu(
        &[0x3E, 0x00],
        &RequestTarget::Physical(String::from("7E0")),
        Duration::from_millis(1500),
    );

    let sent = sent_since(&emulator, mark);
    assert!(
        sent.iter().any(|c| c == "ATSH7E0"),
        "the header the interruption landed on must be set: {sent:?}"
    );
    assert!(
        sent.iter().any(|c| c.starts_with("3E00")),
        "and the request behind it must actually go out: {sent:?}"
    );
}
