//! The shape of an answer a non-mechanic can act on.
//!
//! The model does not write this as prose. It fills in a schema, submitted
//! through a tool call, which is what makes the honesty rules structural rather
//! than hopeful: there is nowhere in this type to put a repair cost without also
//! saying where the number came from, and no way to state a finding without
//! choosing a [`Source`] for it.
//!
//! That matters because the whole product is the difference between "P2463,
//! 392.9 degC" and "the exhaust filter is clogging; here is what that costs".
//! The second is only worth more than the first if you can still see the first
//! underneath it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Where a claim came from. Every finding must declare one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Read from the vehicle in this session. Traceable to a raw exchange.
    Measured,
    /// Derived from something measured, by arithmetic this build performed.
    ///
    /// A fuel trim compared against a threshold, a readiness conclusion drawn
    /// from monitor states. The inputs are traceable; the conclusion is this
    /// project's, not the vehicle's, and saying so is the difference between
    /// "your truck reported this" and "we worked this out from what it
    /// reported".
    IndirectlyMeasured,
    /// A mapping from a vehicle profile: which bits hold which setting.
    ///
    /// Not measured this session and not a public standard either. It is
    /// somebody's recorded observation of a vehicle, carrying its own source
    /// and verification count, and it deserves its own label rather than being
    /// filed under either neighbour.
    Profile,
    /// The SAE catalogue description of a code this build shipped.
    Catalog,
    /// The model's general automotive knowledge. **Not verified against this
    /// vehicle, this session, or any dataset this project owns.** Cost
    /// estimates and failure predictions are always this.
    ModelKnowledge,
    /// Not enough to attribute. A finding that reaches here should usually not
    /// have been made.
    Unknown,
}

/// How much the evidence supports a finding.
///
/// Deliberately **not** severity. "How bad is this if true" and "how sure are
/// we that it is true" are different axes, and collapsing them is how a
/// tentative guess about a serious problem gets rendered as a serious problem.
/// A finding can be critical and low-confidence at once; that combination is
/// exactly the one a person most needs to see labelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceClass {
    /// Read from this vehicle, with the exchange to prove it.
    High,
    /// Derived from measurement, or a standard description of something
    /// measured. Sound reasoning over real inputs.
    Moderate,
    /// General knowledge, or a mapping nobody has verified. Plausible, and not
    /// established for this vehicle.
    Low,
    /// No supporting evidence at all.
    None,
}

/// Why a finding has the confidence it has.
///
/// Machine-readable so the interface can explain a rating rather than assert
/// it, and so a test can prove the rating follows from the evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceBasis {
    /// Read from this vehicle this session, with an event-log row behind it.
    MeasuredThisSession,
    /// Measured, but with no event-log row to point at.
    ClaimedMeasuredWithoutEvidence,
    /// Computed by this build from measured inputs.
    DerivedFromMeasurement,
    /// The published description of a standard code.
    StandardCatalogue,
    /// A vehicle profile mapping, which carries its own verification record.
    ProfileMapping,
    /// The model reasoning from general automotive knowledge.
    ModelReasoning,
    /// Nothing supports this.
    NoEvidence,
}

/// A confidence rating and the reasons for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confidence {
    /// The rating.
    pub class: ConfidenceClass,
    /// What produced it, in order.
    pub basis: Vec<ConfidenceBasis>,
}

/// How much it matters, in words a driver understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Stop driving, or do not buy.
    Critical,
    /// Needs attention soon; budget for it.
    Serious,
    /// Worth knowing, not urgent.
    Caution,
    /// Context, not a problem.
    Info,
}

/// The headline answer to "should I buy this?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Serious faults; walk away unless the price reflects them.
    WalkAway,
    /// Real problems with a knowable cost; use them to negotiate.
    Negotiate,
    /// Nothing alarming found in what could be checked.
    LooksSound,
    /// Not enough was readable to say. An honest non-answer.
    Inconclusive,
}

/// A rough cost band. Never a point figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRange {
    /// Low end.
    pub low: f64,
    /// High end.
    pub high: f64,
    /// Currency code as the model gave it.
    pub currency: String,
    /// What the range is based on, in the model's own words.
    pub basis: String,
}

