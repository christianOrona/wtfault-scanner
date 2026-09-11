//! One way to ask every source of vehicle knowledge the same question.
//!
//! # The problem this solves
//!
//! Three unrelated things in this crate answer overlapping questions. The
//! community signal catalogue knows what a vehicle can be asked to report. The
//! feature catalogue knows where a setting lives. As-built data knows how one
//! particular vehicle left the factory. Each grew its own vocabulary for the
//! same two ideas — *where did this come from* and *how much does it have to do
//! with the vehicle in front of me* — and a caller wanting both had to learn
//! all three.
//!
//! Worse, the caller had to decide which to believe. That decision was being
//! made ad hoc at each call site, which is exactly where it will eventually be
//! made wrong.
//!
//! # Ordered, never merged
//!
//! Every answer arrives ranked by [`Authority`] and nothing is ever combined.
//! This is the rule the whole module exists to enforce.
//!
//! Merging is how a disagreement disappears. If a community definition says a
//! setting lives in one bit and a measurement on this vehicle says it lives in
//! another, a merged answer shows one bit and no argument — and the argument
//! was the valuable part. Two answers, in order, with their sources named,
//! lets a person see that the catalogue and their truck disagree and decide
//! what to do about it. One answer takes that away from them.
//!
//! So [`rank`] sorts and never deduplicates, not even when two providers give
//! byte-identical answers. Agreement between independent sources is worth
//! seeing too.
//!
//! # Licence travels with the answer
//!
//! Not bureaucracy. The community catalogue is CC BY-SA 4.0 and requires
//! attribution wherever its content is shown; as-built data belongs to the
//! owner of one vehicle and must never end up inside a profile file shared with
//! anybody else. Those are constraints on what the *application* may do with an
//! answer, so they have to arrive attached to it rather than be looked up by a
//! caller who may not think to.

use crate::asbuilt::AsBuiltData;
use crate::catalog::{Relevance, SignalCatalog};
use crate::features::{FeatureCatalog, FeatureDef, MappingRelevance};
use crate::signalset::CommandDef;
use serde::{Deserialize, Serialize};

/// How much weight an answer carries, best first.
///
/// The ordering is the point and the declaration order is the ordering:
/// `Ord` puts [`Authority::MeasuredThisSession`] first, so sorting ascending
/// puts the most authoritative answer at the top.
///
/// The shape of the list is one idea repeated. **Measurement beats
/// description, and closeness to this vehicle beats everything else.** A number
/// read off the truck five minutes ago outranks the same number read last
/// week, which outranks the manufacturer's own record of how it was built,
/// which outranks a measurement from a similar truck, which outranks a
/// stranger's note about this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Read off this vehicle during this session.
    MeasuredThisSession,
    /// Read off this vehicle before, and recorded.
    MeasuredEarlier,
    /// Manufacturer data the owner supplied for this exact VIN.
    ///
    /// Below a live measurement on purpose. It records how the vehicle left
    /// the factory, and anything changed since — by a dealer, a previous
    /// owner, or this application — is not in it.
    OwnerSuppliedOemData,
    /// Measured on a different vehicle that this one closely resembles.
    MeasuredOnSimilarVehicle,
    /// A community definition recorded for this make and model.
    CommunityThisModel,
    /// A community definition recorded for a different model from the same
    /// manufacturer.
    CommunityRelatedModel,
    /// A published standard that applies to every vehicle, e.g. SAE J1979.
    GenericStandard,
    /// Something a language model believes about vehicles of this kind.
    ///
    /// Last, always, and nothing may be ranked above it that did not come from
    /// data. A model that has read a million forum posts about F-250s is still
    /// not evidence about *this* F-250, and the moment its recollection is
    /// allowed to outrank a measurement this project stops being worth
    /// trusting. See [`Authority::LOWEST`].
    ModelKnowledge,
}

