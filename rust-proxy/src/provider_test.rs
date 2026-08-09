//! 渠道连通性测试。
//!
//! 旧的连通性测试逻辑原本内联在 TS `src/provider-admin.ts` 的 `testProviderConnectivity`
//! 里：用渠道自己的 `targetBaseUrl` + 真实凭证直连上游。现在这套逻辑迁移到 Rust，
//! TS 侧改为通过本机 HTTP 调用本端点（`POST /admin/providers/{channel}/test`），
//! 字段语义与返回结构保持与旧实现一致，前端 `TestProviderResult` 无需改动。
//!
//! 部分上游（典型为推理类模型，如报错 `{"detail":"Stream must be set to true"}`）
//! 强制要求流式请求。探测默认走非流式（与旧实现一致），当上游错误明确表示“必须
//! 流式”时，自动带 `stream: true` 重试并解析 SSE，保证这类渠道也能测通。
//!
//! 测试过程与真实转发一样记录请求/响应日志（经 IPC → `rust-log-writer` → DB），
//! `source_request_type` 标记为 `connectivity_test`，便于在日志页排查，同时被用量
//! 统计排除，不会污染大盘。

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app_state::AppState;
use crate::config::{RouteAuthHeader, UpstreamType};
use crate::ipc::RustToTsMessage;

/// 上游探测超时，与 TS 旧实现 (PROVIDER_TEST_TIMEOUT_MS = 30000) 一致。
const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_PROMPT: &str = "Reply with exactly \"OK\"";
const MAX_TOKENS: u32 = 1024;
/// 连通性测试在日志里的来源标记，控制台据此区分展示并排除出用量统计。
const SOURCE_REQUEST_TYPE: &str = "connectivity_test";

#[derive(Deserialize, Default)]
pub struct TestRequestBody {
    #[serde(default)]
    pub model: Option<String>,
}

/// 与 TS 旧 `testProviderConnectivity` 返回的 body 同构，前端 `TestProviderResult` 直接消费。
#[derive(serde::Serialize)]
struct TestResult {
    status: String,
    #[serde(rename = "statusCode")]
    status_code: u16,
    message: String,
    #[serde(rename = "latencyMs")]
    latency_ms: u128,
    model: String,
    #[serde(rename = "rawResponse", skip_serializing_if = "Option::is_none")]
    raw_response: Option<Value>,
}

impl TestResult {
    fn new(
        status: &str,
        status_code: u16,
        message: String,
        latency_ms: u128,
        model: &str,
        raw_response: Option<Value>,
    ) -> Self {
        Self {
            status: status.to_string(),
            status_code,
            message,
            latency_ms,
            model: model.to_string(),
            raw_response,
        }
    }
}

/// 一次探测的完整结果：既用于拼装返回给控制台的 HTTP 响应，也用于写响应日志。
struct TestOutcome {
    result: TestResult,
    /// 是否走了流式重试（决定日志里 has_streaming_content）。
    is_stream: bool,
    /// 上游响应头快照，写响应日志时填 response_headers（连接失败/超时为空对象）。
    response_headers: Value,
}

impl TestOutcome {
    fn new(result: TestResult, is_stream: bool, response_headers: Value) -> Self {
        Self { result, is_stream, response_headers }
    }
}

/// 把上游响应头转成 JSON 对象（key→字符串值；非 UTF-8 值按空串兜底），
/// 与真实转发路径（proxy.rs）落库的 response_headers 结构保持一致。
fn header_map_to_json(headers: &HeaderMap) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        map.insert(
            name.as_str().to_string(),
            Value::String(value.to_str().unwrap_or("").to_string()),
        );
    }
    Value::Object(map)
}

