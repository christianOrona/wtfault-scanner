//! What an approval is about, in words both ends build the same way.
//!
//! A person approves one operation on one thing. Whatever asks them names that
//! thing when it asks, and the service names it again from the arguments it
//! was actually called with; the gate lets the operation through only when the
//! two are the same string. These functions are how both get that string, so
//! an approval to turn one setting on cannot be spent turning another off.

use crate::config::DesiredValue;

/// Changing one setting to one value. Covers asking that setting's module
/// whether it accepts writes, which is the first half of the same change.
pub fn change(feature_id: &str, desired: DesiredValue) -> String {
    let value = match desired {
        DesiredValue::On => "on",
        DesiredValue::Off => "off",
    };
    format!("feature:{feature_id}={value}")
}

/// Clearing one module's codes, or every module's when none is named.
pub fn clear(module_key: Option<&str>) -> String {
    match module_key {
        Some(key) => format!("clear:{key}"),
        None => String::from("clear:*"),
    }
}

/// Asking one module whether it accepts writes.
pub fn probe(module_key: &str) -> String {
    format!("module:{module_key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_things_never_share_a_subject() {
        let subjects = [
            change("a", DesiredValue::On),
            change("a", DesiredValue::Off),
            change("b", DesiredValue::On),
            clear(None),
            clear(Some("ECU_7E8")),
            clear(Some("a")),
            probe("ECU_7E8"),
            probe("a"),
        ];
        let distinct: std::collections::BTreeSet<&String> = subjects.iter().collect();
        assert_eq!(distinct.len(), subjects.len());
    }
}
