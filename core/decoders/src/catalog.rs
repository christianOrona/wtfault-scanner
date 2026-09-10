//! Finding the community signal definitions that might apply to a vehicle.
//!
//! [`signalset`](crate::signalset) knows how to read one vehicle's definitions.
//! This decides *which* ones to offer, which is the part where it would be easy
//! to do harm.
//!
//! # Exact, related, or nothing
//!
//! The catalogue is sparse. OBDb has hundreds of repositories and many are
//! still empty — checked on 2026-09-10, `Ford-F-150` held 116 commands while
//! `Ford-F-250` and `Honda-Odyssey` were both empty stubs. So the common case
//! for a given vehicle is that nothing exact exists, and the tempting move is to
//! reach for a sibling model.
//!
//! That move is available here, but it is never silent. A definition from
//! another model of the same make is offered as [`Relevance::RelatedModel`] and
//! must be presented as what it is: a *hypothesis about your vehicle drawn from
//! a different one*. Manufacturers do reuse identifiers across a range, and they
//! also change them between trims of the same year — both happen, so neither can
//! be assumed.
//!
//! # Why offering a hypothesis is safe here
//!
//! Reading a data identifier is a read. If the definition does not apply, the
//! module answers `requestOutOfRange` and we now say so precisely rather than
//! shrugging (see the refusal handling in `aim_protocols`). If it does apply,
//! the value arrives and can be sanity-checked against the definition's own
//! stated bounds. Either way the *vehicle* settles it, which is the same
//! measure-rather-than-guess loop the rest of this project runs on.
//!
//! Nothing here proposes a write, and nothing here promotes a definition to
//! verified. A catalogue entry that decoded into a plausible number is still a
//! catalogue entry.

use crate::signalset::{CommandDef, SignalSet};
use std::collections::BTreeMap;
use std::path::Path;

/// How much a catalogue entry has to do with the vehicle in front of us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relevance {
    /// Same make and model. Still unverified, but about this vehicle.
    Exact,
    /// Same make, different model. A hypothesis drawn from another vehicle.
    RelatedModel,
}

impl Relevance {
    /// Stable identifier for logs and interfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            Relevance::Exact => "exact",
            Relevance::RelatedModel => "related_model",
        }
    }

    /// What this relevance means, in language meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            Relevance::Exact => {
                "A community definition recorded for this make and model. Nobody has verified it \
                 against your vehicle, so what it produces is a reading to check rather than a \
                 fact."
            }
            Relevance::RelatedModel => {
                "A community definition recorded for a DIFFERENT model from the same \
                 manufacturer. Makers reuse identifiers across a range and also change them \
                 between models, so this is a guess worth testing rather than something to rely \
                 on. Reading it is safe: if it does not apply, the module answers that the \
                 identifier is out of range."
            }
        }
    }
}

/// One vehicle's definitions, with where they came from.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    /// Catalogue key, e.g. `Ford-F-150`.
    pub key: String,
    /// Make as parsed from the key.
    pub make: String,
    /// Model as parsed from the key.
    pub model: String,
    /// The definitions.
    pub signals: SignalSet,
}

/// A command offered for a vehicle, and how relevant it is.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    /// Which catalogue entry it came from.
    pub source_key: &'a str,
    /// How much that entry has to do with this vehicle.
    pub relevance: Relevance,
    /// The command itself.
    pub command: &'a CommandDef,
}

/// Every community signalset this build has on disk.
#[derive(Debug, Clone, Default)]
pub struct SignalCatalog {
    entries: BTreeMap<String, CatalogEntry>,
}

