//! The real `Elm327Adapter` driving the emulated adapter.
//!
//! Nothing here is mocked: the adapter runs its actual initialization
//! sequence, parses actual ELM327 text, and reassembles actual ISO-TP frames.
//! If the adapter and the simulator ever disagree about the wire protocol,
//! these tests are what notices.

use aim_adapter::{DiagnosticAdapter, Elm327Adapter, Elm327Config, RequestTarget};
use aim_protocols::{decode_vin, ObdRequest, ObdResponse, Service};
use aim_simulator::{
    AdapterPersonality, InjectedFault, ScenarioId, SimulatedTransport, SIMULATED_VIN,
};
use aim_types::{ConnectionState, ErrorCode, ObdProtocol};
use std::sync::{Arc, Mutex};

fn connected(scenario: ScenarioId) -> (Elm327Adapter, aim_simulator::SharedEmulator) {
    let transport = SimulatedTransport::new(scenario);
    let emulator = transport.emulator();
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());
    adapter.connect().expect("connect against the simulator");
    (adapter, emulator)
}

#[test]
fn connecting_walks_the_state_machine_to_ready() {
    let (adapter, _) = connected(ScenarioId::Healthy);
    assert_eq!(adapter.state(), ConnectionState::Ready);
    assert_eq!(adapter.protocol(), ObdProtocol::Iso15765Can11_500);
    assert!(adapter.descriptor().starts_with("sim:"));
}

#[test]
fn the_cheap_clone_produces_capability_caveats_not_assumptions() {
    let (adapter, _) = connected(ScenarioId::Healthy);
    let caps = adapter.capabilities();

    assert!(caps.elm327_compatible);
    assert!(caps.iso_tp);
    // Demonstrated: the vehicle answered a request this adapter transmitted.
    assert!(caps.supports_transmit);
    // Measured rather than assumed: this clone refuses the programmable-protocol
    // commands a second bus needs, so it does not get the flag. A better adapter
    // that accepts them does, which is why this is no longer hardcoded false.
    assert!(!caps.multiple_can_buses);
    assert!(!caps.j2534);
    // Measured, not declared.
    assert!(caps.max_reliable_throughput > 0.0);

    // The clone claims v2.1 and refuses AT@1; both must be recorded.
    let caveats = caps.caveats.join(" | ");
    assert!(caveats.contains("v2.x"), "missing clone-version caveat: {caveats}");
    assert!(caveats.contains("AT@1"), "missing AT@1 caveat: {caveats}");
    assert!(
        caveats.contains("second CAN bus not established by software"),
        "a device that has not shown it can reach a second bus must say so: {caveats}"
    );
    assert_eq!(caps.vendor, "unknown");
}

#[test]
fn a_genuine_adapter_reports_its_vendor_and_earns_fewer_caveats() {
    let transport = SimulatedTransport::with_personality(
        ScenarioId::Healthy,
        AdapterPersonality::genuine_v1_5(),
    );
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());
    adapter.connect().unwrap();
    let caps = adapter.capabilities();
    assert_eq!(caps.vendor, "OBDII to RS232 Interpreter");
    let caveats = caps.caveats.join(" | ");
    assert!(!caveats.contains("AT@1"));
    assert!(!caveats.contains("v2.x"));
}

/// The response window is learned, and the learning has to stop.
///
/// Widening on a miss is what rescues a vehicle that is merely slow. Doing it
/// on every miss forever would double the cost of every unsupported PID to
/// re-establish a fact the connect handshake already proved.
#[test]
fn an_unsupported_pid_is_not_retried_once_the_window_is_proven() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let before = adapter.health();

    // 0100 answered during connect, so the window is known to suit this
    // vehicle. An unsupported PID is therefore absent, not slow.
    let err =
        adapter.request(&ObdRequest::current_data(0xFE), &RequestTarget::Functional).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoData);

    let after = adapter.health();
    assert_eq!(
        after.no_data,
        before.no_data + 1,
        "one request, one NO DATA - a proven window must not be re-learned"
    );
}

/// An unpowered socket and a powered one with a silent bus look identical —
/// every protocol fails — and they are completely different problems. Getting
/// this backwards once sent a user looking for a blown fuse on a vehicle that
/// turned out to be working perfectly.
#[test]
fn an_unpowered_socket_is_named_rather_than_swept() {
    let mut dead = AdapterPersonality::cheap_clone_v2_1();
    dead.voltage = 1.2;
    let transport = SimulatedTransport::with_personality(ScenarioId::BusSilent, dead);
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());

    adapter.connect().unwrap();
    let caveats = adapter.capabilities().caveats.join(" | ");
    assert!(caveats.contains("not powered"), "an unpowered socket must say so: {caveats}");
    // And having said so, it must not then claim the protocols were tried.
    assert!(
        !caveats.contains("ATSP1 to ATSP9"),
        "no protocol can answer through an unpowered socket, so none should be tried: {caveats}"
    );
}

