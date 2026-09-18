//! Where a vehicle's community signal set lives on OBDb (#56).
//!
//! OBDb publishes one repository per make and model, named like `Ford-F-150`
//! or `Mazda-3`, each holding `signalsets/v3/default.json`. Nothing here
//! fetches anything: it only works out the names, so the fetch can stay on
//! request only and be tested without a network.

use aim_types::{AimError, AimResult, ErrorCode};
use std::path::{Path, PathBuf};

/// The largest signal set kept. OBDb's F-150 set is 38 KB.
pub const MAX_SIGNALSET_BYTES: usize = 2 * 1024 * 1024;

/// The OBDb repository name for a make and model, e.g. `Ford-F-150`.
pub fn repository_name(make: &str, model: &str) -> Option<String> {
    let make = make.trim();
    if make.is_empty() {
        return None;
    }

    // Title-case the make
    let make = make
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
            }
        })
        .collect::<Vec<_>>()
        .join("-");

    // Process the model
    let model = model.trim();
    if model.is_empty() {
        return None;
    }

    // vPIC writes some models with the make in front ("Mazda3", "Mazda 6") and
    // OBDb does not. Only a whole word is removed, so "Minivan" from MINI stays whole.
    let first_word = make.split('-').next().unwrap_or_default();
    let n = first_word.len();
    let mut model = model;
    if model.len() >= n && model.is_char_boundary(n) && model[..n].eq_ignore_ascii_case(first_word)
    {
        let rest = &model[n..];
        if rest.chars().next().is_none_or(|c| c.is_ascii_digit() || c.is_whitespace() || c == '-') {
            model = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '-');
        }
    }

    if model.is_empty() {
        return None;
    }

    // Split model on whitespace and join with hyphens
    let model = model.split_whitespace().collect::<Vec<_>>().join("-");

    Some(format!("{make}-{model}"))
}

/// The raw URL of a repository's signal set.
pub fn signalset_url(repository: &str) -> String {
    format!("https://raw.githubusercontent.com/OBDb/{repository}/main/signalsets/v3/default.json")
}

/// The repository's web page, kept with a cached file for attribution.
pub fn repository_url(repository: &str) -> String {
    format!("https://github.com/OBDb/{repository}")
}

/// A name that can only ever be a file in the catalogue folder: letters, digits
/// and hyphens, in OBDb's `Make-Model` shape. The hyphen also rules out Windows
/// device names such as `CON`, which no folder may hold a file called.
fn valid_repository(repository: &str) -> bool {
    repository.contains('-')
        && !repository.starts_with('-')
        && repository.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Keep a fetched signal set where community sets are loaded from.
///
/// Written to `<profiles_dir>/catalog/obdb/<repository>.json`, the folder
/// [`crate::DecoderSet::with_profiles`] reads on start, with
/// `<repository>.ATTRIBUTION.md` beside it: OBDb data is CC BY-SA 4.0, and the
/// credit travels with the file. Nothing that is not a signal set is kept, and
/// a repository name can never point outside that folder.
pub fn cache_signalset(profiles_dir: &Path, repository: &str, body: &str) -> AimResult<PathBuf> {
    if !valid_repository(repository) {
        return Err(AimError::new(
            ErrorCode::DecoderInputInvalid,
            format!("{repository:?} is not an OBDb repository name"),
        ));
    }

    if body.len() > MAX_SIGNALSET_BYTES {
        return Err(AimError::new(
            ErrorCode::DecoderInputInvalid,
            "reply is too large to be a signal set".to_string(),
        ));
    }

    let _ = crate::signalset::SignalSet::from_json(body)
        .map_err(|e| AimError::new(ErrorCode::DecoderInputInvalid, e))?;

    let dir = profiles_dir.join("catalog").join("obdb");
    std::fs::create_dir_all(&dir).map_err(|e| {
        AimError::new(ErrorCode::StorageError, format!("could not keep the signal set: {e}"))
    })?;

    let json_path = dir.join(format!("{repository}.json"));
    let attribution_path = dir.join(format!("{repository}.ATTRIBUTION.md"));

    std::fs::write(&json_path, body).map_err(|e| {
        AimError::new(ErrorCode::StorageError, format!("could not keep the signal set: {e}"))
    })?;

    let url = repository_url(repository);
    let attribution_content = format!(
        "# {repository}\n\nCommunity signal definitions from OBDb (https://obdb.community), fetched from {url}.\n\nLicensed CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/). Attribution to OBDb must be preserved, and changes to the file stay CC BY-SA 4.0.\n"
    );
    std::fs::write(&attribution_path, attribution_content).map_err(|e| {
        AimError::new(ErrorCode::StorageError, format!("could not keep the signal set: {e}"))
    })?;

    Ok(json_path)
}

/// The kept signal set for a repository, when there is one.
pub fn cached_signalset(profiles_dir: &Path, repository: &str) -> Option<PathBuf> {
    if !valid_repository(repository) {
        return None;
    }
    let path = profiles_dir.join("catalog").join("obdb").join(format!("{repository}.json"));
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// What a fetched signal set records about the vehicle it was fetched for.
///
/// Returns the finding's subject and its claim. A signal set is knowledge
/// about this vehicle and belongs on the scorecard, so that a second visit
/// starts ahead of the first — but it is a list of what to ask, not a
/// measurement, so the claim says plainly that nothing in it is verified until
/// it has actually been read on this vehicle.
pub fn signalset_finding(repository: &str, commands: usize) -> (String, String) {
    let subject = format!("signals.obdb.{repository}");
    let claim = match commands {
        0 => format!("OBDb has a signal set for {repository} with nothing in it yet"),
        1 => format!("OBDb offers 1 signal for {repository}, unverified until it is read on this vehicle"),
        _ => format!("OBDb offers {commands} signals for {repository}, unverified until each is read on this vehicle"),
    };
    (subject, claim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_is_the_repository_under_a_signals_obdb_prefix() {
        assert_eq!(signalset_finding("Mazda-Mazda3", 0).0, "signals.obdb.Mazda-Mazda3");
    }

    #[test]
    fn one_signal_is_not_described_in_the_plural() {
        let claim = signalset_finding("Mazda-Mazda3", 1).1;
        assert!(claim.contains("1 signal for"));
        assert!(!claim.contains("signals"));
    }

    #[test]
    fn several_signals_are_counted() {
        let claim = signalset_finding("Mazda-Mazda3", 214).1;
        assert!(claim.contains("214 signals for Mazda-Mazda3"));
    }

    #[test]
    fn an_empty_set_says_so_rather_than_claiming_nothing() {
        let claim = signalset_finding("Mazda-Mazda3", 0).1;
        assert!(claim.contains("nothing in it yet"));
        assert!(!claim.contains("unverified"));
    }

    #[test]
    fn a_claim_about_a_set_with_signals_in_it_always_says_they_are_unverified() {
        let claim1 = signalset_finding("Mazda-Mazda3", 1).1;
        let claim214 = signalset_finding("Mazda-Mazda3", 214).1;
        assert!(claim1.contains("unverified until"));
        assert!(claim214.contains("unverified until"));
    }
}
