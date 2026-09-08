//! Replay of recorded adapter transcripts.
//!
//! Handoff §15 asks for "golden transcript" tests: known adapter
//! request/response logs replayed as a regression check. A transcript is a
//! plain text file that a human can read, diff and hand-edit:
//!
//! ```text
//! # 2019 F-250, cheap ELM327 clone, engine idling
//! > ATZ
//! < ELM327 v1.5
//! > 0100
//! < SEARCHING...
//! < 7E8 06 41 00 BE 3F A8 13
//! ```
//!
//! `>` lines are what the tool sent, `<` lines what the adapter answered, `#`
//! is a comment. A [`ReplayTransport`] in [`ReplayMode::Strict`] fails when the
//! tool deviates from the recorded command order, which is what turns a
//! transcript into a regression test rather than a lookup table.

use aim_transport::{Transport, TransportStats};
use aim_types::{AimError, AimResult, ErrorCode, TransportKind};
use std::collections::VecDeque;
use std::time::Duration;

/// One recorded command and the lines it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    /// The command the tool sent, without the carriage return.
    pub command: String,
    /// The lines the adapter answered with, prompt excluded.
    pub lines: Vec<String>,
}

/// A parsed transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    /// Leading `#` comment lines, kept so a re-serialized transcript keeps its
    /// provenance note.
    pub comments: Vec<String>,
    /// Exchanges in recorded order.
    pub exchanges: Vec<Exchange>,
}

impl Transcript {
    /// Parse transcript text.
    pub fn parse(text: &str) -> AimResult<Transcript> {
        let mut t = Transcript::default();
        let mut current: Option<Exchange> = None;
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(c) = trimmed.strip_prefix('#') {
                if current.is_none() {
                    t.comments.push(c.trim().to_string());
                }
                continue;
            }
            if let Some(cmd) = trimmed.strip_prefix('>') {
                if let Some(prev) = current.take() {
                    t.exchanges.push(prev);
                }
                current = Some(Exchange {
                    command: cmd.trim().to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(reply) = trimmed.strip_prefix('<') {
                match current.as_mut() {
                    Some(e) => e.lines.push(reply.trim().to_string()),
                    None => {
                        return Err(AimError::new(
                            ErrorCode::BadRequest,
                            format!("transcript line {} is a reply before any command", n + 1),
                        ))
                    }
                }
            } else {
                return Err(AimError::new(
                    ErrorCode::BadRequest,
                    format!(
                        "transcript line {} starts with none of '#', '>', '<': {line:?}",
                        n + 1
                    ),
                ));
            }
        }
        if let Some(last) = current {
            t.exchanges.push(last);
        }
        Ok(t)
    }

    /// Read a transcript from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> AimResult<Transcript> {
        let p = path.as_ref();
        let text = std::fs::read_to_string(p).map_err(|e| {
            AimError::new(
                ErrorCode::NotFound,
                format!("cannot read transcript {}: {e}", p.display()),
            )
        })?;
        Transcript::parse(&text)
    }

    /// Render back to transcript text. Round-trips with [`Transcript::parse`].
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for c in &self.comments {
            s.push_str("# ");
            s.push_str(c);
            s.push('\n');
        }
        for e in &self.exchanges {
            s.push_str("> ");
            s.push_str(&e.command);
            s.push('\n');
            for l in &e.lines {
                s.push_str("< ");
                s.push_str(l);
                s.push('\n');
            }
        }
        s
    }
}

/// How strictly a replay follows the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    /// Commands must arrive in the recorded order. A deviation is an error —
    /// this is what makes a transcript a regression test.
    Strict,
    /// Answer whichever recorded exchange matches the command, in any order.
    /// Useful for exploratory replay of a partial recording.
    Lookup,
}

/// A [`Transport`] that answers from a recorded transcript.
pub struct ReplayTransport {
    transcript: Transcript,
    mode: ReplayMode,
    descriptor: String,
    open: bool,
    position: usize,
    pending: VecDeque<u8>,
    line: Vec<u8>,
    stats: TransportStats,
    /// Commands that had no recorded answer, for the caller to report.
    pub misses: Vec<String>,
}

