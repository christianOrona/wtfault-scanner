//! Which software a module runs, and whether a file is that software.
//!
//! Three routes. One reads a module's software identity from the vehicle, and
//! only reads. The other two never touch the vehicle at all: they look through
//! calibration files on this computer and judge them against an identity they
//! are handed. See `aim_calibration` for the rules, and `docs/CALIBRATION.md`.
//!
//! Nothing here fetches a file from a network, and nothing here can write to a
//! module: there is no route in this file that could.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use aim_calibration::{
    resolve, ArtifactFormat, Cache, CalibrationIdentity, CalibrationSource, DirectorySource,
};
use aim_types::ToolResult;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// What a person finds in the folder the first time they open it.
const FOLDER_README: &str = "Calibration files for WTFault Scanner\r\n\
\r\n\
Put calibration files you are entitled to have in this folder: .rwd, .bin, .gz,\r\n\
.hex or .s19. The app never downloads one and never writes one to a vehicle. It\r\n\
only tells you whether a file here is the calibration a module reports.\r\n\
\r\n\
A file's name proves nothing, so a file with nothing but a name can only ever be\r\n\
a PARTIAL match. To let the app say more, put a metadata file beside it, named\r\n\
<the file's whole name>.json, stating what the file is and where you got that\r\n\
from. Every key is optional:\r\n\
\r\n\
  {\r\n\
    \"sha256\": \"the SHA-256 the file should have\",\r\n\
    \"calibration_id\": \"the calibration identification\",\r\n\
    \"hardware_number\": [\"the hardware it is for\"],\r\n\
    \"make\": \"Honda\", \"model\": \"Odyssey\", \"model_year\": [2023],\r\n\
    \"engine\": \"3.5L V6\"\r\n\
  }\r\n\
\r\n\
The app reports an EXACT match only when a module's own calibration\r\n\
identification equals the calibration_id declared here and nothing disagrees.\r\n";

/// The two folders calibration files live in, made on first use.
struct Places {
    /// Where the person puts files.
    folder: PathBuf,
    /// Where files that matched are kept, by hash.
    kept: PathBuf,
}

fn places(state: &AppState) -> ApiResult<Places> {
    let Some(root) = state.config.calibrations_dir.clone() else {
        return Err(ApiError::bad_request(
            "this build has no folder for calibration files, so there is nowhere to look",
        ));
    };
    let places = Places { folder: root.join("files"), kept: root.join("kept") };
    std::fs::create_dir_all(&places.folder).map_err(|e| {
        ApiError::internal(format!("cannot create {}: {e}", places.folder.display()))
    })?;
    let readme = places.folder.join("README.txt");
    if !readme.exists() {
        // Best effort: the folder works without it.
        let _ = std::fs::write(&readme, FOLDER_README);
    }
    Ok(places)
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
    let places = places(&state)?;
    let cache = Cache::open(&places.kept)
        .map_err(|e| ApiError::internal(format!("cannot open the calibration cache: {e}")))?;
    let folder = DirectorySource::new("folder", &places.folder);
    let kept = cache.list().unwrap_or_default();
    Ok(Json(json!({
        "folder": places.folder.display().to_string(),
        "sources": [folder.info(), CalibrationSource::info(&cache)],
        "kept": kept,
        "formats": ArtifactFormat::SUPPORTED,
        // Said in the data, so no screen has to remember to say it.
        "uses_network": false,
        "writes_to_vehicle": false,
    })))
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
/// Look through the calibration folder and the kept files for the calibration
/// an identity names. Does not touch the vehicle, so it works with an identity
/// read earlier. Finding nothing is a `200` with `NO_ARTIFACT_FOUND`: for most
/// modules that is the true answer.
pub async fn find_calibration(
    State(state): State<AppState>,
    Json(body): Json<FindBody>,
) -> ApiResult<Json<Value>> {
    let places = places(&state)?;
    let now = aim_types::now().to_rfc3339();
    let module = body.identity.module_key.clone();
    // Hashing a folder of files is disk work.
    let (resolution, folder) = tokio::task::spawn_blocking(move || {
        let cache = Cache::open(&places.kept)
            .map_err(|e| ApiError::internal(format!("cannot open the calibration cache: {e}")))?;
        let folder = DirectorySource::new("folder", &places.folder);
        let sources: [&dyn CalibrationSource; 2] = [&folder, &cache];
        Ok::<_, ApiError>((
            resolve(&body.identity, &sources, Some(&cache), &now),
            places.folder.display().to_string(),
        ))
    })
    .await
    .map_err(|e| ApiError::internal(format!("the search did not finish: {e}")))??;

    Ok(Json(json!({ "module": module, "resolution": resolution, "folder": folder })))
}