impl SignalCatalog {
    /// The signalsets compiled into this build.
    ///
    /// Shipped the same way as every other data file here, because a catalogue
    /// that only exists in the source tree reaches nobody who installed the
    /// app. User-supplied files are merged over these by
    /// [`SignalCatalog::merge`], so a locally corrected definition wins without
    /// a rebuild.
    ///
    /// These files are CC BY-SA 4.0 — see
    /// `vehicle-profiles/catalog/obdb/ATTRIBUTION.md`.
    pub fn embedded() -> SignalCatalog {
        const SHIPPED: [(&str, &str); 1] = [(
            "Ford-F-150",
            include_str!("../../../vehicle-profiles/catalog/obdb/Ford-F-150.json"),
        )];

        let mut catalog = SignalCatalog::default();
        for (key, text) in SHIPPED {
            match SignalSet::from_json(text) {
                Ok(signals) => {
                    let (make, model) = split_key(key);
                    catalog.entries.insert(
                        key.to_string(),
                        CatalogEntry { key: key.to_string(), make, model, signals },
                    );
                }
                // Unreachable in a build whose tests pass, and still not a
                // panic: a malformed community file must not stop somebody
                // diagnosing their vehicle.
                Err(e) => {
                    tracing::warn!(vehicle = key, error = %e, "shipped signalset is unreadable")
                }
            }
        }
        catalog
    }

    /// Take everything from `other`, replacing entries with the same key.
    ///
    /// User-supplied definitions win over shipped ones: somebody who has
    /// corrected a definition against their own vehicle knows more about it
    /// than the copy compiled in months ago.
    pub fn merge(&mut self, other: SignalCatalog) {
        self.entries.extend(other.entries);
    }

    /// Load every `*.json` in a directory. The file stem is the catalogue key.
    ///
    /// A missing directory yields an empty catalogue rather than an error: a
    /// build with no community data is a normal build, and the feature it
    /// powers simply has nothing to offer.
    ///
    /// Unreadable or malformed files are skipped and named in the returned
    /// warnings. One bad file must not cost the user the other seven hundred.
    pub fn load_dir(dir: impl AsRef<Path>) -> (SignalCatalog, Vec<String>) {
        let mut catalog = SignalCatalog::default();
        let mut warnings = Vec::new();

        let read = match std::fs::read_dir(dir.as_ref()) {
            Ok(r) => r,
            Err(_) => return (catalog, warnings),
        };
        for item in read.flatten() {
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(key) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{key}: could not be read ({e})"));
                    continue;
                }
            };
            match SignalSet::from_json(&text) {
                Ok(signals) => {
                    let (make, model) = split_key(&key);
                    catalog.entries.insert(key.clone(), CatalogEntry { key, make, model, signals });
                }
                Err(e) => warnings.push(format!("{key}: {e}")),
            }
        }
        (catalog, warnings)
    }

    /// How many vehicles are catalogued.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is catalogued.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every catalogue key held, sorted.
    pub fn keys(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// Commands that might apply to a vehicle, most relevant first.
    ///
    /// `make` and `model` are matched case-insensitively, and a model is also
    /// matched when either side contains the other — a vehicle identified as
    /// "F-150 SuperCrew" should find the `Ford-F-150` catalogue.
    ///
    /// Definitions scoped to model years this vehicle is not in are excluded by
    /// [`CommandDef::applies_to_year`], which fails closed when the year is
    /// unknown. That matters more than it sounds: of the 116 commands recorded
    /// for an F-150, only 7 are unscoped.
    pub fn candidates(
        &self,
        make: Option<&str>,
        model: Option<&str>,
        year: Option<u16>,
    ) -> Vec<Candidate<'_>> {
        let Some(make) = make.map(str::trim).filter(|m| !m.is_empty()) else {
            // Without a make there is no honest way to choose between seven
            // hundred vehicles, and offering all of them is not a feature.
            return Vec::new();
        };

        let mut out = Vec::new();
        for entry in self.entries.values() {
            if !makes_match(&entry.make, make) {
                continue;
            }
            let relevance = match model.map(str::trim).filter(|m| !m.is_empty()) {
                Some(m) if models_match(&entry.model, m) => Relevance::Exact,
                // Same make, different or unknown model.
                _ => Relevance::RelatedModel,
            };
            for command in entry.signals.for_year(year) {
                out.push(Candidate { source_key: &entry.key, relevance, command });
            }
        }
        out.sort_by(|a, b| {
            a.relevance.cmp(&b.relevance).then_with(|| a.source_key.cmp(b.source_key))
        });
        out
    }
}

