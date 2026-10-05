//! Where calibration files are looked for.
//!
//! A source is somewhere files can be found and something that can say what
//! it claims about each. The resolver knows no source by name: it is handed a
//! list, and each one can be switched off without the others noticing.
//!
//! # What ships
//!
//! Two sources, both on the person's own disk: a folder they put files in,
//! and the cache of files already kept. **No source here uses a network.**
//! Manufacturers' calibration files are their copyright and are distributed
//! through their own service tools, and this project knows of no public
//! source it is entitled to fetch them from. A source that does is a new
//! implementation of [`CalibrationSource`]; nothing else has to change, and
//! until one exists the honest answer for a file nobody supplied is that no
//! artifact was found.

use crate::artifact::{ArtifactRecord, Basis, Claim, Claims, SourceRef};
use crate::cache::Cache;
use crate::format::ArtifactFormat;
use crate::identity::{CalibrationIdentity, Field};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a source is, for showing to a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// A short stable id, used in an artifact's record.
    pub id: String,
    /// What it is, in words.
    pub name: String,
    /// Where it looks: a path, or an address.
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

/// Somewhere calibration files can be found.
pub trait CalibrationSource: Send + Sync {
    /// What this source is.
    fn info(&self) -> SourceInfo;

    /// The files this source holds that might bear on `identity`.
    ///
    /// A source may return more than what matches. Deciding what matches is
    /// the resolver's job and is done the same way for every source.
    fn search(&self, identity: &CalibrationIdentity, now: &str) -> std::io::Result<Vec<Candidate>>;
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
        let known =
            ArtifactFormat::SUPPORTED.iter().any(|f| f.extension().eq_ignore_ascii_case(ext));
        if !known || head.is_empty() {
            return stem;
        }
        stem = head;
    }
}

/// What a metadata file beside an artifact declares.
///
/// The file is `<artifact name>.json`. Every key is optional, and each is
/// either one value or a list of values:
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
/// rather than guessed at. Returns the claims and the declared hash.
pub fn read_sidecar(text: &str, stated_in: &str) -> Result<(Claims, Option<String>), String> {
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
                basis: Basis::Declared,
                stated_in: Some(stated_in.to_string()),
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

/// A folder the person puts calibration files in.
///
/// Each file of a known kind is a candidate. What is claimed about it comes
/// from a metadata file beside it when there is one, and otherwise only from
/// its name, which is recorded as a name and never as more.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    id: String,
    dir: PathBuf,
    enabled: bool,
}

impl DirectorySource {
    /// A source over `dir`, called `id` in the records of what it finds.
    pub fn new(id: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        DirectorySource { id: id.into(), dir: dir.into(), enabled: true }
    }

    /// The same source, switched on or off.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn candidate(&self, path: &Path, now: &str) -> std::io::Result<Option<Candidate>> {
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else { return Ok(None) };
        let format = ArtifactFormat::from_filename(filename);
        if is_sidecar(filename) || !format.is_supported() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;

        let mut record = ArtifactRecord {
            sha256: crate::sha256_hex(&bytes),
            size: bytes.len() as u64,
            format,
            filename: filename.to_string(),
            sources: vec![SourceRef {
                source: self.id.clone(),
                reference: path.display().to_string(),
                found_at: now.to_string(),
            }],
            claims: Claims::new(),
            declared_sha256: None,
            added_at: now.to_string(),
            metadata_problem: None,
        };

        let sidecar_name = format!("{filename}.json");
        match std::fs::read_to_string(path.with_file_name(&sidecar_name)) {
            Ok(text) => match read_sidecar(&text, &sidecar_name) {
                Ok((claims, declared)) => {
                    record.claims = claims;
                    record.declared_sha256 = declared;
                }
                // A metadata file that cannot be read claims nothing, and the
                // artifact is still a file with a name. But it is said: a
                // stray character in that file would otherwise turn a declared
                // calibration into an unexplained "cannot tell".
                Err(e) => record.metadata_problem = Some(e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        // What the name suggests, recorded as a name.
        record.claim(
            Field::CalibrationId,
            Claim {
                value: name_stem(filename).to_string(),
                basis: Basis::Filename,
                stated_in: Some(filename.to_string()),
            },
        );
        Ok(Some(Candidate { record, bytes }))
    }
}

impl CalibrationSource for DirectorySource {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: self.id.clone(),
            name: String::from("A folder of calibration files on this computer"),
            location: self.dir.display().to_string(),
            uses_network: false,
            enabled: self.enabled,
        }
    }

