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

pub fn is_responses_request(pathname: &str, target_url: &str) -> bool {
    let (target_path, _) = split_url_suffix(target_url);
    is_responses_endpoint(pathname) || target_path.ends_with("/responses")
}

pub fn rewrite_responses_to_chat_url(target_url: &str) -> String {
    let (target_path, suffix) = split_url_suffix(target_url);
    let rewritten = if let Some(stripped) = target_path.strip_suffix("/responses") {
        format!("{stripped}/chat/completions")
    } else if target_path.ends_with('/') {
        format!("{target_path}chat/completions")
    } else {
        format!("{target_path}/chat/completions")
    };
    format!("{rewritten}{suffix}")
}

fn split_url_suffix(target_url: &str) -> (&str, &str) {
    let suffix_start = target_url
        .char_indices()
        .find_map(|(index, character)| matches!(character, '?' | '#').then_some(index))
        .unwrap_or(target_url.len());
    target_url.split_at(suffix_start)
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
        && let Some(converted) = convert_tool_choice(tc)
    {
        chat.insert("tool_choice".to_string(), converted);
    }

    if let Some(text) = input.get("text")
        && let Some(rf) = convert_text_format(text)
    {
        chat.insert("response_format".to_string(), rf);
    }

    if let Some(reasoning) = input.get("reasoning")
        && let Some(effort) = reasoning.get("effort").and_then(|v| v.as_str())
    {
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
        && !instructions.is_empty()
    {
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
        && let Some(call_id) = obj.get("tool_call_id").or(obj.get("call_id"))
    {
        msg["tool_call_id"] = call_id.clone();
    }
    if role == "assistant"
        && let Some(tool_calls) = obj.get("tool_calls")
    {
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

/// Chat Completions usage(prompt_tokens/completion_tokens)→ Responses usage
/// (input_tokens/output_tokens)。Responses 客户端(codex 等)对 completed 事件与
/// 响应对象里的 usage 做强类型解析,缺 input_tokens 会直接解析失败断流重连。
/// 对齐 TS `convertChatUsageToResponsesUsage`:双向兼容读取,输出统一为
/// Responses 规范结构;输入不是对象(或缺失)时返回全 0,保证字段始终齐全。
fn convert_chat_usage_to_responses_usage(usage: Option<&Value>) -> Value {
    let usage = match usage.filter(|u| u.is_object()) {
        Some(u) => u,
        None => {
            return json!({
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens_details": { "reasoning_tokens": 0 },
            });
        }
    };
    let pick = |names: &[&str]| -> u64 {
        names.iter().find_map(|name| usage.get(*name).and_then(|v| v.as_u64())).unwrap_or(0)
    };
    let detail = |containers: &[&str], field: &str| -> u64 {
        containers
            .iter()
            .find_map(|name| usage.get(*name).and_then(|d| d.get(field)).and_then(|v| v.as_u64()))
            .unwrap_or(0)
    };
    let input_tokens = pick(&["input_tokens", "prompt_tokens"]);
    let output_tokens = pick(&["output_tokens", "completion_tokens"]);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64())
            .unwrap_or(input_tokens + output_tokens),
        "input_tokens_details": {
            "cached_tokens": detail(&["input_tokens_details", "prompt_tokens_details"], "cached_tokens"),
        },
        "output_tokens_details": {
            "reasoning_tokens": detail(&["output_tokens_details", "completion_tokens_details"], "reasoning_tokens"),
        },
    })
}

/// Convert a non-streaming Chat Completions response to Responses API format.
pub fn convert_chat_to_responses(body: &[u8]) -> Result<Vec<u8>, (u16, String)> {
    let chat: Value = serde_json::from_slice(body)
        .map_err(|e| (500, format!("Failed to parse chat response: {e}")))?;

    let model = chat["model"].as_str().unwrap_or("");
    let choice = chat["choices"].as_array().and_then(|c| c.first()).unwrap_or(&Value::Null);
    let message = choice.get("message").unwrap_or(&Value::Null);
    let _finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
    let usage = convert_chat_usage_to_responses_usage(chat.get("usage"));

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
    /// message item 的 added/content_part.added 事件只发一次;文本与 tool_calls
    /// 交错时后续 flush 只继续发 delta/done,避免重复的同 id item 事件。
    message_started: bool,
    /// `response.completed` 是 Responses SSE 的终结事件,全流只能发一次。
    /// 中转上游(kiro/astream 等)常在流尾发多个带 finish_reason 的 chunk,
    /// 再叠加流结束时的兜底补发,没有该标志就会产生重复的 completed 事件,
    /// 导致客户端(codex)在第一个 completed 后正常关闭连接却被记为 499。
    completed_sent: bool,
    /// 全量思考文本(<think> 标签内容),供 completed.output 的 reasoning item。
    thinking_text: String,
    /// 全量正文文本(跨多次 flush 累积),供 completed.output 的 message item。
    message_text: String,
    /// function_call items(按 call_id 去重,多 chunk 时后到覆盖),供
    /// completed.output 汇总。
    function_calls: std::collections::BTreeMap<String, Value>,
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
            message_started: false,
            completed_sent: false,
            thinking_text: String::new(),
            message_text: String::new(),
            function_calls: std::collections::BTreeMap::new(),
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
            // 收到非空 content 即标记 text_started,否则 finish/tool_call 触发的
            // flush 因 text_started 恒为 false 永远不会执行,正文全部滞留丢失。
            if let Some(content) = delta.get("content").and_then(|v| v.as_str())
                && !content.is_empty()
            {
                self.text_started = true;
                self.delta_buf.extend_from_slice(content.as_bytes());
            }

            // Tool calls
            if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array())
                && !tool_calls.is_empty()
            {
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
                    // completed.output 汇总用(多 chunk 增量时按 call_id 覆盖)
                    self.function_calls.insert(
                        id.to_string(),
                        json!({
                            "id": format!("fc_{}", id),
                            "type": "function_call",
                            "call_id": id,
                            "name": func_name,
                            "arguments": func_args,
                            "status": "completed",
                        }),
                    );
                }
            }

            // Flush on finish
            if finish_reason.is_some() {
                // 只有第一个 finish chunk 产生 completed;中转上游可能在流尾
                // 重复下发带 finish_reason 的 chunk,后续的一律忽略。
                if !self.completed_sent {
                    self.completed_sent = true;
                    if self.text_started {
                        self.flush_text_to_output(&mut events);
                    }
                    // usage 必须转成 Responses 规范字段(input_tokens/output_tokens):
                    // codex 等客户端对 completed 事件的 usage 强类型解析,
                    // 直接透传 chat 格式(prompt_tokens)会解析失败断流。
                    let usage = convert_chat_usage_to_responses_usage(parsed.get("usage"));
                    let output = self.build_output_items();
                    events.push(format!(
                        "event: response.completed\ndata: {}\n\n",
                        json!({
                            "type": "response.completed",
                            "response": {
                                "id": self.response_id,
                                "object": "response",
                                "status": "completed",
                                "model": self.model,
                                "output": output,
                                "usage": usage,
                            }
                        })
                    ));
                }
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
            // 累积全量思考文本,completed.output 的 reasoning item 需要
            self.thinking_text.push_str(&think_text);
        }

        // message item 的 added 系列事件只发一次:文本与 tool_calls 交错时
        // 后续 flush 只继续发 delta/done,避免重复的同 id message item。
        if !self.message_started {
            self.message_started = true;
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
        }

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
            // 累积全量正文,completed.output 的 message item 需要
            self.message_text.push_str(&output_text);
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

    /// 汇总 completed 事件的 response.output:reasoning(若有)→ message(若有
    /// 正文)→ function_calls(按插入顺序)。codex 等客户端以 completed.output
    /// 为最终内容依据,缺失该数组会被当成空响应("不可用")。
    fn build_output_items(&self) -> Vec<Value> {
        let mut output = Vec::new();
        if !self.thinking_text.is_empty() {
            output.push(json!({
                "id": format!("rs_{}", self.msg_id),
                "type": "reasoning",
                "status": "completed",
                "summary": [],
            }));
        }
        if self.text_started || !self.message_text.is_empty() {
            output.push(json!({
                "id": self.msg_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": self.message_text,
                    "annotations": [],
                }],
            }));
        }
        output.extend(self.function_calls.values().cloned());
        output
    }

    /// Flush any remaining data and emit completion events
    pub fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        // 残留文本(异常截断)先作为最后一段 delta 发出并累积,再统一走
        // completed 补发 —— 提前 return 会让客户端永远收不到终结事件。
        if self.text_started && !self.delta_buf.is_empty() {
            let output_text = String::from_utf8_lossy(&self.delta_buf).into_owned();
            let clean = strip_think_tags(&output_text);
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
                self.message_text.push_str(&clean);
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
        }

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
        // 兜底补发:仅当流中从未出现 finish_reason chunk(异常截断)时才补一个
        // completed;正常结束(或已经补过)则不再发,避免重复的终结事件。
        if self.completed_sent {
            return events;
        }
        self.completed_sent = true;
        // 兜底场景拿不到 usage,给全 0 对象保证字段齐全(客户端强类型解析
        // 要求 input_tokens 存在,null 同样会解析失败)。
        let usage = convert_chat_usage_to_responses_usage(None);
        let output = self.build_output_items();
        events.push(format!(
            "event: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.completed",
                "response": {
                    "id": self.response_id,
                    "object": "response",
                    "status": "completed",
                    "model": self.model,
                    "output": output,
                    "usage": usage,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_type_forced_responses_after_path_normalization() {
        let (stripped_path, _) = crate::routing::parse_type_forced_prefix("/openai/v1/responses");

        assert_eq!(stripped_path, "/v1/responses");
        assert!(is_responses_request(&stripped_path, "https://upstream.example/v1/not-responses"));
    }

    #[test]
    fn recognizes_responses_from_explicit_route_target() {
        assert!(is_responses_request(
            "/providers/kiro-openai/v1/responses",
            "https://upstream.example/v1/responses?trace=true"
        ));
    }

    #[test]
    fn ignores_non_responses_requests() {
        assert!(!is_responses_request(
            "/v1/chat/completions",
            "https://upstream.example/v1/chat/completions"
        ));
    }

    #[test]
    fn rewrites_responses_url_and_preserves_query() {
        assert_eq!(
            rewrite_responses_to_chat_url(
                "https://upstream.example/v1/responses?trace=true&mode=compat"
            ),
            "https://upstream.example/v1/chat/completions?trace=true&mode=compat"
        );
    }

    /// 中转上游在流尾发多个带 finish_reason 的 chunk 时,`response.completed`
    /// 只能产生一次(含流结束兜底),否则客户端会在第一个 completed 后关闭
    /// 连接,后续重复事件把请求记成 499。
    #[test]
    fn chat_sse_emits_single_completed_despite_repeated_finish_chunks() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let chunk = |finish: serde_json::Value, usage: serde_json::Value| {
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chatcmpl-1",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": "hi" },
                        "finish_reason": finish,
                    }],
                    "usage": usage,
                })
            )
        };

        let events = converter.feed(chunk(Value::Null, Value::Null).as_bytes());
        assert_eq!(events.iter().filter(|e| e.contains("response.completed")).count(), 0);

        // 第一个 finish chunk → 唯一一次 completed(带 usage)
        let events =
            converter.feed(chunk(json!("stop"), json!({ "total_tokens": 7718 })).as_bytes());
        assert_eq!(events.iter().filter(|e| e.contains("response.completed")).count(), 1);
        assert!(events.last().is_some_and(|e| e.contains("7718")));

        // 第二个 finish chunk(中转上游重复下发)→ 不再产生 completed
        let events = converter.feed(chunk(json!("stop"), Value::Null).as_bytes());
        assert_eq!(events.iter().filter(|e| e.contains("response.completed")).count(), 0);

        // 流结束兜底 → 已经发过,不补发
        let events = converter.finish();
        assert_eq!(events.iter().filter(|e| e.contains("response.completed")).count(), 0);
    }

    /// 上游异常截断(从未出现 finish_reason)时,finish() 兜底仍应补一个 completed,
    /// 保证客户端能收到终结事件。
    #[test]
    fn chat_sse_finish_emits_completed_when_stream_never_finished() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let events = converter.feed(
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chatcmpl-1",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": "hi" },
                        "finish_reason": null,
                    }],
                })
            )
            .as_bytes(),
        );
        assert!(!events.is_empty());

        let events = converter.finish();
        assert_eq!(events.iter().filter(|e| e.contains("response.completed")).count(), 1);
        // 兜底 usage 是全 0 对象,字段齐全(强类型客户端解析要求 input_tokens 存在)
        assert!(events.iter().any(|e| e.contains("\"input_tokens\":0")));
        // 异常截断时残留文本也要进入 completed.output,不能丢
        assert!(
            events.iter().any(|e| e.contains("hi") && e.contains("response.completed")),
            "truncated text must appear in completed.output"
        );
    }

    /// 正文必须真正发出去:finish chunk 触发 flush 生成 output_text 事件,
    /// 且 completed.output 的 message item 携带全量文本。此前 text_started
    /// 永远不会被置位,flush 从不执行 → 客户端只收到 created+completed 的
    /// 空响应(codex 表现为"不可用")。
    #[test]
    fn chat_sse_emits_text_output_events_and_completed_output() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let chunk = |content: &str, finish: serde_json::Value| {
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chatcmpl-1",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": content },
                        "finish_reason": finish,
                    }],
                })
            )
        };

        let _ = converter.feed(chunk("你好，", Value::Null).as_bytes());
        let _ = converter.feed(chunk("世界", Value::Null).as_bytes());
        let events = converter.feed(chunk("", json!("stop")).as_bytes());

        // message item 事件序列(added + delta + done)
        assert!(
            events
                .iter()
                .any(|e| e.contains("response.output_item.added")
                    && e.contains("\"type\":\"message\"")),
            "message item added missing"
        );
        assert!(
            events.iter().any(|e| e.contains("response.output_text.delta")),
            "text delta missing"
        );
        assert!(
            events.iter().any(|e| e.contains("response.output_text.done")),
            "text done missing"
        );

        // completed.output 的 message item 携带全量文本
        let completed =
            events.iter().find(|e| e.contains("response.completed")).expect("completed event");
        assert!(
            completed.contains("你好，世界"),
            "full text missing in completed.output: {completed}"
        );
        assert!(completed.contains("\"output\":"), "completed.output missing");
    }

    /// 文本与 tool_calls 交错时,message item 的 added 事件只发一次。
    #[test]
    fn chat_sse_dedupes_message_item_added_across_flushes() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let text_chunk = format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl-1",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "前半段" },
                    "finish_reason": null,
                }],
            })
        );
        let tool_chunk = format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl-1",
                "choices": [{
                    "index": 0,
                    "delta": { "tool_calls": [{
                        "index": 2,
                        "id": "call_1",
                        "function": { "name": "exec", "arguments": "{}" },
                    }] },
                    "finish_reason": null,
                }],
            })
        );

        let _ = converter.feed(text_chunk.as_bytes());
        let events = converter.feed(tool_chunk.as_bytes());

        let message_added = events
            .iter()
            .filter(|e| {
                e.contains("response.output_item.added") && e.contains("\"type\":\"message\"")
            })
            .count();
        assert_eq!(message_added, 1, "message item added must be emitted once");
        // function_call item 同时进入事件流
        assert!(events.iter().any(|e| e.contains("\"type\":\"function_call\"")));
    }

    /// completed 事件的 usage 必须是 Responses 规范字段(input_tokens/output_tokens):
    /// 直接透传 chat 格式(prompt_tokens)会让 codex 等客户端解析
    /// `ResponseCompleted` 时报 `missing field input_tokens` 并断流重连。
    #[test]
    fn chat_sse_completed_usage_uses_responses_field_names() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let events = converter.feed(
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chatcmpl-1",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop",
                    }],
                    "usage": {
                        "prompt_tokens": 15671,
                        "completion_tokens": 3,
                        "total_tokens": 15674,
                        "prompt_tokens_details": { "cached_tokens": 100 },
                    },
                })
            )
            .as_bytes(),
        );

        let completed =
            events.iter().find(|e| e.contains("response.completed")).expect("completed event");
        assert!(completed.contains("\"input_tokens\":15671"), "got: {completed}");
        assert!(completed.contains("\"output_tokens\":3"), "got: {completed}");
        assert!(completed.contains("\"total_tokens\":15674"), "got: {completed}");
        assert!(completed.contains("\"cached_tokens\":100"), "got: {completed}");
        assert!(!completed.contains("prompt_tokens"), "chat field leaked: {completed}");
    }

    /// 上游 finish chunk 不带 usage 时,completed 的 usage 仍应是字段齐全的全 0 对象。
    #[test]
    fn chat_sse_completed_usage_falls_back_to_zeroed_object() {
        let mut converter = ChatSseToResponsesSse::new("gpt-5.6-sol");

        let events = converter.feed(
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chatcmpl-1",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop",
                    }],
                })
            )
            .as_bytes(),
        );

        let completed =
            events.iter().find(|e| e.contains("response.completed")).expect("completed event");
        assert!(completed.contains("\"input_tokens\":0"), "got: {completed}");
        assert!(completed.contains("\"output_tokens\":0"), "got: {completed}");
    }
}
