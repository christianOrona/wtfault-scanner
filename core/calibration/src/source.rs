//! Where calibration files are looked for.
//!
//! A source is somewhere files can be found and something that can say what
//! it claims about each. The resolver knows no source by name: it is handed a
//! list, and each one can be switched off without the others noticing.
//!
//! # What ships
//!
//! Three kinds of source, all on the person's own disk: a folder they put
//! files in, the folder a manufacturer's service tool installed its files in
//! (when that tool is on this computer), and the cache of files already kept.
//! **No source here uses a network.** Manufacturers' calibration files are
//! their copyright and are distributed through their own service tools. That
//! a file can be reached on the internet does not make it anyone's to pass
//! on, and this project knows of no public source it is entitled to fetch
//! them from. A source that is, is a new implementation of
//! [`CalibrationSource`]; nothing else has to change, and until one exists
//! the honest answer for a file nobody supplied is that no artifact was found.

use crate::artifact::{ArtifactRecord, Basis, Claim, Claims, SourceKind, SourceRef};
use crate::cache::Cache;
use crate::format::{ArtifactFormat, Named};
use crate::identity::{CalibrationIdentity, Field};
use crate::validate::MAX_ARTIFACT_BYTES;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a source is, for showing to a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// A short stable id, used in an artifact's record.
    pub id: String,
    /// What it is, in words.
    pub name: String,
    /// What kind of place it is.
    pub kind: SourceKind,
    /// Where it looks: a path, or an address. Empty when it has nowhere to
    /// look yet.
    pub location: String,
    /// Whether it sends anything over a network when searched.
    pub uses_network: bool,
    /// Whether it is switched on.
    pub enabled: bool,
}

/// A file a source holds, with what the source says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The record as this source would write it. Its `sha256` is the hash of
    /// the content the source actually holds.
    pub record: ArtifactRecord,
    /// The content.
    pub bytes: Vec<u8>,
}

/// What a source came back with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offered {
    /// The files it offers for judging.
    pub candidates: Vec<Candidate>,
    /// Files it holds and did not offer, because nothing about them bears on
    /// the identity asked about, or because they are too large to be one.
    pub passed_over: usize,
    /// What it did, in words, when that needs saying.
    pub note: Option<String>,
}

/// Why a source could not be searched. None of these is "it holds nothing":
/// that is an [`Offered`] with no candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// Nobody has said where it should look.
    NotConfigured(String),
    /// It has somewhere to look and that place is not there.
    Unavailable(String),
    /// It was there and looking failed.
    Failed(String),
}

/// Somewhere calibration files can be found.
pub trait CalibrationSource: Send + Sync {
    /// What this source is.
    fn info(&self) -> SourceInfo;

    /// The files this source holds that might bear on `identity`.
    ///
    /// A source may return more than what matches. Deciding what matches is
    /// the resolver's job and is done the same way for every source. A source
    /// that holds a great many files should use the identity to choose which
    /// to open, and count the rest as passed over.
    fn search(&self, identity: &CalibrationIdentity, now: &str) -> Result<Offered, SourceError>;
}

/// The file names that are metadata about another file rather than a file.
fn is_sidecar(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".json")
}

/// A file's name without any of the extensions this build knows, which is what
/// a name can be taken to suggest: `37805-5MR-C120.rwd.gz` suggests
/// `37805-5MR-C120`.
pub fn name_stem(filename: &str) -> &str {
    let mut stem = filename;
    loop {
        let Some((head, ext)) = stem.rsplit_once('.') else { return stem };
        let known = ArtifactFormat::EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext));
        if !known || head.is_empty() {
            return stem;
        }
        stem = head;
    }
}