/// 一次探测的结局：要么给出最终结果，要么提示需要切换为流式重试。
enum ProbeOutcome {
    Final(TestOutcome),
    RetryStreaming,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// 仅限本机 TS 控制台调用：要求请求携带 GATEWAY_API_KEY（x-api-key 或 Bearer）。
/// Rust 默认绑定 127.0.0.1，加上这个共享密钥做二次校验。
fn is_admin(headers: &HeaderMap, gateway_admin_key: &str) -> bool {
    if gateway_admin_key.is_empty() {
        return false;
    }
    if let Some(v) = headers.get("x-api-key").and_then(|h| h.to_str().ok())
        && v.trim() == gateway_admin_key
    {
        return true;
    }
    if let Some(v) = headers.get("authorization").and_then(|h| h.to_str().ok())
        && let Some(token) = v.strip_prefix("Bearer ")
        && token.trim() == gateway_admin_key
    {
        return true;
    }
    false
}

pub async fn test_provider_handler(
    State(state): State<Arc<AppState>>,
    Path(channel_name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TestRequestBody>,
) -> Response {
    if !is_admin(&headers, &state.gateway_admin_key) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "未授权" }))).into_response();
    }
    if !*state.config_synced.read().await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Rust 配置尚未同步，请稍后再试" })),
        )
            .into_response();
    }

    // 解析渠道配置（读锁内取出后续网络请求所需字段，再释放锁）
    let (upstream_type, base_url, auth_header, auth_value, test_model) = {
        let rt = state.routing.read().await;
        let entry = match rt.providers.get(&channel_name) {
            Some(e) => e,
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({ "error": "Provider 不存在" })))
                    .into_response();
            }
        };
        let auth = match entry.auth.as_ref() {
            Some(a) if !a.value.is_empty() => a.clone(),
            _ => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": "认证未配置" })))
                    .into_response();
            }
        };
        let model = body
            .model
            .clone()
            .or_else(|| entry.models.as_ref().and_then(|m| m.first().map(|m| m.model.clone())));
        let model = match model {
            Some(m) => m,
            None => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": "未配置模型" })))
                    .into_response();
            }
        };
        (
            entry.upstream_type.clone(),
            entry.target_base_url.trim_end_matches('/').to_string(),
            auth.header,
            auth.value,
            model,
        )
    };

    // 构造上游探测请求（默认非流式，与旧实现一致）
    let (test_url, is_anthropic) = build_test_target(&upstream_type, &base_url);

    let request_id = uuid::Uuid::new_v4().to_string();
    let created_at = now_ms();

    // 记忆命中：该 (channel, model) 已知「上游强制流式」，直接走流式，跳过必失败的非流式尝试。
    // 内存级，进程重启失效。
    let mem_key = stream_mem_key(&channel_name, &test_model);
    let starts_streaming = state.test_requires_stream.read().await.contains(&mem_key);

    // 先写请求日志：记录「要拿哪个渠道+模型去探哪个上游」，与真实转发的请求日志同构。
    emit_request_log(
        &state,
        &request_id,
        created_at,
        &channel_name,
        &upstream_type,
        &test_url,
        &test_model,
        &auth_header,
        &auth_value,
        is_anthropic,
        starts_streaming,
    );

    let outcome = if starts_streaming {
        run_streaming_probe(&state, &test_url, &auth_header, &auth_value, &test_model, is_anthropic)
            .await
    } else {
        let req_body = build_probe_body(&test_model, false);
        let start = Instant::now();
        let send_result = tokio::time::timeout(
            TEST_TIMEOUT,
            build_probe_request(&state, &test_url, &auth_header, &auth_value, is_anthropic)
                .json(&req_body)
                .send(),
        )
        .await;
        let latency_ms = start.elapsed().as_millis();

        match send_result {
            Ok(Ok(resp)) => {
                match handle_upstream_response(resp, &test_model, latency_ms, is_anthropic).await {
                    ProbeOutcome::Final(o) => o,
                    ProbeOutcome::RetryStreaming => {
                        // 首次探测到上游强制流式：记下，后续该组合直接走流式
                        state.test_requires_stream.write().await.insert(mem_key);
                        run_streaming_probe(
                            &state,
                            &test_url,
                            &auth_header,
                            &auth_value,
                            &test_model,
                            is_anthropic,
                        )
                        .await
                    }
                }
            }
            Ok(Err(err)) => connection_error(&test_model, latency_ms, &err),
            Err(_) => timeout_error(&test_model),
        }
    };

    // 探测结束：写响应日志（状态/耗时/上游响应体/用量），与真实转发的响应日志同构。
    emit_response_log(&state, &request_id, created_at, &outcome);

    ok(outcome.result)
}

