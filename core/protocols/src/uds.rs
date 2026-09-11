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
    /// 0x25 noResponseFromSubnetComponent.
    NoResponseFromSubnetComponent,
    /// 0x26 failurePreventsExecutionOfRequestedAction.
    FailurePreventsExecutionOfRequestedAction,
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
    /// 0x70 uploadDownloadNotAccepted.
    UploadDownloadNotAccepted,
    /// 0x71 transferDataSuspended.
    TransferDataSuspended,
    /// 0x72 generalProgrammingFailure.
    GeneralProgrammingFailure,
    /// 0x73 wrongBlockSequenceCounter.
    WrongBlockSequenceCounter,
    /// 0x78 requestCorrectlyReceived-ResponsePending.
    ResponsePending,
    /// 0x7E subFunctionNotSupportedInActiveSession.
    SubFunctionNotSupportedInActiveSession,
    /// 0x7F serviceNotSupportedInActiveSession.
    ServiceNotSupportedInActiveSession,

    // ---- 0x81..=0x93: what is wrong with the vehicle's state ----
    //
    // The block that matters most here and the one this table was missing
    // entirely. `conditionsNotCorrect` says a module refused because the
    // vehicle is in the wrong state; every code below says *which* state, and
    // therefore what the person in the driver's seat has to change.
    //
    // Without them a module that answered "the engine is running and must not
    // be" arrived as `Other(0x83)` and was reported as an unexplained refusal.
    // The module had already given the answer.
    /// 0x81 rpmTooHigh.
    RpmTooHigh,
    /// 0x82 rpmTooLow.
    RpmTooLow,
    /// 0x83 engineIsRunning.
    EngineIsRunning,
    /// 0x84 engineIsNotRunning.
    EngineIsNotRunning,
    /// 0x85 engineRunTimeTooLow.
    EngineRunTimeTooLow,
    /// 0x86 temperatureTooHigh.
    TemperatureTooHigh,
    /// 0x87 temperatureTooLow.
    TemperatureTooLow,
    /// 0x88 vehicleSpeedTooHigh.
    VehicleSpeedTooHigh,
    /// 0x89 vehicleSpeedTooLow.
    VehicleSpeedTooLow,
    /// 0x8A throttlePedalTooHigh.
    ThrottleTooHigh,
    /// 0x8B throttlePedalTooLow.
    ThrottleTooLow,
    /// 0x8C transmissionRangeNotInNeutral.
    TransmissionRangeNotInNeutral,
    /// 0x8D transmissionRangeNotInGear.
    TransmissionRangeNotInGear,
    /// 0x8F brakeSwitchNotClosed.
    BrakeSwitchNotClosed,
    /// 0x90 shifterLeverNotInPark.
    ShifterLeverNotInPark,
    /// 0x91 torqueConverterClutchLocked.
    TorqueConverterClutchLocked,
    /// 0x92 voltageTooHigh.
    VoltageTooHigh,
    /// 0x93 voltageTooLow.
    VoltageTooLow,

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
            0x25 => NoResponseFromSubnetComponent,
            0x26 => FailurePreventsExecutionOfRequestedAction,
            0x31 => RequestOutOfRange,
            0x33 => SecurityAccessDenied,
            0x35 => InvalidKey,
            0x36 => ExceedNumberOfAttempts,
            0x37 => RequiredTimeDelayNotExpired,
            0x70 => UploadDownloadNotAccepted,
            0x71 => TransferDataSuspended,
            0x72 => GeneralProgrammingFailure,
            0x73 => WrongBlockSequenceCounter,
            0x78 => ResponsePending,
            0x7E => SubFunctionNotSupportedInActiveSession,
            0x7F => ServiceNotSupportedInActiveSession,
            0x81 => RpmTooHigh,
            0x82 => RpmTooLow,
            0x83 => EngineIsRunning,
            0x84 => EngineIsNotRunning,
            0x85 => EngineRunTimeTooLow,
            0x86 => TemperatureTooHigh,
            0x87 => TemperatureTooLow,
            0x88 => VehicleSpeedTooHigh,
            0x89 => VehicleSpeedTooLow,
            0x8A => ThrottleTooHigh,
            0x8B => ThrottleTooLow,
            0x8C => TransmissionRangeNotInNeutral,
            0x8D => TransmissionRangeNotInGear,
            0x8F => BrakeSwitchNotClosed,
            0x90 => ShifterLeverNotInPark,
            0x91 => TorqueConverterClutchLocked,
            0x92 => VoltageTooHigh,
            0x93 => VoltageTooLow,
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
            NoResponseFromSubnetComponent => 0x25,
            FailurePreventsExecutionOfRequestedAction => 0x26,
            RequestOutOfRange => 0x31,
            SecurityAccessDenied => 0x33,
            InvalidKey => 0x35,
            ExceedNumberOfAttempts => 0x36,
            RequiredTimeDelayNotExpired => 0x37,
            UploadDownloadNotAccepted => 0x70,
            TransferDataSuspended => 0x71,
            GeneralProgrammingFailure => 0x72,
            WrongBlockSequenceCounter => 0x73,
            ResponsePending => 0x78,
            SubFunctionNotSupportedInActiveSession => 0x7E,
            ServiceNotSupportedInActiveSession => 0x7F,
            RpmTooHigh => 0x81,
            RpmTooLow => 0x82,
            EngineIsRunning => 0x83,
            EngineIsNotRunning => 0x84,
            EngineRunTimeTooLow => 0x85,
            TemperatureTooHigh => 0x86,
            TemperatureTooLow => 0x87,
            VehicleSpeedTooHigh => 0x88,
            VehicleSpeedTooLow => 0x89,
            ThrottleTooHigh => 0x8A,
            ThrottleTooLow => 0x8B,
            TransmissionRangeNotInNeutral => 0x8C,
            TransmissionRangeNotInGear => 0x8D,
            BrakeSwitchNotClosed => 0x8F,
            ShifterLeverNotInPark => 0x90,
            TorqueConverterClutchLocked => 0x91,
            VoltageTooHigh => 0x92,
            VoltageTooLow => 0x93,
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
            NoResponseFromSubnetComponent => "noResponseFromSubnetComponent",
            FailurePreventsExecutionOfRequestedAction => {
                "failurePreventsExecutionOfRequestedAction"
            }
            UploadDownloadNotAccepted => "uploadDownloadNotAccepted",
            TransferDataSuspended => "transferDataSuspended",
            GeneralProgrammingFailure => "generalProgrammingFailure",
            WrongBlockSequenceCounter => "wrongBlockSequenceCounter",
            ResponsePending => "requestCorrectlyReceived-ResponsePending",
            SubFunctionNotSupportedInActiveSession => "subFunctionNotSupportedInActiveSession",
            ServiceNotSupportedInActiveSession => "serviceNotSupportedInActiveSession",
            RpmTooHigh => "rpmTooHigh",
            RpmTooLow => "rpmTooLow",
            EngineIsRunning => "engineIsRunning",
            EngineIsNotRunning => "engineIsNotRunning",
            EngineRunTimeTooLow => "engineRunTimeTooLow",
            TemperatureTooHigh => "temperatureTooHigh",
            TemperatureTooLow => "temperatureTooLow",
            VehicleSpeedTooHigh => "vehicleSpeedTooHigh",
            VehicleSpeedTooLow => "vehicleSpeedTooLow",
            ThrottleTooHigh => "throttlePedalTooHigh",
            ThrottleTooLow => "throttlePedalTooLow",
            TransmissionRangeNotInNeutral => "transmissionRangeNotInNeutral",
            TransmissionRangeNotInGear => "transmissionRangeNotInGear",
            BrakeSwitchNotClosed => "brakeSwitchNotClosed",
            ShifterLeverNotInPark => "shifterLeverNotInPark",
            TorqueConverterClutchLocked => "torqueConverterClutchLocked",
            VoltageTooHigh => "voltageTooHigh",
            VoltageTooLow => "voltageTooLow",
            Other(_) => "manufacturerSpecificOrReserved",
        }
    }

    /// What the person in the driver's seat would have to change.
    ///
    /// `None` for every refusal that is not about the vehicle's state, which is
    /// most of them — there is nothing for somebody to do about
    /// `securityAccessDenied` except not ask.
    ///
    /// This is the whole reason the `0x81..=0x93` block is worth carrying. A
    /// module answering `conditionsNotCorrect` has said the vehicle is in the
    /// wrong state and left the person to guess which; a module answering
    /// `engineIsRunning` has told them exactly what to do, and repeating that
    /// back as "refused, code 0x83" throws away the useful half of the answer.
    pub fn what_to_change(&self) -> Option<&'static str> {
        use NegativeResponseCode::*;
        Some(match self {
            RpmTooHigh => "The engine is turning too fast for this. Let it drop to idle.",
            RpmTooLow => "The engine is turning too slowly for this. Raise the revs and hold them.",
            EngineIsRunning => "Turn the engine off, leaving the ignition on.",
            EngineIsNotRunning => "Start the engine.",
            EngineRunTimeTooLow => {
                "The engine has not been running long enough. Let it run for a few minutes and \
                 try again."
            }
            TemperatureTooHigh => "Something is too hot for this. Let the vehicle cool down.",
            TemperatureTooLow => {
                "Something is too cold for this. Let the engine reach normal running temperature."
            }
            VehicleSpeedTooHigh => "The vehicle is moving too fast. Come to a stop.",
            VehicleSpeedTooLow => {
                "The vehicle needs to be moving faster than it is. This one needs a second person \
                 driving, or a rolling road — do not attempt it while reading a screen."
            }
            ThrottleTooHigh => "Take your foot off the accelerator.",
            ThrottleTooLow => "Press the accelerator and hold it.",
            TransmissionRangeNotInNeutral => "Put the gearbox in neutral.",
            TransmissionRangeNotInGear => "Put the gearbox in gear.",
            BrakeSwitchNotClosed => "Press and hold the brake pedal.",
            ShifterLeverNotInPark => "Put the selector in park.",
            TorqueConverterClutchLocked => {
                "The torque converter is locked up. Let the vehicle come to rest first."
            }
            VoltageTooHigh => "Supply voltage is too high. If a charger is connected, take it off.",
            VoltageTooLow => {
                "The battery is too low for this. Put a charger on it, or run the engine to \
                 bring the voltage up."
            }
            _ => return None,
        })
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

    /// What this refusal means for the person holding the scanner.
    ///
    /// The standard name is not the useful part. "requestOutOfRange" and
    /// "securityAccessDenied" both read as failure, but one says the thing does
    /// not exist and the other says it exists and is locked — and those are
    /// different facts about somebody's vehicle. Reporting the code without the
    /// distinction is why a truck that refused a write went uninvestigated: the
    /// module had already said which of the two it was.
    pub fn refusal(&self) -> RefusalKind {
        use NegativeResponseCode::*;
        match self {
            SecurityAccessDenied | InvalidKey | ExceedNumberOfAttempts => {
                RefusalKind::SecurityRequired
            }
            RequestOutOfRange | SubFunctionNotSupported | ServiceNotSupported => {
                RefusalKind::NotPresent
            }
            SubFunctionNotSupportedInActiveSession | ServiceNotSupportedInActiveSession => {
                RefusalKind::WrongSession
            }
            // Everything from 0x81 to 0x93 is the same answer as
            // `conditionsNotCorrect` with the condition named, so it classifies
            // the same way — and `what_to_change` carries the part that makes
            // it actionable.
            ConditionsNotCorrect
            | RequestSequenceError
            | RpmTooHigh
            | RpmTooLow
            | EngineIsRunning
            | EngineIsNotRunning
            | EngineRunTimeTooLow
            | TemperatureTooHigh
            | TemperatureTooLow
            | VehicleSpeedTooHigh
            | VehicleSpeedTooLow
            | ThrottleTooHigh
            | ThrottleTooLow
            | TransmissionRangeNotInNeutral
            | TransmissionRangeNotInGear
            | BrakeSwitchNotClosed
            | ShifterLeverNotInPark
            | TorqueConverterClutchLocked
            | VoltageTooHigh
            | VoltageTooLow => RefusalKind::VehicleConditions,
            BusyRepeatRequest | ResponsePending | RequiredTimeDelayNotExpired => RefusalKind::Busy,
            IncorrectMessageLengthOrInvalidFormat | ResponseTooLong => {
                RefusalKind::OurRequestWasWrong
            }
            // A module that could not reach something behind it, or that is
            // already broken enough to refuse. Neither is about our request.
            NoResponseFromSubnetComponent | FailurePreventsExecutionOfRequestedAction => {
                RefusalKind::Unexplained
            }
            // The programming-sequence codes. This build never programs, so
            // seeing one means something else is talking to the module.
            UploadDownloadNotAccepted | TransferDataSuspended | WrongBlockSequenceCounter => {
                RefusalKind::Unexplained
            }
            GeneralReject | GeneralProgrammingFailure | Other(_) => RefusalKind::Unexplained,
        }
    }
}