/// One thing found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// A short title in plain words. Not a code.
    pub title: String,
    /// How much it matters.
    pub severity: Severity,
    /// What it means for whoever drives the car. No jargon, no code numbers.
    pub plain_english: String,
    /// Where this claim comes from.
    pub source: Source,
    /// The specific observations behind it, e.g. `"P2463 confirmed"`.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Event-log rows proving it, so the UI can link to the raw exchange.
    #[serde(default)]
    pub evidence_refs: Vec<i64>,
    /// What to do about it.
    #[serde(default)]
    pub what_to_do: Option<String>,
    /// Rough repair cost, when the model offered one.
    #[serde(default)]
    pub estimated_cost: Option<CostRange>,
}

impl Finding {
    /// Whether this finding rests on something actually read from the vehicle.
    pub fn is_evidenced(&self) -> bool {
        self.source == Source::Measured && !self.evidence_refs.is_empty()
    }

    /// How much the evidence supports this, **derived rather than asserted**.
    ///
    /// This is the whole point. A model asked how confident it is will answer,
    /// fluently, and that answer is a property of the model rather than of the
    /// vehicle. So the report type has nowhere for the model to put one: the
    /// rating is computed here from what the finding actually cites.
    ///
    /// A claim of `Measured` with no event-log row behind it is the case worth
    /// noticing. It is not treated as measured, because the whole meaning of
    /// that word here is "there is a raw exchange you can go and look at".
    pub fn confidence(&self) -> Confidence {
        let (class, basis) = match self.source {
            Source::Measured if !self.evidence_refs.is_empty() => {
                (ConfidenceClass::High, ConfidenceBasis::MeasuredThisSession)
            }
            Source::Measured => {
                (ConfidenceClass::Low, ConfidenceBasis::ClaimedMeasuredWithoutEvidence)
            }
            Source::IndirectlyMeasured => {
                (ConfidenceClass::Moderate, ConfidenceBasis::DerivedFromMeasurement)
            }
            Source::Catalog => (ConfidenceClass::Moderate, ConfidenceBasis::StandardCatalogue),
            Source::Profile => (ConfidenceClass::Low, ConfidenceBasis::ProfileMapping),
            Source::ModelKnowledge => (ConfidenceClass::Low, ConfidenceBasis::ModelReasoning),
            Source::Unknown => (ConfidenceClass::None, ConfidenceBasis::NoEvidence),
        };
        Confidence { class, basis: vec![basis] }
    }
}

/// The whole answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The one-line answer.
    pub verdict: Verdict,
    /// A single sentence a non-mechanic can repeat to a seller.
    pub headline: String,
    /// Two to four sentences of context.
    pub summary: String,
    /// What was found, worst first.
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Things likely to need attention later. Always [`Source::ModelKnowledge`].
    #[serde(default)]
    pub watch_items: Vec<Finding>,
    /// What could not be checked, and why. Absence of evidence, stated.
    #[serde(default)]
    pub not_checked: Vec<String>,
    /// Concrete next steps.
    #[serde(default)]
    pub next_steps: Vec<String>,
}

impl Report {
    /// Findings ordered worst-first, which is the only order a buyer reads in.
    pub fn by_severity(&self) -> Vec<&Finding> {
        let mut v: Vec<&Finding> = self.findings.iter().collect();
        v.sort_by_key(|f| f.severity);
        v
    }

    /// Total of the low and high ends of every cost range mentioned.
    ///
    /// Returned as a band, never a single number, and only meaningful next to
    /// the warning that every part of it is unverified general knowledge.
    pub fn cost_band(&self) -> Option<(f64, f64, String)> {
        let costs: Vec<&CostRange> = self
            .findings
            .iter()
            .chain(self.watch_items.iter())
            .filter_map(|f| f.estimated_cost.as_ref())
            .collect();
        if costs.is_empty() {
            return None;
        }
        let currency = costs[0].currency.clone();
        Some((costs.iter().map(|c| c.low).sum(), costs.iter().map(|c| c.high).sum(), currency))
    }

