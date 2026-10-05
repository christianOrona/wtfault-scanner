//! Calibration files kept by their SHA-256.
//!
//! A file is stored at `<root>/artifacts/<sha256>.<ext>` and its record as
//! JSON at `<root>/metadata/<sha256>.json`. The hash is the file's identity:
//! its name is only what it was called where it was found. The extension
//! keeps both layers of that name, so a packed package is kept as
//! `<sha256>.rwd.gz`, exactly as it arrived: nothing is unpacked into here.
//!
//! The same content added twice, from anywhere and under any name, is one
//! file with one record that lists both places.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::artifact::{ArtifactRecord, Claims, SourceRef};
use crate::format::Named;
use crate::is_sha256_hex;
use crate::sha256_hex;

/// A content-addressed store for calibration files.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

/// What a caller says about a file it is adding.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// The name the file had where it was found.
    pub filename: String,
    /// Where it was found.
    pub source: SourceRef,
    /// What is claimed about it.
    pub claims: Claims,
    /// The SHA-256 a source said it should have, when one did.
    pub declared_sha256: Option<String>,
    /// The time of adding, RFC 3339. Passed in so this crate needs no clock.
    pub now: String,
}

impl Cache {
    /// Open a cache at the given root path.
    ///
    /// Creates `<root>/artifacts` and `<root>/metadata` directories if they don't exist.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Cache> {
        let root = root.into();
        fs::create_dir_all(root.join("artifacts"))?;
        fs::create_dir_all(root.join("metadata"))?;
        Ok(Cache { root })
    }

    /// Return the root path of this cache.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Add a calibration file to the cache.
    ///
    /// If a file with the same content already exists, the metadata is merged
    /// and no new file is written. Otherwise, both the artifact file and its
    /// metadata are written.
    pub fn add(&self, bytes: &[u8], incoming: Incoming) -> io::Result<ArtifactRecord> {
        let sha = sha256_hex(bytes);
        let record_path = self.root.join("metadata").join(format!("{}.json", sha));

        let mut sighting =
            ArtifactRecord::found(&incoming.filename, bytes, incoming.source, &incoming.now);
        sighting.claims = incoming.claims;
        sighting.declared_sha256 = incoming.declared_sha256;

        // Already kept: one more sighting of the same content, not a second
        // artifact. The first name, format and date stay.
        let record = match self.get(&sha) {
            Ok(Some(mut record)) => {
                record.absorb(&sighting);
                record
            }
            _ => sighting,
        };

        // Written when it is not there. For a record that has outlived its
        // file this is the one repair worth making here: the content is in
        // hand and hashes to the record's name.
        let artifact_path = self.file_path(&record);
        if !artifact_path.exists() {
            let tmp = self.root.join("artifacts").join(format!("{}.tmp", record.sha256));
            fs::write(&tmp, bytes)?;
            fs::rename(tmp, &artifact_path)?;
        }

        let tmp_path = record_path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&record)?;
        fs::write(&tmp_path, json)?;
        fs::rename(tmp_path, &record_path)?;
        Ok(record)
    }

    /// Retrieve a calibration file's metadata by its SHA-256 hash.
    ///
    /// Returns `Ok(None)` if the hash is invalid or no such file exists.
    pub fn get(&self, sha256: &str) -> io::Result<Option<ArtifactRecord>> {
        // Also what keeps a caller's string from naming a path.
        if !is_sha256_hex(sha256) {
            return Ok(None);
        }
        let sha256 = sha256.to_ascii_lowercase();

        let path = self.root.join("metadata").join(format!("{sha256}.json"));
        match fs::read_to_string(path) {
            Ok(contents) => {
                let record: ArtifactRecord = serde_json::from_str(&contents)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Ok(Some(by_its_name(record)))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List all calibration files in the cache.
    ///
    /// Files that fail to parse are skipped rather than causing an error.
    pub fn list(&self) -> io::Result<Vec<ArtifactRecord>> {
        let mut records = Vec::new();
        let metadata_dir = self.root.join("metadata");

        for entry in fs::read_dir(metadata_dir)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "json" {
                    match fs::read_to_string(&path) {
                        Ok(contents) => {
                            if let Ok(record) = serde_json::from_str::<ArtifactRecord>(&contents) {
                                records.push(by_its_name(record));
                            }
                        }
                        Err(_) => {
                            // Skip files that fail to parse
                        }
                    }
                }
            }
        }

        records.sort_by(|a, b| a.added_at.cmp(&b.added_at).then_with(|| a.sha256.cmp(&b.sha256)));

        Ok(records)
    }

    /// Return the path to an artifact file for a given record.
    pub fn file_path(&self, record: &ArtifactRecord) -> PathBuf {
        let artifacts = self.root.join("artifacts");
        let path = artifacts.join(format!("{}.{}", record.sha256, record.named().extension()));
        // A packed file kept before both layers of its name were, is under
        // the last extension alone. It is still that file.
        let earlier = artifacts.join(format!("{}.gz", record.sha256));
        if !path.exists() && earlier.exists() {
            return earlier;
        }
        path
    }

    /// Read the bytes of an artifact file.
    pub fn read(&self, record: &ArtifactRecord) -> io::Result<Vec<u8>> {
        let path = self.file_path(record);
        fs::read(path)
    }

    /// Check if an artifact file is intact (matches its SHA-256 and size).
    ///
    /// Returns `Ok(false)` if the file doesn't exist or doesn't match.
    pub fn intact(&self, record: &ArtifactRecord) -> io::Result<bool> {
        let path = self.file_path(record);
        match fs::metadata(&path) {
            Ok(metadata) => {
                if metadata.len() != record.size {
                    return Ok(false);
                }
                let bytes = fs::read(&path)?;
                let actual_sha = sha256_hex(&bytes);
                Ok(actual_sha == record.sha256)
            }
            Err(_) => Ok(false),
        }
    }
}

