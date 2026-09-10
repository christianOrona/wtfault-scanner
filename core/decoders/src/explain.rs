//! Plain-language explanations, loaded from data.
//!
//! Every identifier this product can put on screen — a signal id, a warning
//! code, an interface concept — can carry two explanations: one for someone who
//! has never opened a bonnet, and one for someone who could do the repair. The
//! UI picks which to show; the core only supplies both.
//!
//! This lives beside the PID and DTC catalogues for the same reason they are
//! data files: an explanation is vehicle knowledge, it is versioned, and adding
//! one must not require a code change. It also means a *missing* explanation is
//! a queryable gap rather than a string that quietly never got written.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One explanation, at both reading levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    /// The identifier being explained: a signal id, warning code, or concept.
    pub id: String,
    /// For someone with no mechanical background. No unexpanded jargon.
    pub easy: String,
    /// For someone who could act on it. Allowed to assume the vocabulary.
    pub technical: String,
}

/// Which kind of thing an explanation is about.
///
/// Kept separate rather than merged into one namespace because the same word
/// can legitimately mean different things in each: `unverified` is a warning
/// code and could equally be a concept, and conflating them would make one of
/// the two unwritable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplainKind {
    /// A decoded live-data signal, keyed by `signal_id`.
    Signal,
    /// A warning or error code the core can attach to a result.
    Code,
    /// An idea the interface uses, rather than anything from the vehicle.
    Concept,
}

#[derive(Debug, Deserialize)]
struct ExplainFile {
    #[serde(default)]
    signals: Vec<Explanation>,
    #[serde(default)]
    codes: Vec<Explanation>,
    #[serde(default)]
    concepts: Vec<Explanation>,
}

/// Every explanation this build ships.
#[derive(Debug, Clone, Default)]
pub struct ExplanationCatalog {
    signals: BTreeMap<String, Explanation>,
    codes: BTreeMap<String, Explanation>,
    concepts: BTreeMap<String, Explanation>,
}

impl ExplanationCatalog {
    /// Load the explanations embedded in the binary.
    pub fn generic_obd() -> AimResult<ExplanationCatalog> {
        let mut c = ExplanationCatalog::default();
        c.load_yaml(include_str!("../../../vehicle-profiles/generic-obd/explanations.yaml"))?;
        Ok(c)
    }

    /// Merge one YAML file into the catalogue.
    pub fn load_yaml(&mut self, yaml: &str) -> AimResult<()> {
        let file: ExplainFile = serde_yaml_ng::from_str(yaml).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("could not parse explanations file: {e}"),
            )
        })?;
        for (bucket, entries) in [
            (&mut self.signals, file.signals),
            (&mut self.codes, file.codes),
            (&mut self.concepts, file.concepts),
        ] {
            for e in entries {
                if e.easy.trim().is_empty() || e.technical.trim().is_empty() {
                    return Err(AimError::new(
                        ErrorCode::DecoderInputInvalid,
                        format!("explanation {:?} is missing one of its two levels", e.id),
                    ));
                }
                bucket.insert(e.id.clone(), e);
            }
        }
        Ok(())
    }

    /// Look one up. `None` is a real answer: this build has nothing to say
    /// about that identifier, and the UI shows the identifier itself rather
    /// than inventing a description.
    pub fn get(&self, kind: ExplainKind, id: &str) -> Option<&Explanation> {
        self.bucket(kind).get(id)
    }

    /// Every explanation of one kind, for shipping the whole set to a client
    /// that then needs no further round trips.
    pub fn all(&self, kind: ExplainKind) -> impl Iterator<Item = &Explanation> {
        self.bucket(kind).values()
    }

    /// How many explanations exist of one kind.
    pub fn len(&self, kind: ExplainKind) -> usize {
        self.bucket(kind).len()
    }

    /// True when this build ships no explanations at all.
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty() && self.codes.is_empty() && self.concepts.is_empty()
    }

    fn bucket(&self, kind: ExplainKind) -> &BTreeMap<String, Explanation> {
        match kind {
            ExplainKind::Signal => &self.signals,
            ExplainKind::Code => &self.codes,
            ExplainKind::Concept => &self.concepts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ExplanationCatalog {
        ExplanationCatalog::generic_obd().unwrap()
    }

    #[test]
    fn the_shipped_file_loads_and_covers_all_three_kinds() {
        let c = catalog();
        assert!(c.len(ExplainKind::Signal) > 40);
        assert!(c.len(ExplainKind::Code) > 8);
        assert!(c.len(ExplainKind::Concept) > 8);
    }

    #[test]
    fn every_signal_this_build_can_decode_has_an_explanation() {
        // The point of the whole feature: a reading that reaches the screen
        // with no plain-language description is the thing being fixed, so a new
        // PID definition without one fails here rather than shipping.
        let pids = crate::PidRegistry::generic_obd().unwrap();
        let c = catalog();
        let missing: Vec<&str> = pids
            .iter()
            .map(|d| d.signal_id.as_str())
            .filter(|id| c.get(ExplainKind::Signal, id).is_none())
            .collect();
        assert!(missing.is_empty(), "signals with no explanation: {missing:?}");
    }

    #[test]
    fn an_unknown_identifier_returns_nothing_rather_than_a_guess() {
        assert!(catalog().get(ExplainKind::Signal, "not_a_signal").is_none());
        assert!(catalog().get(ExplainKind::Code, "not_a_code").is_none());
    }

    #[test]
    fn the_two_levels_are_actually_different_text() {
        // A copy-pasted pair would pass a naive "is it present" check while
        // giving the beginner the jargon the whole feature exists to avoid.
        let c = catalog();
        for kind in [ExplainKind::Signal, ExplainKind::Code, ExplainKind::Concept] {
            for e in c.all(kind) {
                assert_ne!(e.easy, e.technical, "{:?} {} says the same thing twice", kind, e.id);
            }
        }
    }

    #[test]
    fn the_easy_text_avoids_the_abbreviations_it_exists_to_replace() {
        // Not a style rule for its own sake. "PID", "ECU" and "DTC" are exactly
        // the words the beginner does not know, and letting them through is how
        // this kind of copy silently rots back into jargon. Concepts may name
        // the thing they define; signals and codes may not.
        const BANNED: [&str; 6] = ["PID", "ECU", "DTC", "ISO-TP", "stoichiometric", "MIL"];
        let c = catalog();
        for kind in [ExplainKind::Signal, ExplainKind::Code] {
            for e in c.all(kind) {
                for word in BANNED {
                    assert!(
                        !e.easy.contains(word),
                        "the easy text for {} uses {word:?}: {}",
                        e.id,
                        e.easy
                    );
                }
            }
        }
    }

    #[test]
    fn a_half_written_explanation_is_rejected_at_load() {
        let mut c = ExplanationCatalog::default();
        let err = c
            .load_yaml("signals:\n  - id: x\n    easy: \"something\"\n    technical: \"  \"\n")
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
    }
}
