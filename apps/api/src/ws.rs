//! WebSocket endpoints.
//!
//! Two streams, both newline-free JSON text frames with a `type` discriminant:
//!
//! * `GET /api/v1/sessions/{id}/stream` — the flight recorder as it happens.
//!   Replays the backlog from `after_seq` first, then follows.
//! * `GET /api/v1/live` — live data. The client subscribes with a module and a
//!   signal list; the server samples on an interval and streams the results.
//!
//! # Lag is reported, never hidden
//!
//! The event broadcast has a bounded buffer. A client that falls too far behind
//! is told exactly how many events it missed and what sequence number to resume
//! from, so it can re-read them over HTTP. Silently dropping events would make
//! the flight recorder a lie for that client.

use crate::state::AppState;
use aim_types::SessionId;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// Smallest sampling interval a client may ask for.
///
/// An ELM327 exchange is one request and one reply per PID over a slow link;
/// asking for 5 ms would just queue requests faster than the adapter can answer
/// them. The floor makes the limit explicit instead of letting a client build a
/// backlog it can never drain.
const MIN_INTERVAL_MS: u64 = 50;

/// Largest number of signals in one live subscription.
const MAX_SIGNALS: usize = 32;

/// Messages the server sends on the session event stream.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventStreamMessage {
    /// Stream established.
    Hello {
        /// Session being followed.
        session_id: SessionId,
        /// Highest sequence number already sent in the backlog.
        from_seq: i64,
    },
    /// One recorded event.
    Event {
        /// The event.
        event: Box<aim_types::SessionEvent>,
    },
    /// The client fell behind the broadcast buffer.
    Lagged {
        /// How many events were dropped for this client.
        missed: u64,
        /// Re-read from here with `GET /api/v1/sessions/{id}/events?after_seq=`.
        resume_after_seq: i64,
    },
    /// Something went wrong on the stream.
    Error {
        /// Structured error.
        error: aim_types::AimError,
    },
}

/// Messages a client sends on the live-data stream.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveCommand {
    /// Start or replace the subscription.
    Subscribe {
        /// Module key, e.g. `ECU_7E8`.
        module: String,
        /// Signal ids or PID numbers to sample.
        signals: Vec<String>,
        /// Sampling interval in milliseconds.
        #[serde(default)]
        interval_ms: Option<u64>,
    },
    /// Stop sampling but keep the socket open.
    Unsubscribe,
}

/// Messages the server sends on the live-data stream.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveMessage {
    /// Stream established.
    Hello {
        /// Smallest interval this server will honour.
        min_interval_ms: u64,
        /// Largest number of signals per subscription.
        max_signals: usize,
    },
    /// Subscription accepted, possibly with an adjusted interval.
    Subscribed {
        /// Module being sampled.
        module: String,
        /// Signals requested.
        signals: Vec<String>,
        /// Interval actually used.
        interval_ms: u64,
    },
    /// Sampling stopped.
    Unsubscribed,
    /// One sample. The payload is the same `ToolResult` envelope the HTTP
    /// endpoints return, so a chart and a one-shot read parse identically.
    Sample {
        /// The sample.
        result: Box<aim_types::ToolResult>,
    },
    /// Something went wrong.
    Error {
        /// Structured error.
        error: aim_types::AimError,
    },
}

fn text<T: Serialize>(value: &T) -> Message {
    match serde_json::to_string(value) {
        Ok(s) => Message::Text(Utf8Bytes::from(s)),
        Err(e) => Message::Text(Utf8Bytes::from(
            json!({ "type": "error", "error": { "code": "internal", "message": e.to_string() } })
                .to_string(),
        )),
    }
}

// ------------------------------------------------------- session event feed

/// Query for the event stream.
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Replay recorded events after this sequence number before following.
    #[serde(default)]
    pub after_seq: i64,
}

/// `GET /api/v1/sessions/{id}/stream`
pub async fn session_events_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Response {
    let session_id = SessionId::from_string(id);
    ws.on_upgrade(move |socket| run_event_stream(socket, state, session_id, q.after_seq))
}

