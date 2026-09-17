//! A scorecard represents the state of knowledge about a vehicle.
//! It captures what identity information is known, how many modules were found,
//! what findings were established, and whether the vehicle was held during build.
//! Two scorecards can be compared to see how knowledge has evolved.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A complete scorecard for a vehicle
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scorecard {
    /// The vehicle identification number
    pub vin: String,
    /// Identity information about the vehicle
    pub identity: IdentityScore,
    /// Module discovery statistics
    pub modules: ModuleScore,
    /// Finding statistics
    pub findings: FindingScore,
    /// As-built information
    pub as_built: AsBuiltScore,
}

/// Identity information about a vehicle
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityScore {
    /// Fields that have been definitively identified
    pub settled: Vec<String>,
    /// Fields that are contested or uncertain
    pub contested: Vec<String>,
    /// Fields that remain unresolved
    pub unresolved: Vec<String>,
}

/// Module discovery statistics
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleScore {
    /// Total number of modules found
    pub found: u32,
    /// Number of modules with a reported name
    pub named: u32,
    /// Number of modules still labeled by address
    pub placeholder: u32,
    /// Count of modules by protocol
    pub by_protocol: BTreeMap<String, u32>,
}

/// Finding statistics
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingScore {
    /// Number of findings that have been established
    pub established: u32,
    /// Number of findings that have been ruled out
    pub ruled_out: u32,
    /// Number of findings that have been observed
    pub observed: u32,
}

/// As-built information
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsBuiltScore {
    /// Whether the vehicle was held during build
    pub held: bool,
}

/// Difference between two sections of a scorecard
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionDiff {
    /// Items that were gained (added or improved)
    pub gained: Vec<String>,
    /// Items that were lost (removed or degraded)
    pub lost: Vec<String>,
    /// Items that remained unchanged
    pub unchanged: Vec<String>,
}

/// Difference between two scorecards
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScorecardDiff {
    /// Difference in identity information
    pub identity: SectionDiff,
    /// Difference in module discovery
    pub modules: SectionDiff,
    /// Difference in findings
    pub findings: SectionDiff,
    /// Difference in as-built information
    pub as_built: SectionDiff,
}

impl Scorecard {
    /// Calculate the difference between this scorecard and another.
    /// This represents how knowledge has evolved from self to after.
    pub fn diff(&self, after: &Scorecard) -> ScorecardDiff {
        let identity = Self::diff_identity(&self.identity, &after.identity);
        let modules = Self::diff_modules(&self.modules, &after.modules);
        let findings = Self::diff_findings(&self.findings, &after.findings);
        let as_built = Self::diff_as_built(&self.as_built, &after.as_built);

        ScorecardDiff { identity, modules, findings, as_built }
    }

    fn diff_identity(before: &IdentityScore, after: &IdentityScore) -> SectionDiff {
        let mut gained = Vec::new();
        let mut lost = Vec::new();
        let mut unchanged = Vec::new();

        // Collect all field names from both scorecards
        let mut all_fields = std::collections::HashSet::new();
        all_fields.extend(before.settled.iter());
        all_fields.extend(before.contested.iter());
        all_fields.extend(before.unresolved.iter());
        all_fields.extend(after.settled.iter());
        all_fields.extend(after.contested.iter());
        all_fields.extend(after.unresolved.iter());

        for field in all_fields {
            let before_state = Self::get_field_state(before, field);
            let after_state = Self::get_field_state(after, field);

            if before_state != after_state {
                let change = format!("{}: {} -> {}", field, before_state, after_state);
                if Self::state_rank(after_state) > Self::state_rank(before_state) {
                    gained.push(change);
                } else {
                    lost.push(change);
                }
            } else {
                unchanged.push(format!("{}: {}", field, before_state));
            }
        }

        // Sort all lists alphabetically
        gained.sort();
        lost.sort();
        unchanged.sort();

        SectionDiff { gained, lost, unchanged }
    }

    fn state_rank(state: &str) -> u8 {
        match state {
            "settled" => 2,
            "contested" => 1,
            _ => 0,
        }
    }

