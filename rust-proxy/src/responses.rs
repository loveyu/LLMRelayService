use serde_json::{Value, json};

const REQUEST_DIRECT_FIELDS: &[&str] = &[
    "model",
    "temperature",
    "top_p",
    "stop",
    "presence_penalty",
    "frequency_penalty",
    "logit_bias",
    "user",
    "seed",
    "stream",
    "stream_options",
    "store",
    "metadata",
    "service_tier",
    "parallel_tool_calls",
    "logprobs",
    "top_logprobs",
];

pub fn is_responses_endpoint(pathname: &str) -> bool {
    pathname == "/v1/responses"
}

pub fn rewrite_responses_to_chat_url(target_url: &str) -> String {
    if let Some(stripped) = target_url.strip_suffix("/responses") {
        format!("{}/chat/completions", stripped)
    } else if target_url.ends_with('/') {
        format!("{}chat/completions", target_url)
    } else {
        format!("{}/chat/completions", target_url)
    }
}

pub fn convert_responses_to_chat_request(body: &[u8]) -> Result<Vec<u8>, (u16, String)> {
    let input: Value =
        serde_json::from_slice(body).map_err(|e| (400, format!("Invalid JSON: {e}")))?;

    let mut chat = serde_json::Map::new();

    for &field in REQUEST_DIRECT_FIELDS {
        if let Some(v) = input.get(field) {
            chat.insert(field.to_string(), v.clone());
        }
    }

    let messages = convert_input_to_messages(&input)?;
    chat.insert("messages".to_string(), Value::Array(messages));

    if let Some(mt) = input.get("max_output_tokens") {
        chat.insert("max_tokens".to_string(), mt.clone());
    }
    if let Some(mt) = input.get("max_completion_tokens") {
        chat.insert("max_completion_tokens".to_string(), mt.clone());
    }

    let has_tools = convert_tools(&input, &mut chat);

    if let Some(tc) = input.get("tool_choice")
        && has_tools
            && let Some(converted) = convert_tool_choice(tc) {
                chat.insert("tool_choice".to_string(), converted);
            }

    if let Some(text) = input.get("text")
        && let Some(rf) = convert_text_format(text) {
            chat.insert("response_format".to_string(), rf);
        }

    if let Some(reasoning) = input.get("reasoning")
        && let Some(effort) = reasoning.get("effort").and_then(|v| v.as_str()) {
            chat.insert("reasoning_effort".to_string(), Value::String(effort.to_string()));
        }

    serde_json::to_vec(&Value::Object(chat)).map_err(|e| (500, format!("Serialization error: {e}")))
}

fn convert_input_to_messages(input: &Value) -> Result<Vec<Value>, (u16, String)> {
    let items = match &input["input"] {
        Value::String(s) => {
            return Ok(vec![json!({"role": "user", "content": s})]);
        }
        Value::Array(arr) => arr,
        _ => {
            return Err((400, "input must be a string or array".into()));
        }
    };

    let mut messages: Vec<Value> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let prefix = format!("input[{}]", i);
        if let Some(msg) = convert_input_item(item, &prefix)? {
            messages.push(msg);
        }
    }

    if let Some(instructions) = input.get("instructions").and_then(|v| v.as_str())
        && !instructions.is_empty() {
            messages.insert(0, json!({"role": "system", "content": instructions}));
        }

    merge_leading_system_messages(&mut messages);

    Ok(messages)
}

fn convert_input_item(item: &Value, prefix: &str) -> Result<Option<Value>, (u16, String)> {
    if item.is_string() {
        return Ok(Some(json!({"role": "user", "content": item})));
    }
    let obj = match item.as_object() {
        Some(o) => o,
        None => return Ok(Some(json!({"role": "user", "content": item.to_string()}))),
    };

    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match item_type {
        "reasoning" => return Ok(None),
        "function_call" => return convert_function_call(item, prefix),
        "function_call_output" => return convert_function_call_output(item, prefix),
        "item_reference" => {
            return Err((400, format!("item_reference not supported in chat compat ({})", prefix)));
        }
        _ => {}
    }

    let role = normalize_role(obj.get("role").and_then(|v| v.as_str()).unwrap_or(""));
    let mut msg = json!({"role": role});
    let content = convert_content(obj.get("content"), role, prefix);

    if role == "assistant" && content.is_null() {
        if obj.get("tool_calls").is_some() {
            msg["content"] = Value::Null;
            msg["tool_calls"] = obj["tool_calls"].clone();
        } else {
            msg["content"] = Value::Null;
        }
    } else {
        msg["content"] = content;
    }

    if role == "tool"
        && let Some(call_id) = obj.get("tool_call_id").or(obj.get("call_id")) {
            msg["tool_call_id"] = call_id.clone();
        }
    if role == "assistant"
        && let Some(tool_calls) = obj.get("tool_calls") {
            msg["tool_calls"] = tool_calls.clone();
        }

    Ok(Some(msg))
}

