//! Getting a finished file to the person.
//!
//! On a desktop an export is written to the Downloads folder and the app says
//! where it went. A phone app has no Downloads folder it may write to: what it
//! keeps is in private storage that nothing else on the phone can open, so a
//! session recorded there could not leave it at all. There the shell supplies a
//! [`FileHandoff`] instead. The file is written to a folder the shell can hand
//! files over from, and the shell offers it to the person, which on Android is
//! the share sheet: they pick Drive, mail, or whatever else is installed.
//!
//! Either way the file goes only where the person puts it. Nothing here
//! uploads anything.

use crate::error::{ApiError, ApiResult};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How a shell with no Downloads folder hands a file to the person.
pub trait FileHandoff: std::fmt::Debug + Send + Sync {
    /// The folder a file has to be in for the shell to hand it over.
    fn staging_dir(&self) -> PathBuf;

    /// Offer a file in that folder to the person.
    ///
    /// Returns once it has been offered, not once they have chosen: nothing
    /// reports back whether they kept it or dismissed the offer.
    fn offer(&self, path: &Path, mime: &str) -> Result<(), String>;
}

/// How long a file offered to the person is kept.
///
/// It cannot be removed as soon as it is offered, because whatever they chose
/// may still be reading it. A day covers the slowest upload and still stops
/// database copies piling up in the phone's storage.
const STAGED_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// Where a file went.
#[derive(Debug, Serialize)]
pub struct Delivered {
    /// The file's full path.
    pub path: String,
    /// The folder it is in.
    pub directory: String,
    /// Its name.
    pub filename: String,
    /// Its size.
    pub bytes: u64,
    /// True when it was offered to the person to send somewhere, rather than
    /// saved where they can find it. `path` is then inside the app.
    pub shared: bool,
}

/// The place one export is written to, and the way it leaves from there.
#[derive(Debug)]
pub struct Outbox<'a> {
    dir: PathBuf,
    handoff: Option<&'a dyn FileHandoff>,
}

impl<'a> Outbox<'a> {
    /// The shell's staging folder when it hands files over itself, otherwise
    /// the person's Downloads folder.
    pub fn open(handoff: Option<&'a dyn FileHandoff>) -> ApiResult<Outbox<'a>> {
        let dir = match handoff {
            Some(h) => h.staging_dir(),
            None => directories::UserDirs::new()
                .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
                .ok_or_else(|| ApiError::internal("cannot locate the Downloads folder"))?,
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;
        if handoff.is_some() {
            remove_stale(&dir, STAGED_FOR);
        }
        Ok(Outbox { dir, handoff })
    }

    /// Where a file called `name` goes. The name is the caller's; see
    /// [`safe_name`] for what is accepted.
    pub fn path(&self, name: &str) -> ApiResult<PathBuf> {
        Ok(self.dir.join(safe_name(name)?))
    }

    /// Write `content` as `name` and deliver it.
    pub fn deliver_text(&self, name: &str, content: &str) -> ApiResult<Delivered> {
        let path = self.path(name)?;
        std::fs::write(&path, content.as_bytes())
            .map_err(|e| ApiError::internal(format!("cannot write {}: {e}", path.display())))?;
        self.deliver(&path)
    }

    /// Deliver a file already written to [`Outbox::path`].
    pub fn deliver(&self, path: &Path) -> ApiResult<Delivered> {
        let bytes = std::fs::metadata(path)
            .map_err(|e| ApiError::internal(format!("cannot read {}: {e}", path.display())))?
            .len();
        if let Some(handoff) = self.handoff {
            handoff.offer(path, mime_for(path)).map_err(|e| {
                ApiError::internal(format!("the file was written but could not be shared: {e}"))
            })?;
        }
        tracing::info!(path = %path.display(), bytes, shared = self.handoff.is_some(), "wrote an export");
        Ok(Delivered {
            path: path.display().to_string(),
            directory: self.dir.display().to_string(),
            filename: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            bytes,
            shared: self.handoff.is_some(),
        })
    }
}

/// Reduce a caller-supplied name to a bare filename inside the target folder.
///
/// The caller is this app's own UI, but that is not a reason to trust the
/// string: a path traversal here writes anywhere the user can write, and the
/// check is three lines.
pub fn safe_name(raw: &str) -> ApiResult<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let ok = !base.is_empty()
        && base != "."
        && base != ".."
        && !base.contains("..")
        && base.len() <= 120
        && base.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
    if ok {
        Ok(base.to_string())
    } else {
        Err(ApiError::bad_request(format!("unusable file name {raw:?}")))
    }
}

/// The type a share target is told the file is. It decides which apps are
/// offered, so anything not plainly text is called a file and nothing more.
fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("csv") => "text/csv",
        Some("txt" | "transcript") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Remove files in the staging folder that were offered long enough ago.
///
/// Best effort: a file that will not go is left for the next export, and
/// failing to tidy up is no reason to refuse this one.
fn remove_stale(dir: &Path, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age > older_than);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct Fake {
        dir: PathBuf,
        offered: Mutex<Vec<(PathBuf, String)>>,
        refuse: bool,
    }

