//! Put an adapter back on the main bus after probing left it elsewhere.
//!
//!     cargo run -p aim-adapter --example restore_bus -- COM4
use aim_transport::serial::{SerialConfig, SerialTransport};
use aim_transport::Transport;
use std::time::Duration;

fn main() {
    let port = std::env::args().nth(1).unwrap_or_else(|| "COM4".into());
    let mut t = SerialTransport::new(SerialConfig::usb(&port));
    t.open().expect("open the port");
    for c in ["ATZ", "ATE0", "ATSP0", "ATDPN"] {
        let _ = t.write_all(format!("{c}\r").as_bytes());
        std::thread::sleep(Duration::from_millis(1200));
        let mut buf = [0u8; 512];
        let n = t.read(&mut buf, Duration::from_millis(1200)).unwrap_or(0);
        println!("{c:<8} -> {}", String::from_utf8_lossy(&buf[..n]).replace('\r', " "));
    }
    let _ = t.close();
}
