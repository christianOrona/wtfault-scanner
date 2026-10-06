//! The one place a request leaves for the vehicle.
//!
//! The service has several dozen public operations and every one of them could
//! reach the adapter directly. What kept a write from going out unauthorised
//! was that each operation remembered to ask the gate first and that nothing
//! which only reads ever built a request that writes. Both were true by
//! inspection and neither was enforced: a community signal file could state
//! `2E` as its service, and the read that sends a catalogue command sent it.
//!
//! So the adapter sits behind this type, and the rule is here once:
//!
//! * A request whose service only reads goes out.
//! * A request whose service changes something goes out only with an
//!   [`Authorization`] that lists that service. Only the gate makes one, and
//!   what it lists comes from the capability that was authorised.
//! * Anything else is refused before the adapter sees a byte.
//!
//! There is no way through this type to send an adapter command or a raw
//! frame.

use aim_adapter::{DiagnosticAdapter, EcuMessage, RequestTarget, VehicleBus};
use aim_protocols::ObdRequest;
use aim_safety::Authorization;
use aim_types::{
    AdapterCapabilities, AdapterHealth, AimError, AimResult, ConnectionState, ErrorCode,
    ObdProtocol,
};
use std::time::Duration;

/// Whether a request only reads, judged by its service and, where the service
/// can do both, its sub-function.
///
/// An allowlist. A service this does not name is treated as one that changes
/// something, which is the safe way round to be wrong.
pub(crate) fn only_reads(pdu: &[u8]) -> bool {
    match pdu {
        // OBD-II: current data, freeze frame, stored codes, monitor results,
        // pending codes, vehicle information, permanent codes.
        [0x01 | 0x02 | 0x03 | 0x06 | 0x07 | 0x09 | 0x0A, ..] => true,
        // Read fault memory, read by local identifier, read by identifier.
        [0x19 | 0x21 | 0x22, ..] => true,
        // TesterPresent, which is how a module is found.
        [0x3E, ..] => true,
        // The default and extended sessions. Never programming (02), and
        // never a session this does not name.
        [0x10, 0x01 | 0x03] => true,
        // Asking for a seed, which establishes whether a module has security
        // at all. Never a key: there is no request here that sends one.
        [0x27, 0x01] => true,
        _ => false,
    }
}

fn not_a_read(pdu: &[u8]) -> AimError {
    let service = pdu.first().map(|b| format!("{b:02X}")).unwrap_or_else(|| String::from("none"));
    tracing::warn!(%service, "refused to send a request that is not a read");
    AimError::new(
        ErrorCode::OperationNotAllowed,
        format!(
            "service {service} is not a read, and a request that can change a vehicle is not \
             sent without being authorised for exactly that. Nothing was sent."
        ),
    )
    .with_details(serde_json::json!({ "service": service, "sent": false }))
}

fn not_permitted(permit: &Authorization, service: u8) -> AimError {
    tracing::warn!(
        capability = permit.capability(),
        service = %format!("{service:02X}"),
        "refused to send a service the authorised operation does not include"
    );
    AimError::new(
        ErrorCode::OperationNotAllowed,
        format!(
            "{} was authorised, and it does not include sending service {service:02X}. Nothing \
             was sent.",
            permit.capability()
        ),
    )
    .with_details(serde_json::json!({
        "capability": permit.capability(),
        "service": format!("{service:02X}"),
        "sent": false,
    }))
}

/// The adapter, reachable only through requests this has looked at.
pub(crate) struct Wire {
    adapter: Box<dyn DiagnosticAdapter>,
}

impl Wire {
    pub(crate) fn new(adapter: Box<dyn DiagnosticAdapter>) -> Self {
        Wire { adapter }
    }

    // ------------------------------------------------- nothing sent to a vehicle

    pub(crate) fn descriptor(&self) -> String {
        self.adapter.descriptor()
    }

    pub(crate) fn state(&self) -> ConnectionState {
        self.adapter.state()
    }

    pub(crate) fn capabilities(&self) -> AdapterCapabilities {
        self.adapter.capabilities()
    }