/// 连通性测试流式记忆的 key：按 (channel, model) 组合，避免不同渠道同名模型互相影响。
fn stream_mem_key(channel: &str, model: &str) -> String {
    format!("{channel}\u{0}{model}")
}

/// 带 `stream: true` 重试上游探测，解析 SSE 聚合出文本/思考内容/stop_reason/usage。
async fn run_streaming_probe(
    state: &Arc<AppState>,
    test_url: &str,
    auth_header: &RouteAuthHeader,
    auth_value: &str,
    test_model: &str,
    is_anthropic: bool,
) -> TestOutcome {
    let stream_body = build_probe_body(test_model, true);

    let start = Instant::now();
    let send_result = tokio::time::timeout(
        TEST_TIMEOUT,
        build_probe_request(state, test_url, auth_header, auth_value, is_anthropic)
            .header("accept", "text/event-stream")
            .json(&stream_body)
            .send(),
    )
    .await;
    let latency_ms = start.elapsed().as_millis();

    match send_result {
        Ok(Ok(resp)) => handle_stream_response(resp, test_model, latency_ms, is_anthropic).await,
        Ok(Err(err)) => connection_error(test_model, latency_ms, &err),
        Err(_) => timeout_error(test_model),
    }
}

/// 探测请求体：非流式与流式共用，仅 `stream` 字段不同。
fn build_probe_body(model: &str, stream: bool) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": PROBE_PROMPT }],
        "max_tokens": MAX_TOKENS,
    });
    if stream {
        body["stream"] = json!(true);
    }
    body
}

/// 构造一条到上游的 POST 探测请求（不含 body，由调用方 `.json(&body)` 注入）。
fn build_probe_request(
    state: &Arc<AppState>,
    test_url: &str,
    auth_header: &RouteAuthHeader,
    auth_value: &str,
    is_anthropic: bool,
) -> reqwest::RequestBuilder {
    let mut req = state.http_client.post(test_url).header("content-type", "application/json");
    match auth_header {
        RouteAuthHeader::Authorization => {
            req = req.header("authorization", auth_value);
        }
        RouteAuthHeader::XApiKey => {
            req = req.header("x-api-key", auth_value);
        }
    }
    if is_anthropic {
        req = req.header("anthropic-version", "2023-06-01");
    }
    req
}

async fn handle_upstream_response(
    resp: reqwest::Response,
    test_model: &str,
    latency_ms: u128,
    is_anthropic: bool,
) -> ProbeOutcome {
    let status_code = resp.status().as_u16();
    let hdrs = header_map_to_json(resp.headers());

    if resp.status().is_success() {
        let data: Value = resp.json().await.unwrap_or(Value::Null);
        let (content, has_thinking, stop_reason) = extract_content(&data, is_anthropic);
        return ProbeOutcome::Final(TestOutcome::new(
            build_success_result(
                &content,
                has_thinking,
                &stop_reason,
                data,
                status_code,
                latency_ms,
                test_model,
            ),
            false,
            hdrs,
        ));
    }

    // 非 2xx：解析上游错误，给出友好提示（与 TS 旧实现一致）
    let error_text = resp.text().await.unwrap_or_default();

    // 上游要求必须流式：切换为 stream=true 重试
    if looks_like_stream_required(&error_text) {
        return ProbeOutcome::RetryStreaming;
    }

    let error_detail = parse_error_detail(&error_text);
    let friendly = if !error_detail.is_empty() {
        format!("HTTP {status_code}: {error_detail}")
    } else {
        match status_code {
            401 => "API Key 无效或已过期".to_string(),
            403 => "无访问权限，请检查 API Key 权限设置".to_string(),
            429 => "请求频率超限，请稍后再试".to_string(),
            400 => "请求参数错误，请检查模型名称是否正确".to_string(),
            other => format!("HTTP {other}"),
        }
    };
    let raw = if error_text.is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<Value>(&error_text)
                .unwrap_or_else(|_| Value::String(error_text.chars().take(1000).collect())),
        )
    };
    ProbeOutcome::Final(TestOutcome::new(
        TestResult::new("error", status_code, friendly, latency_ms, test_model, raw),
        false,
        hdrs,
    ))
}

