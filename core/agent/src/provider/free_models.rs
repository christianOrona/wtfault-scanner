//! Which of OpenRouter's free models the agent can use.
//!
//! OpenRouter's catalogue changes weekly: free models arrive, get busy, and are
//! withdrawn. This reads its public model list and keeps the ones that cost
//! nothing, can call the app's tools, and have room for an inspection, newest
//! first — newer models are, as a rule, the stronger ones.

use serde_json::Value;

/// One free model the agent could use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeModel {
    /// The model identifier, e.g. "qwen/qwen3.8-27b:free"
    pub id: String,
    /// The human-readable name of the model, or the id when missing
    pub name: String,
    /// The context window size in tokens
    pub context_length: u64,
    /// Unix timestamp when the model was created, 0 when missing
    pub created: i64,
}

/// Smallest context window worth trying: the agent's prompt plus a few tool results.
pub const MIN_CONTEXT: u64 = 65_536;

/// Parse the body of OpenRouter's `GET /api/v1/models` and return the models the agent can use for free, best first.
pub fn free_tool_models(body: &str) -> Result<Vec<FreeModel>, serde_json::Error> {
    let parsed: Value = serde_json::from_str(body)?;

    let data = match parsed.get("data") {
        Some(Value::Array(arr)) => arr,
        _ => return Ok(Vec::new()),
    };

    let mut models = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for item in data {
        if !item.is_object() {
            continue;
        }

        // Check id
        let id = match item.get("id") {
            Some(Value::String(s)) => s,
            _ => continue,
        };

        // Skip duplicates
        if !seen_ids.insert(id) {
            continue;
        }

        // Skip openrouter/ models
        if id.starts_with("openrouter/") {
            continue;
        }

        // Check pricing
        let pricing = match item.get("pricing") {
            Some(Value::Object(p)) => p,
            _ => continue,
        };

        let prompt_price = match pricing.get("prompt") {
            Some(Value::String(s)) => s.parse::<f64>().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };

        let completion_price = match pricing.get("completion") {
            Some(Value::String(s)) => s.parse::<f64>().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };

        if prompt_price != Some(0.0) || completion_price != Some(0.0) {
            continue;
        }

        // Check supported_parameters
        let supported_params = match item.get("supported_parameters") {
            Some(Value::Array(arr)) => arr,
            _ => continue,
        };

        if !supported_params.iter().any(|p| p.as_str() == Some("tools")) {
            continue;
        }

        // Check output_modalities
        if let Some(Value::Array(modalities)) =
            item.get("architecture").and_then(|a| a.get("output_modalities"))
        {
            if !modalities.iter().any(|m| m.as_str() == Some("text")) {
                continue;
            }
        }

        // Check context_length
        let context_length = match item.get("context_length") {
            Some(Value::Number(n)) => n.as_u64(),
            _ => None,
        };

        let context_length = match context_length {
            Some(cl) if cl >= MIN_CONTEXT => cl,
            _ => continue,
        };

        // Check expiration_date
        if item.get("expiration_date").is_some() {
            // If the field exists, it must be null to be valid
            if !matches!(item.get("expiration_date"), Some(Value::Null)) {
                continue;
            }
        }

        // Get created timestamp
        let created = match item.get("created") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        };

        // Get name or fall back to id
        let name = match item.get("name") {
            Some(Value::String(s)) => s.clone(),
            _ => id.clone(),
        };

        models.push(FreeModel { id: id.clone(), name, context_length, created });
    }

    // Sort by created desc, then context_length desc, then id asc
    models.sort_by(|a, b| {
        b.created
            .cmp(&a.created)
            .then_with(|| b.context_length.cmp(&a.context_length))
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paid_models_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0.01","completion":"0.03"},"context_length":100000,"supported_parameters":["tools"]}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_models_without_tools_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["max_tokens"]}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_image_only_output_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"],"architecture":{"output_modalities":["image"]}}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_small_context_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":1000,"supported_parameters":["tools"]}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_expiring_model_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"],"expiration_date":"2023-12-31T23:59:59Z"}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_openrouter_prefix_excluded() {
        let json = r#"{"data":[{"id":"openrouter/openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"]}]}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }

    #[test]
    fn test_duplicate_ids_excluded() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"]},{"id":"openai/gpt-4","name":"GPT-4 Duplicate","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"]}]}"#;
        let result = free_tool_models(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "openai/gpt-4");
    }

    #[test]
    fn test_ordering() {
        let json = r#"{"data":[
            {"id":"model3","name":"Model 3","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"],"created":100},
            {"id":"model0","name":"Model 0","pricing":{"prompt":"0","completion":"0"},"context_length":300000,"supported_parameters":["tools"],"created":200},
            {"id":"model1","name":"Model 1","pricing":{"prompt":"0","completion":"0"},"context_length":200000,"supported_parameters":["tools"],"created":200},
            {"id":"model2","name":"Model 2","pricing":{"prompt":"0","completion":"0"},"context_length":200000,"supported_parameters":["tools"],"created":200}
        ]}"#;
        let ids: Vec<_> = free_tool_models(json).unwrap().into_iter().map(|m| m.id).collect();
        // Newest first; equal age falls back to the larger context, then the id.
        assert_eq!(ids, ["model0", "model1", "model2", "model3"]);
    }

    #[test]
    fn test_zero_pricing_formats() {
        let json = r#"{"data":[
            {"id":"model1","name":"Model 1","pricing":{"prompt":"0.0","completion":"0.0"},"context_length":100000,"supported_parameters":["tools"]},
            {"id":"model2","name":"Model 2","pricing":{"prompt":0,"completion":0},"context_length":100000,"supported_parameters":["tools"]}
        ]}"#;
        let result = free_tool_models(json).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "model1");
        assert_eq!(result[1].id, "model2");
    }

    #[test]
    fn test_invalid_json_returns_error() {
        let json = r#"{"data":[{"id":"openai/gpt-4","name":"GPT-4","pricing":{"prompt":"0","completion":"0"},"context_length":100000,"supported_parameters":["tools"]}"#;
        assert!(free_tool_models(json).is_err());
    }

    #[test]
    fn test_empty_object_returns_empty_vec() {
        let json = r#"{}"#;
        assert_eq!(free_tool_models(json).unwrap(), Vec::<FreeModel>::new());
    }
}

/// Against OpenRouter's real reply, trimmed: every free model it listed on the
/// day, and a few paid ones.
#[cfg(test)]
mod recorded {
    use super::*;

    const BODY: &str = include_str!("../../tests/fixtures/openrouter-models-2026-09-28.json");

    #[test]
    fn the_real_catalogue_yields_free_tool_capable_models() {
        let models = free_tool_models(BODY).unwrap();
        // 17 free models could call tools that day; the stricter context and
        // expiry rules here may drop a few, but never all of them.
        assert!(models.len() >= 10, "only {} survived", models.len());
        assert!(models.len() <= 17);
        assert!(models.iter().all(|m| m.context_length >= MIN_CONTEXT));
        assert!(models.windows(2).all(|w| w[0].created >= w[1].created));
    }
}
