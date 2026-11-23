//! Tool calling over plain text, for models reached through a command line.
//!
//! An installed agent CLI (Claude Code, Codex, Gemini CLI) answers a prompt
//! with text. It has no wire format for "run this tool for me", so the request
//! is written into the prompt and the answer is read back out of the text:
//!
//! ```text
//! I will read the stored codes first.
//! <tool_calls>
//! [{"name": "read_dtcs", "input": {}}]
//! </tool_calls>
//! ```
//!
//! The parsed calls become ordinary [`Content::ToolUse`] blocks, so everything
//! downstream — schema validation, the safety gate, the step limit, the claim
//! checks — is the same code path a hosted model goes through. The CLI never
//! holds a route to the vehicle of its own.

use crate::provider::{ChatRequest, Content, Message, Role, ToolSpec};
use serde::Deserialize;
use serde_json::Value;

/// Opening tag of the block a model asks for tools in.
pub const OPEN: &str = "<tool_calls>";
/// Closing tag.
pub const CLOSE: &str = "</tool_calls>";

/// How the model is told to ask for tools. Kept short: it is sent every turn.
fn protocol(tools: &[ToolSpec]) -> String {
    if tools.is_empty() {
        return String::from(
            "# How to answer\n\nYou have no tools in this turn. Answer in plain prose. You cannot \
             read the vehicle, run commands or open files; use only what is written above.\n",
        );
    }
    let mut out = String::from(
        "# How to use tools\n\n\
         You cannot run anything yourself: no shell, no files, no web. The application runs \
         these tools for you against the vehicle and returns their results in the next message.\n\n\
         To call tools, end your reply with exactly one block like this, holding a JSON array \
         (several calls are allowed and run in order):\n\n\
         <tool_calls>\n\
         [{\"name\": \"tool_name\", \"input\": {\"argument\": \"value\"}}]\n\
         </tool_calls>\n\n\
         Write nothing after the block. When you have what you need, answer in plain prose \
         with no block at all; that ends your turn. Never invent a tool result: wait for it.\n\n\
         ## Tools\n",
    );
    for t in tools {
        out.push_str(&format!(
            "\n### {}\n{}\nArguments (JSON Schema): {}\n",
            t.name,
            t.description.trim(),
            serde_json::to_string(&t.input_schema).unwrap_or_else(|_| "{}".into())
        ));
    }
    out
}

/// Write a whole [`ChatRequest`] as one prompt.
///
/// Each CLI call is stateless, so the conversation is replayed in full every
/// turn. Tool results are written with the call they answer, so a model that
/// reads only the bottom of a long prompt still sees what came back.
pub fn render_prompt(request: &ChatRequest) -> String {
    let mut out = String::new();
    out.push_str("# Instructions\n\n");
    out.push_str(request.system.trim());
    out.push_str("\n\n");
    out.push_str(&protocol(&request.tools));
    out.push_str("\n# Conversation\n");
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in &request.messages {
        for c in &m.content {
            if let Content::ToolUse { id, name, .. } = c {
                names.insert(id.as_str(), name.as_str());
            }
        }
    }
    for m in &request.messages {
        render_message(&mut out, m, &names);
    }
    out.push_str("\n# Your turn\n\nReply as the assistant now.");
    if !request.tools.is_empty() {
        out.push_str(" Either call tools with one <tool_calls> block at the end, or answer.");
    }
    out.push('\n');
    out
}

fn render_message(out: &mut String, m: &Message, names: &std::collections::HashMap<&str, &str>) {
    for c in &m.content {
        match (m.role, c) {
            (Role::User, Content::Text { text }) => {
                out.push_str("\n## User\n");
                out.push_str(text.trim());
                out.push('\n');
            }
            (Role::Assistant, Content::Text { text }) => {
                out.push_str("\n## Assistant\n");
                out.push_str(text.trim());
                out.push('\n');
            }
            (_, Content::ToolUse { name, input, .. }) => {
                out.push_str(&format!(
                    "\n## Assistant called a tool\n{OPEN}\n[{{\"name\": {}, \"input\": {}}}]\n{CLOSE}\n",
                    Value::String(name.clone()),
                    input
                ));
            }
            (_, Content::ToolResult { tool_use_id, content, is_error }) => {
                let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                let state = if *is_error { "FAILED" } else { "ok" };
                out.push_str(&format!("\n## Tool result: {name} ({state})\n{content}\n"));
            }
        }
    }
}

#[derive(Deserialize)]
struct WireCall {
    name: String,
    #[serde(default, alias = "arguments", alias = "args", alias = "parameters")]
    input: Value,
}

/// What a reply turned out to say.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// Prose, possibly followed by tool calls.
    Ok(Vec<Content>),
    /// It tried to call tools and the block could not be read.
    Malformed {
        /// The prose before the block, kept so it is not lost.
        text: String,
        /// Why the block was refused, to hand back to the model.
        error: String,
    },
}