impl ReplayTransport {
    /// Replay `transcript`.
    pub fn new(transcript: Transcript, mode: ReplayMode, descriptor: impl Into<String>) -> Self {
        ReplayTransport {
            transcript,
            mode,
            descriptor: descriptor.into(),
            open: false,
            position: 0,
            pending: VecDeque::new(),
            line: Vec::new(),
            stats: TransportStats::default(),
            misses: Vec::new(),
        }
    }

    /// Replay a transcript file.
    pub fn from_file(path: impl AsRef<std::path::Path>, mode: ReplayMode) -> AimResult<Self> {
        let p = path.as_ref();
        let descriptor = format!("replay:{}", p.display());
        Ok(ReplayTransport::new(
            Transcript::from_file(p)?,
            mode,
            descriptor,
        ))
    }

    /// True when every recorded exchange has been replayed.
    pub fn is_exhausted(&self) -> bool {
        self.position >= self.transcript.exchanges.len()
    }

    fn answer(&mut self, command: &str) -> AimResult<Vec<String>> {
        let wanted = normalize(command);
        match self.mode {
            ReplayMode::Strict => {
                let Some(next) = self.transcript.exchanges.get(self.position) else {
                    self.misses.push(command.to_string());
                    return Err(AimError::new(
                        ErrorCode::NotFound,
                        format!("transcript is exhausted but the tool sent {command:?}"),
                    ));
                };
                if normalize(&next.command) != wanted {
                    let expected = next.command.clone();
                    self.misses.push(command.to_string());
                    return Err(AimError::new(
                        ErrorCode::UnexpectedResponse,
                        format!(
                            "transcript expected {expected:?} at position {} but the tool sent \
                             {command:?}",
                            self.position
                        ),
                    )
                    .with_details(serde_json::json!({
                        "position": self.position,
                        "expected": expected,
                        "actual": command,
                    })));
                }
                self.position += 1;
                Ok(next.lines.clone())
            }
            ReplayMode::Lookup => {
                match self
                    .transcript
                    .exchanges
                    .iter()
                    .find(|e| normalize(&e.command) == wanted)
                {
                    Some(e) => Ok(e.lines.clone()),
                    None => {
                        self.misses.push(command.to_string());
                        // A recording that does not cover a command is a gap in
                        // the recording, and the honest answer for a gap is the
                        // same thing a real adapter says when nothing answers.
                        Ok(vec![String::from("NO DATA")])
                    }
                }
            }
        }
    }
}

fn normalize(command: &str) -> String {
    command
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase()
}

