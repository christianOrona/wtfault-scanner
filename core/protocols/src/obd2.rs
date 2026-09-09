//! SAE J1979 (OBD-II) diagnostic services.
//!
//! This module builds requests and classifies responses. It never assigns
//! *meaning* to a payload — `41 05 5A` is "positive response, service 01, PID
//! 0x05, payload `5A`" and nothing more. Scaling `5A` into 50 °C is the
//! decoder's job, driven by data files.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// Standardised OBD-II services (called "modes" in the older literature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    /// 0x01 — current powertrain data.
    CurrentData,
    /// 0x02 — freeze frame data.
    FreezeFrame,
    /// 0x03 — stored (confirmed) DTCs.
    StoredDtcs,
    /// 0x04 — clear DTCs and stored values. Gated off by the safety layer.
    ClearDtcs,
    /// 0x06 — on-board monitoring test results.
    MonitorResults,
    /// 0x07 — pending DTCs.
    PendingDtcs,
    /// 0x09 — vehicle information (VIN, CALID, CVN, ECU name).
    VehicleInfo,
    /// 0x0A — permanent DTCs.
    PermanentDtcs,
}

impl Service {
    /// Service identifier byte.
    pub fn id(&self) -> u8 {
        match self {
            Service::CurrentData => 0x01,
            Service::FreezeFrame => 0x02,
            Service::StoredDtcs => 0x03,
            Service::ClearDtcs => 0x04,
            Service::MonitorResults => 0x06,
            Service::PendingDtcs => 0x07,
            Service::VehicleInfo => 0x09,
            Service::PermanentDtcs => 0x0A,
        }
    }

    /// Positive-response service byte (request + 0x40).
    pub fn response_id(&self) -> u8 {
        self.id() + 0x40
    }

    /// Look up a service by its identifier byte.
    pub fn from_id(id: u8) -> Option<Service> {
        Some(match id {
            0x01 => Service::CurrentData,
            0x02 => Service::FreezeFrame,
            0x03 => Service::StoredDtcs,
            0x04 => Service::ClearDtcs,
            0x06 => Service::MonitorResults,
            0x07 => Service::PendingDtcs,
            0x09 => Service::VehicleInfo,
            0x0A => Service::PermanentDtcs,
            _ => return None,
        })
    }

    /// Whether this service changes vehicle state. Only 0x04 does.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Service::ClearDtcs)
    }
}

/// An OBD-II request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObdRequest {
    /// Service to invoke.
    pub service: Service,
    /// PID / info-type, for services that take one.
    pub pid: Option<u8>,
    /// Extra bytes, e.g. the freeze-frame number for service 02.
    pub extra: Vec<u8>,
}

impl ObdRequest {
    /// Service 01 current-data request for `pid`.
    pub fn current_data(pid: u8) -> Self {
        ObdRequest { service: Service::CurrentData, pid: Some(pid), extra: Vec::new() }
    }

    /// Service 02 freeze-frame request for `pid` in frame `frame`.
    pub fn freeze_frame(pid: u8, frame: u8) -> Self {
        ObdRequest { service: Service::FreezeFrame, pid: Some(pid), extra: vec![frame] }
    }

    /// Service 09 vehicle-information request for `info_type`.
    pub fn vehicle_info(info_type: u8) -> Self {
        ObdRequest { service: Service::VehicleInfo, pid: Some(info_type), extra: Vec::new() }
    }

    /// Service 06 on-board monitor test results for one monitor id.
    ///
    /// `0x00` asks which monitor ids are supported, exactly like PID 00 does
    /// for service 01.
    pub fn monitor_results(mid: u8) -> Self {
        ObdRequest { service: Service::MonitorResults, pid: Some(mid), extra: Vec::new() }
    }

    /// A request carrying only a service byte (03, 04, 07, 0A).
    pub fn bare(service: Service) -> Self {
        ObdRequest { service, pid: None, extra: Vec::new() }
    }

    /// Serialize to request bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.service.id()];
        if let Some(pid) = self.pid {
            v.push(pid);
        }
        v.extend_from_slice(&self.extra);
        v
    }

    /// Serialize to the uppercase hex line an ELM327 expects.
    pub fn to_elm_command(&self) -> String {
        self.to_bytes().iter().map(|b| format!("{b:02X}")).collect()
    }
}

