//! Bluetooth Classic on Android.
//!
//! A phone has no serial ports, so a paired ELM327 cannot be opened the way
//! the desktop opens `COM5`. Android only exposes Bluetooth through its Java
//! API, so the link is split in two:
//!
//! - `BluetoothClassicPlugin.kt` (in `gen/android`) owns the RFCOMM socket. It
//!   lists bonded devices, connects, writes, and keeps a reader thread filling
//!   a buffer. Every command returns at once: plugin commands are dispatched on
//!   the UI thread, and a blocking one would freeze the screen.
//! - This module wraps those commands in the core's blocking [`Transport`],
//!   and installs a [`PlatformLinks`] provider so paired devices are listed and
//!   opened like any other port. The rest of the core never learns the
//!   difference.
//!
//! Links are named `bt:<address>`, so a name from this provider is never
//! mistaken for anything else.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use aim_transport::platform::PlatformLinks;
use aim_transport::{PortInfo, PortKind, Transport, TransportStats};
use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::Wry;

/// Prefix of every link name this provider owns.
const PREFIX: &str = "bt:";

/// How long a read waits between polls of the plugin's buffer.
///
/// Each poll is a round trip through JNI and the UI thread's queue, so this is
/// a trade: shorter answers sooner and costs more. An ELM327 at 38400 baud
/// takes about 2 ms per 8 characters, and its replies are rarely finished in
/// under 20 ms, so 5 ms adds little to any reply.
const POLL: Duration = Duration::from_millis(5);

/// The Tauri plugin that registers the Kotlin side and installs the provider.
pub fn init() -> TauriPlugin<Wry> {
    Builder::new("obd-bluetooth")
        .setup(|_app, api| {
            let handle =
                api.register_android_plugin("com.wtfault.scanner", "BluetoothClassicPlugin")?;
            if !aim_transport::platform::install(Box::new(AndroidLinks { handle })) {
                tracing::warn!("a platform link provider was already installed; Bluetooth is not");
            }
            Ok(())
        })
        .build()
}

#[derive(Debug, Deserialize)]
struct Listing {
    #[serde(default)]
    devices: Vec<Device>,
    /// Why nothing can be listed: Bluetooth off, permission not granted, no
    /// Bluetooth hardware at all.
    problem: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Device {
    name: Option<String>,
    address: String,
}

#[derive(Serialize)]
struct Address<'a> {
    address: &'a str,
}

#[derive(Serialize)]
struct WriteArgs<'a> {
    address: &'a str,
    data: &'a [u8],
}

#[derive(Serialize)]
struct ReadArgs<'a> {
    address: &'a str,
    max: usize,
}

#[derive(Debug, Deserialize)]
struct ReadReply {
    #[serde(default)]
    data: Vec<u8>,
    /// False once the socket has closed, whether we closed it or the adapter
    /// went away.
    open: bool,
}

#[derive(Debug, Deserialize)]
struct Ack {}

struct AndroidLinks {
    handle: PluginHandle<Wry>,
}

impl PlatformLinks for AndroidLinks {
    fn list(&self) -> Vec<PortInfo> {
        let listing: Listing = match self.handle.run_mobile_plugin("list", ()) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "listing paired Bluetooth devices failed");
                return vec![problem_entry(format!(
                    "Could not list paired Bluetooth devices: {e}"
                ))];
            }
        };
        // A problem is shown as an entry in the list, not as an empty one: "no
        // ports found" with no reason is the silent empty result this project
        // treats as a bug.
        if let Some(problem) = listing.problem {
            return vec![problem_entry(problem)];
        }
        listing
            .devices
            .into_iter()
            .map(|d| {
                PortInfo {
                    name: format!("{PREFIX}{}", d.address),
                    kind: PortKind::Bluetooth,
                    vid: None,
                    pid: None,
                    serial_number: None,
                    manufacturer: None,
                    product: Some(d.name.unwrap_or_else(|| "unnamed device".into())),
                    likely_obd_adapter: false,
                    unusable_because: None,
                }
                .classify()
            })
            .collect()
    }

    fn owns(&self, name: &str) -> bool {
        name.starts_with(PREFIX)
    }

    fn transport(&self, name: &str) -> AimResult<Box<dyn Transport>> {
        let address = name.strip_prefix(PREFIX).unwrap_or(name);
        if address.is_empty() || address == "unavailable" {
            return Err(AimError::new(
                ErrorCode::TransportNotFound,
                "no paired Bluetooth adapter was selected",
            ));
        }
        Ok(Box::new(BluetoothTransport {
            handle: self.handle.clone(),
            address: address.to_string(),
            open: false,
            pending: VecDeque::new(),
            stats: TransportStats::default(),
        }))
    }
}