/// What a metadata file beside an artifact declares.
///
/// The file is `<artifact name>.json`, written by the person who put the
/// artifact there. Every key is optional, and each is either one value or a
/// list of values:
///
/// ```json
/// {
///   "sha256": "the SHA-256 the artifact should have",
///   "calibration_id": "37805-5MR-C120",
///   "hardware_number": ["..."],
///   "make": "Honda", "model": "Odyssey", "model_year": [2023, 2024],
///   "engine": "3.5L V6", "module_name": "PCM"
/// }
/// ```
///
/// The keys are the names of [`Field`]. A key that is not one is ignored
/// rather than guessed at. Every claim comes back as [`Basis::UserDeclared`],
/// from `source`: it is what somebody typed, and says nothing about who made
/// the artifact. Returns the claims and the declared hash.
pub fn read_sidecar(
    text: &str,
    stated_in: &str,
    source: &str,
) -> Result<(Claims, Option<String>), String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("{stated_in} is not JSON: {e}"))?;
    let object =
        value.as_object().ok_or_else(|| format!("{stated_in} is not a JSON object of fields"))?;

    let mut claims = Claims::new();
    let mut declared_sha256 = None;
    for (key, value) in object {
        if key == "sha256" {
            let hash = value.as_str().unwrap_or_default().trim().to_ascii_lowercase();
            if !crate::is_sha256_hex(&hash) {
                return Err(format!("{stated_in} states a sha256 that is not a SHA-256"));
            }
            declared_sha256 = Some(hash);
            continue;
        }
        // The field's own serialised name, so the two cannot drift apart.
        let Ok(field) = serde_json::from_value::<Field>(serde_json::Value::String(key.clone()))
        else {
            continue;
        };
        let values: Vec<String> = match value {
            serde_json::Value::Array(items) => items.iter().filter_map(text_of).collect(),
            other => text_of(other).into_iter().collect(),
        };
        for value in values {
            claims.entry(field).or_default().push(Claim {
                value,
                basis: Basis::UserDeclared,
                stated_in: Some(stated_in.to_string()),
                source: Some(source.to_string()),
            });
        }
    }
    Ok((claims, declared_sha256))
}

/// A JSON string or number as the text it states. Nothing else is a value.
fn text_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// How the names of the files for one module on one model begin.
///
/// The public description of Honda's files says each is named for the
/// software it carries, `MODULE-VEHICLE-VERSION`, so the files for one module
/// on one model share everything up to the last hyphen. An identifier with no
/// hyphen is its own family. This chooses which files to open. It is never
/// evidence that a file matches: the resolver decides that from what is
/// inside and what is declared.
pub fn name_families(identity: &CalibrationIdentity) -> Vec<String> {
    let mut families: Vec<String> = Vec::new();
    for field in [
        Field::CalibrationId,
        Field::PartNumber,
        Field::SoftwareNumber,
        Field::ProgramId,
        Field::HardwareNumber,
        Field::RomId,
        Field::StrategyId,
    ] {
        for value in identity.values(field) {
            let value = value.trim().to_ascii_uppercase();
            if value.is_empty() {
                continue;
            }
            let family = match value.rsplit_once('-') {
                Some((head, _)) if !head.is_empty() => format!("{head}-"),
                _ => value,
            };
            if !families.contains(&family) {
                families.push(family);
            }
        }
    }
    families
}

/// A folder of calibration files on this computer.
///
/// Two kinds, which differ in who put the files there:
///
/// - The person's own folder ([`DirectorySource::new`]). Every file of a
///   known kind is a candidate. What is claimed about it comes from a
///   metadata file beside it when there is one, and otherwise only from its
///   name, which is recorded as a name and never as more.
/// - The folder a manufacturer's service tool installed
///   ([`DirectorySource::tool_installation`]). It may hold thousands of
///   files, so only those named in the module's family are opened. Nothing
///   beside a file there is read as a declaration: nobody using this app
///   wrote it. A file found there is a file found there, and is not thereby
///   the manufacturer's.
///
/// Neither kind writes, moves or deletes anything in the folder.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    id: String,
    name: String,
    kind: SourceKind,
    dir: Option<PathBuf>,
    enabled: bool,
}

impl DirectorySource {
    /// The person's own folder at `dir`, called `id` in the records of what
    /// it finds.
    pub fn new(id: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        DirectorySource {
            id: id.into(),
            name: String::from("A folder of calibration files on this computer"),
            kind: SourceKind::UserFolder,
            dir: Some(dir.into()),
            enabled: true,
        }
    }

