//! The calibration files on this computer, and the one way they are searched.
//!
//! Everything that looks for a calibration goes through [`Library::find`]:
//! the screen, the HTTP route, and the tool an AI model calls. There is one
//! implementation, so there is one answer, and a change to how files are
//! judged cannot reach one caller and miss another.
//!
//! A library is a folder with three things in it:
//!
//! - `files/`: where the person puts calibration files they have.
//! - `kept/`: the files that matched a module, kept by their SHA-256.
//! - `sources.json`: optional, and written by the person. It says where a
//!   manufacturer's service tool keeps its files when that is not where this
//!   crate looks for it.
//!
//! Nothing here reaches a network or a vehicle.

use crate::artifact::{ArtifactRecord, SourceKind};
use crate::cache::Cache;
use crate::format::{ArtifactFormat, Named};
use crate::identity::CalibrationIdentity;
use crate::resolve::{resolve, Resolution};
use crate::source::{CalibrationSource, DirectorySource, Offered, SourceError, SourceInfo};
use crate::validate::MAX_ARTIFACT_BYTES;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a person finds in the folder the first time they open it.
const FOLDER_README: &str = "Calibration files for WTFault Scanner\r\n\
\r\n\
Put calibration files you are entitled to have in this folder: .rwd, .rwd.gz,\r\n\
.bin, .gz, .hex or .s19. The app never downloads one and never writes one to a\r\n\
vehicle. It only tells you whether a file here is the calibration a module\r\n\
reports.\r\n\
\r\n\
A file's name proves nothing, so a file with nothing but a name can only ever be\r\n\
a PARTIAL match. To let the app say more, put a metadata file beside it, named\r\n\
<the file's whole name>.json, stating what the file is. Every key is optional:\r\n\
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
identification equals the calibration_id declared here and nothing disagrees.\r\n\
It shows that match as resting on what YOU declared. Nothing in this folder can\r\n\
make the app say a file came from the manufacturer: it has no way to check that,\r\n\
and it says so on every result.\r\n";

/// The service tools whose calibration folders this crate knows how to look
/// for: an id, a name, and where the tool is described as installing them,
/// under the 32-bit Program Files folder.
///
/// One entry, from the public description of Honda's J2534 Rewrite
/// application. It is a place to look, checked for before it is used, and
/// `sources.json` overrides it.
const KNOWN_TOOLS: [(&str, &str, [&str; 3]); 1] = [(
    "honda-j2534-rewrite",
    "Honda J2534 Rewrite's calibration folder",
    ["Honda", "J2534 Pass Thru", "CalibFiles"],
)];

/// One tool folder, as `sources.json` states it.
#[derive(Debug, Clone, Deserialize)]
struct ToolFolder {
    id: String,
    #[serde(default)]
    name: Option<String>,
    path: PathBuf,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

/// `sources.json`.
#[derive(Debug, Clone, Default, Deserialize)]
struct SourcesFile {
    #[serde(default)]
    tool_folders: Vec<ToolFolder>,
}

/// A source that stands in for one that could not even be set up, so that the
/// failure is reported where a person looks for what was searched.
struct Unreachable {
    info: SourceInfo,
    problem: String,
}

impl CalibrationSource for Unreachable {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn search(&self, _identity: &CalibrationIdentity, _now: &str) -> Result<Offered, SourceError> {
        Err(SourceError::Failed(self.problem.clone()))
    }
}

/// The calibration files on this computer.
#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
    /// Where service tools are looked for. `None` when this computer has no
    /// such folder, as anything but Windows does not.
    program_files: Option<PathBuf>,
}

/// The result of looking for one module's calibration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// The module it was looked for.
    pub module: String,
    /// The folder a person puts files in.
    pub folder: String,
    /// What was searched and what was found.
    pub resolution: Resolution,
}

/// Where files are looked for, and what has been kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// The folder a person puts files in.
    pub folder: String,
    /// Every source that would be searched, in order.
    pub sources: Vec<SourceInfo>,
    /// The files already kept.
    pub kept: Vec<ArtifactRecord>,
    /// The file names this build reads, by extension.
    pub formats: Vec<&'static str>,
    /// Always false: no source reaches a network. Said in the data, so no
    /// screen has to remember to say it.
    pub uses_network: bool,
    /// Always false: nothing here can write to a vehicle.
    pub writes_to_vehicle: bool,
}