/// Split `Ford-F-150` into make and model.
///
/// The first segment is the make; everything after it is the model, because
/// models contain hyphens far more often than makes do.
fn split_key(key: &str) -> (String, String) {
    match key.split_once('-') {
        Some((make, model)) => (make.to_string(), model.to_string()),
        None => (key.to_string(), String::new()),
    }
}

/// Whether a catalogue make and a vehicle's reported make are the same maker.
///
/// A catalogue key carries a brand — `Ford`, `Honda`. A vehicle's make comes
/// from decoding the VIN's world manufacturer identifier and is a description
/// of the manufacturer rather than a brand: a real F-250 decodes to
/// `"Ford Motor Company (US, truck)"`. Comparing those for equality finds
/// nothing, which is how a catalogue with the right data in it can appear
/// empty.
///
/// Matched two precise ways rather than by loose substring, because a substring
/// test across seven hundred makers will eventually match the wrong one: the
/// brand appears as a whole word in the description, or the description begins
/// with it.
fn makes_match(catalog: &str, reported: &str) -> bool {
    if catalog.eq_ignore_ascii_case(reported) {
        return true;
    }
    let brand = squash(catalog);
    if brand.is_empty() {
        return false;
    }
    // Whole words only. `Honda` must not match `Hondaro`, and across hundreds
    // of makers that kind of near-miss is a matter of time rather than luck.
    let words: Vec<String> = reported
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(squash)
        .collect();

    // A run of consecutive words, so a hyphenated brand like `Mercedes-Benz`
    // still matches a make written `Mercedes Benz AG`.
    for start in 0..words.len() {
        let mut joined = String::new();
        for word in &words[start..] {
            joined.push_str(word);
            if joined == brand {
                return true;
            }
            if joined.len() >= brand.len() {
                break;
            }
        }
    }
    false
}

