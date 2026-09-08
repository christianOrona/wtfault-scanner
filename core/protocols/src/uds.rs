//! ISO 14229 (UDS) request / response semantics — scaffold.
//!
//! No OEM services are implemented and none are invented. What exists here is
//! the part that must be correct *before* any UDS service is used: how a
//! request is framed, how a positive response is recognised, how a negative
//! response is decoded, and how `requestCorrectlyReceived-ResponsePending`
//! (NRC 0x78) changes the caller's timing expectations.
//!
//! Everything here is exercised by unit tests; nothing here is wired to a
//! transport in this build.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// UDS service identifiers this scaffold knows by name.
///
/// Unknown identifiers are preserved as [`UdsService::Other`] rather than
/// rejected, so a transcript containing an OEM service can still be parsed and
/// recorded — but nothing will *issue* one without an allowlisted capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UdsService {
    /// 0x10 DiagnosticSessionControl.
    DiagnosticSessionControl,
    /// 0x11 ECUReset.
    EcuReset,
    /// 0x14 ClearDiagnosticInformation.
    ClearDiagnosticInformation,
    /// 0x19 ReadDTCInformation.
    ReadDtcInformation,
    /// 0x22 ReadDataByIdentifier.
    ReadDataByIdentifier,
    /// 0x27 SecurityAccess. Never used to circumvent vehicle security.
    SecurityAccess,
    /// 0x2E WriteDataByIdentifier.
    WriteDataByIdentifier,
    /// 0x2F InputOutputControlByIdentifier.
    InputOutputControlByIdentifier,
    /// 0x31 RoutineControl.
    RoutineControl,
    /// 0x3E TesterPresent.
    TesterPresent,
    /// Any other service identifier.
    Other(u8),
}

impl UdsService {
    /// Service identifier byte.
    pub fn id(&self) -> u8 {
        match self {
            UdsService::DiagnosticSessionControl => 0x10,
            UdsService::EcuReset => 0x11,
            UdsService::ClearDiagnosticInformation => 0x14,
            UdsService::ReadDtcInformation => 0x19,
            UdsService::ReadDataByIdentifier => 0x22,
            UdsService::SecurityAccess => 0x27,
            UdsService::WriteDataByIdentifier => 0x2E,
            UdsService::InputOutputControlByIdentifier => 0x2F,
            UdsService::RoutineControl => 0x31,
            UdsService::TesterPresent => 0x3E,
            UdsService::Other(id) => *id,
        }
    }

    /// Classify an identifier byte.
    pub fn from_id(id: u8) -> UdsService {
        match id {
            0x10 => UdsService::DiagnosticSessionControl,
            0x11 => UdsService::EcuReset,
            0x14 => UdsService::ClearDiagnosticInformation,
            0x19 => UdsService::ReadDtcInformation,
            0x22 => UdsService::ReadDataByIdentifier,
            0x27 => UdsService::SecurityAccess,
            0x2E => UdsService::WriteDataByIdentifier,
            0x2F => UdsService::InputOutputControlByIdentifier,
            0x31 => UdsService::RoutineControl,
            0x3E => UdsService::TesterPresent,
            other => UdsService::Other(other),
        }
    }

    /// Positive-response identifier (request + 0x40).
    pub fn response_id(&self) -> u8 {
        self.id().wrapping_add(0x40)
    }

    /// Whether this service can change ECU state or configuration. Used by the
    /// safety layer to refuse anything that is not plainly a read.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            UdsService::EcuReset
                | UdsService::ClearDiagnosticInformation
                | UdsService::WriteDataByIdentifier
                | UdsService::InputOutputControlByIdentifier
                | UdsService::RoutineControl
                | UdsService::SecurityAccess
        ) || matches!(self, UdsService::Other(_))
    }
}

