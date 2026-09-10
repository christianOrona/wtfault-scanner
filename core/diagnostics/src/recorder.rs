//! The Flight Recorder hook.
//!
//! [`SessionRecorder`] implements [`AdapterObserver`] and writes every command,
//! reply, failure and state transition into the session event log. It lives in
//! this crate rather than in `aim-session` so that storage stays storage: the
//! store knows what an event is, not what an adapter is.
//!
//! Recording must never break diagnostics. A failed write is logged and the
//! vehicle work continues — losing a log line is bad, losing the ability to
//! read a truck because the disk is full is worse.

use aim_adapter::{AdapterObserver, AdapterResponse};
use aim_session::SessionStore;
use aim_types::{AdapterCapabilities, AimError, ConnectionState, EventKind, SessionId};
use std::sync::atomic::{AtomicI64, Ordering};

/// Writes adapter activity into one session's event log.
#[derive(Debug)]
pub struct SessionRecorder {
    store: SessionStore,
    session_id: SessionId,
    /// Row id of the most recent recorded adapter response.
    ///
    /// This is how a decoded value gets its `raw_evidence_ref`: the reading and
    /// the bytes it came from point at the same row, so any value in the UI can
    /// be traced back to the exact adapter exchange that produced it.
    last_response_event: AtomicI64,
}

impl SessionRecorder {
    /// Record into `session_id` of `store`.
    pub fn new(store: SessionStore, session_id: SessionId) -> Self {
        SessionRecorder { store, session_id, last_response_event: AtomicI64::new(0) }
    }

    /// The session being recorded.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Row id of the last adapter response event, if any has been recorded.
    pub fn last_response_event(&self) -> Option<i64> {
        match self.last_response_event.load(Ordering::SeqCst) {
            0 => None,
            id => Some(id),
        }
    }

    fn record(&self, kind: EventKind) -> Option<i64> {
        match self.store.append_event(&self.session_id, kind) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::error!(
                    session = %self.session_id,
                    error = %e,
                    "failed to record a session event; diagnostics continue"
                );
                None
            }
        }
    }
}

impl AdapterObserver for SessionRecorder {
    fn on_request(&self, command: &str) {
        self.record(EventKind::AdapterRequest { command: command.to_string() });
    }

    fn on_response(&self, response: &AdapterResponse) {
        let id = self.record(EventKind::AdapterResponse {
            command: response.command.clone(),
            lines: response.lines.clone(),
            elapsed_ms: response.elapsed_ms,
            classification: response.class.as_str().to_string(),
        });
        if let Some(id) = id {
            self.last_response_event.store(id, Ordering::SeqCst);
        }
    }

    fn on_failure(&self, command: Option<&str>, error: &AimError) {
        self.record(EventKind::AdapterFailure {
            command: command.map(|c| c.to_string()),
            error: error.clone(),
        });
    }

    fn on_state_change(&self, from: &ConnectionState, to: &ConnectionState) {
        self.record(EventKind::ConnectionStateChanged {
            from: from.name().to_string(),
            to: to.clone(),
        });
    }

    fn on_identified(&self, capabilities: &AdapterCapabilities) {
        self.record(EventKind::AdapterIdentified { capabilities: capabilities.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_adapter::ResponseClass;
    use aim_types::ErrorCode;

    fn setup() -> (SessionStore, SessionRecorder, SessionId) {
        let store = SessionStore::open_in_memory().unwrap();
        let session = store.create_session(None).unwrap();
        let recorder = SessionRecorder::new(store.clone(), session.id.clone());
        (store, recorder, session.id)
    }

    #[test]
    fn commands_and_replies_are_both_recorded_in_order() {
        let (store, recorder, session) = setup();
        recorder.on_request("0100");
        recorder.on_response(&AdapterResponse {
            command: "0100".into(),
            lines: vec!["7E8 06 41 00 BE 3F A8 13".into()],
            class: ResponseClass::Data,
            elapsed_ms: 42,
            searched: true,
        });

        let events = store.events_since(&session, 0, 10).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.name()).collect();
        assert_eq!(kinds, vec!["session_started", "adapter_request", "adapter_response"]);
    }

    #[test]
    fn a_response_event_id_is_available_as_an_evidence_reference() {
        let (store, recorder, _) = setup();
        assert_eq!(recorder.last_response_event(), None);

        recorder.on_response(&AdapterResponse {
            command: "010C".into(),
            lines: vec!["7E8 04 41 0C 1A F8".into()],
            class: ResponseClass::Data,
            elapsed_ms: 12,
            searched: false,
        });
        let evidence = recorder.last_response_event().expect("evidence ref");

        // The reference resolves to the exact exchange that produced the bytes.
        let event = store.event_by_id(evidence).unwrap();
        match event.kind {
            EventKind::AdapterResponse { command, lines, .. } => {
                assert_eq!(command, "010C");
                assert_eq!(lines[0], "7E8 04 41 0C 1A F8");
            }
            other => panic!("wrong event kind: {other:?}"),
        }
    }

    #[test]
    fn failures_are_recorded_rather_than_swallowed() {
        let (store, recorder, session) = setup();
        recorder.on_failure(
            Some("0902"),
            &AimError::new(ErrorCode::TransportTimeout, "no prompt within 6000 ms"),
        );
        let events = store.events_since(&session, 0, 10).unwrap();
        match &events.last().unwrap().kind {
            EventKind::AdapterFailure { command, error } => {
                assert_eq!(command.as_deref(), Some("0902"));
                assert_eq!(error.code, ErrorCode::TransportTimeout);
            }
            other => panic!("wrong event kind: {other:?}"),
        }
    }

    #[test]
    fn a_failing_reply_is_still_recorded_as_a_response() {
        // NO DATA is an answer. It belongs in the trace, not in a void.
        let (store, recorder, session) = setup();
        recorder.on_response(&AdapterResponse {
            command: "01FE".into(),
            lines: vec![],
            class: ResponseClass::NoData,
            elapsed_ms: 8,
            searched: false,
        });
        let last = store.events_since(&session, 0, 10).unwrap();
        match &last.last().unwrap().kind {
            EventKind::AdapterResponse { classification, .. } => {
                assert_eq!(classification, "no_data");
            }
            other => panic!("wrong event kind: {other:?}"),
        }
    }

    #[test]
    fn state_transitions_and_identification_land_in_the_log() {
        let (store, recorder, session) = setup();
        recorder.on_state_change(&ConnectionState::Connecting, &ConnectionState::Ready);
        recorder.on_identified(&AdapterCapabilities::unknown(aim_types::TransportKind::Bluetooth));

        let events = store.events_since(&session, 0, 10).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.name()).collect();
        assert!(kinds.contains(&"connection_state_changed"));
        assert!(kinds.contains(&"adapter_identified"));
        match &events[1].kind {
            EventKind::ConnectionStateChanged { from, to } => {
                assert_eq!(from, "connecting");
                assert_eq!(to, &ConnectionState::Ready);
            }
            other => panic!("wrong event kind: {other:?}"),
        }
    }
}