/// The other half: powered, and still nothing answered. The voltage belongs in
/// the message so that this is stated rather than left to be worked out.
#[test]
fn a_powered_socket_that_answers_nothing_says_it_was_powered() {
    let transport = SimulatedTransport::new(ScenarioId::BusSilent);
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());
    adapter.connect().unwrap();

    let caveats = adapter.capabilities().caveats.join(" | ");
    assert!(caveats.contains("ATSP1 to ATSP9"), "the sweep should have run: {caveats}");
    assert!(
        caveats.contains("not a power problem"),
        "a powered socket must rule power out explicitly: {caveats}"
    );
}

#[test]
fn a_silent_vehicle_leaves_the_adapter_connected_but_degraded() {
    let transport = SimulatedTransport::new(ScenarioId::BusSilent);
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast());

    // Connecting succeeds: the adapter is fine, the vehicle is not answering,
    // and those are different facts.
    adapter.connect().unwrap();
    match adapter.state() {
        ConnectionState::Degraded { reason } => {
            assert!(reason.contains("did not answer"), "{reason}");
        }
        other => panic!("expected Degraded, got {other:?}"),
    }
    assert!(adapter.capabilities().elm327_compatible);
    // Nothing was ever demonstrated to reach the bus.
    assert!(!adapter.capabilities().supports_transmit);

    // And requests fail loudly rather than returning empty data.
    let err =
        adapter.request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional).unwrap_err();
    assert_eq!(err.code, ErrorCode::VehicleNotResponding);
    assert!(err.capability_state.is_some(), "errors carry capability state");
}

#[test]
fn a_broadcast_request_returns_one_message_per_responding_module() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let messages =
        adapter.request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional).unwrap();
    let addresses: Vec<&str> = messages.iter().map(|m| m.address.as_str()).collect();
    assert_eq!(addresses, vec!["7E8", "7EA", "7EB"]);
    for m in &messages {
        assert_eq!(&m.payload[..2], &[0x41, 0x0C]);
    }
}

#[test]
fn a_physical_request_reaches_exactly_one_module() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let messages = adapter
        .request(&ObdRequest::current_data(0x0C), &RequestTarget::Physical(String::from("7E2")))
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].address, "7EA");
}

#[test]
fn a_multi_frame_vin_survives_the_whole_stack() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let messages =
        adapter.request(&ObdRequest::vehicle_info(0x02), &RequestTarget::Functional).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].raw_lines.len() >= 3, "the VIN should have arrived segmented");

    let response = ObdResponse::parse(&messages[0].payload, true).unwrap();
    let payload = response.payload_for(&ObdRequest::vehicle_info(0x02)).unwrap();
    assert_eq!(decode_vin(payload).unwrap(), SIMULATED_VIN);
}

#[test]
fn an_unsupported_pid_surfaces_as_no_data_rather_than_an_empty_success() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let err =
        adapter.request(&ObdRequest::current_data(0xFE), &RequestTarget::Functional).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoData);
}

#[test]
fn an_ecu_negative_response_reaches_the_caller_intact() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    // Service 0x22 is UDS ReadDataByIdentifier, which this vehicle does not
    // implement; the engine controller answers 7F 22 11.
    let request = ObdRequest { service: Service::CurrentData, pid: None, extra: Vec::new() };
    let raw = adapter.raw_command("22F190").unwrap();
    assert!(raw.class.is_success(), "the adapter itself succeeded");
    let _ = request;

    let messages = aim_adapter::elm327::assemble_can(
        &raw.lines,
        true,
        aim_types::ObdProtocol::Iso15765Can11_500,
    )
    .unwrap();
    let response = ObdResponse::parse(&messages[0].payload, false).unwrap();
    match response {
        ObdResponse::Negative { service, nrc } => {
            assert_eq!(service, 0x22);
            assert_eq!(nrc.byte(), 0x11);
        }
        other => panic!("expected a negative response, got {other:?}"),
    }
}

#[test]
fn adapter_health_counts_what_actually_happened() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let before = adapter.health();
    assert!(before.requests > 0);
    assert!(before.responses > 0);
    assert_eq!(before.protocol, ObdProtocol::Iso15765Can11_500);

    let _ = adapter.request(&ObdRequest::current_data(0xFE), &RequestTarget::Functional);
    let after = adapter.health();
    assert_eq!(after.no_data, before.no_data + 1);
    assert!(after.success_rate() < 1.0);
}

#[test]
fn battery_voltage_comes_from_the_adapter_not_from_a_guess() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    let v = adapter.read_battery_voltage().unwrap();
    assert!((v - 14.1).abs() < 0.01);
    assert_eq!(adapter.health().battery_voltage, Some(v));
}