    fn get_field_state(score: &IdentityScore, field: &str) -> &'static str {
        if score.settled.contains(&field.to_string()) {
            "settled"
        } else if score.contested.contains(&field.to_string()) {
            "contested"
        } else {
            "unresolved"
        }
    }

    fn diff_modules(before: &ModuleScore, after: &ModuleScore) -> SectionDiff {
        let mut gained = Vec::new();
        let mut lost = Vec::new();
        let mut unchanged = Vec::new();

        // Compare numeric metrics
        let metrics = [
            ("found", before.found, after.found),
            ("named", before.named, after.named),
            ("placeholder", before.placeholder, after.placeholder),
        ];

        for (name, before_val, after_val) in metrics {
            if before_val != after_val {
                let change = format!("{}: {} -> {}", name, before_val, after_val);
                if (name == "placeholder" && before_val > after_val)
                    || (name != "placeholder" && before_val < after_val)
                {
                    gained.push(change);
                } else {
                    lost.push(change);
                }
            } else {
                unchanged.push(format!("{}: {}", name, before_val));
            }
        }

        // Compare protocol metrics
        let mut all_protocols = std::collections::HashSet::new();
        all_protocols.extend(before.by_protocol.keys());
        all_protocols.extend(after.by_protocol.keys());

        for protocol in all_protocols {
            let before_val = *before.by_protocol.get(protocol).unwrap_or(&0);
            let after_val = *after.by_protocol.get(protocol).unwrap_or(&0);

            // Skip if both values are zero
            if before_val == 0 && after_val == 0 {
                continue;
            }

            if before_val != after_val {
                let change = format!("protocol {}: {} -> {}", protocol, before_val, after_val);
                if before_val < after_val {
                    gained.push(change);
                } else {
                    lost.push(change);
                }
            } else {
                unchanged.push(format!("protocol {}: {}", protocol, before_val));
            }
        }

        // Sort all lists alphabetically
        gained.sort();
        lost.sort();
        unchanged.sort();

        SectionDiff { gained, lost, unchanged }
    }

    fn diff_findings(before: &FindingScore, after: &FindingScore) -> SectionDiff {
        let mut gained = Vec::new();
        let mut lost = Vec::new();
        let mut unchanged = Vec::new();

        let metrics = [
            ("established", before.established, after.established),
            ("ruled_out", before.ruled_out, after.ruled_out),
            ("observed", before.observed, after.observed),
        ];

        for (name, before_val, after_val) in metrics {
            if before_val != after_val {
                let change = format!("{}: {} -> {}", name, before_val, after_val);
                if before_val < after_val {
                    gained.push(change);
                } else {
                    lost.push(change);
                }
            } else {
                unchanged.push(format!("{}: {}", name, before_val));
            }
        }

        // Sort all lists alphabetically
        gained.sort();
        lost.sort();
        unchanged.sort();

        SectionDiff { gained, lost, unchanged }
    }

    fn diff_as_built(before: &AsBuiltScore, after: &AsBuiltScore) -> SectionDiff {
        let mut gained = Vec::new();
        let mut lost = Vec::new();
        let mut unchanged = Vec::new();

        if before.held && !after.held {
            lost.push("held".to_string());
        } else if !before.held && after.held {
            gained.push("held".to_string());
        } else {
            let state = if before.held { "yes" } else { "no" };
            unchanged.push(format!("held: {}", state));
        }

        SectionDiff { gained, lost, unchanged }
    }
}