    /// Why this report is not usable as it stands, if it is not.
    ///
    /// Returned to the model so it can correct itself, in the same way a schema
    /// error is. These are incoherences rather than schema violations: the JSON
    /// is valid and the meaning is not.
    ///
    /// Measured on two different local models: both reached a sensible verdict,
    /// wrote a sensible summary, and then submitted an **empty `findings`
    /// list** — one of them filing the actual problems under `watch_items`
    /// instead. A report that says "negotiate" and itemises nothing gives the
    /// buyer nothing to negotiate with.
    ///
    /// Assemble an honest partial report from evidence alone, with no model
    /// judgement in it.
    ///
    /// Used when the agent ran out of steps without submitting anything. The
    /// alternative is showing the user nothing after several minutes of real
    /// reading, which throws away evidence that was correctly gathered and paid
    /// for in adapter round trips.
    ///
    /// Everything here is `Catalog` or measured fact: the codes are the ones the
    /// vehicle reported and the descriptions are the ones this build shipped.
    /// Nothing is inferred, no severity is guessed above `caution`, and the
    /// verdict is always `Inconclusive` — because the agent genuinely did not
    /// reach one.
    pub fn from_evidence(codes: &[(String, Option<String>)], steps: usize) -> Report {
        let findings = codes
            .iter()
            .map(|(code, description)| Finding {
                title: description.clone().unwrap_or_else(|| format!("Trouble code {code}")),
                // Not graded: grading is judgement, and the agent did not finish.
                severity: Severity::Caution,
                plain_english: match description {
                    Some(d) => format!(
                        "The vehicle has stored {code}. The standard description for this code is: \
                         {d}. Nobody has interpreted what it means for this particular vehicle — \
                         the inspection did not finish."
                    ),
                    None => format!(
                        "The vehicle has stored {code}. This build has no description for that \
                         code, and the inspection did not finish."
                    ),
                },
                source: Source::Catalog,
                evidence: vec![format!("{code} read from the vehicle")],
                evidence_refs: vec![],
                what_to_do: None,
                estimated_cost: None,
            })
            .collect::<Vec<_>>();

        let headline = if codes.is_empty() {
            "The inspection did not finish, and nothing was concluded.".to_string()
        } else {
            format!(
                "The inspection did not finish. {} trouble code(s) were read before it stopped.",
                codes.len()
            )
        };

        Report {
            verdict: Verdict::Inconclusive,
            headline,
            summary: format!(
                "The agent used all {steps} of its steps without reaching a conclusion. What is \
                 listed below is the raw evidence it had gathered, with the standard description \
                 of each code — not a diagnosis. Try again with a stronger model, or raise the \
                 step budget in Settings."
            ),
            findings,
            watch_items: vec![],
            not_checked: vec![
                "The agent did not finish, so anything it had not yet read is unchecked."
                    .to_string(),
                "No interpretation, severity assessment or cost estimate was produced.".to_string(),
            ],
            next_steps: vec![
                "Run the inspection again — results vary between runs.".to_string(),
                "In Settings, use a stronger model or increase the step budget.".to_string(),
            ],
        }
    }