/// Whether a catalogue model and a vehicle's reported model are the same thing.
///
/// Reported models carry trim and body noise the catalogue does not — "F-150
/// SuperCrew", "F150", "Odyssey EX-L". Comparison is therefore done on letters
/// and digits only, with containment either way.
fn models_match(catalog: &str, reported: &str) -> bool {
    let a = squash(catalog);
    let b = squash(reported);
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue as a user actually gets it: compiled in.
    fn bundled() -> SignalCatalog {
        SignalCatalog::embedded()
    }

    /// The bundled data has to parse with the parser that ships beside it. If
    /// OBDb changes its format, this is where we find out, rather than a user.
    #[test]
    fn the_bundled_catalogue_parses() {
        let catalog = bundled();
        assert!(!catalog.is_empty(), "expected at least one bundled signalset");
        assert!(catalog.keys().contains(&"Ford-F-150"));
    }

    #[test]
    fn an_exact_match_outranks_a_sibling_model() {
        let catalog = bundled();
        let exact = catalog.candidates(Some("Ford"), Some("F-150"), Some(2019));
        assert!(!exact.is_empty());
        assert!(exact.iter().all(|c| c.relevance == Relevance::Exact));

        // Same make, a model with no catalogue of its own: offered, but never
        // as though it were about this vehicle.
        let sibling = catalog.candidates(Some("Ford"), Some("F-250"), Some(2019));
        assert!(!sibling.is_empty(), "a sibling model should still get candidates");
        assert!(sibling.iter().all(|c| c.relevance == Relevance::RelatedModel));
    }

    /// Reported models carry trim noise the catalogue does not.
    #[test]
    fn a_reported_model_with_trim_still_finds_its_catalogue() {
        let catalog = bundled();
        for reported in ["F-150", "F150", "f-150 supercrew", "F-150 XLT"] {
            let found = catalog.candidates(Some("Ford"), Some(reported), Some(2019));
            assert!(
                found.iter().any(|c| c.relevance == Relevance::Exact),
                "{reported:?} should match the F-150 catalogue"
            );
        }
    }

    /// A VIN decodes to a description of the manufacturer, not a brand. Found
    /// live: a real vehicle identified as "Ford Motor Company (US, truck)" and
    /// matched nothing, so a catalogue with the right data in it looked empty.
    #[test]
    fn a_make_from_a_decoded_vin_finds_its_catalogue() {
        let catalog = bundled();
        for reported in
            ["Ford Motor Company (US, truck)", "Ford Motor Company", "FORD", "Ford (US)"]
        {
            assert!(
                !catalog.candidates(Some(reported), Some("F-150"), Some(2019)).is_empty(),
                "{reported:?} should reach the Ford catalogue"
            );
        }
    }

    /// Tolerant make matching must stay precise. Across hundreds of makers a
    /// loose substring test eventually matches the wrong one.
    #[test]
    fn a_make_that_merely_contains_the_brand_late_is_not_a_match() {
        assert!(makes_match("Ford", "Ford Motor Company (US, truck)"));
        assert!(makes_match("Ford", "FORD"));
        // The brand as a whole word, or leading. Not buried mid-word.
        assert!(!makes_match("Ford", "Sandford Autos"));
        assert!(!makes_match("Ford", "Crawfordsville Motors"));
        assert!(!makes_match("Honda", "Hondaro"));
    }

    /// Another manufacturer's definitions are never offered. Identifier reuse
    /// happens within a maker's range, not across makers.
    #[test]
    fn another_manufacturers_definitions_are_never_offered() {
        let catalog = bundled();
        assert!(catalog.candidates(Some("Honda"), Some("Odyssey"), Some(2023)).is_empty());
        assert!(catalog.candidates(Some("Toyota"), None, Some(2019)).is_empty());
    }

    /// Without a make there is no honest way to choose, and offering
    /// everything is not a feature.
    #[test]
    fn an_unidentified_vehicle_gets_nothing() {
        let catalog = bundled();
        assert!(catalog.candidates(None, Some("F-150"), Some(2019)).is_empty());
        assert!(catalog.candidates(Some(""), None, None).is_empty());
    }

    /// The year filter fails closed, and on real data that is most of the
    /// catalogue rather than an edge case.
    #[test]
    fn an_unknown_model_year_costs_most_of_the_catalogue() {
        let catalog = bundled();
        let with_year = catalog.candidates(Some("Ford"), Some("F-150"), Some(2019)).len();
        let without = catalog.candidates(Some("Ford"), Some("F-150"), None).len();
        assert!(without < with_year, "an unknown year must not unlock more than a known one");
        assert!(with_year > 100, "expected the real F-150 catalogue, got {with_year}");
    }

    /// A directory that is not there is a normal state, not a failure.
    #[test]
    fn a_missing_catalogue_directory_is_not_an_error() {
        let (catalog, warnings) = SignalCatalog::load_dir("no/such/place");
        assert!(catalog.is_empty());
        assert!(warnings.is_empty());
    }

    /// One malformed file must not cost the user the rest of the catalogue.
    #[test]
    fn a_malformed_file_is_named_and_skipped() {
        let dir = std::env::temp_dir().join(format!("aim-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Broken-Thing.json"), "{ not json").unwrap();
        std::fs::write(dir.join("Ford-Test.json"), r#"{"commands":[]}"#).unwrap();

        let (catalog, warnings) = SignalCatalog::load_dir(&dir);
        assert_eq!(catalog.len(), 1, "the good file should still load");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Broken-Thing"), "{warnings:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_catalogue_key_splits_into_make_and_model() {
        assert_eq!(split_key("Ford-F-150"), (String::from("Ford"), String::from("F-150")));
        assert_eq!(
            split_key("Jeep-Grand-Cherokee"),
            (String::from("Jeep"), String::from("Grand-Cherokee"))
        );
    }
}