/// Read a reply back into content blocks.
///
/// `id_prefix` makes the generated call ids unique within a run, because a
/// CLI does not invent ids of its own.
pub fn parse_reply(reply: &str, id_prefix: &str) -> Parsed {
    let Some(start) = reply.rfind(OPEN) else {
        return Parsed::Ok(text_only(reply));
    };
    let before = reply[..start].trim().to_string();
    let after = &reply[start + OPEN.len()..];
    let body = match after.find(CLOSE) {
        Some(end) => &after[..end],
        // A model that stops before the closing tag (an output ceiling) has
        // still said what it wanted; read what is there.
        None => after,
    };
    let body = strip_fence(body.trim());
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Parsed::Malformed {
                text: before,
                error: format!("the {OPEN} block is not valid JSON: {e}"),
            }
        }
    };
    let list = match value {
        Value::Array(items) => items,
        single @ Value::Object(_) => vec![single],
        _ => {
            return Parsed::Malformed {
                text: before,
                error: format!("the {OPEN} block must hold a JSON array of calls"),
            }
        }
    };
    let mut content = text_only(&before);
    for (i, item) in list.into_iter().enumerate() {
        match serde_json::from_value::<WireCall>(item) {
            Ok(call) if !call.name.trim().is_empty() => content.push(Content::ToolUse {
                id: format!("{id_prefix}_{i}"),
                name: call.name.trim().to_string(),
                input: if call.input.is_null() {
                    Value::Object(Default::default())
                } else {
                    call.input
                },
            }),
            _ => {
                return Parsed::Malformed {
                    text: before,
                    error: format!(
                        "call {} in the {OPEN} block needs a \"name\" and an \"input\" object",
                        i + 1
                    ),
                }
            }
        }
    }
    Parsed::Ok(content)
}

fn text_only(text: &str) -> Vec<Content> {
    let t = text.trim();
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Content::text(t)]
    }
}

/// Models like to wrap JSON in a Markdown fence even when told not to.
fn strip_fence(body: &str) -> &str {
    let Some(rest) = body.strip_prefix("```") else { return body };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

/// What the model is told when its block could not be read.
pub fn malformed_feedback(error: &str) -> String {
    format!(
        "Your last reply could not be used: {error}. Reply again. To call tools, end with one \
         {OPEN} block holding a JSON array such as [{{\"name\": \"read_dtcs\", \"input\": {{}}}}] \
         and then {CLOSE}. To answer, write prose with no block."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "Reads things.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }

    #[test]
    fn prose_without_a_block_is_an_answer() {
        assert_eq!(
            parse_reply("The cat is fine.\n", "c"),
            Parsed::Ok(vec![Content::text("The cat is fine.")])
        );
    }

    #[test]
    fn a_block_becomes_tool_calls_after_the_prose() {
        let reply = "Reading codes.\n<tool_calls>\n[{\"name\":\"read_dtcs\",\"input\":{}},\
                     {\"name\":\"read_pid\",\"input\":{\"pid\":\"0C\"}}]\n</tool_calls>";
        let Parsed::Ok(content) = parse_reply(reply, "s1") else { panic!() };
        assert_eq!(content[0], Content::text("Reading codes."));
        assert_eq!(
            content[1],
            Content::ToolUse { id: "s1_0".into(), name: "read_dtcs".into(), input: json!({}) }
        );
        assert_eq!(
            content[2],
            Content::ToolUse {
                id: "s1_1".into(),
                name: "read_pid".into(),
                input: json!({"pid": "0C"})
            }
        );
    }

    #[test]
    fn fenced_single_objects_and_missing_close_tags_are_read() {
        let reply = "<tool_calls>\n```json\n{\"name\": \"read_dtcs\", \"arguments\": {}}\n```";
        let Parsed::Ok(content) = parse_reply(reply, "x") else { panic!("{reply}") };
        assert_eq!(content.len(), 1);
        assert!(matches!(&content[0], Content::ToolUse { name, .. } if name == "read_dtcs"));
    }

    #[test]
    fn broken_json_is_handed_back_not_dropped() {
        match parse_reply("Hm.\n<tool_calls>[{name: read}]</tool_calls>", "x") {
            Parsed::Malformed { text, error } => {
                assert_eq!(text, "Hm.");
                assert!(error.contains("not valid JSON"), "{error}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_reply("<tool_calls>[{\"input\":{}}]</tool_calls>", "x"),
            Parsed::Malformed { .. }
        ));
    }

    #[test]
    fn the_prompt_carries_system_tools_and_results_with_their_names() {
        let request = ChatRequest {
            system: "You are a mechanic.".into(),
            messages: vec![
                Message::user("What is wrong?"),
                Message {
                    role: Role::Assistant,
                    content: vec![Content::ToolUse {
                        id: "a".into(),
                        name: "read_dtcs".into(),
                        input: json!({}),
                    }],
                },
                Message::tool_results(vec![Content::ToolResult {
                    tool_use_id: "a".into(),
                    content: "{\"dtcs\":[\"P0420\"]}".into(),
                    is_error: false,
                }]),
            ],
            tools: vec![spec("read_dtcs")],
            max_tokens: 100,
        };
        let p = render_prompt(&request);
        assert!(p.contains("You are a mechanic."));
        assert!(p.contains("### read_dtcs"));
        assert!(p.contains("## Tool result: read_dtcs (ok)"));
        assert!(p.contains("P0420"));
        assert!(p.find("What is wrong?").unwrap() < p.find("P0420").unwrap());
    }

    #[test]
    fn without_tools_the_model_is_told_it_has_none() {
        let request = ChatRequest {
            system: "s".into(),
            messages: vec![Message::user("q")],
            tools: vec![],
            max_tokens: 10,
        };
        let p = render_prompt(&request);
        assert!(p.contains("You have no tools"));
        assert!(!p.contains(OPEN));
    }
}