/// 处理流式重试的响应：解析 SSE 并按非流式结构合成 rawResponse，复用成功判定逻辑。
async fn handle_stream_response(
    resp: reqwest::Response,
    test_model: &str,
    latency_ms: u128,
    is_anthropic: bool,
) -> TestOutcome {
    let status_code = resp.status().as_u16();
    let hdrs = header_map_to_json(resp.headers());

    if !resp.status().is_success() {
        let error_text = resp.text().await.unwrap_or_default();
        let error_detail = parse_error_detail(&error_text);
        let friendly = if !error_detail.is_empty() {
            format!("HTTP {status_code}: {error_detail}")
        } else {
            format!("HTTP {status_code}")
        };
        let raw = if error_text.is_empty() {
            None
        } else {
            Some(
                serde_json::from_str::<Value>(&error_text)
                    .unwrap_or_else(|_| Value::String(error_text.chars().take(1000).collect())),
            )
        };
        return TestOutcome::new(
            TestResult::new("error", status_code, friendly, latency_ms, test_model, raw),
            true,
            hdrs,
        );
    }

    let body = resp.text().await.unwrap_or_default();
    let (content, has_thinking, stop_reason, usage) = parse_streamed_content(&body, is_anthropic);
    let raw = if is_anthropic {
        let mut raw = json!({
            "content": [{ "type": "text", "text": content }],
            "stop_reason": stop_reason,
        });
        if let Some(u) = usage {
            raw["usage"] = u;
        }
        raw
    } else {
        let mut raw = json!({
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": stop_reason,
            }],
        });
        if let Some(u) = usage {
            raw["usage"] = u;
        }
        raw
    };

    TestOutcome::new(
        build_success_result(
            &content,
            has_thinking,
            &stop_reason,
            raw,
            status_code,
            latency_ms,
            test_model,
        ),
        true,
        hdrs,
    )
}

/// 成功响应的统一判定：含 OK 即正常；思考模型/被 max_tokens 截断视为连通正常；其余报空。
fn build_success_result(
    content: &str,
    has_thinking: bool,
    stop_reason: &str,
    raw: Value,
    status_code: u16,
    latency_ms: u128,
    test_model: &str,
) -> TestResult {
    if content.to_uppercase().contains("OK") {
        return TestResult::new(
            "ok",
            status_code,
            "模型响应正常".to_string(),
            latency_ms,
            test_model,
            Some(raw),
        );
    }
    if has_thinking || stop_reason == "max_tokens" || stop_reason == "stop" {
        return TestResult::new(
            "ok",
            status_code,
            "模型连通正常（思考模型，输出被截断）".to_string(),
            latency_ms,
            test_model,
            Some(raw),
        );
    }
    TestResult::new(
        "error",
        status_code,
        format!("HTTP {status_code} - 响应内容为空或不含OK"),
        latency_ms,
        test_model,
        Some(raw),
    )
}