/// A list entry that explains why there is nothing to connect to.
fn problem_entry(reason: String) -> PortInfo {
    PortInfo {
        name: format!("{PREFIX}unavailable"),
        kind: PortKind::Bluetooth,
        vid: None,
        pid: None,
        serial_number: None,
        manufacturer: None,
        product: Some("Bluetooth".into()),
        likely_obd_adapter: false,
        unusable_because: Some(reason),
    }
}

struct BluetoothTransport {
    handle: PluginHandle<Wry>,
    address: String,
    open: bool,
    /// Bytes fetched from the plugin that the caller has not taken yet.
    pending: VecDeque<u8>,
    stats: TransportStats,
}

impl BluetoothTransport {
    fn call<T: serde::de::DeserializeOwned>(
        &mut self,
        command: &str,
        payload: impl Serialize,
        code: ErrorCode,
    ) -> AimResult<T> {
        self.handle.run_mobile_plugin(command, payload).map_err(|e| {
            self.stats.io_errors += 1;
            AimError::new(code, format!("Bluetooth {command} on {}: {e}", self.address))
        })
    }

    fn fetch(&mut self) -> AimResult<()> {
        let address = self.address.clone();
        let reply: ReadReply =
            self.call("read", ReadArgs { address: &address, max: 4096 }, ErrorCode::TransportIo)?;
        self.stats.bytes_read += reply.data.len() as u64;
        self.pending.extend(reply.data);
        if !reply.open && self.pending.is_empty() {
            self.open = false;
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                format!("the Bluetooth link to {} closed", self.address),
            ));
        }
        Ok(())
    }
}

impl Transport for BluetoothTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Bluetooth
    }

    fn descriptor(&self) -> String {
        format!("{PREFIX}{}", self.address)
    }

    fn open(&mut self) -> AimResult<()> {
        if self.open {
            return Ok(());
        }
        let address = self.address.clone();
        let _: Ack =
            self.call("open", Address { address: &address }, ErrorCode::TransportOpenFailed)?;
        self.open = true;
        self.pending.clear();
        self.stats.opens += 1;
        Ok(())
    }

    fn close(&mut self) -> AimResult<()> {
        if !self.open {
            return Ok(());
        }
        self.open = false;
        let address = self.address.clone();
        let _: Ack = self.call("close", Address { address: &address }, ErrorCode::TransportIo)?;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        if !self.open {
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                format!("the Bluetooth link to {} is not open", self.address),
            ));
        }
        let address = self.address.clone();
        let _: Ack =
            self.call("write", WriteArgs { address: &address, data }, ErrorCode::TransportIo)?;
        self.stats.bytes_written += data.len() as u64;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.pending.is_empty() {
                let n = buf.len().min(self.pending.len());
                for (slot, byte) in buf.iter_mut().zip(self.pending.drain(..n)) {
                    *slot = byte;
                }
                return Ok(n);
            }
            self.fetch()?;
            if !self.pending.is_empty() {
                continue;
            }
            if Instant::now() >= deadline {
                self.stats.read_timeouts += 1;
                return Ok(0);
            }
            std::thread::sleep(POLL);
        }
    }

    fn flush_input(&mut self) -> AimResult<()> {
        self.pending.clear();
        if !self.open {
            return Ok(());
        }
        // Drain whatever the reader thread has buffered, so a late reply to
        // the previous command is not taken for this one's.
        loop {
            self.fetch()?;
            if self.pending.is_empty() {
                return Ok(());
            }
            self.pending.clear();
        }
    }

    fn stats(&self) -> TransportStats {
        self.stats
    }
}
