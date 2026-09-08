//! Run the app's own port probe against a real adapter and print everything.
//!
//!     cargo run -p aim-adapter --example probe_port -- COM5
//!
//! Exists because "the app says silent" and "the port is silent" are different
//! claims, and telling them apart needs the app's exact code path rather than a
//! reimplementation of it in a script.

use std::time::Duration;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(true)
        .init();

    let port = std::env::args().nth(1).unwrap_or_else(|| "COM5".into());

    println!("--- ports as the app enumerates them ---");
    for p in aim_transport::list_ports() {
        println!(
            "  {:<8} kind={:?} likely_obd={} vid={:?} pid={:?} product={:?}",
            p.name, p.kind, p.likely_obd_adapter, p.vid, p.pid, p.product
        );
    }

    println!("\n--- each baud, one at a time, through the real transport ---");
    for baud in aim_transport::serial::CANDIDATE_BAUD_RATES {
        let cfg = aim_transport::serial::SerialConfig::usb(&port).with_baud(baud);
        let mut t = aim_transport::serial::SerialTransport::new(cfg);
        let started = std::time::Instant::now();
        let r = aim_adapter::probe::identify_transport(&mut t, Duration::from_millis(1_200));
        match r {
            Ok(id) => println!(
                "  {:>7} baud: responded={} elm={} banner={:?} ({} ms)",
                baud,
                id.responded,
                id.elm327_compatible,
                id.banner,
                started.elapsed().as_millis()
            ),
            Err(e) => println!(
                "  {:>7} baud: ERROR {:?} {} ({} ms)",
                baud,
                e.code,
                e.message,
                started.elapsed().as_millis()
            ),
        }
    }

    println!("
--- raw bytes at 500000, and how the parser classifies them ---");
    raw_dump(&port, 500_000);

    println!("\n--- find_baud, exactly as connect calls it ---");
    match aim_adapter::probe::find_baud(&port, Duration::from_millis(1_200)) {
        Ok((id, baud)) => println!("  baud={baud:?} responded={} banner={:?}", id.responded, id.banner),
        Err(e) => println!("  ERROR {:?} {}", e.code, e.message),
    }
}

/// Dump exactly what the transport receives, with no parsing in the way.
///
/// The parser turning a real reply into "silent" and the port genuinely being
/// silent look identical from the outside. This is how they are told apart.
#[allow(dead_code)]
fn raw_dump(port: &str, baud: u32) {
    use aim_transport::Transport;
    let cfg = aim_transport::serial::SerialConfig::usb(port).with_baud(baud);
    let mut t = aim_transport::serial::SerialTransport::new(cfg);
    if let Err(e) = t.open() {
        println!("  open failed: {}", e.message);
        return;
    }
    for cmd in ["ATZ", "ATE0", "ATI"] {
        let _ = t.flush_input();
        if let Err(e) = t.write_all(format!("{cmd}\r").as_bytes()) {
            println!("  {cmd}: write failed: {}", e.message);
            continue;
        }
        match aim_transport::read_until(&mut t, b'>', std::time::Duration::from_millis(1_200)) {
            Ok((bytes, terminated)) => {
                let text = String::from_utf8_lossy(&bytes)
                    .replace('\r', "<CR>")
                    .replace('\n', "<LF>");
                println!(
                    "  {cmd:<5} terminated={terminated} {} bytes: {text}",
                    bytes.len()
                );
                let parsed = aim_adapter::response::parse(
                    cmd,
                    &String::from_utf8_lossy(&bytes),
                    terminated,
                    0,
                );
                println!(
                    "         -> class={:?} success={} lines={:?}",
                    parsed.class,
                    parsed.class.is_success(),
                    parsed.lines
                );
            }
            Err(e) => println!("  {cmd:<5} read failed: {:?} {}", e.code, e.message),
        }
    }
    let _ = t.close();
}