fn convert_function_call(item: &Value, prefix: &str) -> Result<Option<Value>, (u16, String)> {
    let call_id = item["call_id"].as_str().or(item["id"].as_str()).unwrap_or(prefix).to_string();
    let name = item["name"]
        .as_str()
        .ok_or_else(|| (400, format!("function_call requires name ({})", prefix)))?;
    let arguments = normalize_arguments(&item["arguments"]);

    Ok(Some(json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": call_id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments,
            }
        }]
    })))
}

fn convert_function_call_output(
    item: &Value,
    prefix: &str,
) -> Result<Option<Value>, (u16, String)> {
    let call_id = item["call_id"]
        .as_str()
        .ok_or_else(|| (400, format!("function_call_output requires call_id ({})", prefix)))?;
    let output = match item.get("output") {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    Ok(Some(json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": output,
    })))
}

fn normalize_role(role: &str) -> &str {
    if role == "developer" { "system" } else { role }
}

fn normalize_arguments(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn convert_content(content: Option<&Value>, role: &str, prefix: &str) -> Value {
    let content = match content {
        Some(c) => c,
        None => {
            return if role == "assistant" { Value::Null } else { Value::String(String::new()) };
        }
    };

    match content {
        Value::String(s) => Value::String(s.clone()),
        Value::Null if role == "assistant" => Value::Null,
        Value::Null => Value::String(String::new()),
        Value::Array(parts) => convert_content_parts(parts, prefix),
        v => {
            if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                Value::String(text.to_string())
            } else {
                Value::String(v.to_string())
            }
        }
    }
}

fn convert_content_parts(parts: &[Value], _prefix: &str) -> Value {
    let mut text_parts: Vec<String> = Vec::new();
    let mut converted: Vec<Value> = Vec::new();

    for part in parts {
        if let Some(s) = part.as_str() {
            text_parts.push(s.to_string());
            converted.push(Value::String(s.to_string()));
            continue;
        }

        let part_type = part["type"].as_str().unwrap_or("");
        match part_type {
            "input_text" | "output_text" | "text" => {
                if let Some(text) = part["text"].as_str() {
                    text_parts.push(text.to_string());
                    converted.push(json!({"type": "text", "text": text}));
                }
            }
            "refusal" => {
                if let Some(text) = part["refusal"].as_str() {
                    text_parts.push(text.to_string());
                    converted.push(json!({"type": "text", "text": text}));
                }
            }
            "input_image" | "image_url" => {
                if let Some(image_url) = part.get("image_url") {
                    converted.push(json!({"type": "image_url", "image_url": image_url}));
                }
            }
            _ => {
                if let Some(text) = part["text"].as_str() {
                    text_parts.push(text.to_string());
                    converted.push(json!({"type": "text", "text": text}));
                }
            }
        }
    }

    if converted
        .iter()
        .all(|p| p.get("type").and_then(|t| t.as_str()).map(|t| t == "text").unwrap_or(false))
    {
        Value::String(text_parts.join(""))
    } else {
        Value::Array(converted)
    }
}

fn merge_leading_system_messages(messages: &mut Vec<Value>) {
    if messages.len() < 2 {
        return;
    }
    let first_role = messages[0]["role"].as_str();
    let second_role = messages[1]["role"].as_str();
    if first_role != Some("system") || second_role != Some("system") {
        return;
    }

    let mut system_texts: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < messages.len() && messages[idx]["role"].as_str() == Some("system") {
        let content = content_to_text(&messages[idx]["content"]);
        if !content.is_empty() {
            system_texts.push(content);
        }
        idx += 1;
    }

    let merged = json!({
        "role": "system",
        "content": system_texts.join("\n\n"),
    });

    messages.drain(..idx);
    messages.insert(0, merged);
}