async fn run_event_stream(
    mut socket: WebSocket,
    state: AppState,
    session_id: SessionId,
    after_seq: i64,
) {
    // Subscribe before replaying the backlog. Doing it the other way round
    // would drop anything recorded between the two steps.
    let mut receiver = state.store.subscribe();

    if let Err(e) = state.store.get_session(&session_id) {
        let _ = socket.send(text(&EventStreamMessage::Error { error: e })).await;
        return;
    }

    let mut last_seq = after_seq;
    match state.store.events_since(&session_id, after_seq, 5000) {
        Ok(backlog) => {
            for event in backlog {
                last_seq = event.seq;
                if socket
                    .send(text(&EventStreamMessage::Event { event: Box::new(event) }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        Err(e) => {
            let _ = socket.send(text(&EventStreamMessage::Error { error: e })).await;
            return;
        }
    }

    if socket
        .send(text(&EventStreamMessage::Hello {
            session_id: session_id.clone(),
            from_seq: last_seq,
        }))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                // The client only ever closes this stream; anything else is
                // ignored rather than misinterpreted.
                match incoming {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    _ => {}
                }
            }
            received = receiver.recv() => match received {
                Ok(event) => {
                    if event.session_id != session_id || event.seq <= last_seq {
                        continue;
                    }
                    last_seq = event.seq;
                    if socket
                        .send(text(&EventStreamMessage::Event { event: Box::new(event) }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    if socket
                        .send(text(&EventStreamMessage::Lagged {
                            missed,
                            resume_after_seq: last_seq,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

// ------------------------------------------------------------- live data

/// `GET /api/v1/live`
pub async fn live_data_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| run_live_stream(socket, state))
}

struct Subscription {
    module: String,
    signals: Vec<String>,
    interval: Duration,
}

async fn run_live_stream(mut socket: WebSocket, state: AppState) {
    if socket
        .send(text(&LiveMessage::Hello {
            min_interval_ms: MIN_INTERVAL_MS,
            max_signals: MAX_SIGNALS,
        }))
        .await
        .is_err()
    {
        return;
    }

    let mut subscription: Option<Subscription> = None;

    loop {
        // With no subscription, wait for a command. With one, race the next
        // sample against the next command so a client can retune mid-stream.
        let tick = match &subscription {
            Some(s) => tokio::time::sleep(s.interval),
            None => tokio::time::sleep(Duration::from_secs(3600)),
        };

        tokio::select! {
            incoming = socket.recv() => {
                let message = match incoming {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    // Ping/pong and binary frames are not commands.
                    _ => continue,
                };
                match serde_json::from_str::<LiveCommand>(&message) {
                    Ok(LiveCommand::Subscribe { module, signals, interval_ms }) => {
                        if signals.is_empty() || signals.len() > MAX_SIGNALS {
                            let error = aim_types::AimError::bad_request(format!(
                                "a subscription needs between 1 and {MAX_SIGNALS} signals, got {}",
                                signals.len()
                            ));
                            if socket.send(text(&LiveMessage::Error { error })).await.is_err() {
                                return;
                            }
                            continue;
                        }
                        // Clamp rather than reject: a client asking for 10 ms
                        // wants "as fast as possible", and is told what it got.
                        let interval_ms = interval_ms.unwrap_or(500).max(MIN_INTERVAL_MS);
                        if socket
                            .send(text(&LiveMessage::Subscribed {
                                module: module.clone(),
                                signals: signals.clone(),
                                interval_ms,
                            }))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        subscription = Some(Subscription {
                            module,
                            signals,
                            interval: Duration::from_millis(interval_ms),
                        });
                    }
                    Ok(LiveCommand::Unsubscribe) => {
                        subscription = None;
                        if socket.send(text(&LiveMessage::Unsubscribed)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let error = aim_types::AimError::bad_request(format!(
                            "unrecognised live-stream command: {e}"
                        ));
                        if socket.send(text(&LiveMessage::Error { error })).await.is_err() {
                            return;
                        }
                    }
                }
            }
            _ = tick => {
                let Some(sub) = &subscription else { continue };
                let module = sub.module.clone();
                let signals = sub.signals.clone();
                let sampled = state
                    .with_service(move |s| s.read_live_data(&module, &signals, "user:api:live"))
                    .await;
                let message = match sampled {
                    Ok(result) => LiveMessage::Sample { result: Box::new(result) },
                    Err(e) => {
                        // Losing the adapter mid-stream stops sampling; the
                        // socket stays open so the client sees the reason.
                        subscription = None;
                        LiveMessage::Error { error: e.0 }
                    }
                };
                if socket.send(text(&message)).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_messages_carry_a_type_discriminant() {
        let m = EventStreamMessage::Lagged { missed: 12, resume_after_seq: 400 };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["type"], "lagged");
        assert_eq!(v["missed"], 12);
        assert_eq!(v["resume_after_seq"], 400);
    }

    #[test]
    fn live_commands_parse_from_the_documented_shape() {
        let c: LiveCommand = serde_json::from_str(
            r#"{"type":"subscribe","module":"ECU_7E8","signals":["engine_rpm"],"interval_ms":250}"#,
        )
        .unwrap();
        match c {
            LiveCommand::Subscribe { module, signals, interval_ms } => {
                assert_eq!(module, "ECU_7E8");
                assert_eq!(signals, vec!["engine_rpm"]);
                assert_eq!(interval_ms, Some(250));
            }
            other => panic!("wrong command: {other:?}"),
        }

        // interval_ms is optional.
        let c: LiveCommand = serde_json::from_str(
            r#"{"type":"subscribe","module":"ECU_7E8","signals":["engine_rpm"]}"#,
        )
        .unwrap();
        assert!(matches!(c, LiveCommand::Subscribe { interval_ms: None, .. }));

        assert!(matches!(
            serde_json::from_str::<LiveCommand>(r#"{"type":"unsubscribe"}"#).unwrap(),
            LiveCommand::Unsubscribe
        ));
    }

    #[test]
    fn an_unknown_command_is_a_parse_error_not_a_default() {
        assert!(serde_json::from_str::<LiveCommand>(r#"{"type":"reflash"}"#).is_err());
        assert!(serde_json::from_str::<LiveCommand>(r#"{"module":"ECU_7E8"}"#).is_err());
    }

    #[test]
    fn live_messages_serialize_with_their_type() {
        let v = serde_json::to_value(LiveMessage::Hello {
            min_interval_ms: MIN_INTERVAL_MS,
            max_signals: MAX_SIGNALS,
        })
        .unwrap();
        assert_eq!(v["type"], "hello");
        assert_eq!(v["min_interval_ms"], MIN_INTERVAL_MS);
    }
}