    fn search(
        &self,
        _identity: &CalibrationIdentity,
        now: &str,
    ) -> std::io::Result<Vec<Candidate>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            // A folder nobody has made yet holds nothing.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut paths: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        paths.sort();

        let mut found = Vec::new();
        for path in paths {
            if let Some(candidate) = self.candidate(&path, now)? {
                found.push(candidate);
            }
        }
        Ok(found)
    }
}

/// The files already kept. Searching it is what makes an artifact found once
/// still found after the folder it came from has gone.
impl CalibrationSource for Cache {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: String::from("cache"),
            name: String::from("Calibration files already kept by this app"),
            location: self.root().display().to_string(),
            uses_network: false,
            enabled: true,
        }
    }

    fn search(
        &self,
        _identity: &CalibrationIdentity,
        _now: &str,
    ) -> std::io::Result<Vec<Candidate>> {
        let mut found = Vec::new();
        for record in self.list()? {
            // A kept file that has gone or changed is still reported, with
            // whatever is there now, so validation can say what happened.
            let bytes = self.read(&record).unwrap_or_default();
            found.push(Candidate { record, bytes });
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_suggests_its_stem_and_nothing_more() {
        assert_eq!(name_stem("37805-5MR-C120.rwd.gz"), "37805-5MR-C120");
        assert_eq!(name_stem("TEST-CAL-001.bin"), "TEST-CAL-001");
        assert_eq!(name_stem("v1.2-final.hex"), "v1.2-final");
        assert_eq!(name_stem("no-extension"), "no-extension");
        assert_eq!(name_stem(".bin"), ".bin");
    }

    #[test]
    fn a_metadata_file_declares_fields_by_their_own_names() {
        let (claims, sha) = read_sidecar(
            r#"{ "sha256": "AB12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12",
                 "calibration_id": "TEST-CAL-001",
                 "hardware_number": ["TEST-HW-001", "TEST-HW-002"],
                 "model_year": [2023, 2024],
                 "tuned_by": "nobody",
                 "engine": "" }"#,
            "TEST-CAL-001.bin.json",
        )
        .unwrap();

        assert_eq!(sha.unwrap(), "ab12".repeat(16));
        assert_eq!(claims[&Field::CalibrationId][0].value, "TEST-CAL-001");
        assert_eq!(claims[&Field::CalibrationId][0].basis, Basis::Declared);
        assert_eq!(claims[&Field::HardwareNumber].len(), 2);
        assert_eq!(claims[&Field::ModelYear][1].value, "2024");
        // Not a field, and an empty one: neither becomes a claim.
        assert_eq!(claims.len(), 3);
    }

    #[test]
    fn a_metadata_file_that_is_not_what_it_should_be_is_refused() {
        assert!(read_sidecar("[1, 2]", "x.json").is_err());
        assert!(read_sidecar("not json", "x.json").is_err());
        assert!(read_sidecar(r#"{ "sha256": "abc" }"#, "x.json").is_err());
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
        let identity = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        let found = source.search(&identity, "2026-10-04T00:00:00Z").unwrap();

        let names: Vec<&str> = found.iter().map(|c| c.record.filename.as_str()).collect();
        assert_eq!(names, vec!["OTHER.rwd", "TEST-CAL-001.bin"]);
        // Declared by its metadata, and separately suggested by its name.
        let bases: Vec<Basis> =
            found[1].record.claimed(Field::CalibrationId).iter().map(|c| c.basis).collect();
        assert_eq!(bases, vec![Basis::Declared, Basis::Filename]);
        // The other has only its name to go on.
        assert_eq!(found[0].record.claimed(Field::CalibrationId)[0].basis, Basis::Filename);
        assert_eq!(found[0].record.sha256, crate::sha256_hex(b"two"));
    }

    #[test]
    fn a_folder_that_does_not_exist_holds_nothing() {
        let source = DirectorySource::new("folder", "Z:/nowhere/at/all");
        let identity = CalibrationIdentity::for_module("ECU_18DAF110", "18DAF110");
        assert!(source.search(&identity, "now").unwrap().is_empty());
    }
}