/// A file a person added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Added {
    /// The name it has in the folder.
    pub filename: String,
    /// Its SHA-256.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// True when the very same file was already there, and nothing was written.
    pub already_there: bool,
    /// Where it is.
    pub path: String,
}

/// Why a file could not be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// The name is not one a file in this folder may have.
    BadName(String),
    /// The name is not a kind of file treated as a calibration.
    NotACalibration(String),
    /// It is empty, or larger than any calibration.
    BadSize(String),
    /// A different file of that name is already there. It is not replaced.
    NameTaken(String),
    /// The folder could not be written.
    Io(String),
}

impl AddError {
    /// The reason, in words.
    pub fn message(&self) -> &str {
        match self {
            AddError::BadName(m)
            | AddError::NotACalibration(m)
            | AddError::BadSize(m)
            | AddError::NameTaken(m)
            | AddError::Io(m) => m,
        }
    }
}

impl Library {
    /// The library kept under `root`. Nothing is created until it is used.
    pub fn at(root: impl Into<PathBuf>) -> Library {
        Library {
            root: root.into(),
            program_files: std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        }
    }

    /// The same library, looking for service tools under `dir` and not where
    /// this computer keeps its programs. For a test, and for a computer whose
    /// tools are somewhere unusual.
    pub fn with_program_files(mut self, dir: Option<PathBuf>) -> Library {
        self.program_files = dir;
        self
    }

    /// Where the person puts files.
    pub fn folder(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Where files that matched are kept, by hash.
    pub fn kept(&self) -> PathBuf {
        self.root.join("kept")
    }

    /// The file that says where a tool's folder is, when the person wrote one.
    pub fn sources_file(&self) -> PathBuf {
        self.root.join("sources.json")
    }

    /// Make the folder a person puts files in, with a note in it saying what
    /// it is for.
    pub fn prepare(&self) -> std::io::Result<()> {
        let folder = self.folder();
        std::fs::create_dir_all(&folder)?;
        let readme = folder.join("README.txt");
        if !readme.exists() {
            // Best effort: the folder works without it.
            let _ = std::fs::write(&readme, FOLDER_README);
        }
        Ok(())
    }

    /// The service-tool folders to search: the ones this crate knows how to
    /// look for, with whatever `sources.json` says about them, and any others
    /// it lists. A `sources.json` that cannot be read is an error, not an
    /// empty list: somebody wrote it meaning something.
    fn tool_sources(&self) -> Result<Vec<DirectorySource>, String> {
        let path = self.sources_file();
        let stated: SourcesFile = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} could not be read: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SourcesFile::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };

        let mut sources = Vec::new();
        for (id, name, below_program_files) in KNOWN_TOOLS {
            if stated.tool_folders.iter().any(|t| t.id == id) {
                continue;
            }
            // Looked for, not assumed: a tool that is not installed has no
            // folder, and that is reported as nowhere to look.
            let usual = self
                .program_files
                .as_ref()
                .map(|p| below_program_files.iter().fold(p.clone(), |dir, part| dir.join(part)))
                .filter(|p| p.is_dir());
            sources.push(DirectorySource::tool_installation(id, name, usual));
        }
        for tool in stated.tool_folders {
            let name = tool.name.clone().unwrap_or_else(|| {
                KNOWN_TOOLS
                    .iter()
                    .find(|(id, _, _)| *id == tool.id)
                    .map(|(_, name, _)| name.to_string())
                    .unwrap_or_else(|| format!("A service tool's calibration folder ({})", tool.id))
            });
            sources.push(
                DirectorySource::tool_installation(tool.id, name, Some(tool.path))
                    .enabled(tool.enabled),
            );
        }
        Ok(sources)
    }

    /// Every source a search would use, in the order it would use them, each
    /// as a boxed source so a failure to set one up is itself a source.
    fn sources(&self) -> (Vec<Box<dyn CalibrationSource>>, Option<Cache>) {
        let mut sources: Vec<Box<dyn CalibrationSource>> = Vec::new();

        let folder = DirectorySource::new("folder", self.folder());
        match self.prepare() {
            Ok(()) => sources.push(Box::new(folder)),
            Err(e) => sources.push(Box::new(Unreachable {
                info: folder.info(),
                problem: format!("{} could not be made: {e}", self.folder().display()),
            })),
        }

        match self.tool_sources() {
            Ok(tools) => {
                sources.extend(tools.into_iter().map(|t| Box::new(t) as Box<dyn CalibrationSource>))
            }
            Err(problem) => sources.push(Box::new(Unreachable {
                info: SourceInfo {
                    id: String::from("sources.json"),
                    name: String::from("Service tool folders listed in sources.json"),
                    kind: SourceKind::ToolInstallation,
                    location: self.sources_file().display().to_string(),
                    uses_network: false,
                    enabled: true,
                },
                problem,
            })),
        }

        let cache = match Cache::open(self.kept()) {
            Ok(cache) => {
                sources.push(Box::new(cache.clone()));
                Some(cache)
            }
            Err(e) => {
                sources.push(Box::new(Unreachable {
                    info: SourceInfo {
                        id: String::from("cache"),
                        name: String::from("Calibration files already kept by this app"),
                        kind: SourceKind::Kept,
                        location: self.kept().display().to_string(),
                        uses_network: false,
                        enabled: true,
                    },
                    problem: format!("{} could not be opened: {e}", self.kept().display()),
                }));
                None
            }
        };
        (sources, cache)
    }

    /// Where files are looked for, and what has been kept.
    pub fn status(&self) -> Status {
        let (sources, cache) = self.sources();
        Status {
            folder: self.folder().display().to_string(),
            sources: sources.iter().map(|s| s.info()).collect(),
            kept: cache.and_then(|c| c.list().ok()).unwrap_or_default(),
            formats: ArtifactFormat::EXTENSIONS.to_vec(),
            uses_network: false,
            writes_to_vehicle: false,
        }
    }

    /// Look for the calibration an identity names, in every source.
    ///
    /// Never fails. A source that could not be searched is in the result as
    /// one that could not be searched, and finding nothing is a result.
    pub fn find(&self, identity: &CalibrationIdentity, now: &str) -> Found {
        let (sources, cache) = self.sources();
        let sources: Vec<&dyn CalibrationSource> = sources.iter().map(Box::as_ref).collect();
        Found {
            module: identity.module_key.clone(),
            folder: self.folder().display().to_string(),
            resolution: resolve(identity, &sources, cache.as_ref(), now),
        }
    }

    /// Put a file the person supplied into their folder, as it is.
    ///
    /// The name is the only thing taken from the caller besides the bytes,
    /// and it may name a file in that folder and nothing else: no path, no
    /// drive, no device. A file already there under that name is never
    /// replaced by a different one.
    pub fn add_file(&self, filename: &str, bytes: &[u8]) -> Result<Added, AddError> {
        let name = safe_name(filename)?;
        if bytes.is_empty() {
            return Err(AddError::BadSize(String::from("the file is empty")));
        }
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(AddError::BadSize(format!(
                "{} bytes is larger than any calibration this build expects",
                bytes.len()
            )));
        }
        self.prepare().map_err(|e| AddError::Io(e.to_string()))?;

        let path = self.folder().join(&name);
        let sha256 = crate::sha256_hex(bytes);
        let added = |already_there| Added {
            filename: name.clone(),
            sha256: sha256.clone(),
            size: bytes.len() as u64,
            already_there,
            path: path.display().to_string(),
        };
        match std::fs::read(&path) {
            Ok(existing) if crate::sha256_hex(&existing) == sha256 => return Ok(added(true)),
            Ok(_) => {
                return Err(AddError::NameTaken(format!(
                    "a different file called {name} is already in the folder, and it was not \
                     replaced. Rename one of them"
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AddError::Io(e.to_string())),
        }

        // Whole or not at all: a file half-written under its real name would
        // be offered as a calibration.
        let partial = self.folder().join(format!("{name}.part"));
        std::fs::write(&partial, bytes).and_then(|()| std::fs::rename(&partial, &path)).map_err(
            |e| {
                let _ = std::fs::remove_file(&partial);
                AddError::Io(e.to_string())
            },
        )?;
        Ok(added(false))
    }
}

/// A name a file in the person's folder may have, or why not.
fn safe_name(filename: &str) -> Result<String, AddError> {
    let name = filename.trim();
    let bad = |why: &str| Err(AddError::BadName(format!("{filename:?} {why}")));
    if name.is_empty() || name.len() > 120 {
        return bad("is not a usable file name: it is empty or longer than 120 characters");
    }
    // Letters, digits and a little punctuation. No separator, no colon, no
    // wildcard: nothing a path, a drive or a stream is spelled with.
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || " -_.()+".contains(c)) {
        return bad(
            "has a character a calibration file's name may not have here. Use letters, digits, \
             spaces and - _ . ( ) +",
        );
    }
    if name.starts_with('.') || name.ends_with('.') || name.contains("..") {
        return bad("is not a usable file name");
    }
    let first_part = name.split('.').next().unwrap_or_default().trim().to_ascii_uppercase();
    let device = matches!(first_part.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (first_part.len() == 4
            && (first_part.starts_with("COM") || first_part.starts_with("LPT"))
            && first_part.ends_with(|c: char| c.is_ascii_digit()));
    if device {
        return bad("is a name Windows keeps for a device");
    }
    if !Named::from_filename(name).is_supported() {
        return Err(AddError::NotACalibration(format!(
            "{name} is not a kind of file treated as a calibration. The kinds are: .rwd, \
             .rwd.gz, .bin, .gz, .hex, .s19"
        )));
    }
    Ok(name.to_string())
}

/// The folder a path is in, for a test to look at.
#[cfg(test)]
fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Field, Identified, IdentitySource};
    use crate::resolve::{Outcome, SourceStatus};
    use std::path::Path;

    const NOW: &str = "2026-10-04T12:00:00Z";

    fn engine(calibration: &str) -> CalibrationIdentity {
        let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        id.record(
            Field::CalibrationId,
            Identified {
                value: calibration.into(),
                source: IdentitySource::ObdInfoType { info_type: 0x04 },
                evidence_ref: None,
                raw_hex: None,
            },
        );
        id
    }

    fn library(dir: &Path) -> Library {
        // No service tool is looked for on the computer the tests run on.
        Library::at(dir.join("calibrations")).with_program_files(None)
    }

    fn status_of<'a>(found: &'a Found, id: &str) -> &'a crate::resolve::SourceReport {
        found.resolution.sources.iter().find(|s| s.source.id == id).expect(id)
    }

    #[test]
    fn an_empty_library_says_where_it_looked_and_what_it_could_not() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());

        let found = lib.find(&engine("TEST-MOD-A010"), NOW);

        assert_eq!(found.resolution.outcome, Outcome::NoArtifactFound);
        assert!(!found.resolution.incomplete);
        assert_eq!(status_of(&found, "folder").status, SourceStatus::NoMatch);
        assert_eq!(status_of(&found, "cache").status, SourceStatus::NoMatch);
        // The tool is not installed: that is nowhere to look, not a failure
        // and not "nothing found there".
        let tool = status_of(&found, "honda-j2534-rewrite");
        assert_eq!(tool.status, SourceStatus::NotConfigured);
        assert!(!tool.searched);
        assert!(tool.error.as_deref().is_some_and(|e| e.contains("not found on this computer")));
        // And the folder now exists and explains itself.
        assert!(lib.folder().join("README.txt").is_file());
        assert!(lib.status().sources.iter().all(|s| !s.uses_network));
    }

    #[test]
    fn a_tools_folder_is_found_where_the_tool_installs_it() {
        let dir = tempfile::tempdir().unwrap();
        let programs = dir.path().join("Program Files (x86)");
        let calib = programs.join("Honda").join("J2534 Pass Thru").join("CalibFiles");
        std::fs::create_dir_all(&calib).unwrap();
        std::fs::write(calib.join("TEST-MOD-A010.rwd.gz"), b"not really gzip").unwrap();
        std::fs::write(calib.join("OTHER-MOD-A010.rwd.gz"), b"another").unwrap();
        let lib = Library::at(dir.path().join("calibrations")).with_program_files(Some(programs));

        let found = lib.find(&engine("TEST-MOD-A010"), NOW);

        let tool = status_of(&found, "honda-j2534-rewrite");
        assert!(tool.searched);
        assert_eq!((tool.offered, tool.passed_over), (1, 1));
        assert_eq!(tool.status, SourceStatus::Matched);
        assert_eq!(tool.source.kind, SourceKind::ToolInstallation);
        let one = &found.resolution.matches[0];
        // Named like the module's calibration, in a tool's folder: partial,
        // by name, and nobody's word that the manufacturer made it.
        assert_eq!(one.matching.status, crate::MatchStatus::PartialMatch);
        assert_eq!(one.matching.rests_on, Some(crate::Basis::Filename));
        assert_eq!(one.origin.manufacturer, crate::ManufacturerOrigin::NotEstablished);
        assert!(one.origin.reason.contains("service tool folder"));
        // The tool's folder is as it was.
        assert_eq!(names_in(&calib), vec!["OTHER-MOD-A010.rwd.gz", "TEST-MOD-A010.rwd.gz"]);
    }

    #[test]
    fn sources_json_says_where_a_tool_is_and_a_broken_one_is_an_error_not_silence() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        lib.prepare().unwrap();
        let elsewhere = dir.path().join("elsewhere");

        // Pointed at a folder that is not there: unavailable.
        std::fs::write(
            lib.sources_file(),
            serde_json::json!({ "tool_folders": [
                { "id": "honda-j2534-rewrite", "path": elsewhere } ] })
            .to_string(),
        )
        .unwrap();
        let found = lib.find(&engine("TEST-MOD-A010"), NOW);
        let tool = status_of(&found, "honda-j2534-rewrite");
        assert_eq!(tool.status, SourceStatus::Unavailable);
        assert_eq!(found.resolution.sources.len(), 3, "the known tool is listed once");
        assert!(!found.resolution.incomplete);

        // Now it is there, and empty: searched, nothing found.
        std::fs::create_dir_all(&elsewhere).unwrap();
        let found = lib.find(&engine("TEST-MOD-A010"), NOW);
        assert_eq!(status_of(&found, "honda-j2534-rewrite").status, SourceStatus::NoMatch);

        // Switched off.
        std::fs::write(
            lib.sources_file(),
            serde_json::json!({ "tool_folders": [
                { "id": "honda-j2534-rewrite", "path": elsewhere, "enabled": false } ] })
            .to_string(),
        )
        .unwrap();
        let found = lib.find(&engine("TEST-MOD-A010"), NOW);
        assert_eq!(status_of(&found, "honda-j2534-rewrite").status, SourceStatus::SwitchedOff);

        // A file that does not parse is a source that failed, and the search
        // says it is incomplete.
        std::fs::write(lib.sources_file(), "{ not json").unwrap();
        let found = lib.find(&engine("TEST-MOD-A010"), NOW);
        let broken = status_of(&found, "sources.json");
        assert_eq!(broken.status, SourceStatus::Failed);
        assert!(found.resolution.incomplete);
        assert_eq!(found.resolution.outcome, Outcome::NoArtifactFound);
    }

    #[test]
    fn a_file_a_person_adds_goes_in_their_folder_and_is_then_found() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());

        let added = lib.add_file("TEST-MOD-A010.bin", b"stand-in").unwrap();
        assert!(!added.already_there);
        assert_eq!(added.sha256, crate::sha256_hex(b"stand-in"));
        assert_eq!(std::fs::read(lib.folder().join("TEST-MOD-A010.bin")).unwrap(), b"stand-in");
        // Nothing half-written is left beside it.
        assert_eq!(names_in(&lib.folder()), vec!["README.txt", "TEST-MOD-A010.bin"]);

        // The same file again changes nothing; a different one is refused.
        assert!(lib.add_file("TEST-MOD-A010.bin", b"stand-in").unwrap().already_there);
        assert!(matches!(
            lib.add_file("TEST-MOD-A010.bin", b"something else"),
            Err(AddError::NameTaken(_))
        ));
        assert_eq!(std::fs::read(lib.folder().join("TEST-MOD-A010.bin")).unwrap(), b"stand-in");

        let found = lib.find(&engine("TEST-MOD-A010"), NOW);
        assert_eq!(found.resolution.outcome, Outcome::ArtifactFound);
        assert_eq!(found.resolution.matches[0].artifact.sources[0].kind, SourceKind::UserFolder);
    }

    #[test]
    fn a_name_that_is_not_a_file_in_the_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        for name in [
            "../outside.bin",
            "..\\outside.bin",
            "sub/inside.bin",
            "C:\\Windows\\x.bin",
            "x.bin:stream",
            ".hidden.bin",
            "NUL.bin",
            "com1.bin",
            "a..b.bin",
            "",
            "   ",
            "caf\u{e9}.bin",
        ] {
            assert!(
                matches!(lib.add_file(name, b"x"), Err(AddError::BadName(_))),
                "{name:?} was accepted"
            );
        }
        for name in ["notes.txt", "TEST-MOD-A010.bin.json", "no-extension"] {
            assert!(
                matches!(lib.add_file(name, b"x"), Err(AddError::NotACalibration(_))),
                "{name:?} was accepted"
            );
        }
        assert!(matches!(lib.add_file("empty.bin", b""), Err(AddError::BadSize(_))));
        // Nothing was written anywhere by any of that.
        assert!(!lib.folder().exists() || names_in(&lib.folder()) == vec!["README.txt"]);
        assert!(!dir.path().join("outside.bin").exists());
    }
}
