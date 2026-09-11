//! What vehicle is in front of us, and how we know.
//!
//! # Why this exists
//!
//! The application already collects plenty of evidence about a vehicle and then
//! keeps almost none of it: a VIN decode, calibration identifiers, ECU software
//! and part numbers, which module addresses answered and which they listen on,
//! the protocol and its addressing width, the supported-parameter bitmaps. Each
//! is read by whatever needed it at the time and dropped on the floor.
//!
//! Everything that wants to know *which vehicle this is* — which community
//! definitions apply, whether a measured mapping is about this truck or merely
//! one like it — has been reaching for the VIN alone. That is one piece of
//! evidence treated as the whole answer.
//!
//! # Candidates, not answers
//!
//! A field here holds *candidates*, each carrying the evidence it rests on.
//! Most of the time there is exactly one and it reads like a plain value. When
//! there are two — a VIN read from the engine controller and a different VIN in
//! an as-built file somebody imported — the disagreement is the most important
//! thing on the screen, and collapsing it into a single field by taking the
//! newest would hide precisely the case worth stopping for.
//!
//! # What it refuses to do
//!
//! **It does not infer.** A module reporting `EDC17CP65` at `0xF1F3` is a
//! Bosch controller fitted to a particular engine, and that is not recorded as
//! a fact about the vehicle. It is recorded as *the bytes that module returned
//! at a supplier-defined identifier*, which is what was actually established.
//! Turning a part number into an engine is knowledge this project does not have
//! and will not pretend to.
//!
//! **It states what it does not know.** A missing model is a finding, not a
//! blank to be filled in. Somewhere downstream a decision will be made about
//! whether a mapping applies, and "we never established the model" has to reach
//! that decision intact rather than arriving as an empty string.

use aim_protocols::identification_did_name;
use aim_types::{Module, Vehicle};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The fields this type tracks, in the order a person reads them.
///
/// Named in one place so that "what did we fail to establish" is answerable
/// without a caller having to know the list.
const FIELDS: [&str; 4] = ["vin", "make", "model", "model_year"];

/// Where one piece of evidence came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// Decoded from the VIN's own structure, which is a published standard.
    VinStructure,
    /// A calibration identifier or verification number a module reported.
    CalibrationId,
    /// A data identifier read from a module — software, part or serial number.
    ModuleIdentifier,
    /// A module answered at this address.
    ModulePresence,
    /// The protocol and addressing the vehicle actually negotiated.
    Protocol,
    /// Which parameters a module said it supports.
    SupportedParameters,
    /// Stated by the person using the application.
    UserStatement,
}

impl EvidenceSource {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            EvidenceSource::VinStructure => "vin_structure",
            EvidenceSource::CalibrationId => "calibration_id",
            EvidenceSource::ModuleIdentifier => "module_identifier",
            EvidenceSource::ModulePresence => "module_presence",
            EvidenceSource::Protocol => "protocol",
            EvidenceSource::SupportedParameters => "supported_parameters",
            EvidenceSource::UserStatement => "user_statement",
        }
    }

    /// Whether this kind of evidence came off the vehicle itself.
    ///
    /// A person saying what their truck is may well be right, and it is still a
    /// different kind of thing from a module answering — which matters when two
    /// of them disagree.
    pub fn measured(&self) -> bool {
        !matches!(self, EvidenceSource::UserStatement)
    }
}

/// One thing that was established, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// What this is evidence about, e.g. `vin`, `module`, `identifier`.
    pub about: String,
    /// What was read or decoded, verbatim.
    pub value: String,
    /// How it was established.
    pub source: EvidenceSource,
    /// Which module said it, when a particular one did.
    pub reported_by: Option<String>,
    /// Anything needed to read the value honestly, such as an identifier whose
    /// meaning is not published.
    pub note: Option<String>,
}