/// A parsed OBD-II response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObdResponse {
    /// A positive response.
    Positive {
        /// Service identifier from the response (already minus 0x40).
        service: u8,
        /// Echoed PID / info-type, when the service uses one.
        pid: Option<u8>,
        /// Payload after the service and PID bytes.
        data: Vec<u8>,
    },
    /// A negative response (`7F <service> <nrc>`).
    Negative {
        /// Service that was rejected.
        service: u8,
        /// Negative response code.
        nrc: crate::uds::NegativeResponseCode,
    },
}

impl ObdResponse {
    /// Parse a response PDU.
    ///
    /// `expects_pid` tells the parser whether to peel a PID byte off the front
    /// of the payload; services 03/07/0A do not echo one.
    pub fn parse(bytes: &[u8], expects_pid: bool) -> AimResult<ObdResponse> {
        let first = *bytes.first().ok_or_else(|| {
            AimError::new(ErrorCode::ProtocolMalformedResponse, "empty OBD response")
        })?;
        if first == 0x7F {
            if bytes.len() < 3 {
                return Err(AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("truncated negative response: {}", aim_types::hex(bytes)),
                ));
            }
            return Ok(ObdResponse::Negative {
                service: bytes[1],
                nrc: crate::uds::NegativeResponseCode::from_byte(bytes[2]),
            });
        }
        if first < 0x40 {
            return Err(AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!(
                    "0x{first:02X} is neither a positive response (>=0x40) nor 0x7F: {}",
                    aim_types::hex(bytes)
                ),
            ));
        }
        let service = first - 0x40;
        if expects_pid {
            let pid = *bytes.get(1).ok_or_else(|| {
                AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("service 0x{service:02X} response has no PID byte"),
                )
            })?;
            Ok(ObdResponse::Positive { service, pid: Some(pid), data: bytes[2..].to_vec() })
        } else {
            Ok(ObdResponse::Positive { service, pid: None, data: bytes[1..].to_vec() })
        }
    }

    /// Check that this response answers `request`, returning its payload.
    ///
    /// A response for a different service or PID is [`ErrorCode::UnexpectedResponse`]
    /// rather than being quietly accepted — mismatches usually mean a stale
    /// reply from a previous command is being read.
    pub fn payload_for(&self, request: &ObdRequest) -> AimResult<&[u8]> {
        match self {
            ObdResponse::Negative { service, nrc } => Err(AimError::new(
                ErrorCode::NegativeResponse,
                format!(
                    "service 0x{service:02X} rejected: {} (0x{:02X})",
                    nrc.description(),
                    nrc.byte()
                ),
            )
            .with_details(serde_json::json!({
                "service": service,
                "nrc": nrc.byte(),
                "nrc_name": nrc.name(),
            }))),
            ObdResponse::Positive { service, pid, data } => {
                if *service != request.service.id() {
                    return Err(AimError::new(
                        ErrorCode::UnexpectedResponse,
                        format!(
                            "asked service 0x{:02X}, got 0x{service:02X}",
                            request.service.id()
                        ),
                    ));
                }
                if request.pid.is_some() && *pid != request.pid {
                    return Err(AimError::new(
                        ErrorCode::UnexpectedResponse,
                        format!("asked PID {:02X?}, got {pid:02X?}", request.pid),
                    ));
                }
                Ok(data)
            }
        }
    }
}

/// Decode a 2-byte DTC field into its SAE J2012 string form, e.g. `P0401`.
///
/// The top two bits select the system letter and the next two are the first
/// digit; the remaining twelve bits are three hex digits.
/// One on-board monitor test result, exactly as the vehicle reported it.
///
/// Service 06 is the most useful thing on the bus for seeing a component on its
/// way out: it reports each self-test's **measured value against the limit it is
/// judged by**. A catalyst reading 0.58 against a 0.60 limit has passed and is
/// about to stop passing, and no trouble code will say so until it does.
///
/// Nothing here is interpreted. The raw counts are kept alongside anything
/// scaled, because the scaling table has not been validated against a real
/// vehicle by this project and the pass/fail comparison does not depend on it:
/// value, min and max all arrive in the same units, whatever those units are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorTest {
    /// On-board monitor id — which system was tested.
    pub mid: u8,
    /// Test id within that monitor.
    pub tid: u8,
    /// Unit and scaling id. Says how to read the three raw values.
    pub uasid: u8,
    /// Raw measured value.
    pub value: u16,
    /// Raw lower limit.
    pub min: u16,
    /// Raw upper limit.
    pub max: u16,
}