impl Authority {
    /// The floor. Nothing ranks below model knowledge, and model knowledge
    /// never ranks above anything that came from data.
    pub const LOWEST: Authority = Authority::ModelKnowledge;

    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Authority::MeasuredThisSession => "measured_this_session",
            Authority::MeasuredEarlier => "measured_earlier",
            Authority::OwnerSuppliedOemData => "owner_supplied_oem_data",
            Authority::MeasuredOnSimilarVehicle => "measured_on_similar_vehicle",
            Authority::CommunityThisModel => "community_this_model",
            Authority::CommunityRelatedModel => "community_related_model",
            Authority::GenericStandard => "generic_standard",
            Authority::ModelKnowledge => "model_knowledge",
        }
    }

    /// What this level means, in language meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            Authority::MeasuredThisSession => "Read from your vehicle during this session.",
            Authority::MeasuredEarlier => {
                "Read from your vehicle before and recorded. Anything changed since is not \
                 reflected here."
            }
            Authority::OwnerSuppliedOemData => {
                "From the manufacturer's own record of how this exact vehicle was built. It is a \
                 snapshot of the factory configuration, so anything changed since — by a dealer, \
                 a previous owner, or this app — will not show in it."
            }
            Authority::MeasuredOnSimilarVehicle => {
                "Measured on a DIFFERENT vehicle that closely resembles yours. Configuration \
                 layouts usually match across such vehicles and sometimes do not, so this is \
                 worth reading and checking rather than relying on."
            }
            Authority::CommunityThisModel => {
                "A community definition recorded for this make and model. Nobody has checked it \
                 against your vehicle."
            }
            Authority::CommunityRelatedModel => {
                "A community definition recorded for a DIFFERENT model from the same \
                 manufacturer. Makers reuse identifiers across a range and also change them \
                 between models, so this is a guess worth testing."
            }
            Authority::GenericStandard => {
                "Defined by a published standard that applies to every vehicle sold here."
            }
            Authority::ModelKnowledge => {
                "What an AI model believes about vehicles like yours. It is not evidence about \
                 your vehicle and is ranked below everything that is."
            }
        }
    }

    /// Whether this answer came off a vehicle rather than out of a description.
    pub fn is_measurement(&self) -> bool {
        matches!(
            self,
            Authority::MeasuredThisSession
                | Authority::MeasuredEarlier
                | Authority::MeasuredOnSimilarVehicle
        )
    }

    /// Whether this answer is about *this* vehicle specifically.
    ///
    /// The distinction that decides whether something may be presented as a
    /// fact or must be presented as a hypothesis to test.
    pub fn is_about_this_vehicle(&self) -> bool {
        matches!(
            self,
            Authority::MeasuredThisSession
                | Authority::MeasuredEarlier
                | Authority::OwnerSuppliedOemData
        )
    }
}

/// The terms an answer's content is available under.
///
/// Carried because two of these place real obligations on the application:
/// attribution that must appear wherever the content does, and data belonging
/// to one person that must never leave their machine inside a shared file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Licence {
    /// Ships as part of this application, under the project's own terms.
    ProjectData,
    /// Creative Commons Attribution-ShareAlike 4.0. Attribution is required
    /// wherever the content appears, and derivative definitions must be shared
    /// under the same terms.
    CcBySa4_0,
    /// Supplied by the owner of one vehicle, about that vehicle.
    ///
    /// Not ours to publish. An as-built file contains the VIN it was issued
    /// for, so bundling one into a shared profile would hand out somebody's
    /// vehicle identity along with the mapping.
    OwnerSupplied,
    /// A published standard, used as a specification rather than copied.
    PublicStandard,
}

impl Licence {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Licence::ProjectData => "project_data",
            Licence::CcBySa4_0 => "cc_by_sa_4_0",
            Licence::OwnerSupplied => "owner_supplied",
            Licence::PublicStandard => "public_standard",
        }
    }

    /// Whether this application may include the content in something it shares.
    ///
    /// `false` does not mean the content cannot be *used* — it means it cannot
    /// be redistributed, which is the question an export or a profile-sharing
    /// feature has to ask.
    pub fn may_redistribute(&self) -> bool {
        !matches!(self, Licence::OwnerSupplied)
    }

    /// The credit that must appear wherever this content does, if any.
    pub fn attribution(&self) -> Option<&'static str> {
        match self {
            Licence::CcBySa4_0 => Some(
                "Community signal definitions from the OBDb project, CC BY-SA 4.0. See \
                 vehicle-profiles/catalog/obdb/ATTRIBUTION.md.",
            ),
            _ => None,
        }
    }
}

/// Who answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Stable identifier, e.g. `obdb`.
    pub id: &'static str,
    /// Human name.
    pub name: &'static str,
    /// Terms the content is available under.
    pub licence: Licence,
}

