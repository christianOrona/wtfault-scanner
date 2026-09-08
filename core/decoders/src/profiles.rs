//! User-supplied vehicle knowledge, loaded at startup.
//!
//! Everything this product knows about vehicles is data. This lets that data be
//! extended without rebuilding: a directory of YAML files that is merged over
//! the embedded set every time the core starts.
//!
//! It exists because of a specific, unavoidable problem. The mapping between a
//! feature like "auto-fold mirrors" and the bits that hold it is manufacturer
//! specific and unpublished. Nobody can derive it from first principles, and
//! this project will not let a language model guess it. But somebody who has
//! measured it on their own vehicle — read the configuration, changed it with a
//! tool known to work, read it again, diffed the two — has genuinely produced
//! that knowledge, and there needs to be somewhere to put it that is not a pull
//! request and a new binary.
//!
//! # Everything loaded here is attributed
//!
//! A definition from a user file carries the file it came from all the way
//! through to the screen. A mapping that turns out to be wrong is traceable to
//! whoever supplied it rather than blamed on the tool. That is also why loading
//! is *reported* rather than silent: a user who cannot see what got loaded
//! cannot tell the difference between an app that read their file and an app
//! that ignored it.
//!
//! # A bad file must not stop the app
//!
//! One unparseable file is a warning about that file, not a refusal to start.
//! The alternative — a core that will not boot because of a stray comma in an
//! optional data file — is worse than the gap it is protecting against.

use crate::{ExplanationCatalog, FeatureCatalog, PidRegistry};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What one file contributed, or why it did not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileLoad {
    /// The file, as an absolute path.
    pub path: String,
    /// Its file name, which is also the attribution string used on screen.
    pub source: String,
    /// What kind of content it turned out to hold.
    pub kind: ProfileKind,
    /// How many definitions it contributed.
    pub loaded: usize,
    /// Why it contributed nothing, when it did not.
    pub error: Option<String>,
}

/// What a profile file contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// Configurable feature definitions.
    Features,
    /// Live-data parameter definitions.
    Pids,
    /// Plain-language explanations.
    Explanations,
    /// Parsed as YAML but matched none of the known shapes.
    Unrecognised,
}

/// The outcome of scanning a profile directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileReport {
    /// The directory that was scanned, whether or not it existed.
    pub directory: Option<String>,
    /// One entry per file found, in load order.
    pub files: Vec<ProfileLoad>,
}

impl ProfileReport {
    /// Total definitions added across every file.
    pub fn total_loaded(&self) -> usize {
        self.files.iter().map(|f| f.loaded).sum()
    }

    /// Files that failed, which the UI must surface rather than swallow.
    pub fn failures(&self) -> impl Iterator<Item = &ProfileLoad> {
        self.files.iter().filter(|f| f.error.is_some())
    }
}

/// The decoder data a profile directory can extend.
pub struct Extensible<'a> {
    /// Feature definitions.
    pub features: &'a mut FeatureCatalog,
    /// PID definitions.
    pub pids: &'a mut PidRegistry,
    /// Explanations.
    pub explanations: &'a mut ExplanationCatalog,
}

/// Merge every `.yaml` file in `dir` into the given catalogues.
///
/// A directory that does not exist is not an error — it is the normal state
/// before anyone has added anything.
pub fn load_directory(dir: &Path, into: Extensible<'_>) -> ProfileReport {
    let mut report = ProfileReport {
        directory: Some(dir.display().to_string()),
        files: Vec::new(),
    };

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return report,
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
        })
        .collect();
    // Deterministic order: two files defining the same id must resolve the same
    // way on every machine and every start.
    paths.sort();

    let Extensible { features, pids, explanations } = into;

    for path in paths {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        let source = format!("user:{name}");

        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                report.files.push(ProfileLoad {
                    path: path.display().to_string(),
                    source,
                    kind: ProfileKind::Unrecognised,
                    loaded: 0,
                    error: Some(format!("could not read the file: {e}")),
                });
                continue;
            }
        };

        // Dispatch on the top-level key rather than on the file name, so a file
        // can be called anything the user finds meaningful.
        let kind = classify(&text);
        let result: Result<usize, String> = match kind {
            ProfileKind::Features => features
                .load_yaml(&text, &source)
                .map_err(|e| e.message.clone()),
            ProfileKind::Pids => pids
                .load_yaml(&text)
                .map(|_| 1)
                .map_err(|e| e.message.clone()),
            ProfileKind::Explanations => explanations
                .load_yaml(&text)
                .map(|_| 1)
                .map_err(|e| e.message.clone()),
            ProfileKind::Unrecognised => Err(String::from(
                "no recognised top-level key: expected one of `features`, `pids`, \
                 `signals`, `codes` or `concepts`",
            )),
        };

        let (loaded, error) = match result {
            Ok(n) => (n, None),
            Err(e) => (0, Some(e)),
        };
        report.files.push(ProfileLoad {
            path: path.display().to_string(),
            source,
            kind,
            loaded,
            error,
        });
    }

    report
}

fn classify(text: &str) -> ProfileKind {
    // Cheap and good enough: a top-level key is a line with no leading space.
    let has = |key: &str| {
        text.lines()
            .any(|l| l.starts_with(key) && l[key.len()..].trim_start().starts_with(':'))
    };
    if has("features") {
        ProfileKind::Features
    } else if has("pids") {
        ProfileKind::Pids
    } else if has("signals") || has("codes") || has("concepts") {
        ProfileKind::Explanations
    } else {
        ProfileKind::Unrecognised
    }
}