/// ISO 14229 negative response codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegativeResponseCode {
    /// 0x10 generalReject.
    GeneralReject,
    /// 0x11 serviceNotSupported.
    ServiceNotSupported,
    /// 0x12 subFunctionNotSupported.
    SubFunctionNotSupported,
    /// 0x13 incorrectMessageLengthOrInvalidFormat.
    IncorrectMessageLengthOrInvalidFormat,
    /// 0x14 responseTooLong.
    ResponseTooLong,
    /// 0x21 busyRepeatRequest.
    BusyRepeatRequest,
    /// 0x22 conditionsNotCorrect.
    ConditionsNotCorrect,
    /// 0x24 requestSequenceError.
    RequestSequenceError,
    /// 0x31 requestOutOfRange.
    RequestOutOfRange,
    /// 0x33 securityAccessDenied.
    SecurityAccessDenied,
    /// 0x35 invalidKey.
    InvalidKey,
    /// 0x36 exceedNumberOfAttempts.
    ExceedNumberOfAttempts,
    /// 0x37 requiredTimeDelayNotExpired.
    RequiredTimeDelayNotExpired,
    /// 0x72 generalProgrammingFailure.
    GeneralProgrammingFailure,
    /// 0x78 requestCorrectlyReceived-ResponsePending.
    ResponsePending,
    /// 0x7E subFunctionNotSupportedInActiveSession.
    SubFunctionNotSupportedInActiveSession,
    /// 0x7F serviceNotSupportedInActiveSession.
    ServiceNotSupportedInActiveSession,
    /// Any other code, preserved verbatim.
    Other(u8),
}

impl NegativeResponseCode {
    /// Classify an NRC byte.
    pub fn from_byte(b: u8) -> NegativeResponseCode {
        use NegativeResponseCode::*;
        match b {
            0x10 => GeneralReject,
            0x11 => ServiceNotSupported,
            0x12 => SubFunctionNotSupported,
            0x13 => IncorrectMessageLengthOrInvalidFormat,
            0x14 => ResponseTooLong,
            0x21 => BusyRepeatRequest,
            0x22 => ConditionsNotCorrect,
            0x24 => RequestSequenceError,
            0x31 => RequestOutOfRange,
            0x33 => SecurityAccessDenied,
            0x35 => InvalidKey,
            0x36 => ExceedNumberOfAttempts,
            0x37 => RequiredTimeDelayNotExpired,
            0x72 => GeneralProgrammingFailure,
            0x78 => ResponsePending,
            0x7E => SubFunctionNotSupportedInActiveSession,
            0x7F => ServiceNotSupportedInActiveSession,
            other => Other(other),
        }
    }

    /// The NRC byte.
    pub fn byte(&self) -> u8 {
        use NegativeResponseCode::*;
        match self {
            GeneralReject => 0x10,
            ServiceNotSupported => 0x11,
            SubFunctionNotSupported => 0x12,
            IncorrectMessageLengthOrInvalidFormat => 0x13,
            ResponseTooLong => 0x14,
            BusyRepeatRequest => 0x21,
            ConditionsNotCorrect => 0x22,
            RequestSequenceError => 0x24,
            RequestOutOfRange => 0x31,
            SecurityAccessDenied => 0x33,
            InvalidKey => 0x35,
            ExceedNumberOfAttempts => 0x36,
            RequiredTimeDelayNotExpired => 0x37,
            GeneralProgrammingFailure => 0x72,
            ResponsePending => 0x78,
            SubFunctionNotSupportedInActiveSession => 0x7E,
            ServiceNotSupportedInActiveSession => 0x7F,
            Other(b) => *b,
        }
    }

    /// Stable snake_case identifier.
    pub fn name(&self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(s)) => s,
            _ => format!("other_{:02x}", self.byte()),
        }
    }

    /// Standard-language description. Not a UI string.
    pub fn description(&self) -> &'static str {
        use NegativeResponseCode::*;
        match self {
            GeneralReject => "generalReject",
            ServiceNotSupported => "serviceNotSupported",
            SubFunctionNotSupported => "subFunctionNotSupported",
            IncorrectMessageLengthOrInvalidFormat => "incorrectMessageLengthOrInvalidFormat",
            ResponseTooLong => "responseTooLong",
            BusyRepeatRequest => "busyRepeatRequest",
            ConditionsNotCorrect => "conditionsNotCorrect",
            RequestSequenceError => "requestSequenceError",
            RequestOutOfRange => "requestOutOfRange",
            SecurityAccessDenied => "securityAccessDenied",
            InvalidKey => "invalidKey",
            ExceedNumberOfAttempts => "exceedNumberOfAttempts",
            RequiredTimeDelayNotExpired => "requiredTimeDelayNotExpired",
            GeneralProgrammingFailure => "generalProgrammingFailure",
            ResponsePending => "requestCorrectlyReceived-ResponsePending",
            SubFunctionNotSupportedInActiveSession => "subFunctionNotSupportedInActiveSession",
            ServiceNotSupportedInActiveSession => "serviceNotSupportedInActiveSession",
            Other(_) => "manufacturerSpecificOrReserved",
        }
    }

    /// True for 0x78: the ECU is working and the caller must extend its
    /// deadline rather than treat the exchange as failed.
    pub fn is_response_pending(&self) -> bool {
        matches!(self, NegativeResponseCode::ResponsePending)
    }

    /// True when the same request could succeed if retried later.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            NegativeResponseCode::BusyRepeatRequest
                | NegativeResponseCode::ConditionsNotCorrect
                | NegativeResponseCode::RequiredTimeDelayNotExpired
                | NegativeResponseCode::ResponsePending
        )
    }
}