impl Evidence {
    /// Evidence with no module attribution and nothing to qualify.
    pub fn new(about: impl Into<String>, value: impl Into<String>, source: EvidenceSource) -> Self {
        Evidence { about: about.into(), value: value.into(), source, reported_by: None, note: None }
    }

    /// Attribute this to the module that said it.
    pub fn from_module(mut self, module_key: impl Into<String>) -> Self {
        self.reported_by = Some(module_key.into());
        self
    }

    /// Qualify the value.
    pub fn noting(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// One possible value for a field, with everything supporting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The value itself.
    pub value: String,
    /// Every piece of evidence that produced this value. Two sources agreeing
    /// strengthen one candidate rather than creating a second.
    pub evidence: Vec<Evidence>,
}

impl Candidate {
    /// Whether anything measured off the vehicle supports this.
    pub fn is_measured(&self) -> bool {
        self.evidence.iter().any(|e| e.source.measured())
    }
}

/// One field of the identity and everything it could be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityField {
    /// Field name, e.g. `vin`.
    pub field: String,
    /// What it could be. Empty means nobody established it.
    pub candidates: Vec<Candidate>,
}

impl IdentityField {
    /// The value, when exactly one candidate exists.
    ///
    /// Returns `None` both when nothing was established and when sources
    /// disagree — deliberately the same answer, because in both cases a caller
    /// that proceeds as though it knows is wrong.
    pub fn settled(&self) -> Option<&str> {
        match self.candidates.as_slice() {
            [one] => Some(one.value.as_str()),
            _ => None,
        }
    }

    /// Whether two sources gave different answers for this field.
    pub fn is_contested(&self) -> bool {
        self.candidates.len() > 1
    }
}

/// Everything known about the vehicle in front of us.
///
/// Fields that were established carry candidates; fields that were not appear
/// in [`VehicleIdentity::unresolved`] rather than as empty values a reader has
/// to notice. Both halves travel together deliberately — a decision about
/// whether a definition applies needs the gaps as much as the facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleIdentity {
    /// Fields with at least one candidate.
    pub fields: Vec<IdentityField>,
    /// Evidence about the vehicle that resolves to no field: which modules
    /// answered, what they reported about themselves, what protocol they spoke.
    /// Not filler — this is most of what distinguishes one build of a model
    /// from another, and it is what a mapping will eventually be matched on.
    pub observations: Vec<Evidence>,
    /// Fields nobody established, named rather than left blank.
    pub unresolved: Vec<String>,
}