impl Transport for ReplayTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Replay
    }

    fn descriptor(&self) -> String {
        self.descriptor.clone()
    }

    fn open(&mut self) -> AimResult<()> {
        if !self.open {
            self.open = true;
            self.stats.opens += 1;
            self.position = 0;
            self.pending.clear();
            self.line.clear();
        }
        Ok(())
    }

    fn close(&mut self) -> AimResult<()> {
        self.open = false;
        self.pending.clear();
        self.line.clear();
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn write_all(&mut self, data: &[u8]) -> AimResult<()> {
        if !self.open {
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "replay transport is closed",
            ));
        }
        self.stats.bytes_written += data.len() as u64;
        for b in data {
            match b {
                b'\r' | b'\n' => {
                    let command = String::from_utf8_lossy(&self.line).to_string();
                    self.line.clear();
                    let lines = self.answer(&command)?;
                    let mut reply = String::new();
                    for l in lines {
                        reply.push_str(&l);
                        reply.push('\r');
                    }
                    reply.push('\r');
                    reply.push('>');
                    self.pending.extend(reply.as_bytes());
                }
                other => self.line.push(*other),
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> AimResult<usize> {
        if !self.open {
            return Err(AimError::new(
                ErrorCode::TransportDisconnected,
                "replay transport is closed",
            ));
        }
        if self.pending.is_empty() {
            std::thread::sleep(timeout.min(Duration::from_millis(20)));
            self.stats.read_timeouts += 1;
            return Ok(0);
        }
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().unwrap_or(0);
        }
        self.stats.bytes_read += n as u64;
        Ok(n)
    }

    fn flush_input(&mut self) -> AimResult<()> {
        self.pending.clear();
        Ok(())
    }

    fn stats(&self) -> TransportStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# recorded on the bench
> ATZ
< ELM327 v1.5
> ATE0
< OK
> 0100
< SEARCHING...
< 7E8 06 41 00 BE 3F A8 13
";

    #[test]
    fn transcripts_parse_and_round_trip() {
        let t = Transcript::parse(SAMPLE).unwrap();
        assert_eq!(t.comments, vec!["recorded on the bench"]);
        assert_eq!(t.exchanges.len(), 3);
        assert_eq!(t.exchanges[2].command, "0100");
        assert_eq!(t.exchanges[2].lines.len(), 2);
        assert_eq!(Transcript::parse(&t.to_text()).unwrap(), t);
    }

    #[test]
    fn malformed_transcripts_are_rejected_with_a_line_number() {
        let e = Transcript::parse("hello\n").unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        assert!(e.message.contains("line 1"));
        let e = Transcript::parse("< orphan\n").unwrap_err();
        assert!(e.message.contains("before any command"));
    }

    fn drive(t: &mut ReplayTransport, command: &str) -> AimResult<String> {
        t.write_all(format!("{command}\r").as_bytes())?;
        let (bytes, _) = aim_transport::read_until(t, b'>', Duration::from_millis(100))?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    #[test]
    fn strict_replay_answers_the_recorded_sequence() {
        let mut t = ReplayTransport::new(
            Transcript::parse(SAMPLE).unwrap(),
            ReplayMode::Strict,
            "replay:test",
        );
        t.open().unwrap();
        assert!(drive(&mut t, "ATZ").unwrap().contains("ELM327"));
        assert!(drive(&mut t, "ATE0").unwrap().contains("OK"));
        assert!(drive(&mut t, "0100").unwrap().contains("7E8"));
        assert!(t.is_exhausted());
    }

    #[test]
    fn strict_replay_fails_when_the_tool_deviates_from_the_recording() {
        let mut t = ReplayTransport::new(
            Transcript::parse(SAMPLE).unwrap(),
            ReplayMode::Strict,
            "replay:test",
        );
        t.open().unwrap();
        drive(&mut t, "ATZ").unwrap();
        let e = drive(&mut t, "ATH1").unwrap_err();
        assert_eq!(e.code, ErrorCode::UnexpectedResponse);
        assert!(e.message.contains("ATE0"));
        assert_eq!(t.misses, vec!["ATH1"]);
    }

    #[test]
    fn strict_replay_reports_running_off_the_end_of_the_recording() {
        let mut t = ReplayTransport::new(
            Transcript::parse("> ATZ\n< ELM327 v1.5\n").unwrap(),
            ReplayMode::Strict,
            "replay:test",
        );
        t.open().unwrap();
        drive(&mut t, "ATZ").unwrap();
        assert_eq!(drive(&mut t, "ATE0").unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn lookup_replay_ignores_order_and_reports_gaps_as_no_data() {
        let mut t = ReplayTransport::new(
            Transcript::parse(SAMPLE).unwrap(),
            ReplayMode::Lookup,
            "replay:test",
        );
        t.open().unwrap();
        assert!(drive(&mut t, "0100").unwrap().contains("7E8"));
        assert!(drive(&mut t, "ATZ").unwrap().contains("ELM327"));
        assert!(drive(&mut t, "010C").unwrap().contains("NO DATA"));
        assert_eq!(t.misses, vec!["010C"]);
    }

    #[test]
    fn command_matching_ignores_spacing_and_case() {
        let mut t = ReplayTransport::new(
            Transcript::parse("> 01 0C\n< 7E8 04 41 0C 1A F8\n").unwrap(),
            ReplayMode::Strict,
            "replay:test",
        );
        t.open().unwrap();
        assert!(drive(&mut t, "010c").unwrap().contains("7E8"));
    }
}