impl MonitorTest {
    /// Whether the measured value sits within its limits.
    ///
    /// Scaling-independent: all three numbers share whatever unit the UASID
    /// declares, so this is correct even when that unit is unknown.
    ///
    /// Signed scalings are the one caveat — for those the raw comparison can
    /// mislead, so callers that know the UASID is signed should say so rather
    /// than trusting this. Every unsigned scaling, which is nearly all of them,
    /// compares correctly here.
    pub fn passed(&self) -> bool {
        self.value >= self.min && self.value <= self.max
    }

    /// How much room is left before the test fails, as a fraction of the limit
    /// band, or `None` when the band is degenerate.
    ///
    /// This is the number worth showing. `0.03` means the component is within
    /// three percent of failing — which is a warning a trouble code will not
    /// give you for months.
    pub fn margin(&self) -> Option<f64> {
        let (lo, hi) = (self.min as f64, self.max as f64);
        let band = hi - lo;
        if band <= 0.0 {
            return None;
        }
        let v = self.value as f64;
        // Distance to the nearer limit, normalised by the band.
        Some(((v - lo).min(hi - v) / band).max(0.0))
    }
}

/// Parse a service 06 payload into its test results.
///
/// On CAN each result is nine bytes: monitor id, test id, unit-and-scaling id,
/// then the measured value, lower limit and upper limit as big-endian pairs. A
/// trailing partial record is ignored rather than guessed at.
pub fn decode_monitor_tests(data: &[u8]) -> Vec<MonitorTest> {
    data.chunks_exact(9)
        .map(|c| MonitorTest {
            mid: c[0],
            tid: c[1],
            uasid: c[2],
            value: u16::from_be_bytes([c[3], c[4]]),
            min: u16::from_be_bytes([c[5], c[6]]),
            max: u16::from_be_bytes([c[7], c[8]]),
        })
        .collect()
}

/// Decode a service 06 response that has already been through
/// [`ObdResponse::parse`].
///
/// This exists because service 06 does not really echo a PID. The parser peels
/// the byte after `0x46` off as an echoed pid — correct for services 01, 02 and
/// 09 — but in service 06 that byte is the first record's own MID field. Put it
/// back and the whole payload chunks cleanly into 9-byte records.
///
/// Returns the tests along with the number of trailing bytes that did not form
/// a whole record, so a truncated reply is reported rather than silently
/// shortened.
pub fn decode_monitor_response(echoed_mid: u8, data: &[u8]) -> (Vec<MonitorTest>, usize) {
    let mut records = Vec::with_capacity(data.len() + 1);
    records.push(echoed_mid);
    records.extend_from_slice(data);
    (decode_monitor_tests(&records), records.len() % 9)
}

/// Format a two-byte DTC as its SAE J2012 string, e.g. `P0301`.
///
/// The top two bits select the system letter and the next two the first digit;
/// the remaining twelve bits are three hex digits. This is pure structure — it
/// says nothing about what the code means.
pub fn decode_dtc(hi: u8, lo: u8) -> String {
    const SYSTEM: [char; 4] = ['P', 'C', 'B', 'U'];
    let letter = SYSTEM[(hi >> 6) as usize];
    let d1 = (hi >> 4) & 0x03;
    let d2 = hi & 0x0F;
    let d3 = lo >> 4;
    let d4 = lo & 0x0F;
    format!("{letter}{d1}{d2:X}{d3:X}{d4:X}")
}

/// Encode a DTC string back into its 2-byte wire form.
pub fn encode_dtc(code: &str) -> AimResult<[u8; 2]> {
    let bytes: Vec<char> = code.trim().to_ascii_uppercase().chars().collect();
    if bytes.len() != 5 {
        return Err(AimError::new(
            ErrorCode::BadRequest,
            format!("DTC {code:?} must be a letter followed by 4 hex digits"),
        ));
    }
    let system = match bytes[0] {
        'P' => 0u8,
        'C' => 1,
        'B' => 2,
        'U' => 3,
        other => {
            return Err(AimError::new(
                ErrorCode::BadRequest,
                format!("DTC system letter {other:?} must be one of P, C, B, U"),
            ))
        }
    };
    let mut digits = [0u8; 4];
    for (i, c) in bytes[1..].iter().enumerate() {
        digits[i] = c.to_digit(16).ok_or_else(|| {
            AimError::new(
                ErrorCode::BadRequest,
                format!("DTC {code:?} contains non-hex digit {c:?}"),
            )
        })? as u8;
    }
    if digits[0] > 3 {
        return Err(AimError::new(
            ErrorCode::BadRequest,
            format!("DTC {code:?}: the first digit must be 0-3"),
        ));
    }
    Ok([(system << 6) | (digits[0] << 4) | digits[1], (digits[2] << 4) | digits[3]])
}

