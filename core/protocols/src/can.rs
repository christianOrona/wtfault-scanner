//! CAN frames and OBD-II addressing.
//!
//! An ELM327 hides raw CAN from us most of the time, but the concepts still
//! have to exist: `ATH1` makes the adapter print the arbitration id of every
//! response, which is how modules are told apart during a scan.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// The functional (broadcast) request identifier for ISO 15765-4 OBD-II,
/// 11-bit addressing. Every emissions-related ECU listens on it.
pub const OBD_FUNCTIONAL_REQUEST_ID: u16 = 0x7DF;

/// First physical request id in the 11-bit OBD-II range.
pub const OBD_PHYSICAL_REQUEST_BASE: u16 = 0x7E0;

/// First response id in the 11-bit OBD-II range. Response = request + 8.
pub const OBD_PHYSICAL_RESPONSE_BASE: u16 = 0x7E8;

/// Number of addressable ECUs in the standard 11-bit OBD-II range.
pub const OBD_PHYSICAL_COUNT: u16 = 8;

/// A CAN arbitration identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "width", content = "id", rename_all = "snake_case")]
pub enum CanId {
    /// 11-bit identifier.
    Standard(u16),
    /// 29-bit identifier.
    Extended(u32),
}

impl CanId {
    /// Numeric value regardless of width.
    pub fn value(&self) -> u32 {
        match self {
            CanId::Standard(v) => *v as u32,
            CanId::Extended(v) => *v,
        }
    }

    /// True for 29-bit identifiers.
    pub fn is_extended(&self) -> bool {
        matches!(self, CanId::Extended(_))
    }

    /// Hex rendering exactly as an ELM327 prints it with `ATH1`:
    /// three digits for 11-bit, eight for 29-bit.
    pub fn to_hex(&self) -> String {
        match self {
            CanId::Standard(v) => format!("{v:03X}"),
            CanId::Extended(v) => format!("{v:08X}"),
        }
    }

    /// Parse a header as printed by the adapter. Width is inferred from length:
    /// 3 hex digits means 11-bit, 8 means 29-bit.
    pub fn parse_hex(s: &str) -> AimResult<CanId> {
        let t = s.trim();
        match t.len() {
            3 => u16::from_str_radix(t, 16)
                .map(CanId::Standard)
                .map_err(|e| bad(t, &e.to_string())),
            8 => u32::from_str_radix(t, 16)
                .map(CanId::Extended)
                .map_err(|e| bad(t, &e.to_string())),
            _ => Err(bad(t, "expected 3 or 8 hex digits")),
        }
    }

    /// For an 11-bit OBD-II response id (`0x7E8`–`0x7EF`), the request id that
    /// addresses that same ECU (`0x7E0`–`0x7E7`).
    pub fn obd_response_to_request(&self) -> Option<CanId> {
        match self {
            CanId::Standard(v)
                if (OBD_PHYSICAL_RESPONSE_BASE..OBD_PHYSICAL_RESPONSE_BASE + OBD_PHYSICAL_COUNT)
                    .contains(v) =>
            {
                Some(CanId::Standard(v - OBD_PHYSICAL_COUNT))
            }
            _ => None,
        }
    }

    /// Conventional short key for a module at this address, e.g. `ECU_7E8`.
    pub fn module_key(&self) -> String {
        format!("ECU_{}", self.to_hex())
    }
}

fn bad(input: &str, why: &str) -> AimError {
    AimError::new(
        ErrorCode::ProtocolMalformedResponse,
        format!("invalid CAN identifier {input:?}: {why}"),
    )
}

/// A classical CAN frame. Payload is at most 8 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanFrame {
    /// Arbitration identifier.
    pub id: CanId,
    /// Payload, 0–8 bytes.
    pub data: Vec<u8>,
}

