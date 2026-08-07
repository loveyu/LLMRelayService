use crate::config::UpstreamType;
use serde_json::Value;

/// Transform the request body for Anthropic providers.
/// - Injects route-level system prompt
/// - Applies claudeCodeCompat (moves system into first user message)
pub fn prepare_anthropic_request(
    body: &[u8],
    route_system: Option<&str>,
    claude_code_compat: bool,
) -> Option<Vec<u8>> {
    let mut json: Value = serde_json::from_slice(body).ok()?;

    // Inject route system prompt
    if let Some(system_text) = route_system {
        inject_system_prompt(&mut json, system_text);
    }

    // Claude Code compat: move system into first user message
    if claude_code_compat {
        move_system_into_first_user_turn(&mut json);
    }

    Some(serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec()))
}

fn inject_system_prompt(json: &mut Value, route_system: &str) {
    match json.get_mut("system") {
        Some(Value::String(existing)) => {
            *existing = format!("{route_system}\n\n{existing}");
        }
        Some(Value::Array(blocks)) => {
            let new_block = serde_json::json!({
                "type": "text",
                "text": route_system,
            });
            blocks.insert(0, new_block);
        }
        None => {
            json["system"] = Value::String(route_system.to_string());
        }
        _ => {}
    }
}

fn move_system_into_first_user_turn(json: &mut Value) {
    // Extract system text and cache_control BEFORE removing the field
    let (system_text, cache_control) = match json.get("system") {
        Some(Value::String(s)) => (s.clone(), None),
        Some(Value::Array(blocks)) => {
            let text = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            let cc = blocks.last().and_then(|b| b.get("cache_control").cloned());
            (text, cc)
        }
        _ => return,
    };

    if system_text.is_empty() {
        return;
    }

    // Remove system field
    let _ = json.as_object_mut().map(|o| o.remove("system"));

    let mut moved_block = serde_json::json!({
        "type": "text",
        "text": format!("<system_instructions>\n{system_text}\n</system_instructions>"),
    });
    if let Some(cc) = cache_control {
        moved_block["cache_control"] = cc;
    }

    // Inject into first user message
    let messages = json.get_mut("messages").and_then(|m| m.as_array_mut());
    if let Some(msgs) = messages {
        for msg in msgs.iter_mut() {
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                if let Some(content) = msg.get_mut("content") {
                    match content {
                        Value::String(s) => {
                            let block = serde_json::json!([
                                moved_block,
                                {"type": "text", "text": s.as_str()}
                            ]);
                            *content = block;
                        }
                        Value::Array(arr) => {
                            arr.insert(0, moved_block);
                        }
                        _ => {}
                    }
                }
                break;
            }
        }
    }
}

/// Check if this is an Anthropic request (by route type)
pub fn is_anthropic(route_type: &UpstreamType) -> bool {
    matches!(route_type, UpstreamType::Anthropic)
}

/// Build a model name rewriter for response bodies.
pub struct ModelRewriter {
    pattern: regex::Regex,
    to_model: String,
}

impl ModelRewriter {
    pub fn new(from_model: &str, to_model: &str) -> Option<Self> {
        if from_model == to_model {
            return None;
        }
        let escaped = regex::escape(from_model);
        let pattern_str = format!(r#"("(?:[A-Za-z0-9_]*model)"\s*:\s*"){}(")"#, escaped);
        let pattern = regex::Regex::new(&pattern_str).ok()?;
        Some(ModelRewriter { pattern, to_model: to_model.to_string() })
    }

    /// Rewrite model names in a text buffer.
    /// Uses a safe tail strategy to avoid splitting matches across chunk boundaries.
    /// Group 1 captures `"model_field_name": "` (prefix up to value opening quote)
    /// Group 2 captures `"` (value closing quote)
    pub fn rewrite_chunk(&self, buf: &str) -> String {
        let to = &self.to_model;
        self.pattern
            .replace_all(buf, |caps: &regex::Captures| format!("{}{}{}", &caps[1], to, &caps[2]))
            .into_owned()
    }
}