/// 从上游响应里提取文本内容 / 是否思考模型 / stop_reason，逻辑与 TS 旧实现一致。
fn extract_content(data: &Value, is_anthropic: bool) -> (String, bool, String) {
    if is_anthropic {
        let mut content = String::new();
        let mut has_thinking = false;
        if let Some(blocks) = data.get("content").and_then(|c| c.as_array()) {
            for block in blocks {
                let Some(t) = block.get("type").and_then(|v| v.as_str()) else {
                    continue;
                };
                if t == "text" {
                    if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                        content = s.to_string();
                    }
                } else if t == "thinking" {
                    has_thinking = true;
                    if content.is_empty()
                        && let Some(s) = block.get("thinking").and_then(|v| v.as_str())
                    {
                        content = s.to_string();
                    }
                }
            }
        }
        let stop_reason =
            data.get("stop_reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
        (content, has_thinking, stop_reason)
    } else {
        let first_choice = data.get("choices").and_then(|c| c.get(0));
        let content = first_choice
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut has_thinking = false;
        let eff_content = if content.is_empty() {
            if let Some(rc) = first_choice
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("reasoning_content"))
                .and_then(|v| v.as_str())
            {
                has_thinking = true;
                rc.to_string()
            } else {
                String::new()
            }
        } else {
            content
        };
        let stop_reason = first_choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        (eff_content, has_thinking, stop_reason)
    }
}

/// 解析流式 SSE 响应，聚合出文本内容 / 是否思考模型 / stop_reason / usage。
///
/// - OpenAI 协议：每行 `data: {json}`，增量在 `choices[0].delta`，usage 在末尾 chunk。
/// - Anthropic 协议：按 event 区分，文本/思考增量在 `content_block_delta`，stop_reason 在 `message_delta`。
fn parse_streamed_content(body: &str, is_anthropic: bool) -> (String, bool, String, Option<Value>) {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut has_thinking = false;
    let mut stop_reason = String::new();
    // usage 可能跨多个事件分发（Anthropic: message_start 给 input，message_delta 给 output），
    // 这里按键合并累加，避免后到的事件覆盖掉前面的字段。
    let mut usage_map: serde_json::Map<String, Value> = serde_json::Map::new();

    for line in body.lines() {
        let payload = match line.strip_prefix("data:").map(str::trim) {
            Some(p) if !p.is_empty() && p != "[DONE]" => p,
            _ => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };

        if is_anthropic {
            let event_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match event_type {
                "message_start" => {
                    if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                        merge_usage(&mut usage_map, u);
                    }
                }
                "content_block_delta" => {
                    if let Some(delta) = v.get("delta") {
                        match delta.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                            "text_delta" => {
                                if let Some(t) = delta.get("text").and_then(|x| x.as_str()) {
                                    content.push_str(t);
                                }
                            }
                            "thinking_delta" => {
                                has_thinking = true;
                                if let Some(t) = delta.get("thinking").and_then(|x| x.as_str()) {
                                    reasoning.push_str(t);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "message_delta" => {
                    if let Some(sr) =
                        v.get("delta").and_then(|d| d.get("stop_reason")).and_then(|x| x.as_str())
                    {
                        stop_reason = sr.to_string();
                    }
                    if let Some(u) = v.get("usage") {
                        merge_usage(&mut usage_map, u);
                    }
                }
                _ => {}
            }
        } else {
            if let Some(u) = v.get("usage") {
                merge_usage(&mut usage_map, u);
            }
            let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
                continue;
            };
            if let Some(delta) = choice.get("delta") {
                if let Some(t) = delta.get("content").and_then(|x| x.as_str()) {
                    content.push_str(t);
                }
                if let Some(rc) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
                    has_thinking = true;
                    reasoning.push_str(rc);
                }
            }
            if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str())
                && !fr.is_empty()
            {
                stop_reason = fr.to_string();
            }
        }
    }

    // 与 extract_content 语义一致：文本为空时回退到思考内容
    if content.is_empty() && has_thinking {
        content = reasoning;
    }
    let usage = if usage_map.is_empty() { None } else { Some(Value::Object(usage_map)) };
    (content, has_thinking, stop_reason, usage)
}