    /// Trouble codes this report claims, drawn from every text field a model
    /// actually writes them into.
    ///
    /// Matches the SAE shape — one of P/B/C/U then four hex digits — which is
    /// specific enough not to catch ordinary prose.
    pub fn cited_codes(&self) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for f in self.findings.iter().chain(self.watch_items.iter()) {
            let mut texts: Vec<&str> = vec![&f.title, &f.plain_english];
            texts.extend(f.evidence.iter().map(String::as_str));
            for t in texts {
                out.extend(extract_codes(t));
            }
        }
        out.extend(extract_codes(&self.headline));
        out.extend(extract_codes(&self.summary));
        out
    }

    /// See [`Report::incoherence`]; `codes` are the codes actually read.
    pub fn incoherence_against(
        &self,
        codes: &std::collections::BTreeSet<String>,
    ) -> Option<String> {
        // Fabricated codes are the worst failure this type can carry.
        //
        // Measured on qwen3:8b, and caused by an earlier version of the rule
        // below: told it had to itemise the four codes it had read, it produced
        // four findings citing P0420, P0446, P0300 and P0171 — none of which
        // this vehicle reported. Pressure to produce findings turned an
        // omission into an invention, which is far more dangerous: a buyer
        // could walk away over a misfire that does not exist.
        let invented: Vec<String> = self.cited_codes().difference(codes).cloned().collect();
        if !invented.is_empty() {
            let known = if codes.is_empty() {
                "none were read".to_string()
            } else {
                codes.iter().cloned().collect::<Vec<_>>().join(", ")
            };
            return Some(format!(
                "Your report cites trouble codes this vehicle never reported: {}. \
                 The codes actually read were: {known}. Remove anything you did not read and \
                 rewrite the findings using only those. Do not substitute codes you associate \
                 with the symptoms.",
                invented.join(", ")
            ));
        }
        self.incoherence(codes.len())
    }

    /// Structural problems that do not depend on which codes were read.
    pub fn incoherence(&self, codes_found: usize) -> Option<String> {
        // Evidence beats verdict.
        //
        // Measured on qwen3:8b: once forced to read the codes, it wrote a
        // headline saying "exhaust filter restriction detected; serious issue
        // requiring attention", then set the verdict to `inconclusive` with an
        // empty findings list — sliding under a rule keyed on the verdict. If
        // the truck reported faults, the report has to list them, whatever
        // verdict the model reached.
        if codes_found > 0 && self.findings.is_empty() {
            return Some(format!(
                "The vehicle reported {codes_found} trouble code(s), but your `findings` list is \
                 empty. Every code you read must appear as a finding, explained in plain English. \
                 Add them and submit again."
            ));
        }
        if self.findings.is_empty()
            && matches!(self.verdict, Verdict::WalkAway | Verdict::Negotiate)
        {
            return Some(format!(
                "You set verdict {:?} but left `findings` empty. A verdict that says something is \
                 wrong must list what is wrong. Move anything you actually observed from \
                 `watch_items` into `findings` — `watch_items` is only for things you predict may \
                 fail in future, not for faults the vehicle reported. Then submit again.",
                self.verdict
            ));
        }
        if self.headline.trim().is_empty() || self.summary.trim().is_empty() {
            return Some("`headline` and `summary` cannot be empty.".to_string());
        }
        None
    }

    /// Repair anything the model got structurally wrong.
    ///
    /// Models reliably drift on two points, and both are corrected here rather
    /// than argued about in the prompt: a cost estimate is never a measurement,
    /// and something predicted to fail in future has by definition not been
    /// observed failing.
    pub fn normalise(&mut self) {
        for f in &mut self.findings {
            if f.estimated_cost.is_some() && f.source == Source::Measured {
                f.source = Source::ModelKnowledge;
            }
        }
        for w in &mut self.watch_items {
            w.source = Source::ModelKnowledge;
            w.evidence_refs.clear();
        }
    }
}

/// The name the model calls to deliver a report.
pub const SUBMIT_TOOL: &str = "submit_report";