    /// The folder a manufacturer's service tool keeps its files in. `dir` is
    /// `None` when nobody has said where that is and it was not found.
    pub fn tool_installation(
        id: impl Into<String>,
        name: impl Into<String>,
        dir: Option<PathBuf>,
    ) -> Self {
        DirectorySource {
            id: id.into(),
            name: name.into(),
            kind: SourceKind::ToolInstallation,
            dir,
            enabled: true,
        }
    }

    /// The same source, switched on or off.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The folder, when there is one.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn candidate(&self, path: &Path, filename: &str, now: &str) -> std::io::Result<Candidate> {
        let bytes = std::fs::read(path)?;
        let mut record = ArtifactRecord::found(
            filename,
            &bytes,
            SourceRef {
                source: self.id.clone(),
                kind: self.kind,
                reference: path.display().to_string(),
                found_at: now.to_string(),
            },
            now,
        );

        // Only in the person's own folder is a file beside an artifact
        // something the person declared.
        if self.kind == SourceKind::UserFolder {
            let sidecar_name = format!("{filename}.json");
            match std::fs::read_to_string(path.with_file_name(&sidecar_name)) {
                Ok(text) => match read_sidecar(&text, &sidecar_name, &self.id) {
                    Ok((claims, declared)) => {
                        record.claims = claims;
                        record.declared_sha256 = declared;
                    }
                    // A metadata file that cannot be read claims nothing, and
                    // the artifact is still a file with a name. But it is
                    // said: a stray character in that file would otherwise
                    // turn a declared calibration into an unexplained
                    // "cannot tell".
                    Err(e) => record.metadata_problem = Some(e),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }

        // What the name suggests, recorded as a name.
        record.claim(
            Field::CalibrationId,
            Claim {
                value: name_stem(filename).to_string(),
                basis: Basis::Filename,
                stated_in: Some(filename.to_string()),
                source: Some(self.id.clone()),
            },
        );
        Ok(Candidate { record, bytes })
    }
}

impl CalibrationSource for DirectorySource {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            location: self.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            uses_network: false,
            enabled: self.enabled,
        }
    }

    fn search(&self, identity: &CalibrationIdentity, now: &str) -> Result<Offered, SourceError> {
        let tool = self.kind == SourceKind::ToolInstallation;
        let Some(dir) = &self.dir else {
            return Err(SourceError::NotConfigured(format!(
                "{} was not found on this computer, and nobody has said where it is",
                self.name
            )));
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // The person's folder that nobody has made yet holds nothing. A
            // tool's folder that is not there is a tool that is not there.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !tool => {
                return Ok(Offered::default())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SourceError::Unavailable(format!("{} is not there", dir.display())))
            }
            Err(e) => return Err(SourceError::Failed(format!("{}: {e}", dir.display()))),
        };
        let mut paths: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        paths.sort();

        // A tool's folder is searched by what the module said about itself.
        let families = if tool { name_families(identity) } else { Vec::new() };
        let mut offered = Offered::default();
        let mut artifacts = 0usize;
        let mut too_large: Vec<String> = Vec::new();
        for path in paths {
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if is_sidecar(filename) || !Named::from_filename(filename).is_supported() {
                continue;
            }
            artifacts += 1;
            if tool {
                let upper = filename.to_ascii_uppercase();
                if !families.iter().any(|f| upper.starts_with(f.as_str())) {
                    offered.passed_over += 1;
                    continue;
                }
            }
            // Not read into memory to find out it is a disk image.
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size > MAX_ARTIFACT_BYTES {
                offered.passed_over += 1;
                too_large.push(filename.to_string());
                continue;
            }
            let candidate = self
                .candidate(&path, filename, now)
                .map_err(|e| SourceError::Failed(format!("{}: {e}", path.display())))?;
            offered.candidates.push(candidate);
        }

        let mut notes = Vec::new();
        if tool {
            notes.push(if families.is_empty() {
                format!(
                    "{artifacts} files are there. The module reported nothing that names its \
                     software, so there was no name to look for and none was opened"
                )
            } else {
                format!(
                    "{artifacts} files are there. Only those whose names begin {} were opened: {}",
                    families.join(" or "),
                    offered.candidates.len()
                )
            });
        }
        if !too_large.is_empty() {
            notes.push(format!(
                "too large to be a calibration and not opened: {}",
                too_large.join(", ")
            ));
        }
        offered.note = (!notes.is_empty()).then(|| notes.join(". "));
        Ok(offered)
    }
}