impl VehicleIdentity {
    /// Assemble what is known from a vehicle record and the modules found.
    ///
    /// Takes what it is given and records provenance. It performs no lookups
    /// and reaches no conclusions that were not already reached by whatever
    /// read the data.
    pub fn assemble(vehicle: Option<&Vehicle>, modules: &[Module]) -> VehicleIdentity {
        let mut id = VehicleIdentity::default();

        if let Some(v) = vehicle {
            // The VIN, and the two things its own published structure encodes.
            // Model, trim and engine are only ever set on that record by a
            // validated profile, and when one has set them it says so here the
            // same way anything else does.
            if let Some(vin) = &v.vin {
                id.propose("vin", vin, Evidence::new("vin", vin, EvidenceSource::VinStructure));
            }
            if let Some(make) = &v.make {
                id.propose(
                    "make",
                    make,
                    Evidence::new("make", make, EvidenceSource::VinStructure)
                        .noting("decoded from the VIN's world manufacturer identifier"),
                );
            }
            if let Some(year) = v.year {
                let year = year.to_string();
                id.propose(
                    "model_year",
                    &year,
                    Evidence::new("model_year", &year, EvidenceSource::VinStructure)
                        .noting("decoded from VIN position 10"),
                );
            }
            if let Some(model) = &v.model {
                id.propose(
                    "model",
                    model,
                    Evidence::new("model", model, EvidenceSource::VinStructure),
                );
            }
        }

        for m in modules {
            let where_it_listens = match &m.request_address {
                Some(req) => format!("answers at {}, listens at {req}", m.address),
                None => format!("answers at {}", m.address),
            };
            id.observe(
                Evidence::new("module", where_it_listens, EvidenceSource::ModulePresence)
                    .from_module(&m.module_key),
            );

            // A module's own identity strings. Recorded as what the module
            // said, never turned into a claim about the vehicle: a calibration
            // identifier looks like a part number and often names a controller
            // family, and reading either as a model or an engine would be a
            // guess wearing a serial number.
            for cal in &m.identity.calibration_ids {
                id.observe(
                    Evidence::new("calibration", cal, EvidenceSource::CalibrationId)
                        .from_module(&m.module_key),
                );
            }
            for cvn in &m.identity.calibration_verification_numbers {
                id.observe(
                    Evidence::new("calibration_verification", cvn, EvidenceSource::CalibrationId)
                        .from_module(&m.module_key),
                );
            }
            if let Some(name) = &m.identity.ecu_name {
                id.observe(
                    Evidence::new("ecu_name", name, EvidenceSource::ModuleIdentifier)
                        .from_module(&m.module_key),
                );
            }
            if let Some(sw) = &m.software_version {
                id.observe(
                    Evidence::new("module_software", sw, EvidenceSource::ModuleIdentifier)
                        .from_module(&m.module_key),
                );
            }
        }

        // The protocol is a fact about the vehicle and not only about the link:
        // one answering 29-bit CAN is a different vehicle from one answering
        // ISO 9141, and it narrows what could possibly apply. Recorded once
        // even though every module carries it.
        if let Some(first) = modules.first() {
            id.observe(Evidence::new("protocol", first.protocol.label(), EvidenceSource::Protocol));
        }

        id.note_gaps();
        id
    }

    /// Record a value for a field, with the evidence behind it.
    ///
    /// A value already proposed gains the new evidence rather than becoming a
    /// second candidate: two sources agreeing is one stronger answer, and
    /// showing it twice would read as a disagreement.
    pub fn propose(&mut self, field: &str, value: impl AsRef<str>, evidence: Evidence) {
        let value = value.as_ref();
        let slot = match self.fields.iter().position(|f| f.field == field) {
            Some(i) => &mut self.fields[i],
            None => {
                self.fields
                    .push(IdentityField { field: field.to_string(), candidates: Vec::new() });
                self.fields.last_mut().expect("just pushed")
            }
        };
        match slot.candidates.iter_mut().find(|c| c.value == value) {
            Some(c) => {
                if !c.evidence.contains(&evidence) {
                    c.evidence.push(evidence);
                }
            }
            None => slot
                .candidates
                .push(Candidate { value: value.to_string(), evidence: vec![evidence] }),
        }
        self.note_gaps();
    }

    /// Record evidence about the vehicle that resolves to no particular field.
    pub fn observe(&mut self, evidence: Evidence) {
        if !self.observations.contains(&evidence) {
            self.observations.push(evidence);
        }
    }

    /// Record the identification records read from one module.
    ///
    /// Keyed by data identifier, holding whatever the module returned. The
    /// standard names part of this block and leaves the rest to manufacturers
    /// and suppliers; an unnamed identifier is reported as an identifier rather
    /// than dressed up as a fact, which is why `EDC17CP65` arrives here as "the
    /// bytes at F1F3" and not as an engine.
    ///
    /// One exception, and it is not an inference: `0xF190` **is** the VIN by
    /// definition in ISO 14229, so a module answering there has stated a VIN
    /// and it becomes a candidate like any other. If it disagrees with the one
    /// service 09 reported, that disagreement is now visible instead of one of
    /// them silently winning.
    pub fn record_identifiers(&mut self, module_key: &str, records: &BTreeMap<u16, String>) {
        for (did, text) in records {
            if text.trim().is_empty() {
                continue;
            }
            if *did == 0xF190 {
                self.propose(
                    "vin",
                    text,
                    Evidence::new("vin", text, EvidenceSource::ModuleIdentifier)
                        .from_module(module_key)
                        .noting("read from data identifier F190"),
                );
                continue;
            }
            let note = match identification_did_name(*did) {
                Some(name) => format!("data identifier {did:04X}, {name}"),
                None => format!(
                    "data identifier {did:04X}, defined by the manufacturer or supplier and not \
                     published — the bytes are what was established, not their meaning"
                ),
            };
            self.observe(
                Evidence::new("identifier", text, EvidenceSource::ModuleIdentifier)
                    .from_module(module_key)
                    .noting(note),
            );
        }
        self.note_gaps();
    }

