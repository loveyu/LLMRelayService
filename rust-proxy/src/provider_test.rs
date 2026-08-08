//! 渠道连通性测试。
//!
//! 旧的连通性测试逻辑原本内联在 TS `src/provider-admin.ts` 的 `testProviderConnectivity`
//! 里：用渠道自己的 `targetBaseUrl` + 真实凭证直连上游。现在这套逻辑迁移到 Rust，
//! TS 侧改为通过本机 HTTP 调用本端点（`POST /admin/providers/{channel}/test`），
//! 字段语义与返回结构保持与旧实现一致，前端 `TestProviderResult` 无需改动。

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app_state::AppState;
use crate::config::{RouteAuthHeader, UpstreamType};

/// 上游探测超时，与 TS 旧实现 (PROVIDER_TEST_TIMEOUT_MS = 30000) 一致。
const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_PROMPT: &str = "Reply with exactly \"OK\"";
const MAX_TOKENS: u32 = 1024;

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
            .or_else(|| entry.models.as_ref().and_then(|m| m.first()).map(|m| m.model.clone()));
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

    // 构造上游探测请求
    let (test_url, is_anthropic) = build_test_target(&upstream_type, &base_url);
    let mut req = state.http_client.post(&test_url).header("content-type", "application/json");
    match auth_header {
        RouteAuthHeader::Authorization => {
            req = req.header("authorization", auth_value.as_str());
        }
        RouteAuthHeader::XApiKey => {
            req = req.header("x-api-key", auth_value.as_str());
        }
    }
    if is_anthropic {
        req = req.header("anthropic-version", "2023-06-01");
    }
    let req_body = json!({
        "model": test_model,
        "messages": [{ "role": "user", "content": PROBE_PROMPT }],
        "max_tokens": MAX_TOKENS,
    });

    let start = Instant::now();
    let send_result = tokio::time::timeout(TEST_TIMEOUT, req.json(&req_body).send()).await;
    let latency_ms = start.elapsed().as_millis();

    match send_result {
        Ok(Ok(resp)) => handle_upstream_response(resp, &test_model, latency_ms, is_anthropic).await,
        Ok(Err(err)) => (
            StatusCode::OK,
            Json(TestResult::new(
                "error",
                0,
                format!("连接失败: {err}"),
                latency_ms,
                &test_model,
                None,
            )),
        )
            .into_response(),
        Err(_) => (
            StatusCode::OK,
            Json(TestResult::new(
                "error",
                0,
                "请求超时（30秒）".to_string(),
                TEST_TIMEOUT.as_millis(),
                &test_model,
                None,
            )),
        )
            .into_response(),
    }
}

async fn handle_upstream_response(
    resp: reqwest::Response,
    test_model: &str,
    latency_ms: u128,
    is_anthropic: bool,
) -> Response {
    let status_code = resp.status().as_u16();

    if resp.status().is_success() {
        let data: Value = resp.json().await.unwrap_or(Value::Null);
        let (content, has_thinking, stop_reason) = extract_content(&data, is_anthropic);
        if content.to_uppercase().contains("OK") {
            return ok(TestResult::new(
                "ok",
                status_code,
                "模型响应正常".to_string(),
                latency_ms,
                test_model,
                Some(data),
            ));
        }
        if has_thinking || stop_reason == "max_tokens" || stop_reason == "stop" {
            return ok(TestResult::new(
                "ok",
                status_code,
                "模型连通正常（思考模型，输出被截断）".to_string(),
                latency_ms,
                test_model,
                Some(data),
            ));
        }
        return ok(TestResult::new(
            "error",
            status_code,
            format!("HTTP {status_code} - 响应内容为空或不含OK"),
            latency_ms,
            test_model,
            Some(data),
        ));
    }

    // 非 2xx：解析上游错误，给出友好提示（与 TS 旧实现一致）
    let error_text = resp.text().await.unwrap_or_default();
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
    ok(TestResult::new("error", status_code, friendly, latency_ms, test_model, raw))
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

fn ok(result: TestResult) -> Response {
    (StatusCode::OK, Json(result)).into_response()
}
