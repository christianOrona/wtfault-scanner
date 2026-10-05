//! Android's share sheet, as the way a file leaves the app.
//!
//! Everything the app keeps on a phone is in private storage that no other app
//! and no file manager can open, so a session recorded there has no way out by
//! itself. The core writes an export into the app's cache, and
//! `SharePlugin.kt` (in `gen/android`) hands that one file to whatever the
//! person picks from the share sheet: Drive, mail, a nearby device.
//!
//! The app sends nothing anywhere. It only ever offers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aim_api::handoff::FileHandoff;
use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Wry};

/// The Kotlin side's handle, kept where [`handoff`] can find it.
struct Plugin(PluginHandle<Wry>);

/// The Tauri plugin that registers the Kotlin side.
pub fn init() -> TauriPlugin<Wry> {
    Builder::new("file-share")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.wtfault.scanner", "SharePlugin")?;
            app.manage(Plugin(handle));
            Ok(())
        })
        .build()
}

/// The share sheet as the core's way of handing a file over, or `None` when
/// there is nowhere to stage one. Exports then fail saying so.
pub fn handoff(app: &AppHandle) -> Option<Arc<dyn FileHandoff>> {
    let handle = app.try_state::<Plugin>()?.0.clone();
    // The cache, because it is the one folder `file_paths.xml` lets the app
    // grant another app a read of.
    let staging = app.path().app_cache_dir().ok()?.join("exports");
    Some(Arc::new(ShareSheet { handle, staging }))
}

struct ShareSheet {
    handle: PluginHandle<Wry>,
    staging: PathBuf,
}

impl std::fmt::Debug for ShareSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareSheet").field("staging", &self.staging).finish()
    }
}

#[derive(Serialize)]
struct ShareArgs<'a> {
    path: &'a str,
    mime: &'a str,
}

#[derive(Debug, Deserialize)]
struct Ack {}

impl FileHandoff for ShareSheet {
    fn staging_dir(&self) -> PathBuf {
        self.staging.clone()
    }

    fn offer(&self, path: &Path, mime: &str) -> Result<(), String> {
        let path = path.to_str().ok_or("the file's path is not UTF-8")?;
        self.handle
            .run_mobile_plugin::<Ack>("share", ShareArgs { path, mime })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
