//! A scripted transport for tests.
//!
//! Answers a fixed request→response table with ELM327-style framing (`\r`
//! separated lines followed by the `>` prompt). Unknown requests answer `?`,
//! which is exactly what a real ELM327 does — so tests exercise the adapter's
//! error path rather than a convenient fiction.

use crate::{Transport, TransportStats};
use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use std::collections::VecDeque;
use std::time::Duration;

/// Scripted request/response transport.
pub struct LoopbackTransport {
    script: Vec<(String, Vec<String>)>,
    inbox: VecDeque<u8>,
    open: bool,
    stats: TransportStats,
    /// Requests seen, in order. Lets tests assert the exact AT sequence.
    pub seen: Vec<String>,
}

impl LoopbackTransport {
    /// Build from `(request, response_lines)` pairs. Matching is
    /// case-insensitive and ignores whitespace.
    pub fn new(script: Vec<(&str, Vec<&str>)>) -> Self {
        LoopbackTransport {
            script: script
                .into_iter()
                .map(|(k, v)| (normalize(k), v.into_iter().map(String::from).collect()))
                .collect(),
            inbox: VecDeque::new(),
            open: false,
            stats: TransportStats::default(),
            seen: Vec::new(),
        }
    }

    /// Add or replace a scripted exchange.
    pub fn set(&mut self, request: &str, lines: Vec<&str>) {
        let key = normalize(request);
        let value: Vec<String> = lines.into_iter().map(String::from).collect();
        match self.script.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.script.push((key, value)),
        }
    }

    fn respond(&mut self, request: &str) {
        let key = normalize(request);
        self.seen.push(key.clone());
        let lines = self
            .script
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| vec![String::from("?")]);
        for line in lines {
            self.inbox.extend(line.as_bytes());
            self.inbox.push_back(b'\r');
        }
        self.inbox.push_back(b'\r');
        self.inbox.push_back(b'>');
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

impl Transport for LoopbackTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Simulated
    }

    fn descriptor(&self) -> String {
        String::from("loopback")
    }

    fn open(&mut self) -> AimResult<()> {
        if !self.open {
            self.open = true;
            self.stats.opens += 1;
        }
        Ok(())
    }

    fn close(&mut self) -> AimResult<()> {
        self.open = false;
        self.inbox.clear();
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        if !self.open {
            self.stats.io_errors += 1;
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "loopback transport is closed",
            ));
        }
        self.stats.bytes_written += data.len() as u64;
        let text = String::from_utf8_lossy(data).to_string();
        for request in text.split(['\r', '\n']).filter(|s| !s.trim().is_empty()) {
            self.respond(request);
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], _timeout: Duration) -> AimResult<usize> {
        if !self.open {
            self.stats.io_errors += 1;
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "loopback transport is closed",
            ));
        }
        let n = buf.len().min(self.inbox.len());
        if n == 0 {
            self.stats.read_timeouts += 1;
            return Ok(0);
        }
        for slot in buf.iter_mut().take(n) {
            *slot = self.inbox.pop_front().expect("checked length");
        }
        self.stats.bytes_read += n as u64;
        Ok(n)
    }

    fn flush_input(&mut self) -> AimResult<()> {
        self.inbox.clear();
        Ok(())
    }

    fn stats(&self) -> TransportStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(t: &mut LoopbackTransport) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 64];
        loop {
            let n = t.read(&mut buf, Duration::from_millis(1)).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out).to_string()
    }

    #[test]
    fn scripted_requests_answer_and_terminate_with_a_prompt() {
        let mut t = LoopbackTransport::new(vec![("0100", vec!["41 00 BE 3F A8 13"])]);
        t.open().unwrap();
        t.write_all(b"0100\r").unwrap();
        let reply = drain(&mut t);
        assert!(reply.contains("41 00 BE 3F A8 13"));
        assert!(reply.ends_with('>'));
    }

    #[test]
    fn unknown_requests_answer_question_mark_like_real_hardware() {
        let mut t = LoopbackTransport::new(vec![]);
        t.open().unwrap();
        t.write_all(b"ATNOPE\r").unwrap();
        assert!(drain(&mut t).contains('?'));
    }

    #[test]
    fn matching_ignores_case_and_spaces() {
        let mut t = LoopbackTransport::new(vec![("01 0C", vec!["41 0C 1A F8"])]);
        t.open().unwrap();
        t.write_all(b"010c\r").unwrap();
        assert!(drain(&mut t).contains("41 0C 1A F8"));
        assert_eq!(t.seen, vec!["010C"]);
    }

    #[test]
    fn writing_to_a_closed_transport_is_an_error() {
        let mut t = LoopbackTransport::new(vec![]);
        let err = t.write_all(b"ATZ\r").unwrap_err();
        assert_eq!(err.code, ErrorCode::TransportDisconnected);
    }
}
