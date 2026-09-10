//! Structured errors.
//!
//! Handoff §13: *"The API should return structured errors, capability state,
//! and provenance rather than UI-specific messages."* So every failure carries
//! a stable [`ErrorCode`] the UI can branch on, a developer-facing message, and
//! optional structured details. No error is ever swallowed: adapter errors and
//! ECU negative responses have their own codes and reach the caller intact.

use serde::{Deserialize, Serialize};

/// Stable machine-readable error code. Never renamed once shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // ---- transport ----
    /// The named port/endpoint does not exist or could not be enumerated.
    TransportNotFound,
    /// The port exists but could not be opened (in use, permissions, pairing).
    TransportOpenFailed,
    /// A read or write exceeded its deadline.
    TransportTimeout,
    /// The link dropped mid-exchange.
    TransportDisconnected,
    /// Underlying I/O error.
    TransportIo,
    /// The requested transport is not compiled into this build.
    TransportUnsupported,

    // ---- adapter ----
    /// The device did not answer an ELM327 identification handshake.
    AdapterNotIdentified,
    /// The adapter answered `?` — it did not understand the command.
    AdapterRejectedCommand,
    /// The adapter reported an internal error banner (`ERROR`, `BUFFER FULL`, ...).
    AdapterError,
    /// `UNABLE TO CONNECT` — adapter is fine, the vehicle is not answering.
    VehicleNotResponding,
    /// `NO DATA` — the request was valid but nothing answered it.
    NoData,
    /// The adapter is busy with another exchange.
    AdapterBusy,
    /// Adapter initialization sequence failed.
    AdapterInitFailed,

    // ---- protocol ----
    /// A response could not be parsed as the expected frame/PDU.
    ProtocolMalformedResponse,
    /// ISO-TP reassembly failed (bad sequence number, overflow, timeout).
    IsoTpError,
    /// The ECU returned a negative response (UDS/OBD service 0x7F).
    NegativeResponse,
    /// A response arrived for a service/PID we did not ask about.
    UnexpectedResponse,

    // ---- decoding ----
    /// No decoder definition exists for this signal.
    DecoderNotFound,
    /// The decoder exists but the payload was the wrong length/shape.
    DecoderInputInvalid,
    /// The decoded value fell outside the definition's declared valid range.
    DecodedValueOutOfRange,

    // ---- safety ----
    /// The operation is not in the capability allowlist. Fail closed.
    OperationNotAllowed,
    /// The operation exists but its permission level is disabled in this build.
    PermissionLevelDisabled,
    /// The operation requires explicit user confirmation that was not supplied.
    ConfirmationRequired,
    /// A declared precondition was not satisfied.
    PreconditionFailed,
    /// The connected adapter cannot perform this operation.
    CapabilityMissing,

    // ---- session / storage ----
    /// No active connection/session for the requested operation.
    NoActiveSession,
    /// The requested entity does not exist.
    NotFound,
    /// Storage layer failure.
    StorageError,

    // ---- generic ----
    /// The caller sent a malformed request.
    BadRequest,
    /// The feature exists as a seam but is not enabled in this build.
    NotImplemented,
    /// Operation cancelled by the caller.
    Cancelled,
    /// Anything that does not fit above. Add a specific code instead when you can.
    Internal,
}

impl ErrorCode {
    /// HTTP status the localhost API maps this code to.
    pub fn http_status(&self) -> u16 {
        use ErrorCode::*;
        match self {
            BadRequest | DecoderInputInvalid => 400,
            OperationNotAllowed | PermissionLevelDisabled | ConfirmationRequired => 403,
            TransportNotFound | NotFound | DecoderNotFound => 404,
            NoActiveSession | PreconditionFailed | CapabilityMissing | AdapterBusy => 409,
            TransportTimeout => 504,
            NotImplemented => 501,
            VehicleNotResponding
            | NoData
            | AdapterNotIdentified
            | AdapterRejectedCommand
            | AdapterError
            | AdapterInitFailed
            | TransportOpenFailed
            | TransportDisconnected
            | TransportIo
            | TransportUnsupported
            | ProtocolMalformedResponse
            | IsoTpError
            | NegativeResponse
            | UnexpectedResponse
            | DecodedValueOutOfRange => 502,
            Cancelled => 499,
            StorageError | Internal => 500,
        }
    }

    /// Stable snake_case string used on the wire.
    pub fn as_str(&self) -> &'static str {
        // Serde already produces exactly this; go through it so the two can
        // never drift apart.
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(s)) => Box::leak(s.into_boxed_str()),
            _ => "internal",
        }
    }
}

