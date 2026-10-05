//! A prose answer that says "no faults", checked against what was read.
//!
//! The inspection report has a schema, so its claims are checked exactly:
//! `submit_report` is refused until the trouble codes have been read, and a
//! report citing a code the vehicle never reported is handed back. A chat
//! answer is prose, and nothing checked it.
//!
//! Reproduced on 2026-10-05 against the simulated F-250, with P2463, P242F and
//! P2002 in its engine module. Asked whether it was safe to drive home, the
//! assistant made one read, the full scan, which holds each module's UDS fault
//! memory and none of the emissions codes. It answered "Nothing in the engine
//! module, nothing in the body module. No engine or airbag faults anywhere."
//! It reported on codes it never read.
//!
//! There are two halves here, kept apart because they are different kinds of
//! knowledge:
//!
//! * [`FaultReads`] is exact. It is built from the tool results of one run and
//!   knows which modules had their trouble codes read in it.
//! * [`says_no_faults`] is not exact and cannot be. It looks for the ways
//!   English says "there are no faults", and it errs towards finding one: a
//!   sentence caught wrongly costs one cheap read, and one let through wrongly
//!   tells somebody their vehicle is clean without looking.
//!
//! What the two guarantee together is narrow. An answer recognised as saying
//! "no faults" does not reach a person on the strength of a read that did not
//! happen. An answer phrased some way this does not recognise still can, which
//! is why the full scan's own result also says what it did not ask.

use serde_json::Value;
use std::collections::BTreeSet;

/// What one run has read about faults.
///
/// One run, not one conversation. Only prose comes back from the client
/// between turns, so the tool results of an earlier turn are not in front of
/// the model and a read made then is not evidence now.
#[derive(Debug, Clone, Default)]
pub struct FaultReads {
    /// Modules `read_dtcs` asked for their trouble codes.
    asked: BTreeSet<String>,
    /// Set when a trouble-code read did not say which modules it asked. It is
    /// then taken to have asked all of them rather than none.
    asked_without_saying_which: bool,
    /// Modules a full scan listed without asking for their emissions codes.
    left_unasked: BTreeSet<String>,
    /// Whether a full scan ran.
    scanned: bool,
}