fn content_to_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| {
                if let Some(s) = p.as_str() {
                    Some(s.to_string())
                } else {
                    p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        v => v.to_string(),
    }
}

fn convert_tools(input: &Value, chat: &mut serde_json::Map<String, Value>) -> bool {
    let tools = match input.get("tools") {
        Some(Value::Array(arr)) => arr,
        _ => return false,
    };

    let converted: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let obj = tool.as_object()?;
            if obj.get("type")?.as_str()? != "function" {
                return None;
            }

            if let Some(func_obj) = obj.get("function") {
                return Some(json!({
                    "type": "function",
                    "function": func_obj,
                }));
            }

            let name = obj.get("name")?.as_str()?;
            let mut func = json!({
                "name": name,
            });
            if let Some(desc) = obj.get("description") {
                func["description"] = desc.clone();
            }
            if let Some(params) = obj.get("parameters") {
                func["parameters"] = params.clone();
            }
            Some(json!({
                "type": "function",
                "function": func,
            }))
        })
        .collect();

    if converted.is_empty() {
        false
    } else {
        chat.insert("tools".to_string(), Value::Array(converted));
        true
    }
}

fn convert_tool_choice(tool_choice: &Value) -> Option<Value> {
    match tool_choice {
        Value::String(_) => Some(tool_choice.clone()),
        Value::Object(obj) => {
            let name = obj.get("name").and_then(|v| v.as_str()).or_else(|| {
                obj.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str())
            })?;
            if obj.get("type")?.as_str()? == "function" {
                Some(json!({
                    "type": "function",
                    "function": {"name": name},
                }))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn convert_text_format(text: &Value) -> Option<Value> {
    let format = text.get("format")?;
    let format_type = format.get("type")?.as_str()?;

    match format_type {
        "text" => None,
        "json_object" => Some(json!({"type": "json_object"})),
        "json_schema" => {
            let schema_source = format.get("json_schema").unwrap_or(format);
            let name = schema_source.get("name").and_then(|v| v.as_str()).unwrap_or("Output");
            let schema = schema_source.get("schema").cloned().unwrap_or(json!({"type": "object"}));
            let mut result = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "schema": schema,
                }
            });
            if let Some(strict) = schema_source.get("strict") {
                result["json_schema"]["strict"] = strict.clone();
            }
            Some(result)
        }
        _ => None,
    }
}

/// Convert a non-streaming Chat Completions response to Responses API format.
pub fn convert_chat_to_responses(body: &[u8]) -> Result<Vec<u8>, (u16, String)> {
    let chat: Value = serde_json::from_slice(body)
        .map_err(|e| (500, format!("Failed to parse chat response: {e}")))?;

    let model = chat["model"].as_str().unwrap_or("");
    let choice = chat["choices"].as_array().and_then(|c| c.first()).unwrap_or(&Value::Null);
    let message = choice.get("message").unwrap_or(&Value::Null);
    let _finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
    let usage = chat.get("usage").unwrap_or(&Value::Null);

    let output_text = message["content"].as_str().unwrap_or("");
    let output = vec![json!({
        "type": "message",
        "id": format!("msg_{}", uuid::Uuid::new_v4()),
        "status": "completed",
        "role": "assistant",
        "content": [{"type": "output_text", "text": output_text, "annotations": []}],
    })];

    let response = json!({
        "id": format!("resp_{}", uuid::Uuid::new_v4()),
        "object": "response",
        "created_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": usage,
    });

    serde_json::to_vec(&response).map_err(|e| (500, format!("Serialization error: {e}")))
}

/// SSE event state machine for converting Chat Completions SSE to Responses API SSE.
pub struct ChatSseToResponsesSse {
    response_id: String,
    msg_id: String,
    model: String,
    started: bool,
    text_started: bool,
    thinking_started: bool,
    #[expect(dead_code)]
    in_think_tag: bool,
    #[expect(dead_code)]
    pending_text: String,
    buf: Vec<u8>,
    delta_buf: Vec<u8>,
}