/// What is known about the vehicle being asked about.
///
/// Deliberately small and deliberately all-optional: every field here is
/// something this application frequently fails to establish, and a provider
/// must be able to say "then I have nothing for you" rather than guess.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VehicleContext {
    /// Manufacturer, as the VIN decoder reports it.
    pub make: Option<String>,
    /// Model, when something authoritative supplied one.
    pub model: Option<String>,
    /// Model year.
    pub year: Option<u16>,
    /// The VIN.
    pub vin: Option<String>,
}

impl VehicleContext {
    /// A context that knows nothing, which is the starting state of every
    /// session and a perfectly valid thing to ask with.
    pub fn unknown() -> VehicleContext {
        VehicleContext::default()
    }

    /// Whether anything is known at all.
    pub fn is_unknown(&self) -> bool {
        self == &VehicleContext::default()
    }
}

/// One answer, with everything needed to judge it.
#[derive(Debug, Clone, PartialEq)]
pub struct Knowledge<T> {
    /// The answer itself.
    pub answer: T,
    /// How much weight it carries.
    pub authority: Authority,
    /// Who gave it.
    pub provider: ProviderInfo,
    /// Which record inside that provider it came from, so a wrong answer can
    /// be traced to a file and a line rather than blamed on the application.
    pub source: String,
}

impl<T> Knowledge<T> {
    /// Assemble an answer.
    pub fn new(
        answer: T,
        authority: Authority,
        provider: ProviderInfo,
        source: impl Into<String>,
    ) -> Knowledge<T> {
        Knowledge { answer, authority, provider, source: source.into() }
    }

    /// Replace the answer, keeping everything that says where it came from.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Knowledge<U> {
        Knowledge {
            answer: f(self.answer),
            authority: self.authority,
            provider: self.provider,
            source: self.source,
        }
    }
}

/// Put answers in authority order, most authoritative first.
///
/// **Sorts and never deduplicates.** Two providers giving the same answer stay
/// as two answers, and two providers giving different answers stay as two
/// answers — which is the case this exists for. Collapsing either one would
/// hide the thing a person most needs to see.
///
/// The sort is stable, so answers at the same authority keep the order their
/// provider produced them in.
pub fn rank<T>(answers: &mut [Knowledge<T>]) {
    answers.sort_by_key(|k| k.authority);
}

/// A signal this vehicle might be able to report.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalAnswer<'a> {
    /// The command definition, borrowed from its catalogue.
    pub command: &'a CommandDef,
}

/// Where a configurable setting lives.
#[derive(Debug, Clone, PartialEq)]
pub struct MappingAnswer<'a> {
    /// The feature definition, borrowed from its catalogue.
    pub feature: &'a FeatureDef,
}

/// Recorded configuration bytes for one module.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigurationAnswer<'a> {
    /// Module address as written in the source, e.g. `726`.
    pub module: &'a str,
    /// Block number within that module.
    pub block: u16,
    /// The data identifier this block corresponds to, when there is one.
    pub data_identifier: Option<u16>,
    /// The bytes, checksums already removed.
    pub bytes: &'a [u8],
}

/// Something that knows things about vehicles.
///
/// Every method defaults to answering nothing, so a provider implements only
/// the questions it can actually answer. That is not a convenience — it is the
/// honest shape. The as-built parser knows how one vehicle was configured and
/// nothing whatever about what its signals mean, and a trait that forced it to
/// return something for [`KnowledgeProvider::signals`] would be inviting it to
/// make something up.
pub trait KnowledgeProvider {
    /// Who this is and what its content is licensed under.
    fn provider(&self) -> ProviderInfo;

    /// Signals this provider offers for the vehicle, unranked.
    ///
    /// Callers should pass the result through [`rank`] — or use
    /// [`KnowledgeBase`], which does.
    fn signals<'a>(&'a self, _vehicle: &VehicleContext) -> Vec<Knowledge<SignalAnswer<'a>>> {
        Vec::new()
    }

    /// Settings this provider claims to know the location of, unranked.
    fn mappings<'a>(&'a self, _vehicle: &VehicleContext) -> Vec<Knowledge<MappingAnswer<'a>>> {
        Vec::new()
    }

    /// Configuration records this provider holds for the vehicle, unranked.
    fn configuration<'a>(
        &'a self,
        _vehicle: &VehicleContext,
    ) -> Vec<Knowledge<ConfigurationAnswer<'a>>> {
        Vec::new()
    }
}