    pub(crate) fn health(&self) -> AdapterHealth {
        self.adapter.health()
    }

    pub(crate) fn protocol(&self) -> ObdProtocol {
        self.adapter.protocol()
    }

    pub(crate) fn connect(&mut self) -> AimResult<()> {
        self.adapter.connect()
    }

    pub(crate) fn disconnect(&mut self) -> AimResult<()> {
        self.adapter.disconnect()
    }

    pub(crate) fn select_bus(&mut self, bus: VehicleBus) -> AimResult<()> {
        self.adapter.select_bus(bus)
    }

    pub(crate) fn current_bus(&self) -> VehicleBus {
        self.adapter.current_bus()
    }

    pub(crate) fn secondary_bus_kbits(&self) -> Option<u32> {
        self.adapter.secondary_bus_kbits()
    }

    // ------------------------------------------------------------------ reads

    /// An OBD-II request that only reads.
    pub(crate) fn request(
        &mut self,
        request: &ObdRequest,
        target: &RequestTarget,
    ) -> AimResult<Vec<EcuMessage>> {
        if request.service.is_mutating() {
            return Err(not_a_read(&[request.service.id()]));
        }
        self.adapter.request(request, target)
    }

    /// A service request that only reads.
    pub(crate) fn request_pdu(
        &mut self,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: Duration,
    ) -> AimResult<Vec<EcuMessage>> {
        if !only_reads(pdu) {
            return Err(not_a_read(pdu));
        }
        self.adapter.request_pdu(pdu, target, timeout)
    }

    /// [`Wire::request_pdu`], keeping a reply that stopped partway.
    pub(crate) fn request_pdu_keeping_partial(
        &mut self,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: Duration,
    ) -> AimResult<Vec<EcuMessage>> {
        if !only_reads(pdu) {
            return Err(not_a_read(pdu));
        }
        self.adapter.request_pdu_keeping_partial(pdu, target, timeout)
    }

    /// [`Wire::request_pdu`] for a discovery probe.
    pub(crate) fn probe_pdu(
        &mut self,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: Duration,
    ) -> AimResult<Vec<EcuMessage>> {
        if !only_reads(pdu) {
            return Err(not_a_read(pdu));
        }
        self.adapter.probe_pdu(pdu, target, timeout)
    }

    // --------------------------------------------------- requests that change

    /// An OBD-II request that changes the vehicle, under the authorization
    /// that permits it.
    pub(crate) fn request_changing(
        &mut self,
        permit: &Authorization,
        request: &ObdRequest,
        target: &RequestTarget,
    ) -> AimResult<Vec<EcuMessage>> {
        let service = request.service.id();
        if !permit.permits_service(service) {
            return Err(not_permitted(permit, service));
        }
        self.adapter.request(request, target)
    }