impl CanFrame {
    /// Build a frame, rejecting payloads longer than 8 bytes.
    pub fn new(id: CanId, data: Vec<u8>) -> AimResult<Self> {
        if data.len() > 8 {
            return Err(AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("classical CAN frame payload is {} bytes, max 8", data.len()),
            ));
        }
        Ok(CanFrame { id, data })
    }

    /// Pad the payload to 8 bytes with `fill`, as ISO 15765-4 requires for CAN.
    pub fn padded(mut self, fill: u8) -> Self {
        while self.data.len() < 8 {
            self.data.push(fill);
        }
        self
    }

    /// Render as an ELM327 header + payload line: `7E8 03 41 0C 1A F8`.
    pub fn to_elm_line(&self) -> String {
        let mut s = self.id.to_hex();
        for b in &self.data {
            s.push(' ');
            s.push_str(&format!("{b:02X}"));
        }
        s
    }

    /// Parse a line the adapter printed with headers enabled.
    ///
    /// Accepts both spaced (`7E8 03 41 0C 1A F8`) and unspaced
    /// (`7E803410C1AF8` is *not* accepted — ambiguity is rejected rather than
    /// guessed; the adapter is configured with `ATS0` off for this reason).
    pub fn parse_elm_line(line: &str) -> AimResult<CanFrame> {
        let mut parts = line.split_whitespace();
        let header = parts.next().ok_or_else(|| bad(line, "empty line"))?;
        let id = CanId::parse_hex(header)?;
        let mut data = Vec::new();
        for p in parts {
            let byte = u8::from_str_radix(p, 16).map_err(|e| {
                AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("invalid data byte {p:?} in {line:?}: {e}"),
                )
            })?;
            data.push(byte);
        }
        CanFrame::new(id, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_ids_render_as_three_digits() {
        assert_eq!(CanId::Standard(0x7E8).to_hex(), "7E8");
        assert_eq!(CanId::Standard(0x0DF).to_hex(), "0DF");
        assert_eq!(CanId::Extended(0x18DAF110).to_hex(), "18DAF110");
    }

    #[test]
    fn header_width_selects_the_id_type() {
        assert_eq!(CanId::parse_hex("7E8").unwrap(), CanId::Standard(0x7E8));
        assert_eq!(
            CanId::parse_hex("18DAF110").unwrap(),
            CanId::Extended(0x18DAF110)
        );
        assert_eq!(
            CanId::parse_hex("7E").unwrap_err().code,
            ErrorCode::ProtocolMalformedResponse
        );
        assert!(CanId::parse_hex("ZZZ").is_err());
    }

    #[test]
    fn response_ids_map_back_to_request_ids() {
        for i in 0..8u16 {
            let resp = CanId::Standard(OBD_PHYSICAL_RESPONSE_BASE + i);
            assert_eq!(
                resp.obd_response_to_request(),
                Some(CanId::Standard(OBD_PHYSICAL_REQUEST_BASE + i))
            );
        }
        // Outside the OBD range there is no defined mapping.
        assert_eq!(CanId::Standard(0x123).obd_response_to_request(), None);
        assert_eq!(CanId::Extended(0x18DAF110).obd_response_to_request(), None);
    }

    #[test]
    fn frames_reject_oversized_payloads() {
        assert!(CanFrame::new(CanId::Standard(0x7E0), vec![0; 9]).is_err());
        assert!(CanFrame::new(CanId::Standard(0x7E0), vec![0; 8]).is_ok());
    }

    #[test]
    fn elm_lines_round_trip() {
        let f = CanFrame::new(CanId::Standard(0x7E8), vec![0x03, 0x41, 0x0C, 0x1A, 0xF8]).unwrap();
        let line = f.to_elm_line();
        assert_eq!(line, "7E8 03 41 0C 1A F8");
        assert_eq!(CanFrame::parse_elm_line(&line).unwrap(), f);
    }

    #[test]
    fn padding_fills_to_eight_bytes() {
        let f = CanFrame::new(CanId::Standard(0x7DF), vec![0x02, 0x01, 0x0C])
            .unwrap()
            .padded(0x55);
        assert_eq!(f.data, vec![0x02, 0x01, 0x0C, 0x55, 0x55, 0x55, 0x55, 0x55]);
    }

    #[test]
    fn module_keys_are_derived_from_the_address() {
        assert_eq!(CanId::Standard(0x7E8).module_key(), "ECU_7E8");
    }
}
