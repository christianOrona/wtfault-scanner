//! The Mechanic council: several models answering one question together.
//!
//! One model reading a fault is one opinion. The council asks more than one,
//! over the **same** evidence:
//!
//! 1. **Evidence.** The lead (the selected provider) runs the ordinary agent
//!    loop: it reads what it needs from the vehicle through the safety-gated
//!    tools and drafts an answer. The adapter is read once.
//! 2. **Opinions.** Every other member is shown exactly what the lead read and
//!    its draft, and writes its own diagnosis. Members have **no tools**: they
//!    cannot reach the vehicle, so they cannot bring in a reading nobody can
//!    trace, and a member that is poor at tool calling still contributes.
//! 3. **Merge.** The lead writes the answer the person sees: where the council
//!    agrees, where it does not, and what would settle it.
//!
//! The merged answer is held to the same "no faults without a read" check as a
//! single model's, against the reads the lead actually made. A member that
//! fails is named and skipped; the person still gets an answer.

use crate::error::AgentError;
use crate::provider::{ChatRequest, Content, LlmProvider, Message, Role, Usage};
use crate::run::{Agent, AgentEvent, AgentOutcome, EventSink};
use crate::tools::ToolExecutor;
use serde::Serialize;

/// Longest a single tool result is shown to members, in characters.
const RESULT_CHARS: usize = 4_000;
/// Longest the whole evidence section may be. A small local model handed 200 KB
/// of hex stops reasoning about any of it.
const EVIDENCE_CHARS: usize = 40_000;

/// What one member said.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Opinion {
    /// The member's configured name.
    pub member: String,
    /// The model that answered, as the provider reports it.
    pub model: String,
    /// Its opinion, when it gave one.
    pub text: Option<String>,
    /// Why it did not, when it did not.
    pub error: Option<String>,
}

/// What a council run produced.
#[derive(Debug, Clone)]
pub struct CouncilOutcome {
    /// The lead's run, with `text` replaced by the merged answer.
    pub outcome: AgentOutcome,
    /// The lead's own answer before the council saw it.
    pub draft: String,
    /// Each member's opinion, in the order configured.
    pub opinions: Vec<Opinion>,
    /// Whether the answer is a merge. False when no member answered, or the
    /// merge itself failed and the draft was used instead.
    pub merged: bool,
}

/// A lead and the members who answer alongside it.
pub struct Council<'a> {
    lead: &'a dyn LlmProvider,
    members: Vec<&'a dyn LlmProvider>,
    max_steps: Option<usize>,
    max_tokens: u32,
}

impl<'a> Council<'a> {
    /// A council over `lead`, with `members` giving second opinions.
    pub fn new(lead: &'a dyn LlmProvider, members: Vec<&'a dyn LlmProvider>) -> Self {
        Council { lead, members, max_steps: None, max_tokens: crate::DEFAULT_MAX_TOKENS }
    }

    /// The lead's step limit for gathering evidence.
    pub fn with_max_steps(mut self, steps: Option<usize>) -> Self {
        self.max_steps = steps;
        self
    }

    /// Output ceiling for every turn.
    pub fn with_max_tokens(mut self, tokens: Option<u32>) -> Self {
        if let Some(t) = tokens {
            self.max_tokens = t;
        }
        self
    }

