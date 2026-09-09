//! The HTTP error envelope.
//!
//! Handoff §13: *"The API should return structured errors, capability state,
//! and provenance rather than UI-specific messages."* So every failure that
//! leaves this server has the same shape:
//!
//! ```json
//! {
//!   "error": {
//!     "code": "no_data",
//!     "message": "adapter answered \"01FE\" with no_data",
//!     "details": { "command": "01FE", "lines": ["NO DATA"] },
//!     "capability_state": { "elm327_compatible": true, "...": "..." },
//!     "provenance": { "raw_hex": "...", "decoder_id": "...", "...": "..." }
//!   }
//! }
//! ```
//!
//! `code` is a stable snake_case discriminant the UI branches on. `message` is
//! developer-facing and may change; it is not a string to show a user verbatim.

use aim_types::{AimError, ErrorCode};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// An error on its way out of the API.
#[derive(Debug, Clone)]
pub struct ApiError(pub AimError);

impl ApiError {
    /// Wrap a core error.
    pub fn new(error: AimError) -> Self {
        ApiError(error)
    }

    /// No adapter session is active.
    pub fn no_session() -> Self {
        ApiError(AimError::new(
            ErrorCode::NoActiveSession,
            "no adapter is connected; POST /api/v1/adapter/connect first",
        ))
    }

    /// The caller sent something malformed.
    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError(AimError::bad_request(message))
    }

    /// A bug in the server.
    pub fn internal(message: impl Into<String>) -> Self {
        ApiError(AimError::internal(message))
    }

    /// A feature that exists as a documented seam but is not built yet.
    ///
    /// Used for the agent and active-test endpoints. They answer with a real
    /// code and an explanation rather than a 404, so the UI can render
    /// "not in this build" instead of "your request was wrong".
    pub fn not_implemented(message: impl Into<String>) -> Self {
        ApiError(AimError::new(ErrorCode::NotImplemented, message))
    }
}

impl From<AimError> for ApiError {
    fn from(e: AimError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(serde_json::json!({ "error": self.0 }))).into_response()
    }
}

/// Result alias for handlers.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_nests_the_error_under_a_stable_key() {
        let e = ApiError::new(
            AimError::new(ErrorCode::NoData, "nothing answered")
                .with_details(serde_json::json!({ "command": "01FE" })),
        );
        let response = e.clone().into_response();
        assert_eq!(response.status(), 502);

        let value = serde_json::to_value(serde_json::json!({ "error": e.0 })).unwrap();
        assert_eq!(value["error"]["code"], "no_data");
        assert_eq!(value["error"]["details"]["command"], "01FE");
    }

    #[test]
    fn status_codes_follow_the_error_code() {
        assert_eq!(ApiError::no_session().into_response().status(), StatusCode::CONFLICT);
        // NotFound reaches the API from the store rather than being
        // constructed here, so it is checked through a core error.
        assert_eq!(
            ApiError::new(AimError::not_found("no session")).into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(ApiError::bad_request("x").into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            ApiError::not_implemented("x").into_response().status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