// -------------------------------------------------------------- the sources

/// The community signal catalogue as a provider.
pub const OBDB: ProviderInfo = ProviderInfo {
    id: "obdb",
    name: "OBDb community signal definitions",
    licence: Licence::CcBySa4_0,
};

impl KnowledgeProvider for SignalCatalog {
    fn provider(&self) -> ProviderInfo {
        OBDB
    }

    fn signals<'a>(&'a self, vehicle: &VehicleContext) -> Vec<Knowledge<SignalAnswer<'a>>> {
        self.candidates(vehicle.make.as_deref(), vehicle.model.as_deref(), vehicle.year)
            .into_iter()
            .map(|c| {
                let authority = match c.relevance {
                    Relevance::Exact => Authority::CommunityThisModel,
                    Relevance::RelatedModel => Authority::CommunityRelatedModel,
                };
                Knowledge::new(
                    SignalAnswer { command: c.command },
                    authority,
                    OBDB,
                    c.source_key.to_string(),
                )
            })
            .collect()
    }
}

/// The feature catalogue as a provider.
pub const FEATURES: ProviderInfo = ProviderInfo {
    id: "feature-catalog",
    name: "Configurable feature definitions",
    licence: Licence::ProjectData,
};

impl KnowledgeProvider for FeatureCatalog {
    fn provider(&self) -> ProviderInfo {
        FEATURES
    }

    fn mappings<'a>(&'a self, vehicle: &VehicleContext) -> Vec<Knowledge<MappingAnswer<'a>>> {
        let (make, year, vin) = (vehicle.make.as_deref(), vehicle.year, vehicle.vin.as_deref());
        self.all()
            .filter_map(|f| {
                let authority = feature_authority(f, make, year, vin)?;
                Some(Knowledge::new(
                    MappingAnswer { feature: f },
                    authority,
                    FEATURES,
                    f.source.clone().unwrap_or_else(|| String::from("built in")),
                ))
            })
            .collect()
    }
}

/// How much a feature definition has to do with the vehicle in front of us,
/// or `None` when it has nothing to do with it at all.
///
/// Two questions, and both have to be asked. *Is there a mapping at all* —
/// because a feature this build can only describe is a description regardless
/// of how narrowly it is scoped. And *was that mapping measured on this
/// vehicle* — because a mapping scoped to one exact VIN is a different claim
/// from one that matches every Ford ever built.
///
/// The trap avoided here is the easy one: [`Applicability::matches`] returns
/// true for an unscoped feature, since a feature that does not say it is
/// Ford-only should not be hidden from a Ford. Reading that as "measured on
/// this vehicle" would rank the entire generic catalogue above a community
/// definition recorded for this exact model, which is backwards.
///
/// The other trap is quieter: `matches` fails *closed* on an exact-VIN
/// mismatch, so a mapping measured on one truck and offered as a candidate to
/// a near-identical one would never be reached if this filtered on `matches`
/// alone. That candidate is the whole reason `candidate_vin_prefixes` exists,
/// so the two questions are asked in the other order — what is this, and then
/// does it apply.
fn feature_authority(
    f: &FeatureDef,
    make: Option<&str>,
    year: Option<u16>,
    vin: Option<&str>,
) -> Option<Authority> {
    let relevance = f.applies_to.relevance(make, year, vin);

    // Nothing to locate. The catalogue can say what this feature is and that
    // is the whole of its claim.
    if f.mapping.is_none() {
        return f.applies_to.matches(make, year, vin).then_some(Authority::GenericStandard);
    }
    // A mapping measured elsewhere and offered here on a VIN-prefix match.
    // Checked before `matches`, which fails closed on the VIN it was measured
    // on being a different one.
    if relevance == MappingRelevance::Candidate {
        return Some(Authority::MeasuredOnSimilarVehicle);
    }
    if !f.applies_to.matches(make, year, vin) {
        return None;
    }
    // A mapping nobody checked against a vehicle is somebody's claim about
    // where a setting lives — the same kind of thing a community definition is,
    // and ranked alongside one rather than above it.
    if f.verification != aim_types::VerificationStatus::Verified {
        return Some(Authority::CommunityThisModel);
    }
    // Measured on this exact VIN, recorded in a file. Authority 2 rather than
    // 1: it was written down, not read just now, and whether the setting still
    // holds that value is a question only a live read answers.
    if relevance == MappingRelevance::Measured && !f.applies_to.vins.is_empty() {
        return Some(Authority::MeasuredEarlier);
    }
    // Verified, but scoped no more narrowly than a make and a year range.
    // Somebody measured it on *a* vehicle; this may or may not be one of them,
    // and the scope is not fine enough to say which.
    Some(Authority::CommunityThisModel)
}