impl ChatSseToResponsesSse {
    pub fn new(model: &str) -> Self {
        Self {
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            msg_id: format!("msg_{}", uuid::Uuid::new_v4()),
            model: model.to_string(),
            started: false,
            text_started: false,
            thinking_started: false,
            in_think_tag: false,
            pending_text: String::new(),
            buf: Vec::new(),
            delta_buf: Vec::new(),
        }
    }

    /// Feed raw bytes. Returns Vec of SSE event strings ready to send.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        self.buf.extend_from_slice(chunk);

        loop {
            let line_end = self.buf.windows(2).position(|w| w == b"\n\n");
            let idx = match line_end {
                Some(i) => i,
                None => break,
            };
            let sse_block = self.buf[..idx + 2].to_vec();
            self.buf.drain(..idx + 2);

            let text = match std::str::from_utf8(&sse_block) {
                Ok(t) => t,
                Err(_) => continue,
            };

            if text.starts_with("data: [DONE]") || text.trim().is_empty() {
                continue;
            }

            let data = text.trim_start_matches("data: ").trim();
            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let choice = match parsed["choices"].as_array().and_then(|c| c.first()) {
                Some(c) => c,
                None => continue,
            };

            let delta = match choice.get("delta") {
                Some(d) => d,
                None => continue,
            };

            let finish_reason = choice["finish_reason"].as_str();

            // Start event
            if !self.started {
                self.started = true;
                events.push(format!(
                    "event: response.created\ndata: {}\n\n",
                    json!({
                        "type": "response.created",
                        "response": {
                            "id": self.response_id,
                            "object": "response",
                            "status": "in_progress",
                            "model": self.model,
                            "output": [],
                        }
                    })
                ));
            }

            // Collect text delta
            if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                self.delta_buf.extend_from_slice(content.as_bytes());
            }