/// The error type used throughout the diagnostic core.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{code:?}: {message}")]
pub struct AimError {
    /// Machine-readable classification.
    pub code: ErrorCode,
    /// Developer-facing description. Not a UI string.
    pub message: String,
    /// Structured extras: raw bytes seen, NRC value, port name, and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Adapter capability state at the time of failure, when relevant.
    ///
    /// Boxed, along with [`AimError::provenance`], to keep this type small.
    /// `AimResult` is the return type of nearly every function in the core, so
    /// an error carrying a capability snapshot and a provenance record inline
    /// would widen every `Result` in the codebase for the sake of the path
    /// that is taken least often. Boxing serializes identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_state: Option<Box<crate::AdapterCapabilities>>,
    /// Provenance of the bytes involved, when the failure was a decode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Box<crate::Provenance>>,
}

/// Convenience alias.
pub type AimResult<T> = Result<T, AimError>;

impl AimError {
    /// Build an error with a code and message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        AimError {
            code,
            message: message.into(),
            details: None,
            capability_state: None,
            provenance: None,
        }
    }

    /// Attach structured details.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Attach the adapter capability snapshot.
    pub fn with_capabilities(mut self, caps: crate::AdapterCapabilities) -> Self {
        self.capability_state = Some(Box::new(caps));
        self
    }

    /// Attach provenance for a decode failure.
    pub fn with_provenance(mut self, p: crate::Provenance) -> Self {
        self.provenance = Some(Box::new(p));
        self
    }

    /// True when retrying the same request could plausibly succeed.
    pub fn is_transient(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::TransportTimeout
                | ErrorCode::AdapterBusy
                | ErrorCode::NoData
                | ErrorCode::VehicleNotResponding
        )
    }

    // --- shorthand constructors for the codes used most often ---

    /// `NO DATA` from the adapter.
    pub fn no_data(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NoData, message)
    }

    /// A read/write deadline expired.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TransportTimeout, message)
    }

    /// The request itself was wrong.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    /// An entity lookup missed.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// A bug or an unclassified failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl From<std::io::Error> for AimError {
    fn from(e: std::io::Error) -> Self {
        let code = match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                ErrorCode::TransportTimeout
            }
            std::io::ErrorKind::NotFound => ErrorCode::TransportNotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::TransportOpenFailed,
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                ErrorCode::TransportDisconnected
            }
            _ => ErrorCode::TransportIo,
        };
        AimError::new(code, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_type_stays_small_enough_to_return_everywhere() {
        // AimResult is the return type of nearly every function in the core.
        // The large fields are boxed to keep that cheap; this is the guard that
        // says so, so an added inline field is caught here rather than as a
        // clippy warning across a hundred call sites.
        assert!(
            std::mem::size_of::<AimError>() <= 128,
            "AimError grew to {} bytes; box any large new field",
            std::mem::size_of::<AimError>()
        );
    }

    #[test]
    fn codes_map_to_sane_http_statuses() {
        assert_eq!(ErrorCode::OperationNotAllowed.http_status(), 403);
        assert_eq!(ErrorCode::ConfirmationRequired.http_status(), 403);
        assert_eq!(ErrorCode::NotFound.http_status(), 404);
        assert_eq!(ErrorCode::TransportTimeout.http_status(), 504);
        assert_eq!(ErrorCode::NoData.http_status(), 502);
        assert_eq!(ErrorCode::NotImplemented.http_status(), 501);
    }

    #[test]
    fn code_wire_form_is_snake_case() {
        assert_eq!(ErrorCode::OperationNotAllowed.as_str(), "operation_not_allowed");
        assert_eq!(ErrorCode::NoData.as_str(), "no_data");
    }

    #[test]
    fn io_errors_classify() {
        let e: AimError = std::io::Error::new(std::io::ErrorKind::TimedOut, "slow").into();
        assert_eq!(e.code, ErrorCode::TransportTimeout);
        assert!(e.is_transient());
        let e: AimError = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert_eq!(e.code, ErrorCode::TransportNotFound);
        assert!(!e.is_transient());
    }

    #[test]
    fn errors_serialize_with_details() {
        let e = AimError::new(ErrorCode::NegativeResponse, "service 0x22 rejected")
            .with_details(serde_json::json!({"nrc": 0x31}));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "negative_response");
        assert_eq!(v["details"]["nrc"], 0x31);
        assert!(v.get("capability_state").is_none());
    }
}
