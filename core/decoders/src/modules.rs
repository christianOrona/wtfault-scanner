//! What kind of module a manufacturer part number names.
//!
//! Most modules on a Ford never answer the standard name identifier (F197), so
//! a module found by address stays "Module at 72E". They do report a part
//! number, and the middle of a Ford part number says what the part is. This
//! turns that into a name, only for the bases listed in
//! `vehicle-profiles/ford/modules.yaml`, and leaves every other module named
//! by its address.

use aim_types::{AimError, AimResult, ErrorCode};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
struct ModuleDef {
    base: String,
    name: String,
    #[allow(dead_code)]
    measured: String,
}

#[derive(Debug, Deserialize)]
struct ModuleFile {
    modules: Vec<ModuleDef>,
}

/// Module names by part-number base.
#[derive(Debug, Clone, Default)]
pub struct ModuleNames {
    by_base: BTreeMap<String, String>,
}

impl ModuleNames {
    /// The Ford table embedded in the binary.
    pub fn ford() -> AimResult<ModuleNames> {
        const SRC: &str = include_str!("../../../vehicle-profiles/ford/modules.yaml");
        let file: ModuleFile = serde_yaml_ng::from_str(SRC).map_err(|e| {
            AimError::new(ErrorCode::DecoderInputInvalid, format!("modules.yaml is not valid: {e}"))
        })?;
        Ok(ModuleNames { by_base: file.modules.into_iter().map(|m| (m.base, m.name)).collect() })
    }

    /// The module type a part number names, when its base is listed.
    ///
    /// `HC3A-12A650-JC` names a powertrain control module. Anything that is
    /// not three hyphen-separated parts, or whose base is not listed, names
    /// nothing.
    pub fn name_for_part(&self, part_number: &str) -> Option<&str> {
        let base = ford_base(part_number)?;
        self.by_base.get(base).map(String::as_str)
    }
}

/// The base of a Ford part number: `HC3A-12A650-JC` gives `12A650`.
pub fn ford_base(part_number: &str) -> Option<&str> {
    let mut parts = part_number.trim().split('-');
    let (prefix, base, suffix) = (parts.next()?, parts.next()?, parts.next()?);
    let alnum = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    (parts.next().is_none() && prefix.len() == 4 && alnum(prefix) && alnum(base) && alnum(suffix))
        .then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_base_names_the_module() {
        let names = ModuleNames::ford().unwrap();
        // Part numbers read from a 2019 F-250 on 2026-09-28 (identifier F113).
        assert_eq!(names.name_for_part("HC3A-12A650-JC"), Some("Powertrain control module (PCM)"));
        assert_eq!(names.name_for_part("JU5T-14B476-AAR"), Some("Body control module (BCM)"));
        assert_eq!(names.name_for_part("KC3T-10849-PB"), Some("Instrument panel cluster (IPC)"));
        assert_eq!(names.name_for_part("KC3C-2C219-AA"), Some("Anti-lock brake module (ABS)"));
    }

    #[test]
    fn an_unlisted_or_malformed_part_names_nothing() {
        let names = ModuleNames::ford().unwrap();
        assert_eq!(names.name_for_part("HC3T-19H423-DU"), None);
        assert_eq!(names.name_for_part("12A650"), None);
        assert_eq!(names.name_for_part("HC3A-12A650"), None);
        assert_eq!(names.name_for_part("HC3A-12A650-JC-X"), None);
        assert_eq!(names.name_for_part("SIM-BCM-14B476"), None);
    }

    #[test]
    fn the_base_is_the_middle_part() {
        assert_eq!(ford_base("HC3A-12A650-JC"), Some("12A650"));
        assert_eq!(ford_base(" KC3T-10849-PB "), Some("10849"));
        assert_eq!(ford_base("no-dashes"), None);
    }
}
