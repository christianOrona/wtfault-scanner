//! Classification and parsing of ELM327 replies.
//!
//! The handoff is emphatic that adapter errors must never be hidden. So every
//! reply is classified into an explicit [`ResponseClass`], including the ones a
//! naive implementation would treat as "no answer": `NO DATA`, `?`,
//! `UNABLE TO CONNECT`, `BUS ERROR`, `CAN ERROR`, `BUFFER FULL`, `STOPPED`.
//! Each maps to a distinct [`ErrorCode`] so a caller can tell "the adapter did
//! not understand me" from "the vehicle did not answer".

use aim_types::{AimError, ErrorCode};
use serde::{Deserialize, Serialize};

/// What an ELM327 reply means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseClass {
    /// `OK` — command accepted.
    Ok,
    /// Hex data lines were returned.
    Data,
    /// An identification banner or other informational text.
    Info,
    /// `NO DATA` — request valid, nothing answered.
    NoData,
    /// `?` — the adapter did not understand the command.
    NotUnderstood,
    /// `UNABLE TO CONNECT` — adapter healthy, vehicle not answering.
    UnableToConnect,
    /// `SEARCHING...` with nothing after it.
    SearchingTimedOut,
    /// `BUS ERROR`, `BUS BUSY`, `CAN ERROR`, `FB ERROR`, `DATA ERROR`.
    BusError,
    /// `STOPPED` — the exchange was interrupted.
    Stopped,
    /// `BUFFER FULL` — the adapter's buffer overflowed.
    BufferFull,
    /// `ERROR` or an unrecognised failure banner.
    AdapterError,
    /// The reply was empty or never terminated with a prompt.
    Timeout,
}

impl ResponseClass {
    /// Whether the reply carries usable content.
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            ResponseClass::Ok | ResponseClass::Data | ResponseClass::Info
        )
    }

    /// Stable wire form used in the event log.
    pub fn as_str(&self) -> &'static str {
        match self {
            ResponseClass::Ok => "ok",
            ResponseClass::Data => "data",
            ResponseClass::Info => "info",
            ResponseClass::NoData => "no_data",
            ResponseClass::NotUnderstood => "not_understood",
            ResponseClass::UnableToConnect => "unable_to_connect",
            ResponseClass::SearchingTimedOut => "searching_timed_out",
            ResponseClass::BusError => "bus_error",
            ResponseClass::Stopped => "stopped",
            ResponseClass::BufferFull => "buffer_full",
            ResponseClass::AdapterError => "adapter_error",
            ResponseClass::Timeout => "timeout",
        }
    }

    /// The structured error this class represents, or `None` when it succeeded.
    pub fn to_error(self, command: &str, lines: &[String]) -> Option<AimError> {
        let code = match self {
            ResponseClass::Ok | ResponseClass::Data | ResponseClass::Info => return None,
            ResponseClass::NoData => ErrorCode::NoData,
            ResponseClass::NotUnderstood => ErrorCode::AdapterRejectedCommand,
            ResponseClass::UnableToConnect | ResponseClass::SearchingTimedOut => {
                ErrorCode::VehicleNotResponding
            }
            ResponseClass::BusError | ResponseClass::Stopped => ErrorCode::AdapterError,
            ResponseClass::BufferFull => ErrorCode::AdapterError,
            ResponseClass::AdapterError => ErrorCode::AdapterError,
            ResponseClass::Timeout => ErrorCode::TransportTimeout,
        };
        Some(
            AimError::new(
                code,
                format!("adapter answered {:?} with {}", command, self.as_str()),
            )
            .with_details(serde_json::json!({
                "command": command,
                "classification": self.as_str(),
                "lines": lines,
            })),
        )
    }
}

/// A parsed ELM327 reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterResponse {
    /// The command that produced it.
    pub command: String,
    /// Reply lines with echo, prompt and `SEARCHING...` noise removed.
    pub lines: Vec<String>,
    /// What the reply means.
    pub class: ResponseClass,
    /// Round-trip time.
    pub elapsed_ms: u64,
    /// True when the adapter printed `SEARCHING...` before answering, which
    /// means protocol negotiation happened and the next request will be faster.
    pub searched: bool,
}

