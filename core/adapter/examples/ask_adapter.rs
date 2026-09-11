//! Send a list of commands to a real adapter and print exactly what it says.
//!
//!     cargo run -p aim-adapter --example ask_adapter -- COM4
//!
//! Exists because capability detection has to be written against what a device
//! actually replies, not against what its datasheet implies. The OBDLink MX+
//! reports itself as `ELM327 v1.4b` with vendor `OBD SOLUTIONS LLC`, which is
//! how it slipped past a name-matched STN check; the only way to know what it
//! really supports is to ask it.

use aim_transport::serial::{SerialConfig, SerialTransport};
use aim_transport::Transport;
use std::time::Duration;

/// Send one command and read until the prompt.
///
/// Waiting for the prompt matters more than it looks: an ELM327 that is still
/// working when the next command arrives abandons what it was doing and
/// answers `STOPPED`, so a probe that does not wait measures its own impatience
/// rather than the device.
fn ask(t: &mut SerialTransport, command: &str, wait: Duration) -> String {
    if t.write_all(format!("{command}\r").as_bytes()).is_err() {
        return String::from("<write failed>");
    }
    let mut buf = Vec::new();
    let deadline = std::time::Instant::now() + wait;
    while std::time::Instant::now() < deadline {
        let mut chunk = [0u8; 512];
        match t.read(&mut chunk, Duration::from_millis(200)) {
            Ok(0) => continue,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'>') {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    let text = String::from_utf8_lossy(&buf)
        .replace('\r', " | ")
        .replace('\n', "")
        .trim_matches(|c: char| c == '>' || c.is_whitespace())
        .trim()
        .to_string();
    if !buf.contains(&b'>') {
        return format!("{text}   <<< NO PROMPT: still busy");
    }
    text
}

fn show(t: &mut SerialTransport, command: &str, wait_ms: u64) {
    println!("{command:<40} -> {}", ask(t, command, Duration::from_millis(wait_ms)));
}

fn main() {
    let port = std::env::args().nth(1).unwrap_or_else(|| "COM4".into());
    let mut t = SerialTransport::new(SerialConfig::usb(&port));
    t.open().expect("open the port");

    // Exactly the state the driver is in when it measures long-message
    // support: protocol still on automatic, one successful request behind it,
    // and `ATDPN` not yet asked.
    for setup in ["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATSP0"] {
        show(&mut t, setup, 4000);
    }
    show(&mut t, "0100", 9000);

    println!("\n--- long transmit, with the protocol still unread ---");
    show(&mut t, "010020406080A0C0", 9000);
    // No `h:` parameter: does STPX use whatever header is already configured?
    show(&mut t, "STPX d:0100204060 80A0C0", 9000);
    show(&mut t, "STPX d:0100", 9000);

    let _ = t.close();
}