    /// A service request that changes the vehicle, under the authorization
    /// that permits it.
    pub(crate) fn send_changing(
        &mut self,
        permit: &Authorization,
        pdu: &[u8],
        target: &RequestTarget,
        timeout: Duration,
    ) -> AimResult<Vec<EcuMessage>> {
        let Some(&service) = pdu.first() else {
            return Err(not_a_read(pdu));
        };
        if !permit.permits_service(service) {
            return Err(not_permitted(permit, service));
        }
        self.adapter.request_pdu(pdu, target, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_adapter::{AdapterResponse, ObserverRef};
    use aim_protocols::Service;
    use aim_safety::{ApprovalDesk, OperationRequest, SafetyGate, VehicleConditions};
    use aim_types::TransportKind;
    use std::sync::{Arc, Mutex};

    /// An adapter that does nothing except write down what reached it.
    struct Recording {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl DiagnosticAdapter for Recording {
        fn descriptor(&self) -> String {
            String::from("recording")
        }
        fn state(&self) -> ConnectionState {
            ConnectionState::Ready
        }
        fn capabilities(&self) -> AdapterCapabilities {
            let mut c = AdapterCapabilities::unknown(TransportKind::Simulated);
            c.supports_transmit = true;
            c
        }
        fn health(&self) -> AdapterHealth {
            unimplemented!("not asked for in these tests")
        }
        fn set_observer(&mut self, _observer: ObserverRef) {}
        fn protocol(&self) -> ObdProtocol {
            ObdProtocol::Iso15765Can11_500
        }
        fn connect(&mut self) -> AimResult<()> {
            Ok(())
        }
        fn disconnect(&mut self) -> AimResult<()> {
            Ok(())
        }
        fn read_battery_voltage(&mut self) -> AimResult<f64> {
            Ok(12.6)
        }
        fn request(
            &mut self,
            request: &ObdRequest,
            _target: &RequestTarget,
        ) -> AimResult<Vec<EcuMessage>> {
            self.sent.lock().unwrap().push(vec![request.service.id()]);
            Ok(Vec::new())
        }
        fn request_pdu(
            &mut self,
            pdu: &[u8],
            _target: &RequestTarget,
            _timeout: Duration,
        ) -> AimResult<Vec<EcuMessage>> {
            self.sent.lock().unwrap().push(pdu.to_vec());
            Ok(Vec::new())
        }
        fn raw_command(&mut self, _command: &str) -> AimResult<AdapterResponse> {
            unimplemented!("nothing in the diagnostic core can reach this")
        }
    }

    fn wire() -> (Wire, Arc<Mutex<Vec<Vec<u8>>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        (Wire::new(Box::new(Recording { sent: Arc::clone(&sent) })), sent)
    }

    fn ready() -> VehicleConditions {
        VehicleConditions {
            ignition_on: true,
            engine_running: false,
            battery_voltage: Some(12.8),
            connection_stable: true,
            vehicle_speed_kph: Some(0.0),
        }
    }

    /// What the gate grants for `operation`, with a person's approval when it
    /// needs one.
    fn authorized(operation: &str) -> Authorization {
        let desk = ApprovalDesk::new();
        let gate = SafetyGate::phase1().trusting(&desk);
        let mut request = OperationRequest::new(operation, "user:test").about("it");
        if gate.registry().get(operation).unwrap().level.requires_confirmation() {
            let c = desk.issue(&[operation], "it", "YES", "approved in a test");
            request = request.approved(desk.redeem(&c.id, "YES", operation, "it").unwrap());
        }
        let mut caps = AdapterCapabilities::unknown(TransportKind::Simulated);
        caps.supports_transmit = true;
        gate.authorize(&request, Some(&caps), &ready()).into_result().unwrap()
    }

    const TARGET: RequestTarget = RequestTarget::Functional;
    const BUDGET: Duration = Duration::from_millis(10);

    /// Every service identifier there is, through every way of sending a read.
    /// What reaches the adapter is the allowlist and nothing else.
    #[test]
    fn no_service_that_is_not_a_read_leaves_through_a_read() {
        let (mut wire, sent) = wire();
        for service in 0..=u8::MAX {
            for tail in [vec![], vec![0x01], vec![0x02], vec![0x03], vec![0xDE, 0x01, 0xFF]] {
                let mut pdu = vec![service];
                pdu.extend(tail);
                let _ = wire.request_pdu(&pdu, &TARGET, BUDGET);
                let _ = wire.probe_pdu(&pdu, &TARGET, BUDGET);
                let _ = wire.request_pdu_keeping_partial(&pdu, &TARGET, BUDGET);
            }
        }
        let reached: std::collections::BTreeSet<u8> =
            sent.lock().unwrap().iter().map(|pdu| pdu[0]).collect();
        let allowed: std::collections::BTreeSet<u8> =
            [0x01, 0x02, 0x03, 0x06, 0x07, 0x09, 0x0A, 0x10, 0x19, 0x21, 0x22, 0x27, 0x3E].into();
        assert_eq!(reached, allowed);

        // The services that do damage, by name, so this reads as a list of
        // things that cannot happen: clear, reset, clear fault memory, write,
        // actuator control, routines, download, transfer, write by address.
        for service in [0x04u8, 0x08, 0x11, 0x14, 0x2E, 0x2F, 0x31, 0x34, 0x36, 0x37, 0x3D, 0x85] {
            assert!(!reached.contains(&service), "service {service:02X} reached the adapter");
        }
    }

    /// The two services that can do either are held to the harmless half.
    #[test]
    fn a_session_or_a_security_request_is_only_ever_the_harmless_one() {
        assert!(only_reads(&[0x10, 0x01]));
        assert!(only_reads(&[0x10, 0x03]));
        for pdu in [
            vec![0x10],
            vec![0x10, 0x02], // programming session
            vec![0x10, 0x40],
            vec![0x10, 0x03, 0x00],
            vec![0x27],
            vec![0x27, 0x02, 0xDE, 0xAD], // a key
            vec![0x27, 0x03],
            vec![0x27, 0x01, 0x00],
            vec![],
        ] {
            assert!(!only_reads(&pdu), "{pdu:02X?} would have been sent as a read");
        }
        assert!(only_reads(&[0x27, 0x01]));
    }

    #[test]
    fn clearing_codes_does_not_leave_as_an_ordinary_request() {
        let (mut wire, sent) = wire();
        let e = wire.request(&ObdRequest::bare(Service::ClearDtcs), &TARGET).unwrap_err();
        assert_eq!(e.code, ErrorCode::OperationNotAllowed);
        assert!(wire.request(&ObdRequest::current_data(0x0C), &TARGET).is_ok());
        assert_eq!(*sent.lock().unwrap(), vec![vec![0x01]]);
    }

    /// An authorization permits what its capability lists and nothing else.
    #[test]
    fn a_write_leaves_only_under_the_authorization_that_lists_it() {
        let write = [0x2E, 0xDE, 0x01, 0x00];
        let clear = [0x14, 0xFF, 0xFF, 0xFF];

        // A read, however it was authorised, permits no write at all.
        let (mut wire, sent) = wire();
        for operation in ["obd2.read_catalog_signal", "obd2.read_dtcs", "config.preview_change"] {
            let permit = authorized(operation);
            for pdu in [&write[..], &clear[..], &[0x11, 0x01][..], &[0x31, 0x01, 0xFF, 0x00][..]] {
                let e = wire.send_changing(&permit, pdu, &TARGET, BUDGET).unwrap_err();
                assert_eq!(e.code, ErrorCode::OperationNotAllowed, "{operation} {pdu:02X?}");
            }
            let e = wire
                .request_changing(&permit, &ObdRequest::bare(Service::ClearDtcs), &TARGET)
                .unwrap_err();
            assert_eq!(e.code, ErrorCode::OperationNotAllowed);
        }
        assert!(sent.lock().unwrap().is_empty(), "nothing reached the adapter");

        // A setting change permits the write and not the clear.
        let permit = authorized("config.write_feature");
        assert!(wire.send_changing(&permit, &write, &TARGET, BUDGET).is_ok());
        assert!(wire.send_changing(&permit, &clear, &TARGET, BUDGET).is_err());
        assert!(wire
            .request_changing(&permit, &ObdRequest::bare(Service::ClearDtcs), &TARGET)
            .is_err());

        // Clearing codes permits both clears and not the write.
        let permit = authorized("obd2.clear_dtcs");
        assert!(wire.send_changing(&permit, &clear, &TARGET, BUDGET).is_ok());
        assert!(wire
            .request_changing(&permit, &ObdRequest::bare(Service::ClearDtcs), &TARGET)
            .is_ok());
        assert!(wire.send_changing(&permit, &write, &TARGET, BUDGET).is_err());

        assert_eq!(*sent.lock().unwrap(), vec![write.to_vec(), clear.to_vec(), vec![0x04]]);
    }

    /// No authorization there is permits a reset, an actuator, a routine or a
    /// download. Swept over the registry, so a capability added later is held
    /// to it without anybody remembering this test.
    #[test]
    fn nothing_this_build_authorises_permits_a_service_it_has_no_business_sending() {
        let registry = aim_safety::CapabilityRegistry::phase1();
        for cap in registry.enabled() {
            for service in &cap.wire_services {
                assert!(
                    [0x04u8, 0x14, 0x2E].contains(service),
                    "{} may send service {service:02X}",
                    cap.id
                );
            }
        }
    }
}