/// Split a service 03/07/0A payload into DTC strings.
///
/// All-zero pairs are padding and are dropped. An odd trailing byte is an
/// error, not something to silently ignore.
pub fn decode_dtc_list(data: &[u8]) -> AimResult<Vec<String>> {
    if data.len() % 2 != 0 {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("DTC payload has odd length {}: {}", data.len(), aim_types::hex(data)),
        ));
    }
    Ok(data
        .chunks_exact(2)
        .filter(|p| p != &[0x00, 0x00])
        .map(|p| decode_dtc(p[0], p[1]))
        .collect())
}

/// Split the optional DTC-count byte off a service 03/07/0A payload.
///
/// ISO 15765-4 puts a count of reported codes between the response service
/// byte and the codes themselves; the pre-CAN protocols do not. Rather than
/// asking the caller to know which protocol produced the bytes, the two cases
/// are told apart by parity, which is exact: a payload with a count byte is
/// `1 + 2n` bytes and therefore odd, one without is `2n` and therefore even.
///
/// Returns the code bytes and the count the ECU claimed, when it claimed one.
/// A disagreement between the claimed count and the codes actually present is
/// left for the caller to notice — it is evidence, not something to paper over.
pub fn strip_dtc_count(data: &[u8]) -> (&[u8], Option<u8>) {
    if data.len() % 2 == 1 {
        (&data[1..], Some(data[0]))
    } else {
        (data, None)
    }
}

/// Expand a 4-byte supported-PID bitmask into the PID numbers it reports.
///
/// `base` is the PID that carried the mask (0x00, 0x20, 0x40, ...). Bit 31 of
/// the mask is `base + 1`, bit 0 is `base + 32`.
pub fn decode_supported_pids(base: u8, mask: &[u8]) -> AimResult<Vec<u8>> {
    if mask.len() < 4 {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("supported-PID mask must be 4 bytes, got {}", mask.len()),
        ));
    }
    let bits = u32::from_be_bytes([mask[0], mask[1], mask[2], mask[3]]);
    let mut out = Vec::new();
    for i in 0..32u8 {
        if bits & (1 << (31 - i)) != 0 {
            out.push(base.wrapping_add(i + 1));
        }
    }
    Ok(out)
}

/// Build a 4-byte supported-PID bitmask from a list of PIDs (inverse of
/// [`decode_supported_pids`]). Used by the simulator and by tests.
pub fn encode_supported_pids(base: u8, pids: &[u8]) -> [u8; 4] {
    let mut bits: u32 = 0;
    for pid in pids {
        let offset = pid.wrapping_sub(base);
        if (1..=32).contains(&offset) {
            bits |= 1 << (32 - offset);
        }
    }
    bits.to_be_bytes()
}

/// Assemble the VIN from a service 09 PID 02 payload.
///
/// The payload begins with a message-count byte, then 17 ASCII characters.
/// Some ECUs pad with `0x00` on the left; those are stripped.
pub fn decode_vin(data: &[u8]) -> AimResult<String> {
    // Drop the leading NODI (number of data items) byte when present.
    let body = if data.len() == 18 || data.len() == 20 { &data[1..] } else { data };
    let text: String = body.iter().copied().filter(|b| *b != 0x00).map(|b| b as char).collect();
    let vin: String = text.trim().to_string();
    if vin.len() != 17 {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("VIN payload decoded to {} characters, expected 17: {vin:?}", vin.len()),
        ));
    }
    if !vin.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(AimError::new(
            ErrorCode::ProtocolMalformedResponse,
            format!("VIN {vin:?} contains non-alphanumeric characters"),
        ));
    }
    Ok(vin)
}