    /// Answer one chat turn as a council.
    pub async fn chat(
        &self,
        system: &str,
        history: Vec<Message>,
        executor: &mut dyn ToolExecutor,
        sink: &mut dyn EventSink,
    ) -> Result<CouncilOutcome, AgentError> {
        // 1. Evidence, by the ordinary loop. A failure here is the run's
        //    failure: with no evidence there is nothing to convene over.
        let mut agent = Agent::new(self.lead).with_max_tokens(self.max_tokens);
        if let Some(n) = self.max_steps {
            agent = agent.with_max_steps(n);
        }
        let earlier = history.len();
        let mut outcome = agent.chat(system, history.clone(), executor, sink).await?;
        let draft = outcome.text.clone();
        if self.members.is_empty() || draft.trim().is_empty() {
            return Ok(CouncilOutcome { outcome, draft, opinions: Vec::new(), merged: false });
        }

        let conversation = conversation_of(&history);
        let evidence = evidence_of(&outcome.messages[earlier.min(outcome.messages.len())..]);

        // 2. Opinions, all at once. Members share nothing with each other at
        //    this stage, so one cannot anchor the rest.
        let member_system = format!("{system}\n\n{MEMBER_ROLE}");
        let ask = |m: &'a dyn LlmProvider| {
            let request = ChatRequest {
                system: member_system.clone(),
                messages: vec![Message::user(member_prompt(&conversation, &evidence, &draft))],
                tools: Vec::new(),
                max_tokens: self.max_tokens,
            };
            async move { (m, m.chat(&request).await) }
        };
        let answers = futures_util::future::join_all(self.members.iter().map(|m| ask(*m))).await;

        let mut opinions = Vec::new();
        for (member, answer) in answers {
            let opinion = match answer {
                Ok(r) => {
                    add(&mut outcome.usage, r.usage);
                    let text = r.text();
                    if text.is_empty() {
                        Opinion {
                            member: member.label().into(),
                            model: r.model,
                            text: None,
                            error: Some("answered with nothing".into()),
                        }
                    } else {
                        Opinion {
                            member: member.label().into(),
                            model: r.model,
                            text: Some(text),
                            error: None,
                        }
                    }
                }
                Err(e) => Opinion {
                    member: member.label().into(),
                    model: member.model().into(),
                    text: None,
                    error: Some(e.to_string()),
                },
            };
            sink.emit(AgentEvent::CouncilOpinion {
                member: opinion.member.clone(),
                success: opinion.text.is_some(),
            });
            opinions.push(opinion);
        }

        let heard: Vec<&Opinion> = opinions.iter().filter(|o| o.text.is_some()).collect();
        if heard.is_empty() {
            return Ok(CouncilOutcome { outcome, draft, opinions, merged: false });
        }

        // 3. Merge, by the lead, still with no tools: everything it may cite
        //    is already in front of it.
        let request = ChatRequest {
            system: format!("{system}\n\n{MERGE_ROLE}"),
            messages: vec![Message::user(merge_prompt(&conversation, &evidence, &draft, &heard))],
            tools: Vec::new(),
            max_tokens: self.max_tokens,
        };
        let merged = match self.lead.chat(&request).await {
            Ok(r) => {
                add(&mut outcome.usage, r.usage);
                Some(r.text()).filter(|t| !t.is_empty())
            }
            // The draft is a complete answer already. Losing the merge costs
            // the second opinions, not the answer.
            Err(e) => {
                tracing::warn!(error = %e, "council merge failed; answering with the lead's draft");
                None
            }
        };
        let Some(mut text) = merged else {
            return Ok(CouncilOutcome { outcome, draft, opinions, merged: false });
        };

        // The merge is new prose. Members may have assumed something the lead
        // never read; the person is told before the claim, not after it.
        if let Some(claim) = outcome.fault_reads.unread_claim(&text) {
            text = format!("{}\n\n{text}", claim.correction());
        }
        outcome.text = text.clone();
        if let Some(last) = outcome.messages.iter_mut().rev().find(|m| m.role == Role::Assistant) {
            *last = Message::assistant(text);
        }
        Ok(CouncilOutcome { outcome, draft, opinions, merged: true })
    }
}

fn add(total: &mut Usage, more: Usage) {
    total.input_tokens += more.input_tokens;
    total.output_tokens += more.output_tokens;
    total.cache_read_tokens += more.cache_read_tokens;
}