/// Why a module refused, in terms of what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    /// The module wants a manufacturer seed/key exchange first.
    SecurityRequired,
    /// What was asked for does not exist on this module.
    NotPresent,
    /// It exists, but not in the diagnostic session currently open.
    WrongSession,
    /// The vehicle is not in a state where this is allowed.
    VehicleConditions,
    /// The module is busy or still working on it.
    Busy,
    /// The request was malformed. This one is our fault, not the vehicle's.
    OurRequestWasWrong,
    /// Refused without saying why.
    Unexplained,
}

impl RefusalKind {
    /// A stable code an interface can branch on.
    pub fn code(&self) -> &'static str {
        use RefusalKind::*;
        match self {
            SecurityRequired => "module_refused_security_required",
            NotPresent => "module_refused_not_present",
            WrongSession => "module_refused_wrong_session",
            VehicleConditions => "module_refused_vehicle_conditions",
            Busy => "module_refused_busy",
            OurRequestWasWrong => "module_refused_malformed_request",
            Unexplained => "module_refused_without_saying_why",
        }
    }

    /// What this means and what can be done, in plain language.
    ///
    /// Written to close the question rather than to encourage a retry that
    /// cannot work. A locked module is not a puzzle the user can solve by
    /// trying again, and saying otherwise wastes their evening.
    pub fn explain(&self) -> &'static str {
        use RefusalKind::*;
        match self {
            SecurityRequired => {
                "The module has this, but will not allow it without a manufacturer seed/key \
                 exchange. That algorithm is specific to the carmaker and this build does not \
                 have it, so this is a wall rather than something to retry."
            }
            NotPresent => {
                "The module answered, and says it does not have what was asked for. This is a \
                 real answer about your vehicle, not a failure to communicate."
            }
            WrongSession => {
                "The module has this, but not in the diagnostic session that is currently open. \
                 Opening an extended session and asking again is the next step."
            }
            VehicleConditions => {
                "The module refused because of the state the vehicle is in - commonly engine \
                 running, road speed above zero, or battery voltage. Changing that condition and \
                 asking again is the next step."
            }
            Busy => {
                "The module is busy or still working on the request. Waiting and asking again is \
                 the right response, and is done automatically where it is safe to."
            }
            OurRequestWasWrong => {
                "The module rejected the shape of the request itself. That is a defect in this \
                 application rather than anything about your vehicle - please report it."
            }
            Unexplained => {
                "The module refused without giving a reason the standard defines. Nothing can be \
                 concluded from this beyond the fact that it refused."
            }
        }
    }

    /// Whether asking again, unchanged, could plausibly succeed.
    pub fn worth_retrying(&self) -> bool {
        matches!(self, RefusalKind::Busy)
    }
}