/// JSON Schema for [`SUBMIT_TOOL`].
///
/// Written out rather than derived: the descriptions are prompt text the model
/// reads while filling each field, and they are where most of the "explain it
/// like I am a normal person" instruction actually lands.
pub fn submit_report_schema() -> Value {
    let finding = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string",
                "description": "Short, plain words. 'Exhaust filter is clogging', not 'P2463'." },
            "severity": { "type": "string", "enum": ["critical", "serious", "caution", "info"],
                "description": "critical = do not drive or do not buy. serious = needs attention soon. caution = worth knowing. info = context." },
            "plain_english": { "type": "string",
                "description": "What this means for whoever drives the car, in two or three sentences. No code numbers, no acronyms, no jargon. Assume the reader has never opened a bonnet." },
            "source": { "type": "string",
                "enum": ["measured", "indirectly_measured", "profile", "catalog", "model_knowledge", "unknown"],
                "description": "Where this claim comes from. measured = read from this vehicle in this session, and you must give evidence_refs. indirectly_measured = worked out from measured values, such as comparing a reading against a threshold. profile = a vehicle profile mapping, which is somebody's recorded observation rather than a standard. catalog = the standard description of a code. model_knowledge = your own general automotive knowledge, NOT checked against this vehicle; cost estimates and predictions are ALWAYS this. unknown = you cannot attribute it, in which case reconsider whether to state it at all. Do not rate your own confidence anywhere: it is derived from this field and from evidence_refs, and a rating you supply would be a fact about you rather than about the vehicle." },
            "evidence": { "type": "array", "items": { "type": "string" },
                "description": "The specific observations behind this, e.g. 'P2463 confirmed on ECU_7E8'." },
            "evidence_refs": { "type": "array", "items": { "type": "integer" },
                "description": "raw_evidence_ref or evidence_ref values from the tool results this is based on. Required when source is 'measured'." },
            "what_to_do": { "type": "string", "description": "The concrete next action." },
            "estimated_cost": {
                "type": "object",
                "properties": {
                    "low": { "type": "number" },
                    "high": { "type": "number" },
                    "currency": { "type": "string" },
                    "basis": { "type": "string",
                        "description": "What the range is based on, and that it is unverified general knowledge rather than a quote." }
                },
                "required": ["low", "high", "currency", "basis"],
                "additionalProperties": false
            }
        },
        "required": ["title", "severity", "plain_english", "source"],
        "additionalProperties": false
    });

    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string",
                "enum": ["walk_away", "negotiate", "looks_sound", "inconclusive"],
                "description": "inconclusive is a valid and honest answer when too little could be read. Do not guess." },
            "headline": { "type": "string",
                "description": "One sentence the buyer could say out loud to the seller." },
            "summary": { "type": "string",
                "description": "Two to four sentences. Plain language. Lead with what matters." },
            "findings": { "type": "array", "items": finding,
                "description": "What you actually found, worst first." },
            "watch_items": { "type": "array", "items": finding,
                "description": "Components likely to need attention later. These are predictions, so source is always model_knowledge and evidence_refs must be empty." },
            "not_checked": { "type": "array", "items": { "type": "string" },
                "description": "What you could NOT check and why. Be explicit: a buyer needs to know the limits of the scan." },
            "next_steps": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["verdict", "headline", "summary", "findings"],
        "additionalProperties": false
    })
}

