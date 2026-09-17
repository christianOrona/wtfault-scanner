//! Tests for retrying the first Bluetooth open (issue #61).

use aim_adapter::{
    AdapterObserver, AdapterResponse, DiagnosticAdapter, Elm327Adapter, Elm327Config,
};
use aim_simulator::{ScenarioId, SimulatedTransport};
use aim_transport::{Transport, TransportStats};
use aim_types::{
    AdapterCapabilities, AimError, AimResult, ConnectionState, ErrorCode, TransportKind,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct FlakyTransport {
    inner: SimulatedTransport,
    kind: TransportKind,
    failures_left: u32,
    open_attempts: Arc<AtomicU32>,
}

impl Transport for FlakyTransport {
    fn kind(&self) -> TransportKind {
        self.kind
    }

    fn open(&mut self) -> AimResult<()> {
        self.open_attempts.fetch_add(1, Ordering::SeqCst);
        if self.failures_left > 0 {
            self.failures_left -= 1;
            Err(AimError::new(
                ErrorCode::TransportOpenFailed,
                "could not open COM4: The semaphore timeout period has expired.",
            ))
        } else {
            self.inner.open()
        }
    }

    fn descriptor(&self) -> String {
        self.inner.descriptor()
    }

    fn baud(&self) -> Option<u32> {
        self.inner.baud()
    }

    fn close(&mut self) -> AimResult<()> {
        self.inner.close()
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        self.inner.write_all(data)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        self.inner.read(buf, timeout)
    }

    fn flush_input(&mut self) -> AimResult<()> {
        self.inner.flush_input()
    }

    fn stats(&self) -> TransportStats {
        self.inner.stats()
    }
}

struct RecordingObserver {
    failures: Mutex<Vec<ErrorCode>>,
}

impl AdapterObserver for RecordingObserver {
    fn on_failure(&self, _command: Option<&str>, error: &AimError) {
        self.failures.lock().unwrap().push(error.code);
    }

    fn on_request(&self, _command: &str) {}

    fn on_response(&self, _response: &AdapterResponse) {}

    fn on_state_change(&self, _from: &ConnectionState, _to: &ConnectionState) {}

    fn on_identified(&self, _capabilities: &AdapterCapabilities) {}
}

fn adapter(
    kind: TransportKind,
    failures: u32,
) -> (Elm327Adapter, Arc<AtomicU32>, Arc<RecordingObserver>) {
    let open_attempts = Arc::new(AtomicU32::new(0));
    let transport = FlakyTransport {
        inner: SimulatedTransport::new(ScenarioId::Healthy),
        kind,
        failures_left: failures,
        open_attempts: open_attempts.clone(),
    };
    let observer = Arc::new(RecordingObserver { failures: Mutex::new(Vec::new()) });
    let adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast())
        .with_observer(observer.clone());
    (adapter, open_attempts, observer)
}

#[test]
fn first_bluetooth_open_failure_is_retried_and_recorded() {
    let (mut adapter, open_attempts, observer) = adapter(TransportKind::Bluetooth, 1);
    assert!(adapter.connect().is_ok());
    assert_eq!(open_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(*observer.failures.lock().unwrap(), vec![ErrorCode::TransportOpenFailed]);
}

#[test]
fn bluetooth_port_that_keeps_failing_is_retried_only_once() {
    let (mut adapter, open_attempts, _observer) = adapter(TransportKind::Bluetooth, 5);
    let result = adapter.connect();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::TransportOpenFailed);
    assert_eq!(open_attempts.load(Ordering::SeqCst), 2);
}

#[test]
fn usb_port_is_not_retried() {
    let (mut adapter, open_attempts, _observer) = adapter(TransportKind::Usb, 1);
    let result = adapter.connect();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::TransportOpenFailed);
    assert_eq!(open_attempts.load(Ordering::SeqCst), 1);
}