/// The README written into an empty profile directory the first time it is
/// created, so the feature is discoverable from the filesystem alone.
pub const PROFILE_README: &str = include_str!("profiles_readme.md");

/// Create the profile directory if it is missing, with its README.
///
/// Best effort: a read-only or otherwise unwritable location is not worth
/// failing a startup over.
pub fn ensure_directory(dir: &Path) {
    if dir.exists() {
        return;
    }
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join("README.md"), PROFILE_README);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeatureSupport;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aim-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn catalogs() -> (FeatureCatalog, PidRegistry, ExplanationCatalog) {
        (
            FeatureCatalog::embedded().unwrap(),
            PidRegistry::generic_obd().unwrap(),
            ExplanationCatalog::generic_obd().unwrap(),
        )
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        let (mut f, mut p, mut e) = catalogs();
        let report = load_directory(
            Path::new("./definitely-not-a-real-directory"),
            Extensible { features: &mut f, pids: &mut p, explanations: &mut e },
        );
        assert!(report.files.is_empty());
        assert_eq!(report.total_loaded(), 0);
    }

    #[test]
    fn a_user_file_can_complete_a_shipped_feature() {
        // The whole point of the mechanism: somebody measures the mapping on
        // their own truck and the app starts being able to use it, with the
        // file that supplied it named on screen.
        let dir = temp_dir("complete");
        std::fs::write(
            dir.join("my-f250.yaml"),
            r#"
features:
  - id: mirror_auto_fold
    name: "Automatic folding mirrors"
    easy: "Mirrors fold when you lock the truck."
    technical: "Power-fold auto-actuation bit in the door module."
    risk: convenience
    modules: [DDM, PDM]
    verification: verified
    mapping:
      kind: as_built_bits
      block: "740-01"
      byte: 2
      mask: 0x30
      on: 0x10
      off: 0x00
"#,
        )
        .unwrap();

        let (mut f, mut p, mut e) = catalogs();
        assert_eq!(f.get("mirror_auto_fold").unwrap().support(), FeatureSupport::DescribedOnly);

        let report = load_directory(
            &dir,
            Extensible { features: &mut f, pids: &mut p, explanations: &mut e },
        );

        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].kind, ProfileKind::Features);
        assert_eq!(report.files[0].loaded, 1);
        assert!(report.files[0].error.is_none());

        let after = f.get("mirror_auto_fold").unwrap();
        assert_eq!(after.support(), FeatureSupport::Writable);
        assert_eq!(after.source.as_deref(), Some("user:my-f250.yaml"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_broken_file_does_not_stop_the_others() {
        let dir = temp_dir("broken");
        // Sorts first, so a naive implementation that gave up would lose the
        // good file behind it.
        std::fs::write(dir.join("a-broken.yaml"), "features: [ this is not: valid: yaml").unwrap();
        std::fs::write(
            dir.join("b-good.yaml"),
            "features:\n  - id: t\n    name: T\n    easy: e\n    technical: t\n    risk: cosmetic\n    modules: []\n",
        )
        .unwrap();

        let (mut f, mut p, mut e) = catalogs();
        let report = load_directory(
            &dir,
            Extensible { features: &mut f, pids: &mut p, explanations: &mut e },
        );

        assert_eq!(report.files.len(), 2);
        assert_eq!(report.failures().count(), 1);
        assert!(report.files[0].error.is_some(), "the broken one reports why");
        assert_eq!(report.files[1].loaded, 1, "the good one still loaded");
        assert!(f.get("t").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_of_an_unknown_shape_says_so_rather_than_failing_silently() {
        let dir = temp_dir("unknown");
        std::fs::write(dir.join("mystery.yaml"), "something_else:\n  - 1\n").unwrap();
        let (mut f, mut p, mut e) = catalogs();
        let report = load_directory(
            &dir,
            Extensible { features: &mut f, pids: &mut p, explanations: &mut e },
        );
        assert_eq!(report.files[0].kind, ProfileKind::Unrecognised);
        assert!(report.files[0].error.as_ref().unwrap().contains("features"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explanations_can_be_extended_too() {
        let dir = temp_dir("explain");
        std::fs::write(
            dir.join("extra.yaml"),
            "signals:\n  - id: my_signal\n    easy: \"A thing.\"\n    technical: \"A different thing.\"\n",
        )
        .unwrap();
        let (mut f, mut p, mut e) = catalogs();
        let report = load_directory(
            &dir,
            Extensible { features: &mut f, pids: &mut p, explanations: &mut e },
        );
        assert_eq!(report.files[0].kind, ProfileKind::Explanations);
        assert!(e.get(crate::ExplainKind::Signal, "my_signal").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_order_is_deterministic() {
        // Two files defining the same id must resolve identically on every
        // machine, so the later name always wins rather than whatever the
        // filesystem happened to return first.
        let dir = temp_dir("order");
        for (name, val) in [("01-first.yaml", "First"), ("02-second.yaml", "Second")] {
            std::fs::write(
                dir.join(name),
                format!("features:\n  - id: dup\n    name: {val}\n    easy: e\n    technical: t\n    risk: cosmetic\n    modules: []\n"),
            )
            .unwrap();
        }
        let (mut f, mut p, mut e) = catalogs();
        load_directory(&dir, Extensible { features: &mut f, pids: &mut p, explanations: &mut e });
        assert_eq!(f.get("dup").unwrap().name, "Second");
        assert_eq!(f.get("dup").unwrap().source.as_deref(), Some("user:02-second.yaml"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
