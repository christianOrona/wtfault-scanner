//! Live checks against a real endpoint.
//!
//! Skipped unless `AIM_LIVE_OLLAMA` (or `AIM_LIVE_ANTHROPIC_KEY`) is set, so
//! the normal suite stays hardware- and network-free. These exist because the
//! wire formats are the part most likely to be subtly wrong, and a unit test
//! against a fixture cannot catch a provider that changed its shape.
//!
//! ```text
//! AIM_LIVE_OLLAMA=http://192.168.1.207:11434 \
//!   cargo test -p aim-agent --test live_provider -- --nocapture
//! ```

use aim_agent::provider::{
    ollama::OllamaProvider, ChatRequest, Content, LlmProvider, Message, StopReason, ToolSpec,
};
use aim_agent::settings::Speed;
use serde_json::json;

fn read_dtcs_tool() -> ToolSpec {
    ToolSpec {
        name: "read_dtcs".into(),
        description: "Read stored, pending and permanent diagnostic trouble codes from a module."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "module": { "type": "string", "description": "Module key, e.g. ECU_7E8" }
            },
            "required": ["module"],
            "additionalProperties": false
        }),
    }
}

#[tokio::test]
async fn ollama_probe_and_tool_call() {
    let Ok(base) = std::env::var("AIM_LIVE_OLLAMA") else {
        eprintln!("skipping: set AIM_LIVE_OLLAMA to a base URL to run this");
        return;
    };
    let model = std::env::var("AIM_LIVE_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());

    let speed = if std::env::var("AIM_LIVE_FAST").is_ok() { Speed::Fast } else { Speed::Quality };
    let p = OllamaProvider::new("live", "Live Ollama", &model, Some(&base), speed, None).unwrap();

    let info = p.probe().await.expect("probe failed");
    assert!(info.reachable);
    assert!(!info.models.is_empty(), "endpoint reported no models");
    eprintln!(
        "probe: {} models in {} ms; detail={:?}",
        info.models.len(),
        info.elapsed_ms,
        info.detail
    );

    let req = ChatRequest {
        system: "You are a vehicle diagnostic agent. Use the tools to gather evidence. \
                 Never invent readings."
            .into(),
        messages: vec![Message::user("Read the trouble codes from the engine module ECU_7E8.")],
        tools: vec![read_dtcs_tool()],
        max_tokens: 1024,
    };

    let res = p.chat(&req).await.expect("chat failed");
    eprintln!("stop_reason={:?} usage={:?}", res.stop_reason, res.usage);

    assert_eq!(res.stop_reason, StopReason::ToolUse, "model did not call a tool");
    let calls = res.tool_calls();
    assert_eq!(calls.len(), 1, "expected exactly one tool call");
    assert_eq!(calls[0].1, "read_dtcs");
    assert_eq!(calls[0].2["module"], "ECU_7E8");

    // The second half of the loop: hand a result back and check the model can
    // read it. This is where a wrong tool-result shape shows up - the model
    // either answers from the evidence or starts inventing.
    let (call_id, _, _) = calls[0];
    let mut messages = req.messages.clone();
    messages.push(Message { role: aim_agent::Role::Assistant, content: res.content.clone() });
    messages.push(Message::tool_results(vec![Content::ToolResult {
        tool_use_id: call_id.to_string(),
        content: json!({
            "success": true,
            "dtcs": [{"code": "P2463", "status": "confirmed",
                      "description": "Diesel particulate filter restriction, soot accumulation"}]
        })
        .to_string(),
        is_error: false,
    }]));

    let follow = ChatRequest { messages, ..req };
    let res2 = p.chat(&follow).await.expect("second turn failed");
    let text = res2.text();
    eprintln!("answer: {text}");
    assert!(!text.is_empty(), "model returned nothing after the tool result");
    assert!(
        text.contains("P2463") || text.to_lowercase().contains("particulate"),
        "model did not use the evidence it was given: {text}"
    );
}