            // Tool calls
            if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array())
                && !tool_calls.is_empty() {
                    if self.text_started {
                        self.flush_text_to_output(&mut events);
                    }
                    self.thinking_started = false;
                    for tc in tool_calls {
                        let index = tc["index"].as_u64().unwrap_or(0);
                        let id = tc["id"].as_str().unwrap_or("");
                        let func_name = tc["function"]["name"].as_str().unwrap_or("");
                        let func_args = tc["function"]["arguments"].as_str().unwrap_or("");
                        events.push(format!(
                            "event: response.output_item.added\ndata: {}\n\n",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": index,
                                "item": {
                                    "id": format!("fc_{}", id),
                                    "type": "function_call",
                                    "call_id": id,
                                    "name": func_name,
                                    "arguments": func_args,
                                    "status": "completed",
                                },
                                "response_id": self.response_id,
                            })
                        ));
                    }
                }

            // Flush on finish
            if finish_reason.is_some() {
                if self.text_started {
                    self.flush_text_to_output(&mut events);
                }
                let usage = parsed.get("usage").unwrap_or(&Value::Null);
                events.push(format!(
                    "event: response.completed\ndata: {}\n\n",
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": self.response_id,
                            "object": "response",
                            "status": "completed",
                            "model": self.model,
                            "usage": usage,
                        }
                    })
                ));
            }
        }

        events
    }

    fn flush_text_to_output(&mut self, events: &mut Vec<String>) {
        let text = String::from_utf8_lossy(&self.delta_buf).into_owned();

        // Parse think tags
        let (think_text, output_text) = split_think_tags(&text);

        if !think_text.is_empty() {
            if !self.thinking_started {
                self.thinking_started = true;
                events.push(format!(
                    "event: response.output_item.added\ndata: {}\n\n",
                    json!({
                        "type": "response.output_item.added",
                        "output_index": 1,
                        "item": {
                            "id": format!("rs_{}", self.msg_id),
                            "type": "reasoning",
                            "status": "in_progress",
                        },
                        "response_id": self.response_id,
                    })
                ));
            }
            events.push(format!(
                "event: response.reasoning_text.delta\ndata: {}\n\n",
                json!({
                    "type": "response.reasoning_text.delta",
                    "item_id": format!("rs_{}", self.msg_id),
                    "delta": think_text,
                    "output_index": 1,
                    "response_id": self.response_id,
                })
            ));
            events.push(format!(
                "event: response.reasoning_text.done\ndata: {}\n\n",
                json!({
                    "type": "response.reasoning_text.done",
                    "item_id": format!("rs_{}", self.msg_id),
                    "output_index": 1,
                    "response_id": self.response_id,
                })
            ));
        }

        events.push(format!(
            "event: response.output_item.added\ndata: {}\n\n",
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": self.msg_id,
                    "type": "message",
                    "status": "in_progress",
                    "role": "assistant",
                    "content": [],
                },
                "response_id": self.response_id,
            })
        ));
        events.push(format!(
            "event: response.content_part.added\ndata: {}\n\n",
            json!({
                "type": "response.content_part.added",
                "item_id": self.msg_id,
                "output_index": 0,
                "content_index": 0,
                "part": {
                    "type": "output_text",
                    "text": "",
                    "annotations": [],
                },
                "response_id": self.response_id,
            })
        ));

        if !output_text.is_empty() {
            events.push(format!(
                "event: response.output_text.delta\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.delta",
                    "item_id": self.msg_id,
                    "output_index": 0,
                    "content_index": 0,
                    "delta": output_text,
                    "response_id": self.response_id,
                })
            ));
        }

        events.push(format!(
            "event: response.output_text.done\ndata: {}\n\n",
            json!({
                "type": "response.output_text.done",
                "item_id": self.msg_id,
                "output_index": 0,
                "content_index": 0,
                "text": output_text,
                "response_id": self.response_id,
            })
        ));
        events.push(format!(
            "event: response.content_part.done\ndata: {}\n\n",
            json!({
                "type": "response.content_part.done",
                "item_id": self.msg_id,
                "output_index": 0,
                "content_index": 0,
                "response_id": self.response_id,
            })
        ));

        self.delta_buf.clear();
        self.text_started = true;
    }

    /// Flush any remaining data and emit completion events
    pub fn finish(&mut self) -> Vec<String> {
        if self.text_started && !self.delta_buf.is_empty() {
            let output_text = String::from_utf8_lossy(&self.delta_buf).into_owned();
            let clean = strip_think_tags(&output_text);
            let mut events = Vec::new();
            if !clean.is_empty() {
                events.push(format!(
                    "event: response.output_text.delta\ndata: {}\n\n",
                    json!({
                        "type": "response.output_text.delta",
                        "item_id": self.msg_id,
                        "output_index": 0,
                        "content_index": 0,
                        "delta": clean,
                        "response_id": self.response_id,
                    })
                ));
            }
            events.push(format!(
                "event: response.output_text.done\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.done",
                    "item_id": self.msg_id,
                    "output_index": 0,
                    "content_index": 0,
                    "text": clean,
                    "response_id": self.response_id,
                })
            ));
            events.push(format!(
                "event: response.content_part.done\ndata: {}\n\n",
                json!({
                    "type": "response.content_part.done",
                    "item_id": self.msg_id,
                    "output_index": 0,
                    "content_index": 0,
                    "response_id": self.response_id,
                })
            ));
            self.delta_buf.clear();
            return events;
        }

        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(format!(
                "event: response.created\ndata: {}\n\n",
                json!({
                    "type": "response.created",
                    "response": {
                        "id": self.response_id,
                        "object": "response",
                        "status": "in_progress",
                        "model": self.model,
                        "output": [],
                    }
                })
            ));
        }
        events.push(format!(
            "event: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.completed",
                "response": {
                    "id": self.response_id,
                    "object": "response",
                    "status": "completed",
                    "model": self.model,
                    "usage": null,
                }
            })
        ));
        events
    }
}

fn split_think_tags(text: &str) -> (String, String) {
    if let Some(start) = text.find("<｜end▁of▁thinking｜><｜end▁of▁thinking｜>") {
        let (before, after) = text.split_at(start);
        (before.to_string(), after.to_string())
    } else if let Some(start) = text.find("<｜end▁of▁thinking｜>") {
        let (before, after) = text.split_at(start);
        if let Some(stripped) = before.strip_suffix(" ") {
            (stripped.to_string(), after.to_string())
        } else {
            (before.to_string(), after.to_string())
        }
    } else {
        (String::new(), text.to_string())
    }
}

fn strip_think_tags(text: &str) -> String {
    let (_, out) = split_think_tags(text);
    out
}
