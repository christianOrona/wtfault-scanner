//! Security gateways, as data.
//!
//! Some manufacturers put a module between the diagnostic port and the
//! vehicle's networks that lets an unauthenticated tool read but not change
//! anything. To a tool that does not know about it, every refused write looks
//! like the module's own policy and every silence like an absent module. This
//! names the boundary so it can be reported as one (#15), from
//! `vehicle-profiles/gateways/gateways.yaml`.
//!
//! Nothing here, or anywhere in this build, works around a gateway.

use aim_types::{AimError, AimResult, ErrorCode, VerificationStatus};
use serde::{Deserialize, Serialize};

/// An operation a gateway can stand in the way of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatedOperation {
    /// Writing a configuration value to a module.
    WriteConfiguration,
}

/// One manufacturer's gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gateway {
    /// Stable id.
    pub id: String,
    /// What the manufacturer calls it.
    pub name: String,
    /// Makes it is fitted to, compared whole-word against the identified make.
    pub makes: Vec<String>,
    /// The first model year it was fitted in.
    pub from_model_year: u16,
    /// What it stops an unauthenticated tool doing.
    pub blocks: Vec<GatedOperation>,
    /// What it still lets through.
    pub allows: String,
    /// Where access comes from, which is never this build.
    pub access: String,
    /// How this entry is known.
    pub verification: VerificationStatus,
    /// Where it came from.
    pub source: String,
}

impl Gateway {
    /// Whether it stops `operation`.
    pub fn blocks(&self, operation: GatedOperation) -> bool {
        self.blocks.contains(&operation)
    }

    /// The boundary, in the words a person reads.
    pub fn boundary(&self) -> String {
        format!(
            "Gateway access required. Status: unavailable. Reason: this vehicle is listed as \
             having a {} from model year {}, which blocks this operation for a tool that has \
             not authenticated with the manufacturer. It still allows: {} Access needs: {} \
             This listing is {}, not measured on this vehicle.",
            self.name,
            self.from_model_year,
            self.allows,
            self.access,
            match self.verification {
                VerificationStatus::Verified => "verified",
                _ => "from public documentation",
            }
        )
    }
}

#[derive(Debug, Deserialize)]
struct GatewayFile {
    gateways: Vec<Gateway>,
}

/// Every gateway this build knows of.
#[derive(Debug, Clone, Default)]
pub struct GatewayCatalog {
    gateways: Vec<Gateway>,
}

impl GatewayCatalog {
    /// The table embedded in the binary.
    pub fn shipped() -> AimResult<GatewayCatalog> {
        const SRC: &str = include_str!("../../../vehicle-profiles/gateways/gateways.yaml");
        let file: GatewayFile = serde_yaml_ng::from_str(SRC).map_err(|e| {
            AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!("gateways.yaml is not valid: {e}"),
            )
        })?;
        Ok(GatewayCatalog { gateways: file.gateways })
    }

    /// The gateway a vehicle of this make and model year is listed as having.
    ///
    /// `None` when the make matches nothing, and when the model year is known
    /// and earlier than fitting began. An unknown model year on a listed make
    /// counts as listed: the boundary is the safer thing to have said.
    pub fn for_vehicle(&self, make: &str, model_year: Option<u16>) -> Option<&Gateway> {
        self.gateways.iter().find(|g| {
            g.makes.iter().any(|m| crate::catalog::makes_match(m, make))
                && model_year.is_none_or(|y| y >= g.from_model_year)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_table_loads() {
        let catalog = GatewayCatalog::shipped().unwrap();
        assert!(!catalog.gateways.is_empty());
        for g in &catalog.gateways {
            assert!(!g.blocks.is_empty(), "{} blocks nothing", g.id);
            assert!(!g.access.is_empty() && !g.allows.is_empty(), "{}", g.id);
        }
    }

    #[test]
    fn a_2019_jeep_is_listed_and_a_2016_one_is_not() {
        let catalog = GatewayCatalog::shipped().unwrap();
        // How the VIN decoder names an FCA WMI, and how vPIC names a brand.
        let g = catalog.for_vehicle("Stellantis / FCA US", Some(2019)).expect("listed");
        assert!(g.blocks(GatedOperation::WriteConfiguration));
        assert!(catalog.for_vehicle("JEEP", Some(2021)).is_some());
        assert!(catalog.for_vehicle("JEEP", Some(2016)).is_none(), "before fitting began");
        assert!(catalog.for_vehicle("JEEP", None).is_some(), "unknown year: say the boundary");
    }

    #[test]
    fn a_ford_or_a_honda_is_not_listed() {
        let catalog = GatewayCatalog::shipped().unwrap();
        assert!(catalog.for_vehicle("Ford Motor Company (US, truck)", Some(2019)).is_none());
        assert!(catalog.for_vehicle("Honda (Japan)", Some(2023)).is_none());
    }

    #[test]
    fn the_boundary_says_unavailable_and_where_access_comes_from() {
        let catalog = GatewayCatalog::shipped().unwrap();
        let text = catalog.for_vehicle("Jeep", Some(2020)).unwrap().boundary();
        assert!(text.starts_with("Gateway access required. Status: unavailable."), "{text}");
        assert!(text.contains("AutoAuth"), "{text}");
        assert!(text.contains("not measured on this vehicle"), "{text}");
    }
}
