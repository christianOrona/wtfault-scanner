//! `aim-decoders` — raw bytes to named values, driven by data files.
//!
//! Handoff §5 puts the semantic decoder at the top of the protocol stack and
//! §6 requires vehicle knowledge to be versioned data rather than code. Both
//! are enforced here:
//!
//! * PID scaling comes from YAML under `vehicle-profiles/`, evaluated by a real
//!   expression parser ([`expr::Formula`]) — there is no per-PID `match` arm.
//! * Every definition declares a [`aim_types::VerificationStatus`], and an
//!   unverified definition produces values that report themselves as
//!   untrustworthy all the way up to the tool-result envelope.
//! * DTC descriptions are looked up, never generated. An unknown code keeps its
//!   structural decoding and gets no description at all.

#![warn(missing_docs)]

pub mod asbuilt;
pub mod catalog;
pub mod dtc;
pub mod explain;
pub mod expr;
pub mod features;
pub mod knowledge;
pub mod monitors;
pub mod pids;
pub mod profiles;
pub mod regions;
pub mod signalset;
pub mod vin;

pub use asbuilt::{AsBuiltData, Block as AsBuiltBlock};
pub use dtc::{DtcCatalog, DtcInfo, DtcSystem};
pub use explain::{ExplainKind, Explanation, ExplanationCatalog};
pub use expr::Formula;
pub use features::{
    Applicability, DataIdentifierTarget, FeatureCatalog, FeatureDef, FeatureSupport, Mapping,
    OperationEvidence,
};
pub use knowledge::{
    Authority, ConfigurationAnswer, Knowledge, KnowledgeBase, KnowledgeProvider, Licence,
    MappingAnswer, ProviderInfo, SignalAnswer, VehicleContext,
};
pub use monitors::{MonitorCatalog, MonitorReading, RawCounts};
pub use pids::{PidDefinition, PidKind, PidRegistry};
pub use profiles::{ProfileKind, ProfileLoad, ProfileReport};
pub use regions::{region_for_dtc, Region};
pub use vin::{decode as decode_vin_info, VinInfo};

/// The decoder set the diagnostic core runs with: generic SAE PIDs plus the
/// generic SAE DTC table. Both are embedded in the binary.
#[derive(Debug, Clone)]
pub struct DecoderSet {
    /// PID definitions.
    pub pids: PidRegistry,
    /// DTC descriptions.
    pub dtcs: DtcCatalog,
    /// Plain-language explanations for signals, codes and concepts.
    pub explanations: ExplanationCatalog,
    /// Configurable vehicle features.
    pub features: FeatureCatalog,
    /// What user-supplied profile files contributed at startup.
    pub profiles: ProfileReport,
    /// Service 06 on-board monitor names and scalings.
    pub monitors: MonitorCatalog,
    /// Community signal definitions, by vehicle. Claims, never measurements.
    pub catalog: catalog::SignalCatalog,
}

impl DecoderSet {
    /// Load the generic OBD-II decoder set.
    pub fn generic_obd() -> aim_types::AimResult<DecoderSet> {
        Ok(DecoderSet {
            pids: PidRegistry::generic_obd()?,
            dtcs: DtcCatalog::generic_sae()?,
            monitors: MonitorCatalog::generic_obd()?,
            explanations: ExplanationCatalog::generic_obd()?,
            features: FeatureCatalog::embedded()?,
            profiles: ProfileReport::default(),
            catalog: catalog::SignalCatalog::embedded(),
        })
    }

    /// Load the generic set, then merge any user profiles found in `dir`.
    ///
    /// The directory is created with its README if it is missing, so the
    /// mechanism is discoverable from the filesystem rather than only from the
    /// documentation. Files that fail to load are recorded in
    /// [`DecoderSet::profiles`] and surfaced in the UI; they never prevent
    /// startup, because a stray comma in an optional data file must not stop a
    /// person diagnosing their vehicle.
    pub fn with_profiles(dir: &std::path::Path) -> aim_types::AimResult<DecoderSet> {
        let mut set = DecoderSet::generic_obd()?;

        // Community signalsets live beside the user's own profiles, in their
        // own directory because they are under a different licence. A missing
        // directory is a normal state: this build simply has nothing extra to
        // offer for a vehicle, which is the situation for most vehicles.
        let (community, warnings) = catalog::SignalCatalog::load_dir(dir.join("catalog/obdb"));
        for w in &warnings {
            tracing::warn!(detail = %w, "a community signalset could not be loaded");
        }
        if !community.is_empty() {
            tracing::info!(vehicles = community.len(), "loaded local community signal definitions");
        }
        // Local files win over the shipped copies.
        set.catalog.merge(community);

        profiles::ensure_directory(dir);
        set.profiles = profiles::load_directory(
            dir,
            profiles::Extensible {
                features: &mut set.features,
                pids: &mut set.pids,
                explanations: &mut set.explanations,
            },
        );
        for f in set.profiles.failures() {
            tracing::warn!(
                file = %f.path,
                error = %f.error.as_deref().unwrap_or(""),
                "a vehicle profile file could not be loaded"
            );
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generic_decoder_set_loads_from_embedded_data() {
        let set = DecoderSet::generic_obd().unwrap();
        assert!(set.pids.len() > 40);
        assert!(set.dtcs.len() > 50);
        assert!(set.monitors.known_monitors() > 30);
    }
}