/// What a member is told about its job.
const MEMBER_ROLE: &str = "# This turn: you are on a Mechanic council\n\n\
Another assistant has already read the vehicle and drafted an answer. You are a second mechanic \
giving your own opinion on the same evidence. In this turn you have **no tools**: you cannot read \
the vehicle, and any instruction above about calling tools does not apply.\n\n\
- Base every claim on the evidence shown. If you rely on general knowledge, say so.\n\
- Say plainly where you agree with the draft and where you do not, and why.\n\
- If something important was not read, name the read that would settle it.\n\
- Do not say the vehicle has no faults unless the evidence shows trouble codes were read and \
  none came back.\n\
- Be brief: a few short paragraphs at most.";

/// What the lead is told when merging.
const MERGE_ROLE: &str = "# This turn: write the council's answer\n\n\
You drafted an answer, and other mechanics have given their opinions on the same evidence. Write \
the one answer the person will read. In this turn you have **no tools**.\n\n\
- Lead with what the council agrees on, in plain words.\n\
- If anyone disagrees on something that matters, say what, who, and what reading would settle it.\n\
- Keep every source label honest: a reading is measured only if it is in the evidence.\n\
- Do not mention this process or quote the opinions at length; the person can open them.\n\
- Do not say the vehicle has no faults unless the evidence shows trouble codes were read and \
  none came back.";

/// The prose of the conversation so far, oldest first.
fn conversation_of(history: &[Message]) -> String {
    let mut out = String::new();
    for m in history {
        let text = m.text();
        if text.trim().is_empty() {
            continue;
        }
        let who = match m.role {
            Role::User => "Person",
            Role::Assistant => "Assistant",
        };
        out.push_str(&format!("{who}: {}\n\n", text.trim()));
    }
    out
}

/// What the lead read this turn, as text a member can cite.
fn evidence_of(turn: &[Message]) -> String {
    let mut calls: std::collections::HashMap<&str, (&str, &serde_json::Value)> = Default::default();
    let mut out = String::new();
    for m in turn {
        for c in &m.content {
            match c {
                Content::ToolUse { id, name, input } => {
                    calls.insert(id.as_str(), (name.as_str(), input));
                }
                Content::ToolResult { tool_use_id, content, is_error } => {
                    let (name, input) = calls
                        .get(tool_use_id.as_str())
                        .copied()
                        .unwrap_or(("tool", &serde_json::Value::Null));
                    let state = if *is_error { "FAILED" } else { "ok" };
                    let shown: String = content.chars().take(RESULT_CHARS).collect();
                    let cut = if content.chars().count() > RESULT_CHARS { " …(cut)" } else { "" };
                    out.push_str(&format!("### {name} {input} — {state}\n{shown}{cut}\n\n"));
                }
                Content::Text { .. } => {}
            }
        }
        if out.chars().count() > EVIDENCE_CHARS {
            out = out.chars().take(EVIDENCE_CHARS).collect();
            out.push_str("\n…(more evidence was read than fits here)\n");
            break;
        }
    }
    if out.is_empty() {
        out.push_str("Nothing was read from the vehicle this turn.\n");
    }
    out
}

fn member_prompt(conversation: &str, evidence: &str, draft: &str) -> String {
    format!(
        "## Conversation\n\n{conversation}## What was read from the vehicle this turn\n\n{evidence}\
         ## The draft answer\n\n{draft}\n\n## Your opinion\n\nGive your own diagnosis and say \
         where you agree or disagree with the draft."
    )
}

