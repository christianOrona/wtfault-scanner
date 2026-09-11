//! Making a problem reportable.
//!
//! # Why this exists
//!
//! On 2026-09-11 an installed build stopped responding and was closed. The only
//! record of it anywhere was a hash in the Windows event log: the app wrote its
//! log to stdout, and a Windows GUI binary has no stdout, so eleven minutes of
//! running left nothing behind at all. Everything in this module exists so the
//! next one can be explained instead of guessed at.
//!
//! # The two halves
//!
//! **A log on disk**, written by the shell, so there is something to read.
//!
//! **A marker file**, written at startup and removed on a clean exit. A process
//! that is killed, freezes, or loses power never removes its own marker, so
//! finding one at startup is proof the last run ended badly — the one kind of
//! failure that cannot report itself while it is happening. A crash that takes
//! the process with it still gets noticed, one launch late.
//!
//! # What leaves the machine
//!
//! Nothing. The report is assembled here, shown to the person, and copied or
//! saved by them. Logs carry VINs, fault codes and file paths that include a
//! person's own name, so sending one anywhere is a separate decision with its
//! own consent, and this module does not make it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where this process writes the things a report is made of.
///
/// Process-global because a crash marker is a fact about the process rather
/// than about any one request, and the updater has to be able to clear it from
/// a background task that holds no state.
static PATHS: OnceLock<SupportPaths> = OnceLock::new();

/// Whether the previous run ended without saying so. Read once at startup,
/// because the marker is overwritten immediately afterwards.
static PREVIOUS: OnceLock<Option<RunMarker>> = OnceLock::new();

/// How much of the log a report carries.
///
/// Enough to cover a startup and whatever went wrong after it, not so much that
/// pasting one into a message is unkind. The full log stays on disk and the
/// report says where.
const REPORT_LOG_LINES: usize = 200;

/// How far back to read looking for those lines. A cap rather than a length:
/// reading a whole log file to show the end of it is how a report of a problem
/// becomes a second problem.
const MAX_TAIL_BYTES: u64 = 64 * 1024;

/// Where the log and the marker live.
#[derive(Debug, Clone)]
pub struct SupportPaths {
    /// Directory holding the rotating log files.
    pub log_dir: PathBuf,
    /// File that exists only while a run is in progress.
    pub marker: PathBuf,
}

/// What was running, and when it started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMarker {
    /// Version of the build that was running.
    pub version: String,
    /// When it started, RFC 3339.
    pub started: String,
    /// Its process id, which is what ties it to an operating-system crash
    /// record if one exists.
    pub pid: u32,
}

/// Point this process at a directory to keep its log and marker in.
///
/// Returns `None` when the directory cannot be created, and the app carries on
/// without a log rather than refusing to start: somebody diagnosing a truck at
/// the side of a road does not need a tool that will not open because it could
/// not write a log file.
pub fn init(data_dir: &Path) -> Option<&'static SupportPaths> {
    let log_dir = data_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!(error = %e, path = %log_dir.display(), "cannot create the log directory; this run will not be logged to disk");
        return None;
    }
    let _ = PATHS.set(SupportPaths {
        log_dir,
        marker: data_dir.join("running.marker"),
    });
    PATHS.get()
}

/// Where the log and marker live, once [`init`] has been called.
pub fn paths() -> Option<&'static SupportPaths> {
    PATHS.get()
}

/// Record that this process is running, and report on the last one.
///
/// Returns the previous run's marker when that run left one behind, which means
/// it never reached a clean exit.
pub fn begin_run() -> Option<RunMarker> {
    let Some(paths) = paths() else {
        let _ = PREVIOUS.set(None);
        return None;
    };

    let previous = std::fs::read_to_string(&paths.marker)
        .ok()
        .and_then(|text| serde_json::from_str::<RunMarker>(&text).ok());

    if let Some(p) = &previous {
        tracing::warn!(
            version = %p.version,
            started = %p.started,
            pid = p.pid,
            "the previous run never shut down cleanly"
        );
    }

    let mine = RunMarker {
        version: crate::update::current_version().to_string(),
        started: aim_types::now().to_rfc3339(),
        pid: std::process::id(),
    };
    match serde_json::to_string(&mine) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&paths.marker, text) {
                tracing::warn!(error = %e, "cannot write the run marker");
            }
        }
        Err(e) => tracing::warn!(error = %e, "cannot serialize the run marker"),
    }

    let _ = PREVIOUS.set(previous.clone());
    previous
}

/// Say goodbye.
///
/// Called on a clean exit, and by the updater before it replaces this build: an
/// update is not a crash, and reporting one as a crash on the next launch would
/// train somebody to ignore the warning that matters.
pub fn end_run() {
    if let Some(paths) = paths() {
        if let Err(e) = std::fs::remove_file(&paths.marker) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "cannot remove the run marker");
            }
        }
    }
}

/// The previous run, when it ended badly. `None` when it exited cleanly, when
/// this is the first run, or when [`begin_run`] has not been called.
pub fn previous_bad_run() -> Option<&'static RunMarker> {
    PREVIOUS.get().and_then(|p| p.as_ref())
}

/// The most recently written log file.
fn newest_log(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, p)| p)
}