/// A fault the adapter can be brought back from is recovered, not reported.
///
/// Losing an address in the middle of a 255-address sweep to a transient buffer
/// overflow is a worse outcome than one extra request. The recovery is bounded
/// to a single attempt and stays visible in the health counters — a scan that
/// quietly retried its way to a clean result would be worse than one that
/// reported the glitch.
#[test]
fn a_recoverable_adapter_fault_is_recovered_from_and_still_counted() {
    let (mut adapter, emulator) = connected(ScenarioId::Healthy);

    for fault in [InjectedFault::BufferFull, InjectedFault::Stopped, InjectedFault::BusError] {
        let before = adapter.health().adapter_errors;
        emulator.lock().unwrap().inject(fault);

        let messages = adapter
            .request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional)
            .unwrap_or_else(|e| panic!("{fault:?} should have been recovered from, got {e:?}"));
        assert!(!messages.is_empty(), "{fault:?}: recovery must return real data");

        assert!(
            adapter.health().adapter_errors > before,
            "{fault:?} was recovered from but not recorded, which hides it"
        );
    }
}

/// A fault that recovery cannot help with still reaches the caller intact,
/// with the code that says which one it was.
#[test]
fn an_unrecoverable_fault_surfaces_as_its_own_error_code() {
    let (mut adapter, emulator) = connected(ScenarioId::Healthy);

    for (fault, expected) in [
        // The adapter rejected the command itself. Sending it again unchanged
        // would be rejected again.
        (InjectedFault::NotUnderstood, ErrorCode::AdapterRejectedCommand),
        // Nothing came back at all.
        (InjectedFault::Silence, ErrorCode::TransportTimeout),
    ] {
        emulator.lock().unwrap().inject(fault);
        let err = adapter
            .request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional)
            .unwrap_err();
        assert_eq!(err.code, expected, "wrong code for {fault:?}");
        assert!(err.capability_state.is_some(), "errors carry capability state");

        // And the link is still usable afterwards.
        assert!(adapter
            .request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional)
            .is_ok());
    }
}

#[test]
fn disconnecting_returns_the_adapter_to_disconnected_and_blocks_requests() {
    let (mut adapter, _) = connected(ScenarioId::Healthy);
    adapter.disconnect().unwrap();
    assert_eq!(adapter.state(), ConnectionState::Disconnected);
    let err =
        adapter.request(&ObdRequest::current_data(0x0C), &RequestTarget::Functional).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoActiveSession);
}

#[test]
fn the_observer_sees_every_command_and_state_change() {
    #[derive(Default)]
    struct Recorder {
        commands: Mutex<Vec<String>>,
        states: Mutex<Vec<String>>,
        identified: Mutex<bool>,
    }
    impl aim_adapter::AdapterObserver for Recorder {
        fn on_request(&self, command: &str) {
            self.commands.lock().unwrap().push(command.to_string());
        }
        fn on_response(&self, _r: &aim_adapter::AdapterResponse) {}
        fn on_failure(&self, _c: Option<&str>, _e: &aim_types::AimError) {}
        fn on_state_change(&self, _from: &ConnectionState, to: &ConnectionState) {
            self.states.lock().unwrap().push(to.name().to_string());
        }
        fn on_identified(&self, _c: &aim_types::AdapterCapabilities) {
            *self.identified.lock().unwrap() = true;
        }
    }

    let recorder = Arc::new(Recorder::default());
    let transport = SimulatedTransport::new(ScenarioId::Healthy);
    let mut adapter = Elm327Adapter::new(Box::new(transport), Elm327Config::fast())
        .with_observer(recorder.clone());
    adapter.connect().unwrap();

    let commands = recorder.commands.lock().unwrap().clone();
    for expected in ["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATSP0", "ATI", "0100"] {
        assert!(
            commands.iter().any(|c| c == expected),
            "{expected} was never sent; got {commands:?}"
        );
    }
    let states = recorder.states.lock().unwrap().clone();
    assert_eq!(states, vec!["connecting", "initializing", "identifying", "ready"]);
    assert!(*recorder.identified.lock().unwrap());
}

#[test]
fn the_regen_scenario_reports_its_codes_through_the_adapter() {
    let (mut adapter, _) = connected(ScenarioId::DpfRegen);
    let messages = adapter
        .request(&ObdRequest::bare(Service::StoredDtcs), &RequestTarget::Functional)
        .unwrap();
    let engine = messages.iter().find(|m| m.address == "7E8").unwrap();
    assert_eq!(engine.payload[0], 0x43);
    assert_eq!(engine.payload[1], 2);
    let codes = aim_protocols::decode_dtc_list(&engine.payload[2..]).unwrap();
    assert_eq!(codes, vec!["P2463", "P242F"]);
}