/// What a data identifier holds, when ISO 14229 says so.
///
/// The standard defines meanings for part of `0xF180..=0xF1FF` and leaves the
/// rest of that block to vehicle manufacturers (`0xF1A0..=0xF1EF`) and system
/// suppliers (`0xF1F0..=0xF1FF`). Those get `None`, and that is the whole point
/// of the function: a module answering at `0xF1F3` with `EDC17CP65` has told us
/// something a supplier defined and nobody published, so it is reported as the
/// bytes at an identifier rather than as a fact about the vehicle. Guessing
/// from the shape of the string is exactly the inference this project refuses.
pub fn identification_did_name(did: u16) -> Option<&'static str> {
    Some(match did {
        0xF180 => "boot software identification",
        0xF181 => "application software identification",
        0xF182 => "application data identification",
        0xF183 => "boot software fingerprint",
        0xF184 => "application software fingerprint",
        0xF185 => "application data fingerprint",
        0xF186 => "active diagnostic session",
        0xF187 => "manufacturer spare part number",
        0xF188 => "manufacturer ECU software number",
        0xF189 => "manufacturer ECU software version",
        0xF18A => "system supplier identifier",
        0xF18B => "ECU manufacturing date",
        0xF18C => "ECU serial number",
        0xF18D => "supported functional units",
        0xF18E => "manufacturer kit assembly part number",
        0xF190 => "VIN",
        0xF191 => "manufacturer ECU hardware number",
        0xF192 => "system supplier ECU hardware number",
        0xF193 => "system supplier ECU hardware version",
        0xF194 => "system supplier ECU software number",
        0xF195 => "system supplier ECU software version",
        0xF196 => "exhaust regulation or type-approval number",
        0xF197 => "system name or engine type",
        0xF198 => "repair shop code or tester serial number",
        0xF199 => "programming date",
        0xF19D => "ECU installation date",
        0xF19E => "ODX file identifier",
        _ => return None,
    })
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

    /// WriteDataByIdentifier for one DID.
    ///
    /// The counterpart to [`UdsRequest::read_data_by_identifier`], and the only
    /// way this app changes a vehicle setting. The value is written whole: UDS
    /// has no partial write, so a caller that wants to change one bit must read
    /// the record, modify it, and write the complete record back. That is
    /// deliberate rather than inconvenient — a read-modify-write makes the
    /// before value available to show a person and to verify against
    /// afterwards, which a blind write does not.
    pub fn write_data_by_identifier(did: u16, value: &[u8]) -> Self {
        let mut data = did.to_be_bytes().to_vec();
        data.extend_from_slice(value);
        UdsRequest { service: UdsService::WriteDataByIdentifier, sub_function: None, data }
    }

    /// SecurityAccess, requesting the seed for a given level.
    ///
    /// Levels are odd for requestSeed and even for sendKey, so level 1 asks for
    /// the seed that level 2 answers. Modules that gate configuration behind
    /// security use low levels for exactly that; the high levels used for
    /// immobiliser and key operations are refused by risk class long before
    /// they reach here, so this being present does not make those reachable.
    pub fn security_access_request_seed(level: u8) -> Self {
        UdsRequest {
            service: UdsService::SecurityAccess,
            sub_function: Some(level),
            data: Vec::new(),
        }
    }

    /// SecurityAccess, sending the computed key back.
    pub fn security_access_send_key(level: u8, key: &[u8]) -> Self {
        UdsRequest {
            service: UdsService::SecurityAccess,
            sub_function: Some(level),
            data: key.to_vec(),
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

    /// DiagnosticSessionControl.
    ///
    /// `0x01` (default session) is inert — every module is already in it — and
    /// is therefore usable as a discovery probe on modules that ignore
    /// TesterPresent outside a session.
    pub fn diagnostic_session_control(session: u8) -> Self {
        UdsRequest {
            service: UdsService::DiagnosticSessionControl,
            sub_function: Some(session),
            data: Vec::new(),
        }
    }

    /// ReadDTCInformation, sub-function `reportDTCByStatusMask` (0x02).
    ///
    /// This is the request that makes a full-vehicle fault scan possible.
    /// Service 03 only reaches emissions-related controllers; this reaches
    /// anything that speaks UDS — brakes, airbag, body, transmission — and it
    /// is a public ISO standard, so it works identically across manufacturers
    /// without a single reverse-engineered identifier.
    ///
    /// `status_mask` of `0xFF` asks for every code whatever its status bits.
    pub fn read_dtc_by_status_mask(status_mask: u8) -> Self {
        UdsRequest {
            service: UdsService::ReadDtcInformation,
            sub_function: Some(0x02),
            data: vec![status_mask],
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

/// One trouble code as UDS service 0x19 reports it.
///
/// Three bytes plus a status byte, where service 03 used two bytes. The first
/// two bytes are the same J2012 encoding — `P0301` decodes identically — and
/// the third is a failure type that says *how* the thing failed rather than
/// what failed: `P0301-00` and `P0301-1C` are the same circuit with different
/// symptoms.
///
/// The status byte is the part service 03 never had. It distinguishes a fault
/// happening right now from one that happened once last winter, which is the
/// difference between a repair and a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UdsDtc {
    /// SAE J2012 code with its failure type, e.g. `P0301-00`.
    pub code: String,
    /// The code without the failure type, e.g. `P0301`, for catalogue lookup.
    pub base_code: String,
    /// Failure type byte. `0x00` when the ECU does not use them.
    pub failure_type: u8,
    /// Raw status bits, as reported.
    pub status: u8,
}

impl UdsDtc {
    /// The fault is present at this moment.
    pub fn test_failed(&self) -> bool {
        self.status & 0x01 != 0
    }

    /// The fault has been seen at some point in the current cycle.
    pub fn test_failed_this_cycle(&self) -> bool {
        self.status & 0x02 != 0
    }

    /// Stored in memory, whether or not it is currently failing.
    pub fn confirmed(&self) -> bool {
        self.status & 0x08 != 0
    }

    /// The module is asking for the warning lamp.
    pub fn warning_indicator_requested(&self) -> bool {
        self.status & 0x80 != 0
    }

    /// A short, plain-language reading of the status bits.
    ///
    /// The bits are what the module said; this sentence is how a person should
    /// take it. A confirmed code that is not currently failing is the common
    /// and confusing case — something went wrong, and it is not wrong now.
    pub fn status_summary(&self) -> &'static str {
        match (self.test_failed(), self.confirmed()) {
            (true, _) => "failing right now",
            (false, true) => "stored, but not failing at the moment",
            (false, false) => "recorded, not confirmed",
        }
    }
}

/// Decode the payload of a `0x19` sub-function `0x02` response.
///
/// Layout after the service and sub-function bytes: one availability mask, then
/// four bytes per code. A trailing partial record is dropped rather than
/// guessed at, exactly as service 06 records are.
pub fn decode_dtc_by_status_mask(payload: &[u8]) -> Vec<UdsDtc> {
    // The first byte is the status-availability mask: which status bits this
    // ECU actually maintains. Useful for interpreting the bits strictly; not
    // needed to read the codes themselves.
    let records = match payload.split_first() {
        Some((_availability, rest)) => rest,
        None => return Vec::new(),
    };
    records
        .chunks_exact(4)
        .map(|c| {
            let base = crate::obd2::decode_dtc(c[0], c[1]);
            UdsDtc {
                code: format!("{base}-{:02X}", c[2]),
                base_code: base,
                failure_type: c[2],
                status: c[3],
            }
        })
        .collect()
}

#[cfg(test)]
mod dtc_tests {
    use super::*;

    #[test]
    fn a_status_mask_response_decodes_into_codes() {
        // 59 02 <avail> then 4-byte records. Header already stripped.
        let payload = [
            0xFF, // availability mask
            0x03, 0x01, 0x00, 0x09, // P0301-00, confirmed + failing
            0xC1, 0x23, 0x1C, 0x08, // U0123-1C, confirmed, not failing now
        ];
        let dtcs = decode_dtc_by_status_mask(&payload);
        assert_eq!(dtcs.len(), 2);

        assert_eq!(dtcs[0].code, "P0301-00");
        assert_eq!(dtcs[0].base_code, "P0301");
        assert!(dtcs[0].test_failed(), "bit 0 means failing now");
        assert!(dtcs[0].confirmed());
        assert_eq!(dtcs[0].status_summary(), "failing right now");

        assert_eq!(dtcs[1].code, "U0123-1C");
        assert_eq!(dtcs[1].base_code, "U0123");
        assert!(!dtcs[1].test_failed());
        assert!(dtcs[1].confirmed());
        assert_eq!(dtcs[1].status_summary(), "stored, but not failing at the moment");
    }

    #[test]
    fn the_base_code_matches_what_service_03_would_have_said() {
        // The whole point of keeping both: the catalogue is keyed on the
        // two-byte form, so a three-byte UDS code still gets a description.
        let payload = [0xFF, 0x01, 0x71, 0x00, 0x08];
        let d = &decode_dtc_by_status_mask(&payload)[0];
        assert_eq!(d.base_code, crate::obd2::decode_dtc(0x01, 0x71));
        assert!(d.code.starts_with(&d.base_code));
    }

    #[test]
    fn an_empty_or_truncated_payload_yields_nothing_rather_than_guessing() {
        assert!(decode_dtc_by_status_mask(&[]).is_empty());
        // Availability mask only: the module has no codes.
        assert!(decode_dtc_by_status_mask(&[0xFF]).is_empty());
        // A partial record is dropped, not padded.
        assert_eq!(decode_dtc_by_status_mask(&[0xFF, 0x03, 0x01]).len(), 0);
    }

    #[test]
    fn a_module_with_no_faults_answers_with_just_the_mask() {
        let dtcs = decode_dtc_by_status_mask(&[0x2F]);
        assert!(dtcs.is_empty(), "no codes is a valid, common answer");
    }

    #[test]
    fn the_discovery_probe_is_inert() {
        // TesterPresent does nothing to a vehicle. That is what makes it usable
        // as a "is anyone at this address" sweep across every module.
        let r = UdsRequest::tester_present(false);
        assert_eq!(r.to_bytes(), vec![0x3E, 0x00]);
        assert!(!r.service.is_mutating());

        // Default-session control is equally inert: every module is already in
        // the default session.
        let s = UdsRequest::diagnostic_session_control(0x01);
        assert_eq!(s.to_bytes(), vec![0x10, 0x01]);
    }

    #[test]
    fn the_dtc_request_asks_every_module_for_everything() {
        let r = UdsRequest::read_dtc_by_status_mask(0xFF);
        assert_eq!(r.to_bytes(), vec![0x19, 0x02, 0xFF]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two refusals that look identical to a user and mean opposite things.
    ///
    /// `securityAccessDenied` says the module *has* this and will not allow it.
    /// `requestOutOfRange` says it does not have it. Reporting both as "the
    /// request failed" is what left a real truck's refused write uninvestigated.
    #[test]
    fn locked_and_absent_are_not_the_same_refusal() {
        assert_eq!(
            NegativeResponseCode::SecurityAccessDenied.refusal(),
            RefusalKind::SecurityRequired
        );
        assert_eq!(NegativeResponseCode::RequestOutOfRange.refusal(), RefusalKind::NotPresent);
        assert_ne!(
            NegativeResponseCode::SecurityAccessDenied.refusal().code(),
            NegativeResponseCode::RequestOutOfRange.refusal().code()
        );
    }

    /// A session-scoped refusal is recoverable; a locked one is not, and the
    /// guidance must not imply otherwise.
    #[test]
    fn only_a_busy_module_is_worth_asking_again_unchanged() {
        assert!(RefusalKind::Busy.worth_retrying());
        for kind in [
            RefusalKind::SecurityRequired,
            RefusalKind::NotPresent,
            RefusalKind::WrongSession,
            RefusalKind::VehicleConditions,
            RefusalKind::OurRequestWasWrong,
            RefusalKind::Unexplained,
        ] {
            assert!(!kind.worth_retrying(), "{kind:?} must not invite a blind retry");
        }
        // A wall is described as a wall.
        assert!(RefusalKind::SecurityRequired.explain().contains("wall"));
        // A malformed request is owned rather than blamed on the vehicle.
        assert!(RefusalKind::OurRequestWasWrong.explain().contains("this application"));
    }

    /// Every code classifies, including ones the standard leaves to the maker.
    #[test]
    fn every_negative_response_code_has_a_consequence() {
        for byte in 0x00..=0xFFu8 {
            let nrc = NegativeResponseCode::from_byte(byte);
            let kind = nrc.refusal();
            assert!(!kind.code().is_empty());
            assert!(!kind.explain().is_empty(), "no guidance for {byte:02X}");
        }
    }

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
            r.parameters_for(&UdsRequest::read_data_by_identifier(0xF190)).unwrap(),
            &[0xF1, 0x90, 0x31]
        );
    }

    #[test]
    fn negative_responses_carry_the_full_nrc_detail() {
        let r = UdsResponse::parse(&[0x7F, 0x22, 0x31]).unwrap();
        let err = r.parameters_for(&UdsRequest::read_data_by_identifier(0xF190)).unwrap_err();
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
        let err = r.parameters_for(&UdsRequest::read_data_by_identifier(0xF190)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnexpectedResponse);
    }

    #[test]
    fn malformed_uds_responses_are_rejected() {
        assert!(UdsResponse::parse(&[]).is_err());
        assert!(UdsResponse::parse(&[0x7F, 0x22]).is_err());
        assert!(UdsResponse::parse(&[0x22, 0xF1]).is_err());
    }
    /// The block the table was missing, and the reason it was worth adding.
    ///
    /// Every one of these is `conditionsNotCorrect` with the condition named.
    /// Before they existed each arrived as `Other(0x8n)` and was reported as an
    /// unexplained refusal — while the module had already said exactly what the
    /// person needed to change.
    #[test]
    fn the_condition_codes_say_what_to_change() {
        for (byte, expect) in [
            (0x83u8, "Turn the engine off"),
            (0x84, "Start the engine"),
            (0x88, "Come to a stop"),
            (0x8C, "neutral"),
            (0x90, "park"),
            (0x93, "charger"),
        ] {
            let nrc = NegativeResponseCode::from_byte(byte);
            assert_eq!(
                nrc.refusal(),
                RefusalKind::VehicleConditions,
                "{byte:02X} is a condition, not an unexplained refusal"
            );
            let fix = nrc.what_to_change().expect("a condition names its own remedy");
            assert!(fix.contains(expect), "{byte:02X}: {fix}");
        }
    }

    /// And a refusal that is not about the vehicle's state offers no
    /// instruction, because there is nothing for anybody to do about it.
    #[test]
    fn a_refusal_that_is_not_about_the_vehicle_offers_no_instruction() {
        for byte in [0x33u8, 0x31, 0x13, 0x10] {
            assert!(
                NegativeResponseCode::from_byte(byte).what_to_change().is_none(),
                "{byte:02X} should not pretend to be actionable"
            );
        }
    }

    /// The one that needs a second person. A procedure requiring road speed
    /// must never read as something to attempt alone with a laptop.
    #[test]
    fn a_condition_needing_road_speed_says_not_to_do_it_while_reading() {
        let fix = NegativeResponseCode::from_byte(0x89).what_to_change().unwrap();
        assert!(fix.contains("second person") || fix.contains("rolling road"), "{fix}");
        assert!(fix.contains("do not attempt it while reading"), "{fix}");
    }

    /// Every code round-trips through its byte, including the new ones. A
    /// table that decodes a byte into a name it cannot encode back is a table
    /// with a typo in it.
    #[test]
    fn every_named_code_round_trips_through_its_byte() {
        for byte in 0x00u8..=0xFFu8 {
            let nrc = NegativeResponseCode::from_byte(byte);
            assert_eq!(nrc.byte(), byte, "{byte:02X} did not survive the round trip");
        }
    }
}
