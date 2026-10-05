//! Which software a module runs, and whether a file is that software.
//!
//! Four routes. One reads a module's software identity from the vehicle, and
//! only reads. The others never touch the vehicle at all: they look through
//! calibration files on this computer, judge them against an identity they
//! are handed, and take a file a person supplies. See `aim_calibration` for
//! the rules, and `docs/CALIBRATION.md`.
//!
//! Nothing here decides anything. The search is `aim_calibration::Library`,
//! the same one the `find_calibration` tool an AI model calls goes through,
//! so a screen and a model asked the same question get the same answer.
//!
//! Nothing here fetches a file from a network, and nothing here can write to a
//! module: there is no route in this file that could.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use aim_calibration::{AddError, CalibrationIdentity, Library};
use aim_types::ToolResult;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// The largest file a person may add. A module's whole flash is a few
/// megabytes; this leaves room and still refuses a disk image.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// The calibration files on this computer, or the answer for a build that has
/// no place for them.
fn library(state: &AppState) -> ApiResult<Library> {
    state.config.calibration_library().ok_or_else(|| {
        ApiError::bad_request(
            "this build has no folder for calibration files, so there is nowhere to look",
        )
    })
}

/// `GET /api/v1/modules/{key}/calibration`
///
/// Read everything the module will say about the software it runs. A vehicle
/// read, and only a read: OBD-II service 09 and UDS ReadDataByIdentifier.
pub async fn module_calibration(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ToolResult>> {
    Ok(Json(state.with_service(move |s| s.read_calibration_identity(&key, "user:api")).await?))
}

/// `GET /api/v1/calibration`
///
/// Where calibration files are looked for, and what has been kept. Reads this
/// computer's disk and nothing else.
pub async fn calibration_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let library = library(&state)?;
    let status = tokio::task::spawn_blocking(move || library.status())
        .await
        .map_err(|e| ApiError::internal(format!("the folder could not be read: {e}")))?;
    Ok(Json(json!(status)))
}

/// What to look for.
#[derive(Debug, Deserialize)]
pub struct FindBody {
    /// The identity to match files against, as `GET
    /// /modules/{key}/calibration` returned it.
    identity: CalibrationIdentity,
}

/// `POST /api/v1/calibration/find`
///
/// Look through every calibration source on this computer for the calibration
/// an identity names. Does not touch the vehicle, so it works with an identity
/// read earlier. Finding nothing is a `200` with `NO_ARTIFACT_FOUND`: for most
/// modules that is the true answer. A source that could not be searched is in
/// the answer as that, and is never folded into "nothing found".
pub async fn find_calibration(
    State(state): State<AppState>,
    Json(body): Json<FindBody>,
) -> ApiResult<Json<Value>> {
    let library = library(&state)?;
    let now = aim_types::now().to_rfc3339();
    // Hashing and unpacking a folder of files is disk work.
    let found = tokio::task::spawn_blocking(move || library.find(&body.identity, &now))
        .await
        .map_err(|e| ApiError::internal(format!("the search did not finish: {e}")))?;
    Ok(Json(json!(found)))
}

/// What the file being added is called.
#[derive(Debug, Deserialize)]
pub struct AddQuery {
    /// The file's name, and only its name: no folder.
    name: String,
}

/// `POST /api/v1/calibration/files?name=<file name>`
///
/// Put a calibration file the person has into their folder, as it is: the
/// body is the file. It is copied and nothing else. It is not opened,
/// converted or sent anywhere, and it is not thereby anybody's word about
/// what the file is: a file added this way has only its name until a metadata
/// file says more.
///
/// A file of that name already there is never replaced by a different one.
pub async fn add_calibration_file(
    State(state): State<AppState>,
    Query(q): Query<AddQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if content_type != "application/octet-stream" {
        return Err(ApiError::bad_request(
            "send the file as the body, with Content-Type: application/octet-stream",
        ));
    }
    let library = library(&state)?;
    let added = tokio::task::spawn_blocking(move || library.add_file(&q.name, &body))
        .await
        .map_err(|e| ApiError::internal(format!("the file was not added: {e}")))?;
    match added {
        Ok(added) => {
            tracing::info!(file = %added.filename, sha256 = %added.sha256, "a calibration file was added by hand");
            Ok(Json(json!(added)))
        }
        // The folder not taking the file is ours. Everything else is about
        // the file that was sent.
        Err(AddError::Io(e)) => {
            Err(ApiError::internal(format!("the folder could not be written: {e}")))
        }
        Err(e) => Err(ApiError::bad_request(e.message())),
    }
}