impl FaultReads {
    /// Take in the result of one tool call that succeeded.
    pub fn note(&mut self, tool: &str, result: &Value) {
        let data = result.get("data");
        match tool {
            "read_dtcs" => match data.and_then(|d| d.get("modules_read")).and_then(Value::as_array)
            {
                Some(modules) => {
                    self.asked.extend(modules.iter().filter_map(Value::as_str).map(String::from))
                }
                None => self.asked_without_saying_which = true,
            },
            "scan_all_modules" => {
                self.scanned = true;
                let modules = data.and_then(|d| d.get("modules")).and_then(Value::as_array);
                for module in modules.into_iter().flatten() {
                    let Some(key) = module.get("module_key").and_then(Value::as_str) else {
                        continue;
                    };
                    match module.get("emissions_codes_read").and_then(Value::as_bool) {
                        // On a pre-CAN vehicle the full scan is the read of
                        // the legislated services, and says so.
                        Some(true) => {
                            self.asked.insert(key.to_string());
                        }
                        Some(false) => {
                            self.left_unasked.insert(key.to_string());
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The modules an absence of faults cannot be claimed for, when there are
    /// any. `Some` of an empty list means no trouble codes were read at all.
    fn unread(&self) -> Option<Vec<String>> {
        if self.asked_without_saying_which {
            return None;
        }
        let missing: Vec<String> = self.left_unasked.difference(&self.asked).cloned().collect();
        // A scan that left nothing unasked covered what it found.
        let read_something = !self.asked.is_empty() || self.scanned;
        match missing.is_empty() && read_something {
            true => None,
            false => Some(missing),
        }
    }

    /// Why `answer` cannot go to a person as it is, if it cannot: it says
    /// there are no faults, and the trouble codes that would show it were not
    /// read in this run.
    pub fn unread_claim(&self, answer: &str) -> Option<UnreadClaim> {
        let modules = self.unread()?;
        let said = says_no_faults(answer)?;
        Some(UnreadClaim { said, modules })
    }
}

/// An answer that says there are no faults without the read behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadClaim {
    /// The sentence that says it.
    pub said: String,
    /// Modules whose trouble codes were not read. Empty when none were read
    /// from any module.
    pub modules: Vec<String>,
}

impl UnreadClaim {
    /// What the model is told when its answer is handed back.
    pub fn handed_back(&self) -> String {
        let (which, how) = match self.modules.is_empty() {
            true => (
                String::from("any trouble codes"),
                String::from("Call `read_dtcs` with no module, which reads every module"),
            ),
            false => (
                format!("the trouble codes of {}", self.modules.join(", ")),
                String::from("Call `read_dtcs` for them, or with no module to read every module"),
            ),
        };
        format!(
            "That answer was not shown to the person. It says \"{}\", and you have not read \
             {which} in this turn, so you do not know that. A full scan reads each module's fault \
             memory and does not include the stored, pending and permanent emissions codes: an \
             empty list there for the engine is not an engine with no codes. Nothing said earlier \
             in the conversation counts either, only a read made now. {how}, then answer again \
             from what it returns. If the codes cannot be read, say that instead of saying there \
             are none.",
            self.said
        )
    }

    /// What the person is told when the claim went out anyway.
    ///
    /// The application's words, not the model's, and put before the answer
    /// rather than after it so the claim is not read first.
    pub fn correction(&self) -> String {
        let which = match self.modules.is_empty() {
            true => String::from("no trouble codes were read for it"),
            false => {
                format!("the trouble codes of {} were not read for it", self.modules.join(", "))
            }
        };
        format!(
            "A note from the app, not the assistant: the answer below says there are no faults, \
             but {which}. Treat that part as not known until the codes are read, here or on the \
             Codes screen."
        )
    }
}

/// The first sentence of `text` that says there are no faults, if one does.
///
/// A sentence that only supposes it ("if there are no codes"), asks it, or
/// says it is not known ("I cannot say there are no faults") is not one.
pub fn says_no_faults(text: &str) -> Option<String> {
    sentences(text).into_iter().find_map(|(sentence, asked)| {
        if asked {
            return None;
        }
        let owned = words(sentence);
        let t: Vec<&str> = owned.iter().map(String::as_str).collect();
        let claims = (0..t.len()).any(|i| says_none_at(&t, i) && !supposed_or_doubted(&t[..i]));
        claims.then(|| quoted(sentence))
    })
}

/// A sentence as it will be quoted back: one line, and not a paragraph.
fn quoted(sentence: &str) -> String {
    let one_line = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line = one_line.trim_matches(|c: char| !c.is_alphanumeric()).replace('"', "'");
    match one_line.char_indices().nth(200) {
        Some((cut, _)) => format!("{}...", &one_line[..cut]),
        None => one_line,
    }
}

/// `text` as sentences, each with whether it ended in a question mark.
fn sentences(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (n, &(at, c)) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?' | ';' | '\n') {
            continue;
        }
        // "14.1 V" is one number, not the end of a sentence.
        let digit = |m: Option<&(usize, char)>| m.is_some_and(|(_, c)| c.is_ascii_digit());
        if c == '.' && digit(n.checked_sub(1).and_then(|p| chars.get(p))) && digit(chars.get(n + 1))
        {
            continue;
        }
        out.push((&text[start..at], c == '?'));
        start = at + c.len_utf8();
    }
    out.push((&text[start..], false));
    out
}

/// A sentence as lower-case words, with a `,` wherever a clause was marked off.
fn words(sentence: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let end = |word: &mut String, out: &mut Vec<String>| {
        let done = word.trim_end_matches('\'');
        if !done.is_empty() {
            out.push(done.to_string());
        }
        word.clear();
    };
    for c in sentence.chars() {
        let c = if c == '\u{2019}' { '\'' } else { c };
        if c.is_alphanumeric() || (c == '\'' && !word.is_empty()) {
            word.extend(c.to_lowercase());
            continue;
        }
        end(&mut word, &mut out);
        if matches!(c, ',' | ':' | '(' | ')' | '\u{2014}' | '\u{2013}') {
            out.push(String::from(","));
        }
    }
    end(&mut word, &mut out);
    out
}

fn names_a_fault(word: &str) -> bool {
    matches!(
        word,
        "fault"
            | "faults"
            | "code"
            | "codes"
            | "dtc"
            | "dtcs"
            | "problems"
            | "issues"
            | "errors"
            | "malfunctions"
    )
}

/// Words that end the thing "no" was said of before a fault was named: "no way
/// to read the codes" and "no description for this code" are about something
/// else.
const SOMETHING_ELSE: &[&str] = &[
    "to",
    "for",
    "that",
    "which",
    "who",
    "this",
    "these",
    "those",
    "the",
    "a",
    "an",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "than",
    "about",
    "from",
    "with",
    "in",
    "on",
    "at",
    "by",
    "if",
    "when",
    "because",
    "but",
    "so",
    "it",
    "its",
    "i",
    "you",
    "we",
    "they",
    "there",
    "here",
    "way",
    "need",
    "reason",
    "longer",
    "idea",
    "doubt",
    "matter",
    "description",
    "one",
];

/// Whether a fault is named within `reach` words from `from`, before the
/// subject changes.
fn a_fault_follows(t: &[&str], from: usize, reach: usize) -> bool {
    t.iter()
        .skip(from)
        .filter(|w| **w != ",")
        .take(reach)
        .find_map(|w| match (names_a_fault(w), SOMETHING_ELSE.contains(w)) {
            (true, _) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        })
        .unwrap_or(false)
}

fn negates(word: &str) -> bool {
    matches!(word, "not" | "never" | "cannot") || word.ends_with("n't")
}

/// Whether the words before a claim make it a supposition or a doubt.
fn supposed_or_doubted(before: &[&str]) -> bool {
    const SUPPOSES: &[&str] =
        &["if", "unless", "whether", "assuming", "suppose", "supposing", "until", "would"];
    // "cannot say", "does not mean", "have not read": what follows is not
    // being asserted.
    const NOT_ASSERTED: &[&str] = &[
        "say",
        "tell",
        "know",
        "mean",
        "sure",
        "confirm",
        "claim",
        "promise",
        "guarantee",
        "conclude",
        "assume",
        "same",
        "read",
        "check",
        "checked",
        "looked",
        "scanned",
        "asked",
        "yet",
    ];
    before.iter().any(|w| SUPPOSES.contains(w))
        || before.iter().enumerate().any(|(i, w)| {
            negates(w) && before.iter().skip(i + 1).take(3).any(|w| NOT_ASSERTED.contains(w))
        })
}

/// Whether the word at `i` begins a statement that there are no faults.
fn says_none_at(t: &[&str], i: usize) -> bool {
    let next = t.get(i + 1).copied().unwrap_or("");
    let previous = i.checked_sub(1).and_then(|p| t.get(p)).copied().unwrap_or("");
    match t[i] {
        // "no stored codes", "zero faults", "without any trouble codes". A
        // comma straight after is "No, ..." and not a count of anything.
        "no" | "zero" | "0" | "without" => next != "," && a_fault_follows(t, i + 1, 6),
        "nothing" => nothing_is_there(t, i, previous, next),
        "none" => none_of_them(t, i, next),
        "free" | "clear" if next == "of" => a_fault_follows(t, i + 2, 4),
        // "fault-free", "all clear".
        "free" => matches!(previous, "fault" | "code" | "trouble"),
        "clear" => previous == "all",
        "clean" => {
            next == "bill"
                || matches!(
                    previous,
                    "is" | "are"
                        | "was"
                        | "were"
                        | "back"
                        | "looks"
                        | "look"
                        | "looked"
                        | "reads"
                        | "came"
                        | "comes"
                        | "come"
                        | "completely"
                        | "entirely"
                        | "perfectly"
                        | "all"
                )
        }
        // "did not find any codes", "is not showing any faults". Not "have not
        // read any codes", which is the opposite of a claim.
        word if negates(word) => {
            const NOT_A_FINDING: &[&str] = &[
                "read", "reading", "check", "checked", "checking", "scan", "scanned", "scanning",
                "look", "looked", "looking", "ask", "asked", "asking", "able", "clear", "cleared",
                "clearing",
            ];
            let ahead: Vec<&str> = t.iter().skip(i + 1).take(5).copied().collect();
            ahead.iter().position(|w| *w == "any").is_some_and(|any| {
                !ahead[..any].iter().any(|w| *w == "," || NOT_A_FINDING.contains(w))
                    && a_fault_follows(t, i + any + 2, 5)
            })
        }
        _ => false,
    }
}

/// "Nothing stored", "nothing in the engine module", "found nothing".
fn nothing_is_there(t: &[&str], i: usize, previous: &str, next: &str) -> bool {
    const SAID_OF_IT: &[&str] = &[
        "stored", "wrong", "logged", "recorded", "flagged", "found", "failing", "active",
        "pending", "set",
    ];
    const BETWEEN: &[&str] = &["is", "was", "has", "been", "currently"];
    const PLACES: &[&str] = &[
        "module",
        "modules",
        "controller",
        "controllers",
        "ecu",
        "ecus",
        "computer",
        "computers",
        "memory",
        "system",
        "systems",
        "engine",
        "body",
        "airbag",
        "airbags",
        "brake",
        "brakes",
        "transmission",
        "abs",
        "pcm",
        "bcm",
        "tcm",
        "vehicle",
        "car",
        "truck",
    ];
    const FOUND_BY: &[&str] = &[
        "found",
        "find",
        "finds",
        "shows",
        "show",
        "showing",
        "reported",
        "reports",
        "reporting",
        "stored",
        "storing",
        "holds",
        "holding",
        "logged",
        "saw",
    ];
    let said_of_it = t
        .iter()
        .skip(i + 1)
        .take(3)
        .find(|w| !BETWEEN.contains(w))
        .is_some_and(|w| SAID_OF_IT.contains(w));
    let somewhere = matches!(next, "in" | "on" | "from" | "for" | "at")
        && t.iter().skip(i + 2).take(5).take_while(|w| **w != ",").any(|w| PLACES.contains(w));
    let to_report = next == "to" && matches!(t.get(i + 2).copied(), Some("report" | "flag"));
    said_of_it || somewhere || to_report || FOUND_BY.contains(&previous)
}

/// "None stored", "stored codes: none", "none of the modules reported a fault".
fn none_of_them(t: &[&str], i: usize, next: &str) -> bool {
    const SAID_OF_THEM: &[&str] = &[
        "stored", "found", "present", "logged", "recorded", "set", "reported", "pending", "active",
        "showing", "at",
    ];
    const HOLDS: &[&str] = &[
        "reported", "report", "reports", "has", "have", "had", "show", "shows", "showed",
        "showing", "hold", "holds", "holding", "store", "stores", "stored", "logged", "carries",
        "returned", "returns",
    ];
    let of_faults_just_named =
        t[..i].iter().rev().filter(|w| **w != ",").take(3).any(|w| names_a_fault(w));
    let of_modules_holding_none = next == "of"
        && t.iter().enumerate().skip(i + 2).take(5).any(|(at, w)| {
            HOLDS.contains(w) && t.iter().skip(at + 1).take(4).any(|w| names_a_fault(w))
        });
    SAID_OF_THEM.contains(&next) || of_faults_just_named || of_modules_holding_none
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The answer that was given, word for word, and other ways of saying it.
    #[test]
    fn the_ways_an_answer_says_there_are_no_faults_are_recognised() {
        for answer in [
            "Nothing in the engine module, nothing in the body module. No engine or airbag \
             faults anywhere.",
            "No engine or airbag faults anywhere.",
            "There are no stored, pending or permanent codes.",
            "I read every module and there are no stored codes anywhere.",
            "No active diagnostic trouble codes were detected during the scan.",
            "The engine module has zero faults.",
            "The ABS light is real, but the engine is free of any stored codes.",
            "It didn't find any trouble codes in the engine.",
            "The engine isn\u{2019}t showing any faults.",
            "The engine module reported nothing.",
            "Nothing is stored in the powertrain controller.",
            "Stored codes: none.",
            "None of the modules reported a fault.",
            "The engine module came back clean.",
            "**Engine:** no DTCs.",
            "- No codes stored\n- Battery 14.1 V, which is normal",
            "That means there are no engine faults, so it is safe to drive.",
        ] {
            assert!(says_no_faults(answer).is_some(), "not recognised: {answer}");
        }
    }

    /// Supposing it, asking it, or saying it is not known is not claiming it,
    /// and neither is a "no" about something other than faults.
    #[test]
    fn a_sentence_that_does_not_claim_it_is_left_alone() {
        for answer in [
            "The engine module has P2463 stored and P2002 pending.",
            "If there are no codes, a shake like that is usually mechanical.",
            "I cannot say there are no faults, because the codes were not read.",
            "I have not read any codes yet.",
            "An empty list there does not mean no faults.",
            "Are there no warning lights besides the ABS one?",
            "No, that code is about the exhaust filter.",
            "There is no description for this code in this build.",
            "There is no way to clear codes from here.",
            "No problem. Which light is on?",
            "A clean scan is not a clean car.",
            "The vehicle did not answer, so I could not read it.",
            "Coolant is at 88.5 \u{b0}C, which is normal running temperature.",
        ] {
            assert_eq!(says_no_faults(answer), None, "wrongly caught: {answer}");
        }
    }

    /// It is quoted back to the model, so it has to be the sentence and not
    /// the whole answer.
    #[test]
    fn the_sentence_that_says_it_is_the_one_quoted() {
        let said = says_no_faults("The brake module has C0035. No engine faults. Drive gently.");
        assert_eq!(said.as_deref(), Some("No engine faults"));
    }

    fn full_scan() -> Value {
        json!({ "data": { "modules": [
            { "module_key": "ECU_7E8", "fault_count": 0, "emissions_codes_read": false },
            { "module_key": "ECU_7EA", "fault_count": 0, "emissions_codes_read": false },
            { "module_key": "ECU_768", "fault_count": 2, "emissions_codes_read": null },
        ] } })
    }

    const CLAIM: &str = "No engine faults.";

    #[test]
    fn a_full_scan_alone_does_not_stand_behind_no_faults() {
        let mut reads = FaultReads::default();
        reads.note("scan_all_modules", &full_scan());

        let claim = reads.unread_claim(CLAIM).expect("the emissions codes were not read");
        assert_eq!(claim.modules, vec!["ECU_7E8", "ECU_7EA"]);
        assert!(claim.handed_back().contains("`read_dtcs`"));
        assert!(claim.handed_back().contains("ECU_7E8"));
        assert!(claim.correction().contains("ECU_7E8"));
        // What it found is still its to report.
        assert_eq!(reads.unread_claim("The module at 768 has two faults stored."), None);
    }

    #[test]
    fn nothing_read_at_all_stands_behind_nothing() {
        let claim = FaultReads::default().unread_claim(CLAIM).expect("no read was made");
        assert!(claim.modules.is_empty());
        assert!(claim.handed_back().contains("any trouble codes"));
    }

    #[test]
    fn reading_the_trouble_codes_is_what_stands_behind_it() {
        let mut reads = FaultReads::default();
        reads.note("scan_all_modules", &full_scan());
        reads.note(
            "read_dtcs",
            &json!({ "data": { "dtcs": [], "modules_read": ["ECU_7E8", "ECU_7EA", "ECU_768"] } }),
        );
        assert_eq!(reads.unread_claim(CLAIM), None);

        // Without a full scan too: the codes were asked for.
        let mut reads = FaultReads::default();
        reads.note("read_dtcs", &json!({ "data": { "dtcs": [], "modules_read": ["ECU_7E8"] } }));
        assert_eq!(reads.unread_claim(CLAIM), None);
    }

    /// Reading one module is not reading the engine.
    #[test]
    fn a_read_of_another_module_does_not_cover_the_engine() {
        let mut reads = FaultReads::default();
        reads.note("scan_all_modules", &full_scan());
        reads.note("read_dtcs", &json!({ "data": { "dtcs": [], "modules_read": ["ECU_7EA"] } }));

        let claim = reads.unread_claim(CLAIM).expect("the engine module was not read");
        assert_eq!(claim.modules, vec!["ECU_7E8"]);
    }

    /// On a pre-CAN vehicle the full scan is the read of the legislated
    /// services, and its result says so.
    #[test]
    fn a_full_scan_that_read_the_emissions_codes_counts_as_reading_them() {
        let mut reads = FaultReads::default();
        reads.note(
            "scan_all_modules",
            &json!({ "data": { "modules": [
                { "module_key": "ECU_10", "fault_count": 0, "emissions_codes_read": true },
            ] } }),
        );
        assert_eq!(reads.unread_claim(CLAIM), None);
    }
}
