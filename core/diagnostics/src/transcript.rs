//! A real session's adapter exchanges, exported as a replay transcript (#59).
//!
//! The flight recorder keeps every command sent to the adapter and every line
//! it answered. Written out in the simulator's transcript format (`# note`,
//! `> command`, `< line`), a session from a real vehicle becomes a regression
//! test anyone can run without that vehicle. Before one is shared, its VIN is
//! replaced, including where the vehicle sent it as hex across several frames.

use aim_decoders::vin;
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

/// Create an anonymous VIN from a real VIN.
///
/// Keeps the first 8 characters (manufacturer code), model year (character 10),
/// and plant code (character 11), replaces the serial number (characters 12-17)
/// with "000000", and recomputes the check digit so the result is still valid.
pub fn anonymous_vin(vin: &str) -> AimResult<String> {
    let vin = vin::validate(vin)?;

    // Keep first 8 chars (manufacturer code), model year (pos 10), plant code (pos 11)
    // Replace serial number (positions 12-17) with "000000"
    let mut candidate = vin.chars().collect::<Vec<_>>();
    candidate[11] = '0'; // Position 12 (0-indexed) - start of serial
    candidate[12] = '0';
    candidate[13] = '0';
    candidate[14] = '0';
    candidate[15] = '0';
    candidate[16] = '0';

    // Recompute check digit (position 9, 0-indexed)
    let check_digit = vin::check_digit(&candidate.iter().collect::<String>())?;
    candidate[8] = check_digit;

    Ok(candidate.into_iter().collect())
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

/// Replace `vin` with its anonymous form throughout a transcript, and refuse
/// if any trace of it is left.
///
/// The one way a transcript is anonymised, for the API's export and for the
/// fixture tool alike, so a fix to one cannot miss the other.
pub fn anonymise(transcript: &str, vin: &str) -> AimResult<String> {
    let anonymous = anonymous_vin(vin)?;
    let text = redact_vin(transcript, vin, &anonymous);
    match vin_residue(&text, vin) {
        Some(residue) => Err(aim_types::AimError::new(
            aim_types::ErrorCode::PreconditionFailed,
            format!("the transcript was not anonymised: {residue} after replacing it"),
        )),
        None => Ok(text),
    }
}

/// The most bytes allowed between two characters of a VIN that still count as
/// the VIN, for [`vin_residue`]. A non-CAN reply repeats its header and the
/// service bytes on every line, which is six bytes on J1850.
const VIN_MAX_GAP: usize = 8;

/// Where a real VIN still shows in a transcript meant to have lost it, or
/// `None` when it shows nowhere this can look.
///
/// Checked after [`redact_vin`] and before a transcript leaves the machine it
/// was recorded on. Deliberately cruder than the redaction and independent of
/// it: it looks for the VIN as text; for its serial number (the part the
/// anonymous VIN zeroes) in the reply bytes, run together and per module; and
/// for all seventeen characters in order with a few bytes between each, which
/// is how a VIN looks when every line of a non-CAN reply repeats its header.
/// Lines printed without spaces are split into bytes too. A reply layout the
/// redaction does not understand is then a refusal to export rather than a VIN
/// in a public fixture. It can refuse a clean transcript whose bytes happen to
/// spell the serial, which is the cheap way to be wrong.
pub fn vin_residue(transcript: &str, vin: &str) -> Option<String> {
    let vin = vin.trim().to_ascii_uppercase();
    if vin.len() != 17 {
        return None;
    }
    if transcript.to_ascii_uppercase().contains(&vin) {
        return Some(format!("the VIN {vin} appears as text"));
    }
    let serial = &vin.as_bytes()[11..];

    let mut everything = Vec::new();
    let mut by_address: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for line in transcript.lines() {
        let Some(reply) = line.strip_prefix("< ") else { continue };
        let (address, bytes) = reply_bytes(reply);
        everything.extend_from_slice(&bytes);
        by_address.entry(address.unwrap_or_default()).or_default().extend_from_slice(&bytes);
    }

    let spells_serial = |bytes: &[u8]| bytes.windows(serial.len()).any(|w| w == serial);
    if spells_serial(&everything) {
        return Some(format!("the serial of {vin} appears in the reply bytes"));
    }
    if let Some((address, _)) = by_address.iter().find(|(_, bytes)| spells_serial(bytes)) {
        return Some(format!("the serial of {vin} appears in the replies from {address}"));
    }
    spelled_with_gaps(&everything, vin.as_bytes(), VIN_MAX_GAP)
        .then(|| format!("the VIN {vin} appears in the reply bytes, split across lines"))
}

/// The address a reply line names, if it names one, and its data bytes.
///
/// Spaced lines (`7E8 10 14 49 02 ...`) give their two-digit tokens. A line
/// printed without spaces (`7E8101449020131...`) is split into pairs, after
/// its 11-bit address when its length is odd.
fn reply_bytes(reply: &str) -> (Option<String>, Vec<u8>) {
    let tokens: Vec<&str> = reply.split_whitespace().collect();
    let hex = |t: &str| t.chars().all(|c| c.is_ascii_hexdigit());
    if let [only] = tokens.as_slice() {
        if only.len() > 3 && hex(only) {
            let (address, data) = if only.len() % 2 == 1 { only.split_at(3) } else { ("", *only) };
            let bytes = data
                .as_bytes()
                .chunks(2)
                .filter_map(|pair| std::str::from_utf8(pair).ok())
                .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
                .collect();
            let address = (!address.is_empty()).then(|| address.to_ascii_uppercase());
            return (address, bytes);
        }
    }
    let mut rest = tokens.as_slice();
    let mut address = None;
    if let Some(first) = rest.first().filter(|t| matches!(t.len(), 3 | 8) && hex(t)) {
        address = Some(first.to_ascii_uppercase());
        rest = &rest[1..];
    }
    let bytes = rest
        .iter()
        .filter(|t| t.len() == 2)
        .filter_map(|t| u8::from_str_radix(t, 16).ok())
        .collect();
    (address, bytes)
}

/// Whether `needle` appears in `haystack` in order, with at most `max_gap`
/// other bytes between each of its bytes and the next.
fn spelled_with_gaps(haystack: &[u8], needle: &[u8], max_gap: usize) -> bool {
    let Some((&first, rest)) = needle.split_first() else { return false };
    // Positions where the needle so far could have ended.
    let mut ends: Vec<usize> =
        haystack.iter().enumerate().filter(|(_, &b)| b == first).map(|(i, _)| i).collect();
    for &wanted in rest {
        let mut next = Vec::new();
        for &end in &ends {
            let window = haystack.iter().enumerate().skip(end + 1).take(max_gap + 1);
            for (i, &b) in window {
                if b == wanted && next.last() != Some(&i) {
                    next.push(i);
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        if next.is_empty() {
            return false;
        }
        ends = next;
    }
    true
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
        let reply = line.strip_prefix("< ")?;
        // A 29-bit address printed with spaces on is four separate bytes,
        // `18 DA F1 10`, not one token. Measured on a 2023 Honda Odyssey
        // through an OBDLink MX+ (2026-10-04): its VIN went unreplaced, and
        // the export was refused, because no line of it parsed as a frame.
        // Only the two ISO 15765-4 29-bit forms are taken as an address, so a
        // reply printed without headers is still not mistaken for one.
        let spaced_29_bit = ["18 DA ", "18 DB "].iter().any(|p| reply.starts_with(p));
        let (address, rest) = if spaced_29_bit && reply.len() > 11 && reply.is_char_boundary(11) {
            reply.split_at(11)
        } else {
            reply.split_once(char::is_whitespace)?
        };
        let tokens = rest.split_whitespace();
        if !spaced_29_bit
            && (!matches!(address.len(), 3 | 8) || !address.chars().all(|c| c.is_ascii_hexdigit()))
        {
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
            address: address.trim_end().to_string(),
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

    /// As an OBDLink MX+ printed a 2023 Odyssey's VIN reply: the 29-bit
    /// address as four bytes on every line. The VIN here is made up.
    #[test]
    fn a_vin_behind_a_spaced_29_bit_address_is_replaced_in_place() {
        let real = "5FNRL6H72PB123456";
        let new = anonymous_vin(real).unwrap();
        let frames = |v: &str| {
            let b: Vec<String> = v.bytes().map(|x| format!("{x:02X}")).collect();
            format!(
                "> 0902
< 18 DA F1 10 10 14 49 02 01 {}
< 18 DA F1 10 21 {}
< 18 DA F1 10 22 {}
< 18 DA F1 1E 03 41 00 00
",
                b[..3].join(" "),
                b[3..10].join(" "),
                b[10..].join(" ")
            )
        };
        assert!(new.ends_with("000000"));
        let out = redact_vin(&frames(real), real, &new);
        assert_eq!(out, frames(&new));
        assert!(vin_residue(&out, real).is_none());
        // And a reply printed without headers is still not taken for a frame.
        assert!(Frame::parse("< 49 02 01 35 46 4E").is_none());
    }

    #[test]
    fn a_reply_from_another_module_between_frames_is_not_read_as_part_of_it() {
        let text = "< 7E8 10 14 49 02 01 31 46 54\n< 7E9 03 41 00 00\n< 7E8 21 37 57 32 42 54 36 4B\n< 7E8 22 45 43 30 30 30 30 31";
        let out = redact_vin(text, VIN, NEW);
        assert!(out.contains("< 7E9 03 41 00 00"));
        assert!(out.ends_with("< 7E8 22 46 41 30 30 30 30 30"), "{out}");
    }

    #[test]
    fn anonymous_vin_creates_valid_anonymous_vin() {
        let vin = "1FT7W2BT7KEF78036";
        let result = anonymous_vin(vin).unwrap();

        // Should start with same manufacturer code
        assert_eq!(&result[..8], "1FT7W2BT");

        // Should have model year and plant code in correct positions
        assert_eq!(&result[9..11], "KE");

        // Should end with serial number replaced by zeros
        assert_eq!(&result[11..], "000000");

        // Should be a valid VIN (check digit should validate)
        assert_eq!(vin::check_digit(&result).unwrap(), result.chars().nth(8).unwrap());

        // Should differ from input
        assert_ne!(vin, result);
    }

    #[test]
    fn anonymous_vin_handles_invalid_vin() {
        let result = anonymous_vin("INVALID_VIN");
        assert!(result.is_err());
    }

    #[test]
    fn anonymous_vin_is_idempotent() {
        let vin = "1FT7W2BT7KEF78036";
        let result1 = anonymous_vin(vin).unwrap();
        let result2 = anonymous_vin(&result1).unwrap();
        assert_eq!(result1, result2);
    }

    #[test]
    fn a_redacted_transcript_has_no_residue() {
        let text = "> 0902\n< 7E8 10 14 49 02 01 31 46 54\n< 7E8 21 37 57 32 42 54 36 4B\n< 7E8 22 45 43 30 30 30 30 31\n";
        assert!(vin_residue(text, VIN).is_some());
        assert_eq!(vin_residue(&redact_vin(text, VIN, NEW), VIN), None);
    }

    #[test]
    fn a_vin_in_a_layout_the_redaction_does_not_parse_is_still_found() {
        // Headers off, CAN formatting on: the `0:` framing redact_vin leaves alone.
        let text = "> 0902\n< 014\n< 0: 49 02 01 31 46 54\n< 1: 37 57 32 42 54 36 4B\n< 2: 45 43 30 30 30 30 31\n";
        assert_eq!(redact_vin(text, VIN, NEW), text);
        assert!(vin_residue(text, VIN).is_some());
    }

    #[test]
    fn a_vin_as_text_is_residue() {
        assert!(vin_residue("# notes on 1ft7w2bt6kec00001\n", VIN).is_some());
    }

    #[test]
    fn a_vin_split_by_repeated_non_can_headers_is_found() {
        // J1850 with headers on: every line repeats `48 6B 10 49 02 nn`.
        let text = "> 0902\n< 48 6B 10 49 02 01 00 00 00 31\n< 48 6B 10 49 02 02 46 54 37 57\n\
                    < 48 6B 10 49 02 03 32 42 54 36\n< 48 6B 10 49 02 04 4B 45 43 30\n\
                    < 48 6B 10 49 02 05 30 30 30 31\n";
        assert_eq!(redact_vin(text, VIN, NEW), text, "the redaction does not parse this");
        let found = vin_residue(text, VIN).expect("found anyway");
        assert!(found.contains("split across lines"), "{found}");
        assert!(anonymise(text, VIN).is_err(), "and the export is refused");
    }

    #[test]
    fn a_vin_on_a_line_printed_without_spaces_is_found() {
        let text =
            "> 0902\n< 7E8101449020131465437\n< 7E82157324254364B45\n< 7E8224330303030310000\n";
        assert!(vin_residue(text, VIN).is_some());
    }

    #[test]
    fn anonymise_replaces_the_vin_or_refuses() {
        let text = "> 0902\n< 7E8 10 14 49 02 01 31 46 54\n< 7E8 21 37 57 32 42 54 36 4B\n< 7E8 22 45 43 30 30 30 30 31\n";
        let out = anonymise(text, VIN).unwrap();
        assert_eq!(vin_residue(&out, VIN), None);
        assert!(!out.contains("30 30 30 31"), "{out}");
    }
}