/// 将一个 usage 对象按键合并进累加 map（后到不覆盖前到，仅补充缺失字段）。
fn merge_usage(map: &mut serde_json::Map<String, Value>, value: &Value) {
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            map.entry(k.clone()).or_insert(v.clone());
        }
    }
}

/// 判断上游错误是否表示“必须使用流式”，用于触发 stream=true 重试。
/// 典型如 `{"detail":"Stream must be set to true"}`、`streaming is required` 等。
fn looks_like_stream_required(text: &str) -> bool {
    let l = text.to_lowercase();
    l.contains("stream")
        && (l.contains("must")
            || l.contains("set to true")
            || l.contains("requir")
            || l.contains("needs to"))
}

/// 解析上游错误文本，优先取 `error.message` / `message` / `error.type`，否则截断原文。
fn parse_error_detail(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(msg) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            return msg.to_string();
        }
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
        if let Some(t) = v.get("error").and_then(|e| e.get("type")).and_then(|m| m.as_str()) {
            return t.to_string();
        }
    }
    text.chars().take(200).collect()
}

/// 路径拼接规则与 TS 旧实现一致：
/// - OpenAI：不补 /v1，用户需在 targetBaseUrl 中包含 /v1
/// - Anthropic：不含 /v1 则补，这是行业惯例
fn build_test_target(upstream_type: &UpstreamType, base_url: &str) -> (String, bool) {
    match upstream_type {
        UpstreamType::Anthropic => {
            let v1_prefix = if base_url.ends_with("/v1") { "" } else { "/v1" };
            (format!("{base_url}{v1_prefix}/messages"), true)
        }
        UpstreamType::OpenAI => (format!("{base_url}/chat/completions"), false),
    }
}

fn connection_error(test_model: &str, latency_ms: u128, err: &reqwest::Error) -> TestOutcome {
    TestOutcome::new(
        TestResult::new("error", 0, format!("连接失败: {err}"), latency_ms, test_model, None),
        false,
        json!({}),
    )
}

fn timeout_error(test_model: &str) -> TestOutcome {
    TestOutcome::new(
        TestResult::new(
            "error",
            0,
            "请求超时（30秒）".to_string(),
            TEST_TIMEOUT.as_millis(),
            test_model,
            None,
        ),
        false,
        json!({}),
    )
}

fn ok(result: TestResult) -> Response {
    (StatusCode::OK, Json(result)).into_response()
}

// ── 日志（与真实转发同构，经 IPC 写入 console_requests） ───────────────────────

/// 构造发往上游的请求头 map（content-type + 认证 + 可选 anthropic-version），用于日志。
fn build_forward_headers_value(
    auth_header: &RouteAuthHeader,
    auth_value: &str,
    is_anthropic: bool,
) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("content-type".to_string(), Value::String("application/json".to_string()));
    let key = match auth_header {
        RouteAuthHeader::Authorization => "authorization",
        RouteAuthHeader::XApiKey => "x-api-key",
    };
    map.insert(key.to_string(), Value::String(auth_value.to_string()));
    if is_anthropic {
        map.insert("anthropic-version".to_string(), Value::String("2023-06-01".to_string()));
    }
    Value::Object(map)
}