impl AdapterResponse {
    /// The structured error for this reply, or `None` when it succeeded.
    pub fn error(&self) -> Option<AimError> {
        self.class.to_error(&self.command, &self.lines)
    }

    /// Reply lines, or the structured error if the reply was a failure.
    pub fn ok_lines(&self) -> Result<&[String], AimError> {
        match self.error() {
            Some(e) => Err(e),
            None => Ok(&self.lines),
        }
    }
}

/// Parse a raw reply buffer into lines and classify it.
///
/// `terminated` is false when [`aim_transport::read_until`] hit its deadline
/// without seeing the `>` prompt.
pub fn parse(command: &str, raw: &str, terminated: bool, elapsed_ms: u64) -> AdapterResponse {
    let echo = normalize(command);
    let mut searched = false;
    let mut lines: Vec<String> = Vec::new();
    for line in raw.split(['\r', '\n']) {
        let t = line.trim().trim_end_matches('>').trim();
        if t.is_empty() {
            continue;
        }
        // The adapter echoes the command until ATE0 takes effect.
        if normalize(t) == echo {
            continue;
        }
        // "SEARCHING..." is progress, not content.
        if t.eq_ignore_ascii_case("SEARCHING...") || t.eq_ignore_ascii_case("SEARCHING") {
            searched = true;
            continue;
        }
        lines.push(t.to_string());
    }

    let class = classify(&lines, terminated);
    AdapterResponse { command: command.to_string(), lines, class, elapsed_ms, searched }
}

fn classify(lines: &[String], terminated: bool) -> ResponseClass {
    if !terminated {
        return ResponseClass::Timeout;
    }
    if lines.is_empty() {
        return ResponseClass::NoData;
    }
    let upper: Vec<String> = lines.iter().map(|l| l.to_ascii_uppercase()).collect();
    for l in &upper {
        if l == "?" {
            return ResponseClass::NotUnderstood;
        }
        if l.contains("UNABLE TO CONNECT") {
            return ResponseClass::UnableToConnect;
        }
        if l.contains("NO DATA") {
            return ResponseClass::NoData;
        }
        if l.contains("BUFFER FULL") {
            return ResponseClass::BufferFull;
        }
        if l.contains("STOPPED") {
            return ResponseClass::Stopped;
        }
        if l.contains("BUS BUSY")
            || l.contains("BUS ERROR")
            || l.contains("CAN ERROR")
            || l.contains("FB ERROR")
            || l.contains("DATA ERROR")
            || l.contains("<RX ERROR")
        {
            return ResponseClass::BusError;
        }
        if l == "ERROR" || l.starts_with("ERR") {
            return ResponseClass::AdapterError;
        }
    }
    if upper.iter().all(|l| l == "OK") {
        return ResponseClass::Ok;
    }
    if lines.iter().any(|l| is_hex_line(l)) {
        return ResponseClass::Data;
    }
    ResponseClass::Info
}