/// A UDS request PDU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UdsRequest {
    /// Service to invoke.
    pub service: UdsService,
    /// Sub-function byte, when the service takes one. The suppress-positive-
    /// response bit (0x80) is the caller's responsibility.
    pub sub_function: Option<u8>,
    /// Remaining request parameters.
    pub data: Vec<u8>,
}

impl UdsRequest {
    /// ReadDataByIdentifier for one DID.
    pub fn read_data_by_identifier(did: u16) -> Self {
        UdsRequest {
            service: UdsService::ReadDataByIdentifier,
            sub_function: None,
            data: did.to_be_bytes().to_vec(),
        }
    }

    /// TesterPresent, optionally suppressing the positive response.
    pub fn tester_present(suppress_response: bool) -> Self {
        UdsRequest {
            service: UdsService::TesterPresent,
            sub_function: Some(if suppress_response { 0x80 } else { 0x00 }),
            data: Vec::new(),
        }
    }

    /// Serialize to wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.service.id()];
        if let Some(sf) = self.sub_function {
            v.push(sf);
        }
        v.extend_from_slice(&self.data);
        v
    }

    /// True when the request set the suppress-positive-response bit, meaning a
    /// silent ECU is the expected outcome rather than a timeout.
    pub fn suppresses_positive_response(&self) -> bool {
        self.sub_function.is_some_and(|sf| sf & 0x80 != 0)
    }
}

/// A parsed UDS response PDU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UdsResponse {
    /// Positive response.
    Positive {
        /// The service being answered (already minus 0x40).
        service: UdsService,
        /// Response parameters.
        data: Vec<u8>,
    },
    /// Negative response.
    Negative {
        /// The service being rejected.
        service: UdsService,
        /// Why.
        nrc: NegativeResponseCode,
    },
}

impl UdsResponse {
    /// Parse a response PDU.
    pub fn parse(bytes: &[u8]) -> AimResult<UdsResponse> {
        let first = *bytes.first().ok_or_else(|| {
            AimError::new(ErrorCode::ProtocolMalformedResponse, "empty UDS response")
        })?;
        if first == 0x7F {
            if bytes.len() < 3 {
                return Err(AimError::new(
                    ErrorCode::ProtocolMalformedResponse,
                    format!("truncated UDS negative response: {}", aim_types::hex(bytes)),
                ));
            }
            return Ok(UdsResponse::Negative {
                service: UdsService::from_id(bytes[1]),
                nrc: NegativeResponseCode::from_byte(bytes[2]),
            });
        }
        if first < 0x40 {
            return Err(AimError::new(
                ErrorCode::ProtocolMalformedResponse,
                format!("0x{first:02X} is not a UDS positive response identifier"),
            ));
        }
        Ok(UdsResponse::Positive {
            service: UdsService::from_id(first - 0x40),
            data: bytes[1..].to_vec(),
        })
    }