/// As-built data as a provider.
pub const AS_BUILT: ProviderInfo = ProviderInfo {
    id: "as-built",
    name: "Manufacturer as-built configuration",
    licence: Licence::OwnerSupplied,
};

impl KnowledgeProvider for AsBuiltData {
    fn provider(&self) -> ProviderInfo {
        AS_BUILT
    }

    /// Configuration for the vehicle this file was issued for — and only that
    /// vehicle.
    ///
    /// A VIN mismatch answers nothing. The file contains the VIN it belongs to,
    /// and its values are that vehicle's, so offering them for a different
    /// truck would be presenting one vehicle's configuration as another's. What
    /// generalises across vehicles is the *labelling* — which block and byte
    /// holds which feature — and that is a mapping, handled by the feature
    /// catalogue, not a value.
    ///
    /// An unidentified vehicle answers nothing for the same reason: there is
    /// nothing to check the file against, and "probably the right truck" is not
    /// a standard this project works to.
    fn configuration<'a>(
        &'a self,
        vehicle: &VehicleContext,
    ) -> Vec<Knowledge<ConfigurationAnswer<'a>>> {
        let (Some(file_vin), Some(vehicle_vin)) = (self.vin.as_deref(), vehicle.vin.as_deref())
        else {
            return Vec::new();
        };
        if !file_vin.eq_ignore_ascii_case(vehicle_vin) {
            return Vec::new();
        }

        let mut out = Vec::new();
        for (module, blocks) in &self.modules {
            for (number, block) in blocks {
                out.push(Knowledge::new(
                    ConfigurationAnswer {
                        module: module.as_str(),
                        block: *number,
                        data_identifier: AsBuiltData::did_for_block(*number),
                        bytes: &block.bytes,
                    },
                    Authority::OwnerSuppliedOemData,
                    AS_BUILT,
                    format!("as-built {file_vin} block {module}-{number:02}"),
                ));
            }
        }
        out
    }
}

// ------------------------------------------------------------ asking all of them

/// Every source, asked together and answered in order.
///
/// Holds borrowed providers rather than owning them, because the catalogues
/// live in [`crate::DecoderSet`] for the life of the process and copying them
/// per question would be absurd.
pub struct KnowledgeBase<'p> {
    providers: Vec<&'p dyn KnowledgeProvider>,
}

impl<'p> KnowledgeBase<'p> {
    /// An empty base.
    pub fn new() -> KnowledgeBase<'p> {
        KnowledgeBase { providers: Vec::new() }
    }

    /// Add a provider. Order of addition decides ties within one authority
    /// level, since [`rank`] is stable.
    pub fn with(mut self, provider: &'p dyn KnowledgeProvider) -> Self {
        self.providers.push(provider);
        self
    }

    /// Who is being asked.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.providers.iter().map(|p| p.provider()).collect()
    }

    /// Every attribution that must be shown alongside these answers.
    pub fn attributions(&self) -> Vec<&'static str> {
        let mut seen: Vec<&'static str> = Vec::new();
        for p in &self.providers {
            if let Some(a) = p.provider().licence.attribution() {
                if !seen.contains(&a) {
                    seen.push(a);
                }
            }
        }
        seen
    }

    /// Signals every provider offers, most authoritative first.
    pub fn signals(&self, vehicle: &VehicleContext) -> Vec<Knowledge<SignalAnswer<'p>>> {
        let mut out: Vec<_> = self.providers.iter().flat_map(|p| p.signals(vehicle)).collect();
        rank(&mut out);
        out
    }

    /// Settings every provider claims to locate, most authoritative first.
    pub fn mappings(&self, vehicle: &VehicleContext) -> Vec<Knowledge<MappingAnswer<'p>>> {
        let mut out: Vec<_> = self.providers.iter().flat_map(|p| p.mappings(vehicle)).collect();
        rank(&mut out);
        out
    }

    /// Configuration records every provider holds, most authoritative first.
    pub fn configuration(
        &self,
        vehicle: &VehicleContext,
    ) -> Vec<Knowledge<ConfigurationAnswer<'p>>> {
        let mut out: Vec<_> =
            self.providers.iter().flat_map(|p| p.configuration(vehicle)).collect();
        rank(&mut out);
        out
    }
}