    impl Fake {
        fn in_dir(dir: &Path) -> Fake {
            Fake { dir: dir.join("staging"), offered: Mutex::new(Vec::new()), refuse: false }
        }
    }

    impl FileHandoff for Fake {
        fn staging_dir(&self) -> PathBuf {
            self.dir.clone()
        }

        fn offer(&self, path: &Path, mime: &str) -> Result<(), String> {
            if self.refuse {
                return Err("nothing can receive it".into());
            }
            self.offered.lock().unwrap().push((path.to_path_buf(), mime.to_string()));
            Ok(())
        }
    }

    #[test]
    fn a_file_is_staged_and_offered_with_its_type() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::in_dir(tmp.path());
        let outbox = Outbox::open(Some(&fake)).unwrap();

        let d = outbox.deliver_text("wtfault-codes.csv", "code,status\r\n").unwrap();

        assert!(d.shared);
        assert_eq!(d.filename, "wtfault-codes.csv");
        assert_eq!(d.bytes, 13);
        let staged = fake.dir.join("wtfault-codes.csv");
        assert_eq!(std::fs::read_to_string(&staged).unwrap(), "code,status\r\n");
        assert_eq!(*fake.offered.lock().unwrap(), vec![(staged, "text/csv".to_string())]);
    }

    #[test]
    fn a_file_nothing_could_receive_is_an_error_not_a_success() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake { refuse: true, ..Fake::in_dir(tmp.path()) };
        let outbox = Outbox::open(Some(&fake)).unwrap();

        let err = outbox.deliver_text("report.txt", "text").unwrap_err();
        assert!(format!("{err:?}").contains("nothing can receive it"), "{err:?}");
    }

    #[test]
    fn a_name_cannot_leave_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::in_dir(tmp.path());
        let outbox = Outbox::open(Some(&fake)).unwrap();

        // Separators are stripped rather than followed.
        assert_eq!(outbox.path("../../etc/passwd").unwrap(), fake.dir.join("passwd"));
        assert_eq!(outbox.path(r"C:\Windows\x.csv").unwrap(), fake.dir.join("x.csv"));
        for bad in ["", "..", "a..b", "x\0.csv", "dir/", "semi;colon.csv"] {
            assert!(outbox.path(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(fake.offered.lock().unwrap().is_empty());
    }

    #[test]
    fn only_plain_text_is_called_text() {
        assert_eq!(mime_for(Path::new("a.csv")), "text/csv");
        assert_eq!(mime_for(Path::new("a.txt")), "text/plain");
        assert_eq!(mime_for(Path::new("a.transcript")), "text/plain");
        assert_eq!(mime_for(Path::new("a.sqlite")), "application/octet-stream");
        assert_eq!(mime_for(Path::new("a")), "application/octet-stream");
    }

    #[test]
    fn old_staged_files_are_removed_and_recent_ones_kept() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("yesterday.sqlite"), b"old").unwrap();
        std::fs::create_dir(tmp.path().join("a-folder")).unwrap();

        // Nothing here is older than an hour.
        remove_stale(tmp.path(), Duration::from_secs(3600));
        assert!(tmp.path().join("yesterday.sqlite").exists());

        // Everything here is older than no time at all; folders are not files.
        std::thread::sleep(Duration::from_millis(20));
        remove_stale(tmp.path(), Duration::ZERO);
        assert!(!tmp.path().join("yesterday.sqlite").exists());
        assert!(tmp.path().join("a-folder").exists());
    }
}