/// True when a line looks like adapter hex output (`7E8 06 41 00 BE 3F A8 13`).
fn is_hex_line(line: &str) -> bool {
    let mut any = false;
    for token in line.split_whitespace() {
        // ELM327 multi-line non-header output prefixes each row with "0:".
        let token = token.trim_end_matches(':');
        if token.is_empty() || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        any = true;
    }
    any
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(cmd: &str, raw: &str) -> AdapterResponse {
        parse(cmd, raw, true, 10)
    }

    #[test]
    fn echo_and_prompt_are_stripped() {
        let r = p("0100", "0100\r41 00 BE 3F A8 13\r\r>");
        assert_eq!(r.lines, vec!["41 00 BE 3F A8 13"]);
        assert_eq!(r.class, ResponseClass::Data);
        assert!(r.error().is_none());
    }

    #[test]
    fn searching_is_progress_not_content() {
        let r = p("0100", "SEARCHING...\r41 00 BE 3F A8 13\r\r>");
        assert!(r.searched);
        assert_eq!(r.lines, vec!["41 00 BE 3F A8 13"]);
        assert_eq!(r.class, ResponseClass::Data);
    }

    #[test]
    fn searching_alone_means_the_vehicle_never_answered() {
        let r = p("0100", "SEARCHING...\r\r>");
        assert!(r.searched);
        assert_eq!(r.class, ResponseClass::NoData);
        assert_eq!(r.error().unwrap().code, ErrorCode::NoData);
    }

    #[test]
    fn ok_replies_are_recognised() {
        assert_eq!(p("ATE0", "ATE0\rOK\r\r>").class, ResponseClass::Ok);
        assert_eq!(p("ATH1", "OK\r\r>").class, ResponseClass::Ok);
    }

    #[test]
    fn every_failure_banner_maps_to_a_distinct_error_code() {
        for (raw, class, code) in [
            ("NO DATA\r\r>", ResponseClass::NoData, ErrorCode::NoData),
            ("?\r\r>", ResponseClass::NotUnderstood, ErrorCode::AdapterRejectedCommand),
            (
                "UNABLE TO CONNECT\r\r>",
                ResponseClass::UnableToConnect,
                ErrorCode::VehicleNotResponding,
            ),
            ("BUS ERROR\r\r>", ResponseClass::BusError, ErrorCode::AdapterError),
            ("BUS BUSY\r\r>", ResponseClass::BusError, ErrorCode::AdapterError),
            ("CAN ERROR\r\r>", ResponseClass::BusError, ErrorCode::AdapterError),
            ("STOPPED\r\r>", ResponseClass::Stopped, ErrorCode::AdapterError),
            ("BUFFER FULL\r\r>", ResponseClass::BufferFull, ErrorCode::AdapterError),
            ("ERROR\r\r>", ResponseClass::AdapterError, ErrorCode::AdapterError),
        ] {
            let r = p("0100", raw);
            assert_eq!(r.class, class, "{raw:?}");
            let err = r.error().expect("must produce an error");
            assert_eq!(err.code, code, "{raw:?}");
            // The raw lines travel with the error rather than being dropped.
            assert!(err.details.unwrap()["lines"].is_array());
        }
    }

    #[test]
    fn an_unterminated_reply_is_a_timeout_even_with_content() {
        let r = parse("0100", "41 00 BE", false, 5000);
        assert_eq!(r.class, ResponseClass::Timeout);
        assert_eq!(r.error().unwrap().code, ErrorCode::TransportTimeout);
    }

    #[test]
    fn identification_banners_classify_as_info() {
        let r = p("ATI", "ELM327 v1.5\r\r>");
        assert_eq!(r.class, ResponseClass::Info);
        assert_eq!(r.lines, vec!["ELM327 v1.5"]);
        assert!(r.error().is_none());
    }

    #[test]
    fn voltage_replies_are_info_not_hex() {
        let r = p("ATRV", "12.6V\r\r>");
        assert_eq!(r.class, ResponseClass::Info);
    }

    #[test]
    fn multi_frame_hex_output_is_data() {
        let r = p(
            "0902",
            "7E8 10 14 49 02 01 31 46 54\r7E8 21 37 57 32 42 54 36 4B\r7E8 22 45 43 30 30 30 30 31\r\r>",
        );
        assert_eq!(r.class, ResponseClass::Data);
        assert_eq!(r.lines.len(), 3);
    }

    #[test]
    fn ok_lines_returns_the_error_for_failures() {
        let r = p("0100", "NO DATA\r\r>");
        assert!(r.ok_lines().is_err());
        let r = p("0100", "41 00 BE 3F A8 13\r\r>");
        assert_eq!(r.ok_lines().unwrap().len(), 1);
    }
}