impl Default for KnowledgeBase<'_> {
    fn default() -> Self {
        KnowledgeBase::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f250() -> VehicleContext {
        VehicleContext {
            make: Some(String::from("Ford Motor Company (US, truck)")),
            model: None,
            year: Some(2019),
            vin: Some(String::from("1FT7W2BT7KEF78036")),
        }
    }

    /// The ordering is the whole contract, so it is asserted directly rather
    /// than left to the declaration order being read correctly.
    #[test]
    fn authority_runs_from_measured_here_down_to_the_model() {
        let mut levels = vec![
            Authority::ModelKnowledge,
            Authority::CommunityRelatedModel,
            Authority::MeasuredThisSession,
            Authority::GenericStandard,
            Authority::OwnerSuppliedOemData,
            Authority::CommunityThisModel,
            Authority::MeasuredEarlier,
            Authority::MeasuredOnSimilarVehicle,
        ];
        levels.sort();
        assert_eq!(
            levels,
            vec![
                Authority::MeasuredThisSession,
                Authority::MeasuredEarlier,
                Authority::OwnerSuppliedOemData,
                Authority::MeasuredOnSimilarVehicle,
                Authority::CommunityThisModel,
                Authority::CommunityRelatedModel,
                Authority::GenericStandard,
                Authority::ModelKnowledge,
            ]
        );
    }

    /// What a model recalls never outranks anything that came from data. This
    /// is the one ordering the project cannot afford to get wrong.
    #[test]
    fn model_knowledge_is_the_floor() {
        for level in [
            Authority::MeasuredThisSession,
            Authority::MeasuredEarlier,
            Authority::OwnerSuppliedOemData,
            Authority::MeasuredOnSimilarVehicle,
            Authority::CommunityThisModel,
            Authority::CommunityRelatedModel,
            Authority::GenericStandard,
        ] {
            assert!(level < Authority::ModelKnowledge, "{level:?} should outrank model knowledge");
        }
        assert_eq!(Authority::LOWEST, Authority::ModelKnowledge);
    }

    /// The rule the module exists for. Two sources disagreeing produces two
    /// answers, in order — never one answer with the argument removed.
    #[test]
    fn disagreement_survives_ranking() {
        let mut answers = vec![
            Knowledge::new("bit 2", Authority::CommunityThisModel, OBDB, "Ford-F-150"),
            Knowledge::new("bit 4", Authority::MeasuredThisSession, FEATURES, "measured"),
        ];
        rank(&mut answers);
        assert_eq!(answers.len(), 2, "nothing may be merged away");
        assert_eq!(answers[0].answer, "bit 4", "the measurement leads");
        assert_eq!(answers[1].answer, "bit 2", "and the claim is still there to see");
    }

    /// Including when they agree. Two independent sources reaching the same
    /// answer is information, and collapsing it to one loses it.
    #[test]
    fn agreement_is_not_collapsed_either() {
        let mut answers = vec![
            Knowledge::new("bit 4", Authority::CommunityThisModel, OBDB, "Ford-F-150"),
            Knowledge::new("bit 4", Authority::MeasuredEarlier, FEATURES, "measured"),
        ];
        rank(&mut answers);
        assert_eq!(answers.len(), 2);
    }

    /// Same authority keeps the order the providers were asked in, so a
    /// deterministic list stays deterministic.
    #[test]
    fn ranking_is_stable_within_a_level() {
        let mut answers = vec![
            Knowledge::new("first", Authority::CommunityThisModel, OBDB, "a"),
            Knowledge::new("second", Authority::CommunityThisModel, OBDB, "b"),
            Knowledge::new("third", Authority::CommunityThisModel, OBDB, "c"),
        ];
        rank(&mut answers);
        assert_eq!(
            answers.iter().map(|k| k.answer).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
    }

    /// The community catalogue keeps its licence attached wherever its content
    /// goes, because CC BY-SA requires the credit to travel with the content.
    #[test]
    fn community_content_carries_its_attribution() {
        let catalog = SignalCatalog::embedded();
        let answers = catalog.signals(&f250());
        assert!(!answers.is_empty(), "the shipped F-150 set should answer for a Ford");
        for a in &answers {
            assert_eq!(a.provider.licence, Licence::CcBySa4_0);
            assert!(a.provider.licence.attribution().is_some());
        }
    }

    /// An F-250 asking the catalogue gets F-150 definitions, and gets told so.
    /// This is the behaviour `catalog::Relevance` already had, arriving through
    /// the common vocabulary unchanged.
    #[test]
    fn a_definition_from_another_model_is_ranked_as_one() {
        let catalog = SignalCatalog::embedded();
        let answers = catalog.signals(&f250());
        assert!(answers.iter().all(|a| a.authority == Authority::CommunityRelatedModel));
        assert!(answers.iter().all(|a| !a.authority.is_about_this_vehicle()));
    }

    /// Nothing known about the vehicle means nothing offered. Seven hundred
    /// catalogues and no way to choose is not a list worth showing.
    #[test]
    fn an_unidentified_vehicle_gets_nothing_from_the_catalogue() {
        let catalog = SignalCatalog::embedded();
        assert!(catalog.signals(&VehicleContext::unknown()).is_empty());
    }

    /// As-built data belongs to one vehicle. Offering it for another would be
    /// presenting that vehicle's configuration as this one's.
    #[test]
    fn as_built_answers_only_for_the_vin_it_was_issued_for() {
        let file = AsBuiltData::parse(
            "<VIN>1FT7W2BT7KEF78036</VIN>\
             <DATA LABEL=\"726-15-01\"><CODE>0101</CODE><CODE>0148</CODE></DATA>",
        )
        .unwrap();

        assert_eq!(file.configuration(&f250()).len(), 1, "its own vehicle");

        let other = VehicleContext { vin: Some(String::from("1FT7W2BT7KEF99999")), ..f250() };
        assert!(file.configuration(&other).is_empty(), "a different truck gets nothing");
        assert!(
            file.configuration(&VehicleContext::unknown()).is_empty(),
            "and an unidentified one gets nothing either"
        );
    }

    /// Block-to-identifier is the bridge that makes as-built executable, and it
    /// comes through on the answer rather than being rediscovered by callers.
    #[test]
    fn as_built_answers_carry_the_identifier_the_block_corresponds_to() {
        let file = AsBuiltData::parse(
            "<VIN>1FT7W2BT7KEF78036</VIN>\
             <DATA LABEL=\"726-15-01\"><CODE>0101</CODE><CODE>0148</CODE></DATA>",
        )
        .unwrap();
        let answers = file.configuration(&f250());
        assert_eq!(answers[0].answer.data_identifier, Some(0xDE0E));
        assert_eq!(answers[0].authority, Authority::OwnerSuppliedOemData);
    }

    /// Somebody's as-built file must never end up inside a profile shared with
    /// other people: it carries the VIN it was issued for.
    #[test]
    fn owner_supplied_data_is_never_redistributable() {
        assert!(!Licence::OwnerSupplied.may_redistribute());
        assert!(Licence::CcBySa4_0.may_redistribute());
        assert!(Licence::ProjectData.may_redistribute());
    }

    /// Asking every source at once is the same answers in one order.
    #[test]
    fn the_base_asks_everybody_and_orders_the_result() {
        let catalog = SignalCatalog::embedded();
        let features = FeatureCatalog::embedded().unwrap();
        let base = KnowledgeBase::new().with(&catalog).with(&features);

        assert_eq!(base.providers().len(), 2);
        assert_eq!(base.attributions().len(), 1, "only the community data needs credit");

        let signals = base.signals(&f250());
        assert!(!signals.is_empty());
        assert!(
            signals.windows(2).all(|w| w[0].authority <= w[1].authority),
            "answers must arrive in authority order"
        );

        let mappings = base.mappings(&f250());
        assert!(mappings.windows(2).all(|w| w[0].authority <= w[1].authority), "and so must these");
    }
    // ------------------------------------------------- ranking a feature

    fn feature_yaml(extra: &str) -> String {
        format!(
            "features:\n  - id: test.feature\n    name: Test\n    easy: e\n    technical: t\n\
             \x20   risk: convenience\n    modules: [BCM]\n{extra}"
        )
    }

    fn catalog_of(extra: &str) -> FeatureCatalog {
        let mut c = FeatureCatalog::default();
        c.load_yaml(&feature_yaml(extra), "test").unwrap();
        c
    }

    /// A feature this build can only describe is a description, however
    /// narrowly it is scoped. Without this a catalogue of things nobody has
    /// located would outrank every community definition on the machine.
    #[test]
    fn a_feature_with_no_mapping_is_only_a_description() {
        let c = catalog_of("    applies_to:\n      vins: [\"1FT7W2BT7KEF78036\"]\n");
        let answers = c.mappings(&f250());
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].authority, Authority::GenericStandard);
    }

    /// The trap. An unscoped feature matches every vehicle by design — so that
    /// a feature which does not declare itself Ford-only is not hidden from a
    /// Ford — and reading that as "measured on this vehicle" would rank the
    /// whole generic catalogue above definitions recorded for this exact model.
    #[test]
    fn matching_every_vehicle_is_not_the_same_as_being_measured_on_this_one() {
        let c = catalog_of(
            "    verification: verified\n    mapping:\n      kind: data_identifier_bits\n\
             \x20     module: \"726\"\n      did: 0xDE0E\n      byte: 4\n      mask: 0x01\n\
             \x20     on: 0x01\n      off: 0x00\n",
        );
        let answers = c.mappings(&f250());
        assert_eq!(answers.len(), 1);
        assert_eq!(
            answers[0].authority,
            Authority::CommunityThisModel,
            "an unscoped mapping is somebody's claim, not a measurement on this truck"
        );
        assert!(!answers[0].authority.is_about_this_vehicle());
    }

    /// Scoped to this exact VIN and verified: measured here, recorded earlier.
    #[test]
    fn a_mapping_measured_on_this_exact_vin_ranks_as_one() {
        let c = catalog_of(
            "    verification: verified\n    applies_to:\n      vins: [\"1FT7W2BT7KEF78036\"]\n\
             \x20   mapping:\n      kind: data_identifier_bits\n      module: \"726\"\n\
             \x20     did: 0xDE0E\n      byte: 4\n      mask: 0x01\n      on: 0x01\n      off: 0x00\n",
        );
        let answers = c.mappings(&f250());
        assert_eq!(answers[0].authority, Authority::MeasuredEarlier);
        assert!(answers[0].authority.is_about_this_vehicle());
    }

    /// The same mapping offered to a truck that merely resembles the one it
    /// was measured on. Offered, and labelled as coming from elsewhere.
    #[test]
    fn a_mapping_from_a_similar_truck_is_offered_and_labelled() {
        let c = catalog_of(
            "    verification: verified\n    applies_to:\n      vins: [\"1FT7W2BT7KEF00001\"]\n\
             \x20     candidate_vin_prefixes: [\"1FT7W2BT\"]\n    mapping:\n\
             \x20     kind: data_identifier_bits\n      module: \"726\"\n      did: 0xDE0E\n\
             \x20     byte: 4\n      mask: 0x01\n      on: 0x01\n      off: 0x00\n",
        );
        let answers = c.mappings(&f250());
        assert_eq!(answers[0].authority, Authority::MeasuredOnSimilarVehicle);
        assert!(answers[0].authority.is_measurement());
        assert!(!answers[0].authority.is_about_this_vehicle(), "a similar truck is not this truck");
    }

    /// A mapping nobody checked is the same kind of claim a community
    /// definition is, and ranks alongside one rather than above it.
    #[test]
    fn an_unverified_mapping_ranks_with_the_community_claims() {
        let c = catalog_of(
            "    applies_to:\n      vins: [\"1FT7W2BT7KEF78036\"]\n    mapping:\n\
             \x20     kind: data_identifier_bits\n      module: \"726\"\n      did: 0xDE0E\n\
             \x20     byte: 4\n      mask: 0x01\n      on: 0x01\n      off: 0x00\n",
        );
        let answers = c.mappings(&f250());
        assert_eq!(answers[0].authority, Authority::CommunityThisModel);
        assert!(!answers[0].authority.is_measurement());
    }
}
