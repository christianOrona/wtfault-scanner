//! A real session's adapter exchanges, exported as a replay transcript (#59).
//!
//! The flight recorder keeps every command sent to the adapter and every line
//! it answered. Written out in the simulator's transcript format (`# note`,
//! `> command`, `< line`), a session from a real vehicle becomes a regression
//! test anyone can run without that vehicle. Before one is shared, its VIN is
//! replaced, including where the vehicle sent it as hex across several frames.

use aim_session::SessionStore;
use aim_types::{AimResult, EventKind, SessionId};
use std::collections::BTreeMap;

/// Every adapter exchange recorded for a session, as transcript text.
///
/// Starts with `# {note}`, then one `> command` line per `AdapterRequest` event
/// followed by one `< line` per line of its `AdapterResponse`, in recorded order.
pub fn export_transcript(
    store: &SessionStore,
    session_id: &SessionId,
    note: &str,
) -> AimResult<String> {
    let mut out = format!("# {note}\n");
    let mut after_seq = 0;
    loop {
        let events = store.events_since(session_id, after_seq, 1000)?;
        let Some(last) = events.last() else { break };
        after_seq = last.seq;
        for event in &events {
            match &event.kind {
                EventKind::AdapterRequest { command } => out.push_str(&format!("> {command}\n")),
                EventKind::AdapterResponse { lines, .. } => {
                    for line in lines {
                        out.push_str(&format!("< {line}\n"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Replace a VIN everywhere it appears in a transcript.
///
/// Both as text and as the hex bytes of its ASCII, including a VIN split across
/// the consecutive frames of one reply (`7E8 10 14 49 02 01 31 46 54`, then
/// `7E8 21 ...`). `replacement` must be 17 characters.
pub fn redact_vin(transcript: &str, vin: &str, replacement: &str) -> String {
    if vin.len() != 17 || replacement.len() != 17 || vin == replacement {
        return transcript.to_string();
    }
    let text = transcript.replace(vin, replacement);
    // `split` rather than `lines` so a trailing newline survives the rejoin.
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();

    let mut i = 0;
    while i < lines.len() {
        let start = i;
        let mut frames = Vec::new();
        while let Some(frame) = lines.get(i).and_then(|l| Frame::parse(l)) {
            frames.push(frame);
            i += 1;
        }
        if frames.is_empty() {
            i += 1;
            continue;
        }
        if replace_in_run(&mut frames, vin.as_bytes(), replacement.as_bytes()) {
            for (k, frame) in frames.iter().enumerate() {
                lines[start + k] = frame.render();
            }
        }
    }
    lines.join("\n")
}

/// One reply line from an adapter with headers on: address, ISO-TP PCI, data.
struct Frame {
    address: String,
    pci: Vec<u8>,
    data: Vec<u8>,
}

impl Frame {
    /// `< 7E8 21 37 57 ...`, or `None` for anything that is not a frame
    /// (`ELM327 v1.5`, `SEARCHING...`, `NO DATA`, a headerless reply).
    fn parse(line: &str) -> Option<Frame> {
        let mut tokens = line.strip_prefix("< ")?.split_whitespace();
        let address = tokens.next()?;
        if !matches!(address.len(), 3 | 8) || !address.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let bytes = tokens
            .map(|t| if t.len() == 2 { u8::from_str_radix(t, 16).ok() } else { None })
            .collect::<Option<Vec<u8>>>()?;
        // First frame carries a length byte after the PCI; single and consecutive frames do not.
        let pci_len = match bytes.first()? >> 4 {
            0x1 => 2,
            0x0 | 0x2 => 1,
            _ => return None,
        };
        if bytes.len() < pci_len {
            return None;
        }
        Some(Frame {
            address: address.to_string(),
            pci: bytes[..pci_len].to_vec(),
            data: bytes[pci_len..].to_vec(),
        })
    }

    fn render(&self) -> String {
        let hex: Vec<String> =
            self.pci.iter().chain(&self.data).map(|b| format!("{b:02X}")).collect();
        format!("< {} {}", self.address, hex.join(" "))
    }
}

/// Overwrite every occurrence of `vin` in each module's reassembled data.
///
/// Grouped by address so that interleaved replies from two modules are never
/// read as one message.
fn replace_in_run(frames: &mut [Frame], vin: &[u8], replacement: &[u8]) -> bool {
    let mut by_address: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (k, frame) in frames.iter().enumerate() {
        by_address.entry(frame.address.clone()).or_default().push(k);
    }
    let mut replaced = false;
    for indices in by_address.values() {
        let data: Vec<u8> = indices.iter().flat_map(|&k| frames[k].data.iter().copied()).collect();
        let mut hits = Vec::new();
        let mut from = 0;
        while let Some(p) =
            data.get(from..).and_then(|d| d.windows(vin.len()).position(|w| w == vin))
        {
            hits.push(from + p);
            from += p + vin.len();
        }
        if hits.is_empty() {
            continue;
        }
        replaced = true;
        let mut offset = 0;
        for &k in indices {
            for (j, byte) in frames[k].data.iter_mut().enumerate() {
                let at = offset + j;
                if let Some(&hit) = hits.iter().find(|&&h| (h..h + vin.len()).contains(&at)) {
                    *byte = replacement[at - hit];
                }
            }
            offset += frames[k].data.len();
        }
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIN: &str = "1FT7W2BT6KEC00001";
    const NEW: &str = "1FTEX1EP5JFA00000";

    #[test]
    fn a_vin_split_across_three_frames_is_replaced_in_place() {
        let text = "> 0902\n< 7E8 10 14 49 02 01 31 46 54\n< 7E8 21 37 57 32 42 54 36 4B\n< 7E8 22 45 43 30 30 30 30 31\n";
        let out = redact_vin(text, VIN, NEW);
        assert_eq!(
            out,
            "> 0902\n< 7E8 10 14 49 02 01 31 46 54\n< 7E8 21 45 58 31 45 50 35 4A\n< 7E8 22 46 41 30 30 30 30 30\n"
        );
    }

    #[test]
    fn a_reply_from_another_module_between_frames_is_not_read_as_part_of_it() {
        let text = "< 7E8 10 14 49 02 01 31 46 54\n< 7E9 03 41 00 00\n< 7E8 21 37 57 32 42 54 36 4B\n< 7E8 22 45 43 30 30 30 30 31";
        let out = redact_vin(text, VIN, NEW);
        assert!(out.contains("< 7E9 03 41 00 00"));
        assert!(out.ends_with("< 7E8 22 46 41 30 30 30 30 30"), "{out}");
    }
}
