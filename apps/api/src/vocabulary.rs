//! Keeping two lists that must agree from silently disagreeing.
//!
//! # Why this exists
//!
//! Twice in one day, a list of names drifted from the list of things it named,
//! and nothing noticed until somebody was looking at a vehicle.
//!
//! A guided procedure declared it measured `short_term_fuel_trim_1`. The signal
//! catalogue calls it `short_fuel_trim_b1`. The procedure ran on a warm engine,
//! measured nothing it existed for, and reported success.
//!
//! The progress panel turns tool names into sentences from a dictionary.
//! `scan_all_modules` had no entry, so it appeared as a raw identifier in the
//! middle of plain English while somebody watched their truck being inspected.
//!
//! Both are the same shape: one list is the truth, another list describes it,
//! and no test compares them. A comment saying "keep these in sync" is not a
//! mechanism. This is.
//!
//! # Why it reads source files
//!
//! The dictionary lives in TypeScript and the tool names live in Rust, so there
//! is no type they can share. Reading both as text is coarse and will need
//! updating if either moves — which is a cost worth paying for a check that
//! fails loudly in CI instead of quietly in a garage.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        // CARGO_MANIFEST_DIR is apps/api.
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo root")
    }

    /// Every tool name that reaches the flight recorder, and therefore the
    /// progress panel.
    fn recorded_tool_names(source: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for (_, rest) in source.match_indices("record_invocation(").map(|(i, m)| (i, &source[i + m.len()..])) {
            // The name is the first string literal after the call opens. It is
            // written inline at every call site in this codebase; a variable
            // there would be missed, and the test below would then be checking
            // less than it claims — so it also asserts a plausible count.
            let Some(open) = rest.find('"') else { continue };
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { continue };
            let name = &after[..close];
            // Skip anything that is plainly not a tool name.
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                names.insert(name.to_string());
            }
        }
        names
    }

    /// The keys of the progress panel's plain-language dictionary.
    fn dictionary_keys(source: &str) -> BTreeSet<String> {
        let Some(start) = source.find("const PLAIN") else {
            panic!("the progress dictionary has been renamed; this test needs updating");
        };
        let body = &source[start..];
        let Some(end) = body.find("\n};") else {
            panic!("cannot find the end of the progress dictionary");
        };

        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with("//") {
                    return None;
                }
                let (key, rest) = line.split_once(':')?;
                // Values are quoted strings; anything else is not an entry.
                if !rest.trim_start().starts_with('"') {
                    return None;
                }
                let key = key.trim();
                key.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
                    .then(|| key.to_string())
            })
            .collect()
    }

    /// A tool the person can watch running must be described in words.
    ///
    /// Reintroducing the original bug — removing `scan_all_modules` from the
    /// dictionary — makes this fail and name it.
    #[test]
    fn every_recorded_tool_has_a_plain_english_name() {
        let root = repo_root();
        let service = std::fs::read_to_string(root.join("core/diagnostics/src/service.rs"))
            .expect("the diagnostic service source");
        let dictionary = std::fs::read_to_string(root.join("apps/desktop/src/hooks/useAgentProgress.ts"))
            .expect("the progress dictionary source");

        let recorded = recorded_tool_names(&service);
        let described = dictionary_keys(&dictionary);

        assert!(
            recorded.len() > 8,
            "only found {} recorded tool names, so the extraction has stopped working and this \
             test is no longer checking anything: {recorded:?}",
            recorded.len()
        );

        // `tool` is the generic fallback the recorder uses for a call it has no
        // specific name for, not something a person ever sees named.
        let missing: Vec<&String> =
            recorded.iter().filter(|n| *n != "tool" && !described.contains(*n)).collect();

        assert!(
            missing.is_empty(),
            "these tools can appear in the progress panel with no plain-English name, so they \
             render as raw identifiers while somebody watches their vehicle being inspected: \
             {missing:?}"
        );
    }
}