/// A record with its format read from its name, so one written before
/// packing was kept apart from content says the same as one written now.
fn by_its_name(mut record: ArtifactRecord) -> ArtifactRecord {
    let named: Named = record.named();
    record.format = named.payload;
    record.compression = named.compression;
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::SourceKind;
    use crate::format::{ArtifactFormat, Compression};
    use tempfile::tempdir;

    #[test]
    fn adding_bytes_stores_file_with_correct_extension() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let bytes = b"test data";
        let incoming = Incoming {
            filename: "TEST-CAL-001.bin".to_string(),
            source: SourceRef {
                source: "test-source".to_string(),
                reference: "ref1".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:00:00Z".to_string(),
        };

        let record = cache.add(bytes, incoming).unwrap();
        assert_eq!(record.sha256.len(), 64);
        assert_eq!(record.size, bytes.len() as u64);
        assert_eq!(record.filename, "TEST-CAL-001.bin");
        assert_eq!(record.format, ArtifactFormat::Bin);

        let retrieved = cache.get(&record.sha256).unwrap().unwrap();
        assert_eq!(retrieved, record);

        let read_bytes = cache.read(&record).unwrap();
        assert_eq!(&read_bytes[..], bytes);

        assert!(cache.intact(&record).unwrap());
    }

    #[test]
    fn the_same_bytes_from_two_sources_are_one_artifact() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let bytes = b"test data";

        let incoming1 = Incoming {
            filename: "file1.bin".to_string(),
            source: SourceRef {
                source: "source1".to_string(),
                reference: "ref1".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:00:00Z".to_string(),
        };

        let incoming2 = Incoming {
            filename: "file2.bin".to_string(),
            source: SourceRef {
                source: "source2".to_string(),
                reference: "ref2".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:00:00Z".to_string(),
        };

        let record1 = cache.add(bytes, incoming1).unwrap();
        let record2 = cache.add(bytes, incoming2).unwrap();

        assert_eq!(record1.sha256, record2.sha256);
        assert_eq!(record2.filename, "file1.bin"); // Should keep the first filename
        assert_eq!(record2.sources.len(), 2); // Should have both sources

        let list = cache.list().unwrap();
        assert_eq!(list.len(), 1);

        // Check that only one artifact file exists
        let artifacts_dir = dir.path().join("artifacts");
        let artifact_files = fs::read_dir(artifacts_dir).unwrap().collect::<Vec<_>>();
        assert_eq!(artifact_files.len(), 1);

        let metadata_dir = dir.path().join("metadata");
        let metadata_files = fs::read_dir(metadata_dir).unwrap().collect::<Vec<_>>();
        assert_eq!(metadata_files.len(), 1);
    }

    #[test]
    fn different_bytes_give_two_artifacts_and_list_is_sorted() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();

        let bytes1 = b"test data 1";
        let bytes2 = b"test data 2";

        let incoming1 = Incoming {
            filename: "file1.bin".to_string(),
            source: SourceRef {
                source: "source1".to_string(),
                reference: "ref1".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:00:00Z".to_string(),
        };

        let incoming2 = Incoming {
            filename: "file2.bin".to_string(),
            source: SourceRef {
                source: "source2".to_string(),
                reference: "ref2".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:01:00Z".to_string(), // Later timestamp
        };

        let record1 = cache.add(bytes1, incoming1).unwrap();
        let record2 = cache.add(bytes2, incoming2).unwrap();

        assert_ne!(record1.sha256, record2.sha256);

        let list = cache.list().unwrap();
        assert_eq!(list.len(), 2);
        // Should be sorted by added_at
        assert!(list[0].added_at <= list[1].added_at);
    }

    #[test]
    fn intact_is_false_after_overwrite_or_delete() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let bytes = b"test data";

        let incoming = Incoming {
            filename: "TEST-CAL-001.bin".to_string(),
            source: SourceRef {
                source: "test-source".to_string(),
                reference: "ref1".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2023-01-01T00:00:00Z".to_string(),
        };

        let record = cache.add(bytes, incoming).unwrap();
        assert!(cache.intact(&record).unwrap());

        // Overwrite with different data
        let new_bytes = b"different data";
        fs::write(cache.file_path(&record), new_bytes).unwrap();
        assert!(!cache.intact(&record).unwrap());

        // Delete the file
        fs::remove_file(cache.file_path(&record)).unwrap();
        assert!(!cache.intact(&record).unwrap());
    }

    #[test]
    fn get_returns_none_for_invalid_hashes() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();

        assert_eq!(cache.get("../../etc/passwd").unwrap(), None);
        assert_eq!(cache.get("zz").unwrap(), None);
    }

    #[test]
    fn junk_metadata_file_does_not_break_list() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();

        // Create a junk metadata file
        let junk_path = cache.root.join("metadata").join("junk.json");
        fs::write(&junk_path, "not json").unwrap();

        // This should not panic or fail
        let list = cache.list().unwrap();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn a_packed_package_is_kept_under_both_layers_of_its_name_and_untouched() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let bytes = b"\x1f\x8bstand-in for a packed package";
        let incoming = Incoming {
            filename: "TEST-MOD-A010.rwd.gz".to_string(),
            source: SourceRef {
                source: "folder".to_string(),
                reference: "ref1".to_string(),
                kind: SourceKind::UserFolder,
                found_at: "now".to_string(),
            },
            claims: Claims::new(),
            declared_sha256: None,
            now: "2026-10-04T00:00:00Z".to_string(),
        };

        let record = cache.add(bytes, incoming).unwrap();
        assert_eq!(record.format, ArtifactFormat::Rwd);
        assert_eq!(record.compression, Some(Compression::Gzip));
        let kept = cache.file_path(&record);
        assert!(kept.to_string_lossy().ends_with(".rwd.gz"), "{}", kept.display());
        // Kept as it arrived: the same bytes, not what they unpack to.
        assert_eq!(fs::read(&kept).unwrap(), bytes);
        assert!(cache.intact(&record).unwrap());
        // Nothing is left behind from writing it.
        let files: Vec<_> = fs::read_dir(dir.path().join("artifacts")).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
    }

    /// A packed file kept by the first build is at `<sha256>.gz` with a
    /// record that calls it `gz`. It is found where it is and read as what
    /// its name says.
    #[test]
    fn a_file_kept_before_layers_were_named_is_still_found() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let bytes = b"\x1f\x8bearlier";
        let sha = sha256_hex(bytes);
        fs::write(dir.path().join("artifacts").join(format!("{sha}.gz")), bytes).unwrap();
        fs::write(
            dir.path().join("metadata").join(format!("{sha}.json")),
            format!(
                r#"{{ "sha256": "{sha}", "size": {}, "format": "gz",
                      "filename": "TEST-MOD-A010.rwd.gz",
                      "sources": [{{ "source": "folder", "reference": "x", "found_at": "t" }}],
                      "added_at": "t" }}"#,
                bytes.len()
            ),
        )
        .unwrap();

        let record = cache.get(&sha).unwrap().expect("the earlier record reads");
        assert_eq!(record.format, ArtifactFormat::Rwd);
        assert_eq!(record.compression, Some(Compression::Gzip));
        assert_eq!(cache.read(&record).unwrap(), bytes);
        assert!(cache.intact(&record).unwrap());
        assert_eq!(cache.list().unwrap().len(), 1);
    }
}