/// The end of the newest log file.
fn tail_of(path: &Path, lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(MAX_TAIL_BYTES);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }

    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    // Lossy on purpose: seeking to a byte offset can land mid-character, and a
    // replacement character in one line of a log is better than no log.
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// Everything a person would be asked for when reporting a problem.
#[derive(Debug, Clone, Serialize)]
pub struct SupportReport {
    /// When this report was assembled, RFC 3339.
    pub generated: String,
    /// The running build.
    pub app_version: String,
    /// Operating system and architecture this build was compiled for.
    pub os: String,
    /// Set when the previous run never removed its marker.
    pub previous_run_ended_badly: Option<RunMarker>,
    /// Folder holding the logs, when this build keeps them.
    pub log_dir: Option<String>,
    /// The log file the tail was read from.
    pub log_file: Option<String>,
    /// Whether anything has been written to a log yet.
    pub has_log: bool,
    /// The whole report as plain text, ready to copy or save. Assembled here
    /// rather than in the interface so that what is saved to a file and what is
    /// copied to a clipboard cannot drift apart.
    pub text: String,
}

/// Assemble a report from what this machine knows.
pub fn report() -> SupportReport {
    let generated = aim_types::now().to_rfc3339();
    let app_version = crate::update::current_version().to_string();
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let previous = previous_bad_run().cloned();

    let log_dir = paths().map(|p| p.log_dir.display().to_string());
    let log_file = paths().and_then(|p| newest_log(&p.log_dir));
    let tail = log_file
        .as_deref()
        .map(|p| tail_of(p, REPORT_LOG_LINES))
        .unwrap_or_default();

    let mut text = String::new();
    text.push_str("WTFault Scanner problem report\n");
    text.push_str(&format!("Generated : {generated}\n"));
    text.push_str(&format!("Version   : {app_version}\n"));
    text.push_str(&format!("System    : {os}\n"));

    match &previous {
        Some(p) => text.push_str(&format!(
            "\nThe previous run ended without shutting down cleanly.\n  version {}, started {}, process {}\n",
            p.version, p.started, p.pid
        )),
        None => text.push_str("\nThe previous run shut down cleanly.\n"),
    }

    match &log_dir {
        Some(dir) => text.push_str(&format!("\nFull logs: {dir}\n")),
        None => text.push_str("\nThis build is not keeping logs on disk.\n"),
    }

    if tail.is_empty() {
        text.push_str("\nNothing has been written to a log yet.\n");
    } else {
        text.push_str(&format!(
            "\n--- last {REPORT_LOG_LINES} log lines ---\n{tail}\n"
        ));
    }

    SupportReport {
        generated,
        app_version,
        os,
        previous_run_ended_badly: previous,
        log_dir,
        log_file: log_file.map(|p| p.display().to_string()),
        has_log: !tail.is_empty(),
        text,
    }
}

/// Show the log folder in the desktop's own file manager.
///
/// Takes no argument on purpose. The path comes from this process's own
/// configuration, so there is nothing a caller can point it at.
pub fn reveal_logs() -> Result<String, String> {
    let paths = paths().ok_or_else(|| String::from("this build is not keeping logs on disk"))?;
    let dir = &paths.log_dir;

    #[cfg(target_os = "windows")]
    let command = ("explorer.exe", dir.as_os_str().to_os_string());
    #[cfg(target_os = "macos")]
    let command = ("open", dir.as_os_str().to_os_string());
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = ("xdg-open", dir.as_os_str().to_os_string());

    // Explorer answers 1 for a perfectly successful open, so the exit status is
    // not worth reading; failing to start the program at all is the real error.
    std::process::Command::new(command.0)
        .arg(command.1)
        .spawn()
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;

    Ok(dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_returns_the_end_of_a_file_not_the_start() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wtfault.log");
        let body: String = (0..500).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, body).expect("write");

        let tail = tail_of(&path, 10);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[9], "line 499");
        assert!(!tail.contains("line 0\n"), "the start should not be in a tail");
    }

    #[test]
    fn a_tail_of_a_file_shorter_than_the_request_is_the_whole_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wtfault.log");
        std::fs::write(&path, "one\ntwo\n").expect("write");

        assert_eq!(tail_of(&path, 200), "one\ntwo");
    }

    #[test]
    fn a_missing_file_tails_to_nothing_rather_than_failing() {
        assert_eq!(tail_of(Path::new("no-such-log-file.log"), 10), "");
    }

    /// The newest file wins, whatever it is called. Log names carry dates, and
    /// sorting them as strings would work until the day a rotation scheme
    /// changed.
    #[test]
    fn the_newest_log_is_the_one_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = dir.path().join("wtfault.2026-09-10.log");
        let new = dir.path().join("wtfault.2026-09-11.log");
        std::fs::write(&old, "old").expect("write");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&new, "new").expect("write");

        assert_eq!(newest_log(dir.path()), Some(new));
    }

    #[test]
    fn a_report_without_any_configured_paths_still_produces_text() {
        // PATHS is process-global and other tests may have set it; what matters
        // is that a report is always a complete document rather than an error.
        let r = report();
        assert!(r.text.contains("WTFault Scanner problem report"));
        assert!(r.text.contains(&r.app_version));
        assert_eq!(r.app_version, env!("CARGO_PKG_VERSION"));
    }
}