    /// Check the response answers `request` and return its parameters.
    ///
    /// NRC 0x78 becomes [`ErrorCode::NegativeResponse`] with
    /// `details.response_pending = true`, so the caller can extend its deadline
    /// instead of concluding the ECU is dead. The negative response itself is
    /// never hidden.
    pub fn parameters_for(&self, request: &UdsRequest) -> AimResult<&[u8]> {
        match self {
            UdsResponse::Negative { service, nrc } => Err(AimError::new(
                ErrorCode::NegativeResponse,
                format!(
                    "UDS service 0x{:02X} rejected: {} (0x{:02X})",
                    service.id(),
                    nrc.description(),
                    nrc.byte()
                ),
            )
            .with_details(serde_json::json!({
                "service": service.id(),
                "nrc": nrc.byte(),
                "nrc_name": nrc.name(),
                "response_pending": nrc.is_response_pending(),
                "retryable": nrc.is_retryable(),
            }))),
            UdsResponse::Positive { service, data } => {
                if service.id() != request.service.id() {
                    return Err(AimError::new(
                        ErrorCode::UnexpectedResponse,
                        format!(
                            "asked UDS service 0x{:02X}, got 0x{:02X}",
                            request.service.id(),
                            service.id()
                        ),
                    ));
                }
                Ok(data)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_ids_round_trip_including_unknown_ones() {
        for id in [0x10u8, 0x11, 0x14, 0x19, 0x22, 0x27, 0x2E, 0x2F, 0x31, 0x3E] {
            assert_eq!(UdsService::from_id(id).id(), id);
        }
        assert_eq!(UdsService::from_id(0xB1), UdsService::Other(0xB1));
        assert_eq!(UdsService::Other(0xB1).id(), 0xB1);
    }

    #[test]
    fn unknown_services_are_treated_as_mutating() {
        // Fail closed: if we do not know what a service does, it is not a read.
        assert!(UdsService::Other(0xB1).is_mutating());
        assert!(!UdsService::ReadDataByIdentifier.is_mutating());
        assert!(UdsService::WriteDataByIdentifier.is_mutating());
        assert!(UdsService::SecurityAccess.is_mutating());
    }

    #[test]
    fn read_data_by_identifier_frames_the_did_big_endian() {
        let r = UdsRequest::read_data_by_identifier(0xF190);
        assert_eq!(r.to_bytes(), vec![0x22, 0xF1, 0x90]);
    }

    #[test]
    fn tester_present_suppress_bit() {
        assert_eq!(UdsRequest::tester_present(true).to_bytes(), vec![0x3E, 0x80]);
        assert!(UdsRequest::tester_present(true).suppresses_positive_response());
        assert_eq!(UdsRequest::tester_present(false).to_bytes(), vec![0x3E, 0x00]);
        assert!(!UdsRequest::tester_present(false).suppresses_positive_response());
    }

    #[test]
    fn positive_responses_are_recognised() {
        let r = UdsResponse::parse(&[0x62, 0xF1, 0x90, 0x31]).unwrap();
        assert_eq!(
            r,
            UdsResponse::Positive {
                service: UdsService::ReadDataByIdentifier,
                data: vec![0xF1, 0x90, 0x31]
            }
        );
        assert_eq!(
            r.parameters_for(&UdsRequest::read_data_by_identifier(0xF190))
                .unwrap(),
            &[0xF1, 0x90, 0x31]
        );
    }

    #[test]
    fn negative_responses_carry_the_full_nrc_detail() {
        let r = UdsResponse::parse(&[0x7F, 0x22, 0x31]).unwrap();
        let err = r
            .parameters_for(&UdsRequest::read_data_by_identifier(0xF190))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NegativeResponse);
        let d = err.details.unwrap();
        assert_eq!(d["nrc"], 0x31);
        assert_eq!(d["nrc_name"], "request_out_of_range");
        assert_eq!(d["response_pending"], false);
        assert_eq!(d["retryable"], false);
    }

    #[test]
    fn response_pending_is_marked_retryable_and_never_swallowed() {
        let nrc = NegativeResponseCode::from_byte(0x78);
        assert!(nrc.is_response_pending());
        assert!(nrc.is_retryable());
        let r = UdsResponse::parse(&[0x7F, 0x31, 0x78]).unwrap();
        let err = r
            .parameters_for(&UdsRequest {
                service: UdsService::RoutineControl,
                sub_function: Some(0x01),
                data: vec![],
            })
            .unwrap_err();
        assert_eq!(err.details.unwrap()["response_pending"], true);
    }

    #[test]
    fn unknown_nrcs_are_preserved_not_dropped() {
        let nrc = NegativeResponseCode::from_byte(0x9A);
        assert_eq!(nrc, NegativeResponseCode::Other(0x9A));
        assert_eq!(nrc.byte(), 0x9A);
        assert_eq!(nrc.description(), "manufacturerSpecificOrReserved");
    }

    #[test]
    fn wrong_service_in_response_is_an_error() {
        let r = UdsResponse::parse(&[0x50, 0x03]).unwrap();
        let err = r
            .parameters_for(&UdsRequest::read_data_by_identifier(0xF190))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnexpectedResponse);
    }

    #[test]
    fn malformed_uds_responses_are_rejected() {
        assert!(UdsResponse::parse(&[]).is_err());
        assert!(UdsResponse::parse(&[0x7F, 0x22]).is_err());
        assert!(UdsResponse::parse(&[0x22, 0xF1]).is_err());
    }
}