/// Decode a service 09 payload that carries one or more fixed-width ASCII
/// strings (PID 04 CALID at 16 bytes each, PID 0A ECU name at 20).
pub fn decode_ascii_records(data: &[u8], width: usize) -> Vec<String> {
    let body = if !data.is_empty() && data.len() % width == 1 { &data[1..] } else { data };
    body.chunks(width)
        .map(|chunk| {
            chunk
                .iter()
                .copied()
                .filter(|b| (0x20..0x7F).contains(b))
                .map(|b| b as char)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uds::NegativeResponseCode;

    #[test]
    fn requests_serialize_to_elm_commands() {
        assert_eq!(ObdRequest::current_data(0x0C).to_elm_command(), "010C");
        assert_eq!(ObdRequest::bare(Service::StoredDtcs).to_elm_command(), "03");
        assert_eq!(ObdRequest::vehicle_info(0x02).to_elm_command(), "0902");
        assert_eq!(ObdRequest::freeze_frame(0x0C, 0x00).to_elm_command(), "020C00");
    }

    #[test]
    fn service_ids_and_response_ids_line_up() {
        for s in [
            Service::CurrentData,
            Service::FreezeFrame,
            Service::StoredDtcs,
            Service::ClearDtcs,
            Service::PendingDtcs,
            Service::VehicleInfo,
            Service::PermanentDtcs,
        ] {
            assert_eq!(Service::from_id(s.id()), Some(s));
            assert_eq!(s.response_id(), s.id() + 0x40);
        }
        assert_eq!(Service::from_id(0x22), None);
        assert!(Service::ClearDtcs.is_mutating());
        assert!(!Service::CurrentData.is_mutating());
    }

    #[test]
    fn positive_responses_split_service_pid_and_payload() {
        let r = ObdResponse::parse(&[0x41, 0x0C, 0x1A, 0xF8], true).unwrap();
        assert_eq!(
            r,
            ObdResponse::Positive { service: 0x01, pid: Some(0x0C), data: vec![0x1A, 0xF8] }
        );
        let payload = r.payload_for(&ObdRequest::current_data(0x0C)).unwrap();
        assert_eq!(payload, &[0x1A, 0xF8]);
    }

    #[test]
    fn services_without_a_pid_keep_the_whole_payload() {
        let r = ObdResponse::parse(&[0x43, 0x01, 0x43, 0x01, 0x33], false).unwrap();
        assert_eq!(
            r,
            ObdResponse::Positive { service: 0x03, pid: None, data: vec![0x01, 0x43, 0x01, 0x33] }
        );
    }

    #[test]
    fn negative_responses_surface_as_errors_with_the_nrc() {
        let r = ObdResponse::parse(&[0x7F, 0x01, 0x12], true).unwrap();
        assert_eq!(
            r,
            ObdResponse::Negative {
                service: 0x01,
                nrc: NegativeResponseCode::SubFunctionNotSupported
            }
        );
        let err = r.payload_for(&ObdRequest::current_data(0x0C)).unwrap_err();
        assert_eq!(err.code, ErrorCode::NegativeResponse);
        assert_eq!(err.details.unwrap()["nrc"], 0x12);
    }

    #[test]
    fn a_response_for_the_wrong_pid_is_not_accepted() {
        let r = ObdResponse::parse(&[0x41, 0x05, 0x5A], true).unwrap();
        let err = r.payload_for(&ObdRequest::current_data(0x0C)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnexpectedResponse);
    }

    #[test]
    fn a_response_for_the_wrong_service_is_not_accepted() {
        let r = ObdResponse::parse(&[0x43, 0x01, 0x33], false).unwrap();
        let err = r.payload_for(&ObdRequest::bare(Service::PendingDtcs)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnexpectedResponse);
    }

    #[test]
    fn malformed_responses_are_rejected() {
        assert!(ObdResponse::parse(&[], true).is_err());
        assert!(ObdResponse::parse(&[0x7F, 0x01], true).is_err());
        assert!(ObdResponse::parse(&[0x01, 0x0C], true).is_err(), "request echoed as response");
        assert!(ObdResponse::parse(&[0x41], true).is_err(), "no PID byte");
    }

    #[test]
    fn dtc_encoding_round_trips_for_every_system_letter() {
        for (bytes, code) in [
            ([0x01u8, 0x33u8], "P0133"),
            ([0x04, 0x01], "P0401"),
            ([0x22, 0x63], "P2263"),
            ([0x43, 0x21], "C0321"),
            ([0x81, 0x23], "B0123"),
            ([0xC1, 0x00], "U0100"),
            ([0xD1, 0xFF], "U11FF"),
        ] {
            assert_eq!(decode_dtc(bytes[0], bytes[1]), code, "{code}");
            assert_eq!(encode_dtc(code).unwrap(), bytes, "{code}");
        }
    }

    #[test]
    fn invalid_dtc_strings_are_rejected() {
        assert!(encode_dtc("X0401").is_err());
        assert!(encode_dtc("P040").is_err());
        assert!(encode_dtc("P04G1").is_err());
        assert!(encode_dtc("P9401").is_err(), "first digit must be 0-3");
    }

    #[test]
    fn the_dtc_count_byte_is_separated_by_parity() {
        // CAN form: 43 02 <two codes> -> data is 02 01 43 01 96, odd.
        let (codes, count) = strip_dtc_count(&[0x02, 0x01, 0x43, 0x01, 0x96]);
        assert_eq!(count, Some(2));
        assert_eq!(decode_dtc_list(codes).unwrap(), vec!["P0143", "P0196"]);

        // No codes at all, CAN form.
        let (codes, count) = strip_dtc_count(&[0x00]);
        assert_eq!(count, Some(0));
        assert!(codes.is_empty());

        // Pre-CAN form: no count byte, even length.
        let (codes, count) = strip_dtc_count(&[0x01, 0x43]);
        assert_eq!(count, None);
        assert_eq!(decode_dtc_list(codes).unwrap(), vec!["P0143"]);

        let (codes, count) = strip_dtc_count(&[]);
        assert_eq!(count, None);
        assert!(codes.is_empty());
    }

    #[test]
    fn dtc_lists_drop_padding_and_reject_odd_lengths() {
        let list = decode_dtc_list(&[0x01, 0x33, 0x04, 0x01, 0x00, 0x00]).unwrap();
        assert_eq!(list, vec!["P0133", "P0401"]);
        assert!(decode_dtc_list(&[]).unwrap().is_empty());
        assert!(decode_dtc_list(&[0x01, 0x33, 0x04]).is_err());
    }

    #[test]
    fn supported_pid_masks_round_trip() {
        // The classic 0x00 response: PIDs 01,03,04,05,06,07,0C,0D,0E,0F,10,11,1C,1F,20
        let pids = vec![
            0x01, 0x03, 0x04, 0x05, 0x06, 0x07, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x1C, 0x1F,
            0x20,
        ];
        let mask = encode_supported_pids(0x00, &pids);
        assert_eq!(decode_supported_pids(0x00, &mask).unwrap(), pids);
    }

    #[test]
    fn supported_pid_bit_order_is_msb_first() {
        // 0x80000000 means only PID base+1 is supported.
        assert_eq!(decode_supported_pids(0x00, &[0x80, 0x00, 0x00, 0x00]).unwrap(), vec![0x01]);
        // 0x00000001 means only PID base+32.
        assert_eq!(decode_supported_pids(0x00, &[0x00, 0x00, 0x00, 0x01]).unwrap(), vec![0x20]);
        assert_eq!(decode_supported_pids(0x20, &[0x00, 0x00, 0x00, 0x01]).unwrap(), vec![0x40]);
        assert!(decode_supported_pids(0x00, &[0x00]).is_err());
    }

    #[test]
    fn vin_is_assembled_and_validated() {
        let mut payload = vec![0x01];
        payload.extend_from_slice(b"1FT7W2BT6KEC00001");
        assert_eq!(decode_vin(&payload).unwrap(), "1FT7W2BT6KEC00001");
        // Left-padded form seen on some ECUs.
        let mut padded = vec![0x01, 0x00, 0x00];
        padded.extend_from_slice(b"1FT7W2BT6KEC00001");
        assert_eq!(decode_vin(&padded).unwrap(), "1FT7W2BT6KEC00001");
        assert!(decode_vin(b"TOOSHORT").is_err());
        assert!(decode_vin(b"1FT7W2BT9KEC0000!").is_err());
    }

    #[test]
    fn ascii_records_are_split_on_fixed_widths() {
        let mut payload = vec![0x01];
        payload.extend_from_slice(b"CAL-ID-EXAMPLE\0\0");
        assert_eq!(decode_ascii_records(&payload, 16), vec![String::from("CAL-ID-EXAMPLE")]);
    }
}

#[cfg(test)]
mod monitor_tests {
    use super::*;

    #[test]
    fn a_real_frame_survives_the_parser_stripping_the_echoed_mid() {
        // 46 21 ... — the byte after the service is the first record's MID, not
        // a separate echo. Two whole records here.
        let frame = [
            0x46, //
            0x21, 0x80, 0x0B, 0x02, 0x44, 0x00, 0x00, 0x02, 0x58, //
            0x21, 0x81, 0x0B, 0x00, 0xC8, 0x00, 0x00, 0x02, 0x58,
        ];
        let parsed = ObdResponse::parse(&frame, true).unwrap();
        let ObdResponse::Positive { pid, data, .. } = parsed else {
            panic!("expected a positive response");
        };
        let (tests, leftover) = decode_monitor_response(pid.unwrap(), &data);
        assert_eq!(leftover, 0);
        assert_eq!(tests.len(), 2, "the echoed byte belongs to record one");
        assert_eq!(tests[0].mid, 0x21);
        assert_eq!(tests[0].tid, 0x80);
        assert_eq!(tests[0].value, 0x0244);
        assert_eq!(tests[0].max, 0x0258);
        assert!(tests[0].passed());
        assert_eq!(tests[1].tid, 0x81);
    }

    #[test]
    fn a_truncated_reply_reports_its_leftover_instead_of_hiding_it() {
        // One whole record plus four stray bytes.
        let data = [
            0x80, 0x0B, 0x02, 0x44, 0x00, 0x00, 0x02, 0x58, // finishes record one
            0x22, 0x80, 0x0B, 0x01, // and then stops mid-record
        ];
        let (tests, leftover) = decode_monitor_response(0x21, &data);
        assert_eq!(tests.len(), 1);
        assert_eq!(leftover, 4);
    }

    #[test]
    fn a_service_06_payload_decodes_into_records() {
        // Two results: catalyst bank 1 (MID 0x21) and EGR (MID 0x31).
        let data = [
            0x21, 0x80, 0x0B, 0x00, 0x3A, 0x00, 0x00, 0x00, 0x3C, // value 58, max 60
            0x31, 0x01, 0x0B, 0x00, 0x50, 0x00, 0x20, 0x00, 0x80,
        ];
        let tests = decode_monitor_tests(&data);
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].mid, 0x21);
        assert_eq!(tests[0].tid, 0x80);
        assert_eq!(tests[0].value, 58);
        assert_eq!(tests[0].max, 60);
        assert_eq!(tests[1].mid, 0x31);
    }

    #[test]
    fn a_truncated_trailing_record_is_dropped_not_guessed() {
        // A short final chunk would otherwise be read as zeroes and presented
        // as a real measurement.
        let data = [0x21, 0x80, 0x0B, 0x00, 0x3A, 0x00, 0x00, 0x00, 0x3C, 0x31, 0x01];
        assert_eq!(decode_monitor_tests(&data).len(), 1);
        assert!(decode_monitor_tests(&[0x21, 0x80]).is_empty());
    }

    #[test]
    fn pass_and_fail_are_decided_by_the_vehicles_own_limits() {
        let t =
            |v, lo, hi| MonitorTest { mid: 0x21, tid: 1, uasid: 0x0B, value: v, min: lo, max: hi };
        assert!(t(58, 0, 60).passed());
        assert!(t(60, 0, 60).passed(), "a value on the limit still passes");
        assert!(!t(61, 0, 60).passed());
        assert!(!t(5, 10, 60).passed(), "below the lower limit fails too");
    }

    #[test]
    fn the_margin_says_how_close_to_failing_a_component_is() {
        let t = |v| MonitorTest { mid: 0x21, tid: 1, uasid: 0x0B, value: v, min: 0, max: 100 };
        // Right in the middle: half the band away from either limit.
        assert_eq!(t(50).margin(), Some(0.5));
        // Nearly at the upper limit - the case worth warning about.
        assert_eq!(t(97).margin(), Some(0.03));
        // Outside the limits clamps to zero rather than going negative.
        assert_eq!(t(120).margin(), Some(0.0));
    }

    #[test]
    fn a_degenerate_limit_band_has_no_margin_rather_than_dividing_by_zero() {
        let t = MonitorTest { mid: 1, tid: 1, uasid: 1, value: 5, min: 10, max: 10 };
        assert_eq!(t.margin(), None);
    }

    #[test]
    fn the_request_asks_the_right_service() {
        assert_eq!(ObdRequest::monitor_results(0x21).to_elm_command(), "0621");
        // MID 0 is the supported-monitor bitmask, like PID 00 in service 01.
        assert_eq!(ObdRequest::monitor_results(0x00).to_elm_command(), "0600");
    }
}