/// Pull SAE-shaped trouble codes out of free text.
fn extract_codes(text: &str) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    for i in 0..bytes.len() {
        let c = bytes[i].to_ascii_uppercase();
        if !matches!(c, 'P' | 'B' | 'C' | 'U') {
            continue;
        }
        // Must start a word, or P inside SUPPLY0000 would match.
        if i > 0 && (bytes[i - 1].is_alphanumeric()) {
            continue;
        }
        if i + 4 >= bytes.len() {
            continue;
        }
        let digits: String = bytes[i + 1..i + 5].iter().collect();
        if digits.len() == 4 && digits.chars().all(|d| d.is_ascii_hexdigit()) {
            // Reject a longer alphanumeric run: P12345 is not a code.
            let after = bytes.get(i + 5);
            if after.is_none_or(|a| !a.is_alphanumeric()) {
                out.push(format!("{c}{}", digits.to_ascii_uppercase()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(source: Source, cost: Option<CostRange>) -> Finding {
        Finding {
            title: "Exhaust filter clogging".into(),
            severity: Severity::Serious,
            plain_english: "The filter that catches soot is blocked.".into(),
            source,
            evidence: vec!["P2463 confirmed".into()],
            evidence_refs: vec![103],
            what_to_do: None,
            estimated_cost: cost,
        }
    }

    fn cost() -> CostRange {
        CostRange {
            low: 800.0,
            high: 2500.0,
            currency: "USD".into(),
            basis: "general knowledge".into(),
        }
    }

    #[test]
    fn a_costed_finding_cannot_stay_labelled_as_measured() {
        let mut r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![finding(Source::Measured, Some(cost()))],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        r.normalise();
        // The code was measured; the price was not.
        assert_eq!(r.findings[0].source, Source::ModelKnowledge);
    }

    #[test]
    fn predictions_are_always_unverified_and_carry_no_evidence() {
        let mut r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![],
            watch_items: vec![finding(Source::Measured, None)],
            not_checked: vec![],
            next_steps: vec![],
        };
        r.normalise();
        assert_eq!(r.watch_items[0].source, Source::ModelKnowledge);
        // Nothing has been observed failing in the future.
        assert!(r.watch_items[0].evidence_refs.is_empty());
    }

    #[test]
    fn a_measured_finding_without_a_reference_is_not_evidenced() {
        let mut f = finding(Source::Measured, None);
        assert!(f.is_evidenced());
        f.evidence_refs.clear();
        assert!(!f.is_evidenced());
    }

    #[test]
    fn severity_sorts_worst_first() {
        let mut mild = finding(Source::Measured, None);
        mild.severity = Severity::Info;
        let mut bad = finding(Source::Measured, None);
        bad.severity = Severity::Critical;
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![mild, bad],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        assert_eq!(r.by_severity()[0].severity, Severity::Critical);
    }

    #[test]
    fn cost_band_totals_a_range_never_a_figure() {
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![
                finding(Source::ModelKnowledge, Some(cost())),
                finding(Source::ModelKnowledge, Some(cost())),
            ],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        let (low, high, cur) = r.cost_band().unwrap();
        assert_eq!((low, high), (1600.0, 5000.0));
        assert_eq!(cur, "USD");
    }

    #[test]
    fn a_verdict_with_nothing_itemised_is_rejected() {
        // Both local models tested did exactly this: a sensible verdict and
        // summary, with an empty findings list.
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![],
            watch_items: vec![finding(Source::ModelKnowledge, None)],
            not_checked: vec![],
            next_steps: vec![],
        };
        let why = r.incoherence(0).expect("an empty negotiate report should be rejected");
        assert!(why.contains("watch_items"));
    }

    #[test]
    fn a_report_citing_codes_the_vehicle_never_reported_is_rejected() {
        // Exactly what qwen3:8b produced once it was pushed to itemise: four
        // plausible generic codes, none of them on this truck.
        let invented = |code: &str| Finding {
            title: format!("Trouble code {code} detected"),
            severity: Severity::Caution,
            plain_english: format!("Code {code} suggests a problem."),
            source: Source::Measured,
            evidence: vec![],
            evidence_refs: vec![],
            what_to_do: None,
            estimated_cost: None,
        };
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![invented("P0420"), invented("P0300")],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        let real: std::collections::BTreeSet<String> =
            ["P2463", "P242F", "P2002"].iter().map(|s| s.to_string()).collect();

        let why = r.incoherence_against(&real).expect("invented codes must be rejected");
        assert!(why.contains("P0300") && why.contains("P0420"));
        // And it is told what was actually read, so it can correct itself.
        assert!(why.contains("P2463"));
    }

    #[test]
    fn real_codes_pass_the_fabrication_check() {
        let f = Finding {
            title: "Exhaust filter clogging".into(),
            severity: Severity::Serious,
            plain_english: "P2463 means soot has built up.".into(),
            source: Source::Measured,
            evidence: vec!["P2463 confirmed".into()],
            evidence_refs: vec![103],
            what_to_do: None,
            estimated_cost: None,
        };
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![f],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        let real: std::collections::BTreeSet<String> =
            ["P2463", "P242F"].iter().map(|s| s.to_string()).collect();
        assert!(r.incoherence_against(&real).is_none());
    }

    #[test]
    fn code_extraction_does_not_fire_on_ordinary_prose() {
        assert!(extract_codes("the filter is clogged and needs attention").is_empty());
        assert!(extract_codes("call 0300 for support").is_empty());
        // A hex-shaped code with a letter still counts.
        assert_eq!(extract_codes("code P242F was stored"), vec!["P242F"]);
        // A longer run is not a code.
        assert!(extract_codes("part number P12345X").is_empty());
    }

    #[test]
    fn codes_read_from_the_vehicle_must_appear_in_the_findings() {
        // qwen3:8b headlined "serious issue requiring attention", then set
        // verdict `inconclusive` with no findings — sliding under a rule keyed
        // on the verdict. Evidence has to beat the verdict.
        let r = Report {
            verdict: Verdict::Inconclusive,
            headline: "Exhaust filter restriction detected".into(),
            summary: "s".into(),
            findings: vec![],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        let why = r.incoherence(3).expect("three codes and no findings should be rejected");
        assert!(why.contains("3 trouble code"));
        // With nothing read, the same report is an honest non-answer.
        assert!(r.incoherence(0).is_none());
    }

    #[test]
    fn an_empty_inconclusive_report_is_allowed() {
        // "I could not read enough to tell you" is an honest answer with
        // nothing to itemise, and must not be argued with.
        let r = Report {
            verdict: Verdict::Inconclusive,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![],
            watch_items: vec![],
            not_checked: vec!["the vehicle did not answer".into()],
            next_steps: vec![],
        };
        assert!(r.incoherence(0).is_none());
    }

    #[test]
    fn a_populated_report_passes() {
        let r = Report {
            verdict: Verdict::Negotiate,
            headline: "h".into(),
            summary: "s".into(),
            findings: vec![finding(Source::Measured, None)],
            watch_items: vec![],
            not_checked: vec![],
            next_steps: vec![],
        };
        assert!(r.incoherence(0).is_none());
    }

    #[test]
    fn the_schema_forces_a_source_on_every_finding() {
        let s = submit_report_schema();
        let req = s["properties"]["findings"]["items"]["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "source"));
        assert!(req.iter().any(|v| v == "plain_english"));
    }
}

#[cfg(test)]
mod confidence_tests {
    use super::*;

    fn finding(source: Source, severity: Severity, refs: Vec<i64>) -> Finding {
        Finding {
            title: "t".into(),
            severity,
            plain_english: "p".into(),
            source,
            evidence: Vec::new(),
            evidence_refs: refs,
            what_to_do: None,
            estimated_cost: None,
        }
    }

    /// The rule the whole type exists to enforce: a model cannot state its own
    /// confidence, because there is nowhere on the type to put one.
    #[test]
    fn confidence_is_derived_and_not_settable_by_the_model() {
        let json =
            serde_json::to_string(&finding(Source::Measured, Severity::Info, vec![7])).unwrap();
        assert!(
            !json.contains("confidence"),
            "the serialised finding must have no confidence field for a model to fill in: {json}"
        );
    }

    #[test]
    fn measured_with_a_raw_exchange_is_high() {
        let c = finding(Source::Measured, Severity::Serious, vec![42]).confidence();
        assert_eq!(c.class, ConfidenceClass::High);
        assert_eq!(c.basis, vec![ConfidenceBasis::MeasuredThisSession]);
    }

    /// The case worth catching. "Measured" means there is an exchange you can
    /// go and look at; a claim of measurement with nothing to point at is not
    /// a measurement, and must not inherit its authority.
    #[test]
    fn claiming_measured_without_evidence_does_not_get_measured_confidence() {
        let c = finding(Source::Measured, Severity::Critical, vec![]).confidence();
        assert_eq!(c.class, ConfidenceClass::Low);
        assert_eq!(c.basis, vec![ConfidenceBasis::ClaimedMeasuredWithoutEvidence]);
    }

    #[test]
    fn the_other_sources_land_where_their_evidence_puts_them() {
        for (source, expected) in [
            (Source::IndirectlyMeasured, ConfidenceClass::Moderate),
            (Source::Catalog, ConfidenceClass::Moderate),
            (Source::Profile, ConfidenceClass::Low),
            (Source::ModelKnowledge, ConfidenceClass::Low),
            (Source::Unknown, ConfidenceClass::None),
        ] {
            let c = finding(source, Severity::Info, vec![1]).confidence();
            assert_eq!(c.class, expected, "{source:?}");
        }
    }

    /// Severity and confidence are independent axes, and the combination that
    /// matters most is the one a single "how serious is this" number destroys:
    /// a potentially critical problem that the evidence does not establish.
    #[test]
    fn a_critical_finding_can_be_low_confidence_and_says_both() {
        let f = finding(Source::ModelKnowledge, Severity::Critical, vec![]);
        assert_eq!(f.severity, Severity::Critical, "how bad it would be");
        assert_eq!(f.confidence().class, ConfidenceClass::Low, "how sure we are");
        assert!(!f.is_evidenced());
    }

    /// And the reverse: something certain and unimportant.
    #[test]
    fn an_informational_finding_can_be_high_confidence() {
        let f = finding(Source::Measured, Severity::Info, vec![3]);
        assert_eq!(f.severity, Severity::Info);
        assert_eq!(f.confidence().class, ConfidenceClass::High);
    }
}