    /// Record which parameters a module said it supports.
    ///
    /// Kept because it is a fingerprint rather than a curiosity: two vehicles of
    /// the same model year differ here when they were built differently, and
    /// when a mapping eventually has to be judged as applying or not applying,
    /// this is the sort of thing that judges it.
    pub fn record_supported_parameters(&mut self, module_key: &str, pids: &[u8]) {
        if pids.is_empty() {
            return;
        }
        let listed = pids.iter().map(|p| format!("{p:02X}")).collect::<Vec<_>>().join(" ");
        self.observe(
            Evidence::new("supported_parameters", listed, EvidenceSource::SupportedParameters)
                .from_module(module_key)
                .noting(format!("{} parameters reported as supported", pids.len())),
        );
    }

    /// Name every field nobody established.
    ///
    /// Re-derived rather than maintained, so that a caller adding evidence
    /// later — a person saying what their vehicle is, a profile supplying a
    /// model — never has to remember to refresh it.
    fn note_gaps(&mut self) {
        self.unresolved = FIELDS
            .iter()
            .filter(|name| {
                !self.fields.iter().any(|f| &&f.field == name && !f.candidates.is_empty())
            })
            .map(|name| name.to_string())
            .collect();
    }

    /// Whether anything at all has been established.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.observations.is_empty()
    }

    /// One field, when it has any candidates.
    pub fn field(&self, name: &str) -> Option<&IdentityField> {
        self.fields.iter().find(|f| f.field == name)
    }

    /// A field's value, when exactly one candidate exists for it.
    pub fn settled(&self, name: &str) -> Option<&str> {
        self.field(name).and_then(IdentityField::settled)
    }

    /// Every field two sources disagreed about.
    ///
    /// The thing a caller should check before acting on an identity. An empty
    /// list is the normal case; a non-empty one is a question for a person.
    pub fn contested(&self) -> Vec<&IdentityField> {
        self.fields.iter().filter(|f| f.is_contested()).collect()
    }

    /// Observations about one subject, in the order they were gathered.
    pub fn observations_about(&self, about: &str) -> Vec<&Evidence> {
        self.observations.iter().filter(|e| e.about == about).collect()
    }
}