/// Compute a scorecard for a VIN from the session database.
///
/// Returns an empty scorecard for unknown VINs, not an error.
pub fn scorecard(
    store: &aim_session::SessionStore,
    vin: &str,
) -> Result<Scorecard, aim_types::AimError> {
    let vin = vin.trim().to_uppercase();

    let vehicle = store.vehicle_by_vin(&vin)?;
    let modules = match &vehicle {
        Some(v) => store.modules_for_vehicle(&v.id)?,
        None => Vec::new(),
    };

    // Build identity
    let mut identity_score = crate::identity::VehicleIdentity::assemble(vehicle.as_ref(), &modules);
    crate::lookup::merge_cached_vpic(&mut identity_score, store);
    let mut settled = Vec::new();
    let mut contested = Vec::new();
    let mut unresolved = Vec::new();

    for field in &identity_score.fields {
        if field.is_contested() {
            contested.push(field.field.clone());
        } else if field.settled().is_some() {
            settled.push(field.field.clone());
        } else {
            unresolved.push(field.field.clone());
        }
    }

    // Add unresolved fields from the identity
    for field in &identity_score.unresolved {
        if !settled.contains(field) && !contested.contains(field) {
            unresolved.push(field.clone());
        }
    }

    // Remove duplicates and sort
    settled.sort();
    settled.dedup();
    contested.sort();
    contested.dedup();
    unresolved.sort();
    unresolved.dedup();

    // Build module stats
    let found = modules.len() as u32;
    let mut placeholder_count = 0;
    let mut named_count = 0;
    let mut by_protocol = BTreeMap::new();

    for module in &modules {
        if module.name.starts_with("Module at ") || module.name.starts_with("OBD module at ") {
            placeholder_count += 1;
        } else {
            named_count += 1;
        }

        let protocol_label = module.protocol.label().to_string();
        *by_protocol.entry(protocol_label).or_insert(0) += 1;
    }

    // Build findings
    let findings = store.knowledge(&vin)?;
    let mut established = 0;
    let mut ruled_out = 0;
    let mut observed = 0;

    for finding in findings {
        match finding.outcome {
            aim_session::FindingOutcome::Established => established += 1,
            aim_session::FindingOutcome::RuledOut => ruled_out += 1,
            aim_session::FindingOutcome::Observed => observed += 1,
        }
    }

    // Build as_built
    let held = store.as_built(&vin)?.is_some();

    Ok(Scorecard {
        vin,
        identity: IdentityScore { settled, contested, unresolved },
        modules: ModuleScore {
            found,
            named: named_count,
            placeholder: placeholder_count,
            by_protocol,
        },
        findings: FindingScore { established, ruled_out, observed },
        as_built: AsBuiltScore { held },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_gained() {
        let before = IdentityScore {
            settled: vec!["make".to_string()],
            contested: vec!["year".to_string()],
            unresolved: vec!["model".to_string()],
        };
        let after = IdentityScore {
            settled: vec!["make".to_string(), "model".to_string(), "year".to_string()],
            contested: vec![],
            unresolved: vec!["fuel".to_string()],
        };

        let diff = Scorecard::diff_identity(&before, &after);
        assert_eq!(diff.gained, vec!["model: unresolved -> settled", "year: contested -> settled"]);
        assert_eq!(diff.lost, Vec::<String>::new());
        assert_eq!(diff.unchanged, vec!["fuel: unresolved", "make: settled"]);
    }

    #[test]
    fn test_identity_lost() {
        let before = IdentityScore {
            settled: vec!["engine".to_string()],
            contested: vec![],
            unresolved: vec![],
        };
        let after = IdentityScore {
            settled: vec![],
            contested: vec!["engine".to_string()],
            unresolved: vec![],
        };

        let diff = Scorecard::diff_identity(&before, &after);
        assert_eq!(diff.lost, vec!["engine: settled -> contested"]);
        assert_eq!(diff.gained, Vec::<String>::new());
        assert_eq!(diff.unchanged, Vec::<String>::new());
    }

    #[test]
    fn test_modules_gained() {
        let before = ModuleScore {
            found: 4,
            named: 1,
            placeholder: 3,
            by_protocol: BTreeMap::from([("CAN 11-bit 500k".to_string(), 4)]),
        };
        let after = ModuleScore {
            found: 6,
            named: 4,
            placeholder: 2,
            by_protocol: BTreeMap::from([
                ("CAN 11-bit 500k".to_string(), 5),
                ("CAN 29-bit 500k".to_string(), 1),
            ]),
        };

        let diff = Scorecard::diff_modules(&before, &after);
        assert_eq!(
            diff.gained,
            vec![
                "found: 4 -> 6",
                "named: 1 -> 4",
                "placeholder: 3 -> 2",
                "protocol CAN 11-bit 500k: 4 -> 5",
                "protocol CAN 29-bit 500k: 0 -> 1"
            ]
        );
        assert_eq!(diff.lost, Vec::<String>::new());
        assert_eq!(diff.unchanged, Vec::<String>::new());
    }

    #[test]
    fn test_modules_placeholder_up() {
        let before =
            ModuleScore { found: 0, named: 0, placeholder: 1, by_protocol: BTreeMap::new() };
        let after =
            ModuleScore { found: 0, named: 0, placeholder: 2, by_protocol: BTreeMap::new() };

        let diff = Scorecard::diff_modules(&before, &after);
        assert_eq!(diff.lost, vec!["placeholder: 1 -> 2"]);
        assert_eq!(diff.gained, Vec::<String>::new());
        assert_eq!(diff.unchanged, vec!["found: 0", "named: 0"]);
    }

    #[test]
    fn test_findings_gained() {
        let before = FindingScore { established: 2, ruled_out: 1, observed: 5 };
        let after = FindingScore { established: 3, ruled_out: 1, observed: 4 };

        let diff = Scorecard::diff_findings(&before, &after);
        assert_eq!(diff.gained, vec!["established: 2 -> 3"]);
        assert_eq!(diff.lost, vec!["observed: 5 -> 4"]);
        assert_eq!(diff.unchanged, vec!["ruled_out: 1"]);
    }

    #[test]
    fn test_as_built() {
        let before = AsBuiltScore { held: false };
        let after = AsBuiltScore { held: true };

        let diff = Scorecard::diff_as_built(&before, &after);
        assert_eq!(diff.gained, vec!["held"]);
        assert_eq!(diff.lost, Vec::<String>::new());
        assert_eq!(diff.unchanged, Vec::<String>::new());
    }

    #[test]
    fn test_as_built_unchanged() {
        let before = AsBuiltScore { held: true };
        let after = AsBuiltScore { held: true };

        let diff = Scorecard::diff_as_built(&before, &after);
        assert_eq!(diff.gained, Vec::<String>::new());
        assert_eq!(diff.lost, Vec::<String>::new());
        assert_eq!(diff.unchanged, vec!["held: yes"]);
    }

    #[test]
    fn test_as_built_lost() {
        let before = Scorecard { as_built: AsBuiltScore { held: true }, ..Default::default() };
        let after = Scorecard { as_built: AsBuiltScore { held: false }, ..Default::default() };
        let diff = before.diff(&after);
        assert_eq!(diff.as_built.lost, vec!["held"]);
        assert!(diff.as_built.gained.is_empty());
        assert!(diff.as_built.unchanged.is_empty());
    }

    #[test]
    fn test_empty_diff() {
        let before = Scorecard::default();
        let after = Scorecard::default();

        let diff = before.diff(&after);
        assert_eq!(diff.identity.gained, Vec::<String>::new());
        assert_eq!(diff.identity.lost, Vec::<String>::new());
        assert_eq!(diff.identity.unchanged, Vec::<String>::new());
        assert_eq!(diff.modules.gained, Vec::<String>::new());
        assert_eq!(diff.modules.lost, Vec::<String>::new());
        assert_eq!(diff.modules.unchanged, vec!["found: 0", "named: 0", "placeholder: 0"]);
        assert_eq!(diff.findings.gained, Vec::<String>::new());
        assert_eq!(diff.findings.lost, Vec::<String>::new());
        assert_eq!(diff.findings.unchanged, vec!["established: 0", "observed: 0", "ruled_out: 0"]);
        assert_eq!(diff.as_built.gained, Vec::<String>::new());
        assert_eq!(diff.as_built.lost, Vec::<String>::new());
        assert_eq!(diff.as_built.unchanged, vec!["held: no"]);
    }

    #[test]
    fn test_serde() {
        let scorecard = Scorecard::default();
        let json = serde_json::to_string(&scorecard).unwrap();
        let deserialized: Scorecard = serde_json::from_str(&json).unwrap();
        assert_eq!(scorecard, deserialized);
    }

    #[test]
    fn scorecard_of_an_unknown_vin_is_empty() {
        let store = aim_session::SessionStore::open_in_memory().unwrap();
        let result = scorecard(&store, " 1ft7w2bt6kec00001");
        assert!(result.is_ok());

        let scorecard = result.unwrap();
        assert_eq!(scorecard.vin, "1FT7W2BT6KEC00001");
        assert_eq!(scorecard.identity.settled, Vec::<String>::new());
        assert_eq!(scorecard.identity.contested, Vec::<String>::new());
        assert_eq!(scorecard.modules.found, 0);
        assert_eq!(scorecard.modules.named, 0);
        assert_eq!(scorecard.modules.placeholder, 0);
        assert_eq!(scorecard.findings.established, 0);
        assert_eq!(scorecard.findings.ruled_out, 0);
        assert_eq!(scorecard.findings.observed, 0);
        assert!(!scorecard.as_built.held);
    }

    #[test]
    fn scorecard_counts_named_and_placeholder_modules() {
        let store = aim_session::SessionStore::open_in_memory().unwrap();

        // Create vehicle
        let vehicle = aim_types::Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()));
        let vehicle = store.upsert_vehicle(&vehicle).unwrap();

        // Create session
        let session = store.create_session(None).unwrap();
        store.attach_vehicle(&session.id, &vehicle.id).unwrap();

        // Create modules
        let module1 = aim_types::Module {
            id: aim_types::ModuleId::new(),
            session_id: session.id.clone(),
            module_key: "key1".to_string(),
            name: "ENGINE CONTROL".to_string(),
            address: "7E0".to_string(),
            request_address: None,
            protocol: aim_types::ObdProtocol::Iso15765Can11_500,
            identity: aim_types::ModuleIdentity::default(),
            software_version: None,
            discovered_at: aim_types::now(),
        };

        let module2 = aim_types::Module {
            id: aim_types::ModuleId::new(),
            session_id: session.id,
            module_key: "key2".to_string(),
            name: "Module at 760".to_string(),
            address: "760".to_string(),
            request_address: None,
            protocol: aim_types::ObdProtocol::Iso15765Can11_500,
            identity: aim_types::ModuleIdentity::default(),
            software_version: None,
            discovered_at: aim_types::now(),
        };

        store.upsert_module(&module1).unwrap();
        store.upsert_module(&module2).unwrap();

        let result = scorecard(&store, "1FT7W2BT6KEC00001");
        assert!(result.is_ok());

        let scorecard = result.unwrap();
        assert_eq!(scorecard.modules.found, 2);
        assert_eq!(scorecard.modules.named, 1);
        assert_eq!(scorecard.modules.placeholder, 1);
        assert_eq!(scorecard.modules.by_protocol.len(), 1);
        assert_eq!(
            *scorecard
                .modules
                .by_protocol
                .get(aim_types::ObdProtocol::Iso15765Can11_500.label())
                .unwrap(),
            2
        );
    }

    #[test]
    fn scorecard_counts_findings_and_as_built() {
        let store = aim_session::SessionStore::open_in_memory().unwrap();

        // Create vehicle
        let vehicle = aim_types::Vehicle::from_vin(Some("1FT7W2BT6KEC00001".into()));
        let _vehicle = store.upsert_vehicle(&vehicle).unwrap();

        // Record findings
        let finding1 = aim_session::Finding {
            subject: "a".to_string(),
            outcome: aim_session::FindingOutcome::Established,
            claim: "".to_string(),
            evidence: "".to_string(),
            authority: "".to_string(),
            observed_at: aim_types::now().to_string(),
            session_id: None,
        };

        let finding2 = aim_session::Finding {
            subject: "b".to_string(),
            outcome: aim_session::FindingOutcome::Established,
            claim: "".to_string(),
            evidence: "".to_string(),
            authority: "".to_string(),
            observed_at: aim_types::now().to_string(),
            session_id: None,
        };

        let finding3 = aim_session::Finding {
            subject: "c".to_string(),
            outcome: aim_session::FindingOutcome::RuledOut,
            claim: "".to_string(),
            evidence: "".to_string(),
            authority: "".to_string(),
            observed_at: aim_types::now().to_string(),
            session_id: None,
        };

        store.record_finding("1FT7W2BT6KEC00001", &finding1).unwrap();
        store.record_finding("1FT7W2BT6KEC00001", &finding2).unwrap();
        store.record_finding("1FT7W2BT6KEC00001", &finding3).unwrap();

        // Store as-built
        store.store_as_built("1FT7W2BT6KEC00001", None, "{}").unwrap();

        let result = scorecard(&store, "1FT7W2BT6KEC00001");
        assert!(result.is_ok());

        let scorecard = result.unwrap();
        assert_eq!(scorecard.findings.established, 2);
        assert_eq!(scorecard.findings.ruled_out, 1);
        assert_eq!(scorecard.findings.observed, 0);
        assert!(scorecard.as_built.held);
    }
}