/// 发送请求日志，字段与 `proxy.rs::send_request_log` 对齐：
/// route_prefix=渠道、target_url=上游探测地址、forwarded_payload=探测请求体、
/// source_request_type=connectivity_test，api_key_id/name 留空（测试不走业务 Key）。
#[allow(clippy::too_many_arguments)]
fn emit_request_log(
    state: &AppState,
    request_id: &str,
    created_at: u64,
    channel_name: &str,
    upstream_type: &UpstreamType,
    test_url: &str,
    test_model: &str,
    auth_header: &RouteAuthHeader,
    auth_value: &str,
    is_anthropic: bool,
    starts_streaming: bool,
) {
    let ipc = state.ipc.clone();
    let rid = request_id.to_string();
    let rp = channel_name.to_string();
    let ut = format!("{upstream_type:?}").to_lowercase();
    let path = format!("/admin/providers/{channel_name}/test");
    let tu = test_url.to_string();
    let rm = test_model.to_string();
    let fp = build_probe_body(test_model, starts_streaming).to_string();
    let fh = build_forward_headers_value(auth_header, auth_value, is_anthropic);

    tokio::spawn(async move {
        ipc.send(RustToTsMessage::RequestLog {
            request_id: rid,
            created_at,
            method: "POST".to_string(),
            route_prefix: rp,
            upstream_type: ut,
            path,
            url: tu.clone(),
            target_url: tu,
            request_model: rm,
            original_payload: None,
            forwarded_payload: Some(fp),
            original_headers: json!({}),
            forward_headers: fh,
            api_key_id: None,
            api_key_name: None,
            source_request_type: SOURCE_REQUEST_TYPE.to_string(),
        });
    });
}

/// 发送响应日志：状态码取上游返回（连接失败/超时为 0），响应体=上游响应 JSON，
/// 用量/stop_reason 从响应体里尽力提取；耗时由 latency_ms 推出 completed_at。
fn emit_response_log(state: &AppState, request_id: &str, created_at: u64, outcome: &TestOutcome) {
    let result = &outcome.result;
    let raw = result.raw_response.as_ref();
    let usage = raw.and_then(|r| r.get("usage")).cloned();
    let response_payload = raw.map(|v| v.to_string());
    let response_body_bytes = response_payload.as_ref().map_or(0, String::len) as u64;
    let (input_tokens, output_tokens, total_tokens) = extract_token_counts(&usage);
    let stop_reason = raw.and_then(extract_stop_reason);

    let latency_ms = result.latency_ms as u64;
    let completed_at = created_at.checked_add(latency_ms);
    let status_text = if result.status_code == 0 {
        "ERROR".to_string()
    } else if result.status == "ok" {
        "OK".to_string()
    } else {
        "ERROR".to_string()
    };

    let ipc = state.ipc.clone();
    let rid = request_id.to_string();
    let status = result.status_code;
    let resp_model = result.model.clone();
    let is_stream = outcome.is_stream;
    let response_headers = outcome.response_headers.clone();

    tokio::spawn(async move {
        ipc.send(RustToTsMessage::ResponseLog {
            request_id: rid,
            response_status: status,
            response_status_text: status_text,
            response_headers,
            response_body_bytes,
            first_chunk_at: Some(created_at),
            first_token_at: Some(created_at),
            completed_at,
            has_streaming_content: is_stream,
            response_model: Some(resp_model),
            stop_reason,
            input_tokens,
            output_tokens,
            total_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            cached_input_tokens: None,
            response_payload,
        });
    });
}

/// 从 usage 对象里取 input/output/total tokens，键名兼容 OpenAI 与 Anthropic 两套。
fn extract_token_counts(usage: &Option<Value>) -> (Option<u32>, Option<u32>, Option<u32>) {
    match usage {
        Some(u) => (
            u.get("prompt_tokens").or(u.get("input_tokens")).and_then(tokens_as_u32),
            u.get("completion_tokens").or(u.get("output_tokens")).and_then(tokens_as_u32),
            u.get("total_tokens").and_then(tokens_as_u32),
        ),
        None => (None, None, None),
    }
}

fn tokens_as_u32(v: &Value) -> Option<u32> {
    v.as_u64().map(|n| n as u32)
}

/// 从上游响应体里提取 stop_reason：Anthropic 顶层 `stop_reason`；OpenAI `choices[0].finish_reason`。
fn extract_stop_reason(raw: &Value) -> Option<String> {
    if let Some(sr) = raw.get("stop_reason").and_then(|v| v.as_str()) {
        return Some(sr.to_string());
    }
    raw.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