/// What the knowledge providers get to ask about.
///
/// **Only settled fields cross over.** A contested field arrives as unknown
/// rather than as whichever candidate happened to be first, because a provider
/// asked with the wrong VIN answers confidently about a different vehicle —
/// which is worse than answering nothing. The providers are built to handle
/// not knowing; they are not built to handle being told something false.
impl From<&VehicleIdentity> for aim_decoders::VehicleContext {
    fn from(id: &VehicleIdentity) -> Self {
        aim_decoders::VehicleContext {
            make: id.settled("make").map(String::from),
            model: id.settled("model").map(String::from),
            year: id.settled("model_year").and_then(|y| y.parse().ok()),
            vin: id.settled("vin").map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_types::{now, ModuleId, ModuleIdentity, ObdProtocol, SessionId, VehicleId};

    fn module(key: &str, address: &str, cal: &[&str]) -> Module {
        Module {
            id: ModuleId::new(),
            session_id: SessionId::from_string("ses_test"),
            module_key: key.into(),
            name: key.into(),
            address: address.into(),
            request_address: None,
            protocol: ObdProtocol::Iso15765Can11_500,
            identity: ModuleIdentity {
                ecu_name: None,
                calibration_ids: cal.iter().map(|c| c.to_string()).collect(),
                calibration_verification_numbers: Vec::new(),
            },
            software_version: None,
            discovered_at: now(),
        }
    }

    fn truck() -> Vehicle {
        Vehicle {
            id: VehicleId::new(),
            vin: Some(String::from("1FT7W2BT7KEF78036")),
            make: Some(String::from("Ford Motor Company (US, truck)")),
            model: None,
            year: Some(2019),
            trim: None,
            engine: None,
            transmission: None,
            discovered_at: now(),
        }
    }

    /// The evidence was always collected and always thrown away. It now travels
    /// together, each piece saying where it came from.
    #[test]
    fn evidence_is_gathered_with_its_source() {
        let modules = vec![module("ECU_7E8", "7E8", &["KC3A-14C204-AVK"])];
        let id = VehicleIdentity::assemble(Some(&truck()), &modules);

        assert_eq!(id.settled("vin"), Some("1FT7W2BT7KEF78036"));
        assert_eq!(id.settled("model_year"), Some("2019"));

        let cal = id.observations_about("calibration");
        assert_eq!(cal.len(), 1);
        assert_eq!(cal[0].value, "KC3A-14C204-AVK");
        assert_eq!(cal[0].source, EvidenceSource::CalibrationId);
        // Attributed to the module that said it, not to "the vehicle".
        assert_eq!(cal[0].reported_by.as_deref(), Some("ECU_7E8"));
    }

    /// The gap that matters. A VIN does not encode the model in any way this
    /// project can decode, so it is missing on nearly every vehicle — and a
    /// reader has to be able to tell "not established" from "checked and
    /// empty".
    #[test]
    fn what_was_never_established_is_named() {
        let id = VehicleIdentity::assemble(Some(&truck()), &[]);
        assert_eq!(id.unresolved, vec!["model"]);
        assert_eq!(id.settled("model"), None);
    }

    /// A part number is what a module said, never what the vehicle is. Turning
    /// `EDC17CP65` into an engine is knowledge this project does not have —
    /// and `F1F3` is supplier-defined, so even the field name is unpublished.
    #[test]
    fn an_unpublished_identifier_stays_an_identifier() {
        let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
        id.record_identifiers("ECU_7E8", &BTreeMap::from([(0xF1F3u16, String::from("EDC17CP65"))]));

        assert_eq!(id.settled("model"), None, "a controller family is not a model");
        assert!(id.unresolved.contains(&String::from("model")));

        let found = id.observations_about("identifier");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, "EDC17CP65");
        let note = found[0].note.as_deref().unwrap_or_default();
        assert!(note.contains("F1F3"), "the identifier itself is the finding: {note}");
        assert!(note.contains("not published"), "and its meaning is not: {note}");
    }

    /// The standard names part of that block, and where it does, saying so is
    /// reporting rather than guessing.
    #[test]
    fn a_standard_identifier_is_named_from_the_standard() {
        let mut id = VehicleIdentity::default();
        id.record_identifiers(
            "ECU_7E8",
            &BTreeMap::from([(0xF188u16, String::from("HC3A-14C204-AB"))]),
        );
        let note = id.observations_about("identifier")[0].note.as_deref().unwrap();
        assert!(note.contains("manufacturer ECU software number"), "{note}");
    }

    /// Two sources agreeing is one answer with two reasons behind it, not two
    /// answers.
    #[test]
    fn agreement_strengthens_rather_than_splits() {
        let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
        id.record_identifiers(
            "ECU_7E8",
            &BTreeMap::from([(0xF190u16, String::from("1FT7W2BT7KEF78036"))]),
        );

        let vin = id.field("vin").unwrap();
        assert_eq!(vin.candidates.len(), 1, "one VIN, confirmed twice");
        assert_eq!(vin.candidates[0].evidence.len(), 2);
        assert!(id.contested().is_empty());
    }

    /// And disagreement is the one thing that must never be quietly resolved.
    /// This is the as-built import case before it exists: a file claiming a VIN
    /// the truck does not report is the whole reason to check.
    #[test]
    fn disagreement_is_kept_visible() {
        let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
        id.record_identifiers(
            "ECU_7E8",
            &BTreeMap::from([(0xF190u16, String::from("1FT7W2BT7KEF99999"))]),
        );

        let vin = id.field("vin").unwrap();
        assert_eq!(vin.candidates.len(), 2);
        assert_eq!(vin.settled(), None, "a contested field has no settled value");
        assert_eq!(id.contested().len(), 1);
        // Not unresolved — two answers is not the same as none, and a person
        // reading this needs to see both.
        assert!(!id.unresolved.contains(&String::from("vin")));
    }

    /// An unidentified vehicle is every field unresolved and no evidence, not
    /// an identity that happens to be full of blanks.
    #[test]
    fn nothing_read_is_nothing_claimed() {
        let id = VehicleIdentity::assemble(None, &[]);
        assert!(id.is_empty());
        assert_eq!(id.unresolved, FIELDS.to_vec());
    }

    #[test]
    fn the_protocol_a_vehicle_answered_on_is_evidence_about_it() {
        let modules = vec![module("ECU_7E8", "7E8", &[])];
        let id = VehicleIdentity::assemble(Some(&truck()), &modules);
        let protocol = id.observations_about("protocol");
        assert_eq!(protocol.len(), 1);
        assert_eq!(protocol[0].source, EvidenceSource::Protocol);
    }

    /// Where a module listens is half of what a scan learns and the half that
    /// used to be dropped — a module that can be found and never spoken to
    /// again is not much of a finding.
    #[test]
    fn both_halves_of_an_address_are_kept() {
        let mut m = module("BCM_726", "72E", &[]);
        m.request_address = Some(String::from("726"));
        let id = VehicleIdentity::assemble(None, &[m]);
        let found = id.observations_about("module");
        assert!(found[0].value.contains("listens at 726"), "{}", found[0].value);
    }

    /// The bridge to the knowledge providers, and the one thing it must not do.
    ///
    /// A provider given the wrong VIN answers confidently about a different
    /// vehicle. A provider given no VIN says it has nothing. The second is the
    /// failure worth having, so a contested field crosses over as unknown.
    #[test]
    fn a_contested_field_reaches_the_providers_as_unknown() {
        let mut id = VehicleIdentity::assemble(Some(&truck()), &[]);
        let settled: aim_decoders::VehicleContext = (&id).into();
        assert_eq!(settled.vin.as_deref(), Some("1FT7W2BT7KEF78036"));
        assert_eq!(settled.year, Some(2019));

        id.record_identifiers(
            "ECU_7E8",
            &BTreeMap::from([(0xF190u16, String::from("1FT7W2BT7KEF99999"))]),
        );
        let contested: aim_decoders::VehicleContext = (&id).into();
        assert_eq!(contested.vin, None, "neither VIN may be passed off as the answer");
        // Everything else it does know still goes through — one disagreement
        // does not blind the providers to the rest.
        assert_eq!(contested.year, Some(2019));
        assert!(contested.make.is_some());
    }

    #[test]
    fn supported_parameters_are_kept_as_a_fingerprint() {
        let mut id = VehicleIdentity::default();
        id.record_supported_parameters("ECU_7E8", &[0x01, 0x05, 0x0C]);
        let found = id.observations_about("supported_parameters");
        assert_eq!(found[0].value, "01 05 0C");
        assert!(found[0].note.as_deref().unwrap().contains("3 parameters"));
    }
}
