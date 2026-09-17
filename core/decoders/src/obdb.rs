//! Where a vehicle's community signal set lives on OBDb (#56).
//!
//! OBDb publishes one repository per make and model, named like `Ford-F-150`
//! or `Mazda-3`, each holding `signalsets/v3/default.json`. Nothing here
//! fetches anything: it only works out the names, so the fetch can stay on
//! request only and be tested without a network.

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