/// The files already kept. Searching it is what makes an artifact found once
/// still found after the folder it came from has gone.
impl CalibrationSource for Cache {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: String::from("cache"),
            name: String::from("Calibration files already kept by this app"),
            kind: SourceKind::Kept,
            location: self.root().display().to_string(),
            uses_network: false,
            enabled: true,
        }
    }

    fn search(&self, _identity: &CalibrationIdentity, _now: &str) -> Result<Offered, SourceError> {
        let records = self
            .list()
            .map_err(|e| SourceError::Failed(format!("{}: {e}", self.root().display())))?;
        let mut offered = Offered::default();
        for record in records {
            // A kept file that has gone or changed is still reported, with
            // whatever is there now, so validation can say what happened.
            let bytes = self.read(&record).unwrap_or_default();
            offered.candidates.push(Candidate { record, bytes });
        }
        Ok(offered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identified, IdentitySource};

    fn engine(calibration: Option<&str>) -> CalibrationIdentity {
        let mut id = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        if let Some(value) = calibration {
            id.record(
                Field::CalibrationId,
                Identified {
                    value: value.into(),
                    source: IdentitySource::ObdInfoType { info_type: 0x04 },
                    evidence_ref: None,
                    raw_hex: None,
                },
            );
        }
        id
    }

    #[test]
    fn a_name_suggests_its_stem_and_nothing_more() {
        assert_eq!(name_stem("37805-5MR-C120.rwd.gz"), "37805-5MR-C120");
        assert_eq!(name_stem("TEST-CAL-001.bin"), "TEST-CAL-001");
        assert_eq!(name_stem("v1.2-final.hex"), "v1.2-final");
        assert_eq!(name_stem("no-extension"), "no-extension");
        assert_eq!(name_stem(".bin"), ".bin");
    }

    #[test]
    fn a_metadata_file_declares_fields_by_their_own_names_and_as_the_persons_word() {
        let (claims, sha) = read_sidecar(
            r#"{ "sha256": "AB12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12",
                 "calibration_id": "TEST-CAL-001",
                 "hardware_number": ["TEST-HW-001", "TEST-HW-002"],
                 "model_year": [2023, 2024],
                 "tuned_by": "nobody",
                 "made_by": "Honda",
                 "engine": "" }"#,
            "TEST-CAL-001.bin.json",
            "folder",
        )
        .unwrap();

        assert_eq!(sha.unwrap(), "ab12".repeat(16));
        let calibration = &claims[&Field::CalibrationId][0];
        assert_eq!(calibration.value, "TEST-CAL-001");
        assert_eq!(calibration.basis, Basis::UserDeclared);
        assert_eq!(calibration.source.as_deref(), Some("folder"));
        assert_eq!(claims[&Field::HardwareNumber].len(), 2);
        assert_eq!(claims[&Field::ModelYear][1].value, "2024");
        // Not a field, a say-so about who made it, and an empty one: none
        // becomes a claim, and there is no claim a file could make about its
        // maker.
        assert_eq!(claims.len(), 3);
    }

    #[test]
    fn a_metadata_file_that_is_not_what_it_should_be_is_refused() {
        assert!(read_sidecar("[1, 2]", "x.json", "folder").is_err());
        assert!(read_sidecar("not json", "x.json", "folder").is_err());
        assert!(read_sidecar(r#"{ "sha256": "abc" }"#, "x.json", "folder").is_err());
    }

    #[test]
    fn a_folder_offers_its_calibration_files_and_not_its_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TEST-CAL-001.bin"), b"one").unwrap();
        std::fs::write(
            dir.path().join("TEST-CAL-001.bin.json"),
            r#"{ "calibration_id": "TEST-CAL-001" }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"notes").unwrap();
        std::fs::write(dir.path().join("OTHER.rwd"), b"two").unwrap();

        let source = DirectorySource::new("folder", dir.path());
        let found = source.search(&engine(None), "2026-10-04T00:00:00Z").unwrap().candidates;

        let names: Vec<&str> = found.iter().map(|c| c.record.filename.as_str()).collect();
        assert_eq!(names, vec!["OTHER.rwd", "TEST-CAL-001.bin"]);
        // Declared by its metadata, and separately suggested by its name.
        let bases: Vec<Basis> =
            found[1].record.claimed(Field::CalibrationId).iter().map(|c| c.basis).collect();
        assert_eq!(bases, vec![Basis::UserDeclared, Basis::Filename]);
        // The other has only its name to go on.
        assert_eq!(found[0].record.claimed(Field::CalibrationId)[0].basis, Basis::Filename);
        assert_eq!(found[0].record.sha256, crate::sha256_hex(b"two"));
        assert_eq!(found[0].record.sources[0].kind, SourceKind::UserFolder);
    }

    #[test]
    fn the_persons_folder_that_does_not_exist_holds_nothing() {
        let source = DirectorySource::new("folder", "Z:/nowhere/at/all");
        assert_eq!(source.search(&engine(None), "now").unwrap(), Offered::default());
    }

    #[test]
    fn files_for_one_module_on_one_model_share_the_start_of_their_names() {
        assert_eq!(name_families(&engine(Some("37805-5MR-C120"))), vec!["37805-5MR-"]);
        assert_eq!(name_families(&engine(Some(" nohyphen "))), vec!["NOHYPHEN"]);
        assert!(name_families(&engine(None)).is_empty());
    }

    #[test]
    fn a_tools_folder_is_searched_by_what_the_module_said_and_declares_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["TEST-MOD-A010.rwd.gz", "TEST-MOD-A020.rwd.gz", "OTHER-MOD-A010.rwd.gz"] {
            std::fs::write(dir.path().join(name), name.as_bytes()).unwrap();
        }
        // A file beside one of them saying what it is. In a tool's folder
        // nobody using this app wrote it, so it declares nothing.
        std::fs::write(
            dir.path().join("TEST-MOD-A010.rwd.gz.json"),
            r#"{ "calibration_id": "TEST-MOD-A010" }"#,
        )
        .unwrap();
        let before: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();

        let tool = DirectorySource::tool_installation(
            "test-tool",
            "A test tool's folder",
            Some(dir.path().to_path_buf()),
        );
        let offered = tool.search(&engine(Some("TEST-MOD-A010")), "now").unwrap();

        let names: Vec<&str> =
            offered.candidates.iter().map(|c| c.record.filename.as_str()).collect();
        assert_eq!(names, vec!["TEST-MOD-A010.rwd.gz", "TEST-MOD-A020.rwd.gz"]);
        assert_eq!(offered.passed_over, 1);
        assert!(offered.note.as_deref().is_some_and(|n| n.contains("TEST-MOD-")));
        for c in &offered.candidates {
            assert_eq!(c.record.sources[0].kind, SourceKind::ToolInstallation);
            assert!(c.record.claims.values().flatten().all(|claim| claim.basis == Basis::Filename));
        }
        assert_eq!(tool.info().kind, SourceKind::ToolInstallation);

        // A module that names no software gives it nothing to look for.
        let nothing = tool.search(&engine(None), "now").unwrap();
        assert!(nothing.candidates.is_empty());
        assert_eq!(nothing.passed_over, 3);

        // And the folder is exactly as it was.
        let after: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn a_tool_that_is_not_installed_is_not_the_same_as_one_that_holds_nothing() {
        let unset = DirectorySource::tool_installation("t", "A test tool's folder", None);
        assert!(matches!(
            unset.search(&engine(Some("TEST-MOD-A010")), "now"),
            Err(SourceError::NotConfigured(_))
        ));
        let gone = DirectorySource::tool_installation(
            "t",
            "A test tool's folder",
            Some(PathBuf::from("Z:/nowhere/at/all")),
        );
        assert!(matches!(
            gone.search(&engine(Some("TEST-MOD-A010")), "now"),
            Err(SourceError::Unavailable(_))
        ));
    }
}