fn merge_prompt(conversation: &str, evidence: &str, draft: &str, heard: &[&Opinion]) -> String {
    let mut out = format!(
        "## Conversation\n\n{conversation}## What was read from the vehicle this turn\n\n{evidence}\
         ## Your draft\n\n{draft}\n\n## The other mechanics\n\n"
    );
    for o in heard {
        out.push_str(&format!(
            "### {} ({})\n{}\n\n",
            o.member,
            o.model,
            o.text.as_deref().unwrap_or("")
        ));
    }
    out.push_str("## Now\n\nWrite the council's answer to the person's last message.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatResponse, ProviderInfo, StopReason, ToolSpec};
    use crate::run::NullSink;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// Replays turns and records requests.
    struct Scripted {
        name: &'static str,
        turns: Mutex<Vec<Result<ChatResponse, AgentError>>>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Scripted {
        fn new(name: &'static str, turns: Vec<Result<ChatResponse, AgentError>>) -> Self {
            Scripted { name, turns: Mutex::new(turns), seen: Mutex::new(vec![]) }
        }
    }

    fn say(t: &str) -> Result<ChatResponse, AgentError> {
        Ok(ChatResponse {
            model: "m".into(),
            content: vec![Content::text(t)],
            stop_reason: StopReason::EndTurn,
            usage: Usage { input_tokens: 1, output_tokens: 1, cache_read_tokens: 0 },
        })
    }

    fn call(name: &str) -> Result<ChatResponse, AgentError> {
        Ok(ChatResponse {
            model: "m".into(),
            content: vec![Content::ToolUse {
                id: "t1".into(),
                name: name.into(),
                input: json!({}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        })
    }

    #[async_trait::async_trait]
    impl LlmProvider for Scripted {
        fn id(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.name
        }
        fn model(&self) -> &str {
            "m"
        }
        async fn chat(&self, r: &ChatRequest) -> Result<ChatResponse, AgentError> {
            self.seen.lock().unwrap().push(r.clone());
            let mut t = self.turns.lock().unwrap();
            if t.is_empty() {
                return say("(script ended)");
            }
            t.remove(0)
        }
        async fn probe(&self) -> Result<ProviderInfo, AgentError> {
            Ok(ProviderInfo { reachable: true, models: vec![], elapsed_ms: 0, detail: None })
        }
    }

    /// Answers every read with three stored codes from the engine module.
    struct Truck;

    #[async_trait::async_trait]
    impl ToolExecutor for Truck {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "read_dtcs".into(),
                description: "Read trouble codes.".into(),
                input_schema: json!({"type": "object"}),
            }]
        }
        async fn run(&mut self, _name: &str, _args: &Value, _origin: &str) -> Value {
            json!({"success": true, "data": {"modules_read": ["7E0"],
                   "dtcs": [{"code": "P2463", "status": "confirmed"}]}})
        }
    }

    fn err() -> AgentError {
        AgentError::LocalAgent { provider: "Codex".into(), detail: "not signed in".into() }
    }

    #[tokio::test]
    async fn members_see_the_leads_evidence_and_draft_and_have_no_tools() {
        let lead = Scripted::new(
            "Claude Code",
            vec![call("read_dtcs"), say("Soot filter is loading up (P2463)."), say("MERGED")],
        );
        let member = Scripted::new("Codex", vec![say("Agree: P2463, regen it.")]);
        let council = Council::new(&lead, vec![&member]);
        let out = council
            .chat("sys", vec![Message::user("why is it smoky?")], &mut Truck, &mut NullSink)
            .await
            .unwrap();

        assert!(out.merged);
        assert_eq!(out.outcome.text, "MERGED");
        assert_eq!(out.draft, "Soot filter is loading up (P2463).");
        assert_eq!(out.opinions[0].text.as_deref(), Some("Agree: P2463, regen it."));

        let asked = &member.seen.lock().unwrap()[0];
        assert!(asked.tools.is_empty(), "a member can never reach the vehicle");
        let prompt = asked.messages[0].text();
        assert!(prompt.contains("P2463") && prompt.contains("read_dtcs"), "{prompt}");
        assert!(prompt.contains("Soot filter is loading up"), "{prompt}");
        assert!(prompt.contains("why is it smoky?"));

        let merge = lead.seen.lock().unwrap().last().cloned().unwrap();
        assert!(merge.tools.is_empty());
        assert!(merge.messages[0].text().contains("Agree: P2463"));
    }

    #[tokio::test]
    async fn a_failed_member_is_named_and_the_rest_still_merge() {
        let lead = Scripted::new("Lead", vec![say("Draft."), say("Merged.")]);
        let ok = Scripted::new("Ollama", vec![say("Second opinion.")]);
        let bad = Scripted::new("Codex", vec![Err(err())]);
        let out = Council::new(&lead, vec![&bad, &ok])
            .chat("sys", vec![Message::user("q")], &mut Truck, &mut NullSink)
            .await
            .unwrap();
        assert!(out.merged);
        assert_eq!(out.opinions.len(), 2);
        assert!(out.opinions[0].error.as_deref().unwrap().contains("not signed in"));
        assert_eq!(out.opinions[1].text.as_deref(), Some("Second opinion."));
    }

    #[tokio::test]
    async fn when_nobody_else_answers_the_draft_is_the_answer() {
        let lead = Scripted::new("Lead", vec![say("Draft stands.")]);
        let bad = Scripted::new("Codex", vec![Err(err())]);
        let out = Council::new(&lead, vec![&bad])
            .chat("sys", vec![Message::user("q")], &mut Truck, &mut NullSink)
            .await
            .unwrap();
        assert!(!out.merged);
        assert_eq!(out.outcome.text, "Draft stands.");
        assert_eq!(lead.seen.lock().unwrap().len(), 1, "no merge turn was spent");
    }

    #[tokio::test]
    async fn a_failed_merge_falls_back_to_the_draft() {
        let lead = Scripted::new("Lead", vec![say("Draft."), Err(err())]);
        let ok = Scripted::new("Ollama", vec![say("Opinion.")]);
        let out = Council::new(&lead, vec![&ok])
            .chat("sys", vec![Message::user("q")], &mut Truck, &mut NullSink)
            .await
            .unwrap();
        assert!(!out.merged);
        assert_eq!(out.outcome.text, "Draft.");
        assert_eq!(out.opinions.len(), 1);
    }

    /// The merge is held to the same rule as any answer: "no faults" needs the
    /// codes to have been read in this run.
    #[tokio::test]
    async fn a_merged_no_faults_claim_without_a_read_carries_the_apps_note() {
        let lead = Scripted::new(
            "Lead",
            vec![say("I have not read the codes yet."), say("There are no faults on this truck.")],
        );
        let member = Scripted::new("Codex", vec![say("Looks clean to me.")]);
        let out = Council::new(&lead, vec![&member])
            .chat("sys", vec![Message::user("anything wrong?")], &mut Truck, &mut NullSink)
            .await
            .unwrap();
        assert!(out.outcome.text.starts_with("A note from the app"), "{}", out.outcome.text);
    }

    #[tokio::test]
    async fn without_members_it_is_an_ordinary_turn() {
        let lead = Scripted::new("Lead", vec![say("Just me.")]);
        let out = Council::new(&lead, vec![])
            .chat("sys", vec![Message::user("q")], &mut Truck, &mut NullSink)
            .await
            .unwrap();
        assert_eq!(out.outcome.text, "Just me.");
        assert!(out.opinions.is_empty() && !out.merged);
    }

    #[test]
    fn evidence_is_capped_and_says_when_nothing_was_read() {
        assert!(evidence_of(&[]).contains("Nothing was read"));
        let big = "x".repeat(RESULT_CHARS * 2);
        let turn = vec![
            Message {
                role: Role::Assistant,
                content: vec![Content::ToolUse {
                    id: "a".into(),
                    name: "read_pid".into(),
                    input: json!({}),
                }],
            },
            Message::tool_results(vec![Content::ToolResult {
                tool_use_id: "a".into(),
                content: big,
                is_error: false,
            }]),
        ];
        let e = evidence_of(&turn);
        assert!(e.contains("read_pid") && e.contains("(cut)"));
        assert!(e.chars().count() < RESULT_CHARS + 200);
    }
}
