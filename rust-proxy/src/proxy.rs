use crate::app_state::AppState;
use crate::auth;
use crate::circuit_breaker::{self, Admission};
use crate::concurrency_limit::ConcurrencyPermit;
use crate::failover::{self, FailoverTrigger};
use crate::ipc::RustToTsMessage;
use crate::rate_limit_cooldown;
use crate::responses::{self, ChatSseToResponsesSse};
use crate::routing::{self, RouteResult};
use crate::sse_observer::{ObservingSseStream, SseObserverHandle};
use crate::transform::{self, ModelRewriter};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use bytes::Bytes;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const HOP_BY_HOP_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "accept-encoding",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

const CLIENT_CLOSED_REQUEST_STATUS: u16 = 499;

/// 将并发 permit 绑定到下游 body 的生命周期；SSE 直到客户端/上游关闭才释放槽位。
struct ReleaseConcurrencyBody {
    inner: Body,
    _permit: ConcurrencyPermit,
}

impl http_body::Body for ReleaseConcurrencyBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }
}

/// Records a downstream disconnect when Axum drops the in-flight handler future.
/// The guard is armed only after RequestLog has entered the same IPC queue, so the
/// sequential TS consumer always has a row to update with the 499 terminal event.
struct ClientDisconnectLogGuard {
    ipc: Arc<crate::ipc::IpcSender>,
    request_id: String,
    armed: AtomicBool,
}

impl ClientDisconnectLogGuard {
    fn new(ipc: Arc<crate::ipc::IpcSender>, request_id: String) -> Self {
        Self { ipc, request_id, armed: AtomicBool::new(false) }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }
}

impl Drop for ClientDisconnectLogGuard {
    fn drop(&mut self) {
        if !self.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let disconnected_at =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        self.ipc.send(RustToTsMessage::ResponseLog {
            request_id: self.request_id.clone(),
            response_status: CLIENT_CLOSED_REQUEST_STATUS,
            response_status_text: "Client Closed Request".to_string(),
            response_headers: serde_json::json!({}),
            response_body_bytes: 0,
            first_chunk_at: None,
            first_token_at: None,
            completed_at: Some(disconnected_at),
            has_streaming_content: false,
            response_model: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            cached_input_tokens: None,
            response_payload: Some("客户端在响应完成前断开连接".to_string()),
            disconnect_source: Some("client".to_string()),
            disconnected_at: Some(disconnected_at),
        });
    }
}

fn hop_by_hop_set() -> &'static HashSet<String> {
    use std::sync::OnceLock;
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| HOP_BY_HOP_HEADERS.iter().map(|s| s.to_string()).collect())
}

fn build_forward_headers(
    original_headers: &HeaderMap,
    route: &RouteResult,
) -> reqwest::header::HeaderMap {
    let hop_by_hop = hop_by_hop_set();
    let mut fwd = reqwest::header::HeaderMap::new();

    for (key, value) in original_headers.iter() {
        let lower = key.as_str().to_lowercase();
        if hop_by_hop.contains(&lower) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            let _ = fwd.insert(
                reqwest::header::HeaderName::from_bytes(key.as_str().as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
    }

    if let (Some(header), Some(value)) = (&route.auth_header, &route.auth_value) {
        fwd.remove("authorization");
        fwd.remove("x-api-key");
        let _ = fwd.insert(
            reqwest::header::HeaderName::from_bytes(header.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
    }

    fwd
}

fn redacted_headers(headers: &HeaderMap) -> Value {
    let mut values = Map::new();
    for (name, value) in headers {
        let value = if matches!(name.as_str(), "authorization" | "x-api-key") {
            "[REDACTED]".to_string()
        } else {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        };
        values.insert(name.to_string(), Value::String(value));
    }
    Value::Object(values)
}

pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let created_at =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    let disconnect_guard = ClientDisconnectLogGuard::new(state.ipc.clone(), request_id.clone());
    let result = proxy_handler_inner(
        state,
        method,
        uri,
        headers,
        body,
        request_id,
        created_at,
        &disconnect_guard,
    )
    .await;
    disconnect_guard.disarm();
    result
}

#[allow(clippy::too_many_arguments)]
async fn proxy_handler_inner(
    state: Arc<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    request_id: String,
    created_at: u64,
    disconnect_guard: &ClientDisconnectLogGuard,
) -> Result<Response, StatusCode> {
    state.wait_for_config_sync().await;

    let _t_start = Instant::now();

    let pathname = uri.path();
    let search = uri.query().unwrap_or("");

    let (stripped_path, forced_type) = routing::parse_type_forced_prefix(pathname);
    let model = if method == Method::POST { extract_model_from_body(&body) } else { String::new() };

    // Resolve initial route
    let rt = state.routing.read().await;
    let initial_route = resolve_route(
        &stripped_path,
        search,
        &model,
        forced_type.clone(),
        &rt.providers,
        &rt.aliases,
    );
    let failover_policy = rt.failover.clone();
    let api_keys = rt.api_keys.clone();
    let gateway_key = state.gateway_admin_key.clone();
    drop(rt);

    let initial_route = match initial_route {
        Some(r) => r,
        None => {
            let headers = redacted_headers(&headers);
            let preview_len = body.len().min(4 * 1024);
            let body_preview = String::from_utf8_lossy(&body[..preview_len]);
            warn!(
                method = %method,
                path = pathname,
                query = search,
                model,
                headers = %headers,
                body_preview = %body_preview,
                "No route found"
            );
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // Authenticate
    let auth_result = match auth::authenticate(&headers, &initial_route, &gateway_key, &api_keys) {
        Ok(a) => a,
        Err((status, msg)) => {
            warn!("Auth failed: {msg}");
            return Err(status);
        }
    };

    // Enforce per-key model allowlist
    if !auth_result.allowed_models.is_empty() {
        if model.is_empty() || model == "unknown" {
            warn!("Cannot determine request model for API key with model restrictions");
            return Err(StatusCode::FORBIDDEN);
        }
        if !auth::is_model_allowed(&model, &auth_result.allowed_models) {
            warn!("Model '{model}' not in API key allowlist");
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // Authoritative provider type for this request (from the resolved initial route).
    // All failover fallbacks are constrained to this type so an Anthropic request can
    // never be forwarded to an OpenAI upstream (whose body format is incompatible) —
    // forwarding across types is always invalid, and the last same-type upstream's
    // error is what we want to surface to the client when everything fails.
    let request_type = initial_route.upstream_type.clone();

    // Collect all candidate routes (initial + potential fallbacks)
    let mut active_routes = vec![initial_route.clone()];
    let mut attempt_index: usize = 0;
    let mut retry_count: u32 = 0;

    // Failover observability: accumulate failed route labels + last trigger reason so the
    // request log can show the failover trajectory (matches TS `index.ts` semantics).
    let mut failed_route_chain: Vec<String> = Vec::new();
    let mut failover_reason: Option<String> = None;
    let mut initial_response_status: Option<u16> = None;
    let mut initial_response_status_text: Option<String> = None;
    let mut initial_completed_at: Option<u64> = None;
    // 链上最后一个真实收到的上游错误响应快照:当后续渠道只出网络错误/超时/全部冷却
    // 耗尽时,回传该响应而不是网关自造的 502/504,保证客户端拿到真实的 HTTP 错误。
    let mut last_upstream_error: Option<CachedUpstreamError> = None;
    let rate_limit_cooldown_enabled =
        failover_policy.enabled && failover_policy.retry_on_status_codes.contains(&429);

    loop {
        if attempt_index >= active_routes.len() {
            warn!("All routes exhausted for {}", pathname);
            if let Some(cached) = last_upstream_error.as_ref() {
                return Ok(return_cached_upstream_error(
                    &state,
                    &request_id,
                    created_at,
                    cached,
                    None,
                ));
            }
            emit_terminal_response_log(
                &state,
                &request_id,
                created_at,
                502,
                "BAD_GATEWAY",
                serde_json::json!({}),
                Some(format!("所有上游路由均已失败: {pathname}")),
                0,
                None,
                None,
            );
            return Err(StatusCode::BAD_GATEWAY);
        }

        let route = active_routes[attempt_index].clone();
        let is_retry = attempt_index == 0 && retry_count > 0;
        let route_model = rate_limit_cooldown::route_model(route.resolved_model.as_deref(), &model);

        // 规则是数据库配置、计数是 Rust 内存。满载时不发送上游请求，直接寻找下一个候选。
        let concurrency_rule = {
            let rt = state.routing.read().await;
            route.concurrency_rule_id.as_ref().and_then(|id| rt.concurrency_rules.get(id).cloned())
        };
        let mut concurrency_permit = concurrency_rule.as_ref().and_then(|rule| {
            state.concurrency_limits.try_acquire(
                &rule.id,
                rule.max_concurrency,
                &route.channel_name,
                route_model,
            )
        });
        if let Some(rule) = concurrency_rule.as_ref()
            && concurrency_permit.is_none()
        {
            let skipped = format!("{} (并发规则 {} 已满)", describe_route(&route), rule.name);
            failed_route_chain.push(skipped.clone());
            if advance_to_next_route(
                &mut active_routes,
                &mut attempt_index,
                &failover_policy,
                &model,
                &state,
                pathname,
                search,
                request_type.clone(),
                &stripped_path,
            ) {
                failover_reason = Some("concurrency_limit".to_string());
                continue;
            }
            // 所有候选都已满：按可用性优先原则放行当前渠道，交给上游/外层自行排队。
            warn!(
                "All candidate routes are concurrency-limited; bypassing limit for {}",
                route.channel_name
            );
            concurrency_permit = None;
        }
        if concurrency_permit.is_none() {
            concurrency_permit =
                Some(state.concurrency_limits.track(&route.channel_name, route_model));
        }

        let t_total = Instant::now();

        // ── OpenAI Responses API handling ──────────────────────────────
        // Route matching uses the type-prefix-normalized path. Responses handling must do
        // the same; otherwise `/openai/v1/responses` is forwarded unchanged and a
        // `chat_compat` provider receives the unsupported upstream `/v1/responses` path.
        // The resolved target also covers explicit `/providers/{name}/...` routes.
        let is_responses = responses::is_responses_request(&stripped_path, &route.target_url);
        let responses_mode = route.responses_mode.as_ref();
        let mut converting_responses = false;

        // 该路由无法承接本次 Responses 请求(显式 disabled 或未配置支持)。
        // 与冷却一样按"跳过该渠道"处理:初始路由命中时也继续 failover 到
        // 同模型/站点策略下支持 Responses 的渠道,只有全部不可用才返回 501。
        let responses_blocked = is_responses
            && matches!(responses_mode, Some(crate::config::OpenAiResponsesMode::Disabled) | None);

        if !responses_blocked
            && is_responses
            && matches!(responses_mode, Some(crate::config::OpenAiResponsesMode::ChatCompat))
        {
            converting_responses = true;
        }

        let target_url = if converting_responses {
            responses::rewrite_responses_to_chat_url(&route.target_url)
        } else {
            route.target_url.clone()
        };

        let fwd_headers = build_forward_headers(&headers, &route);

        // Prepare request body (Anthropic transforms + model name rewriting)
        let request_body = {
            let mut b = body.to_vec();
            // Responses chat_compat: convert before model rewrite
            if converting_responses && !b.is_empty() {
                b = responses::convert_responses_to_chat_request(&b).map_err(|(status, msg)| {
                    warn!("Responses chat compat error: {msg}");
                    StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST)
                })?;
            }
            // Rewrite model name if resolved (alias or model-based route)
            if let Some(ref resolved) = route.resolved_model
                && let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&b)
            {
                json["model"] = serde_json::Value::String(resolved.clone());
                b = serde_json::to_vec(&json).unwrap_or(b);
            }
            // Anthropic-specific transforms
            if transform::is_anthropic(&route.upstream_type) && !b.is_empty() {
                transform::prepare_anthropic_request(
                    &b,
                    route.system_prompt.as_deref(),
                    route.claude_code_compat,
                )
                .unwrap_or(b)
            } else {
                b
            }
        };

        // Responses 请求 + 当前路由不支持:跳过该路由继续 failover(初始路由
        // 命中同样适用),只有所有路由都不支持时才返回 501。
        if responses_blocked {
            let was_fallback = attempt_index > 0;
            warn!(
                "Responses API unsupported on {} (responsesMode={:?}), trying next route",
                route.channel_name, responses_mode
            );
            if advance_to_next_route(
                &mut active_routes,
                &mut attempt_index,
                &failover_policy,
                &model,
                &state,
                pathname,
                search,
                request_type.clone(),
                &stripped_path,
            ) {
                let label = describe_route(&route);
                if !failed_route_chain.contains(&label) {
                    failed_route_chain.push(label);
                }
                failover_reason = Some("responses_unsupported".to_string());
                if initial_response_status.is_none() {
                    initial_response_status = Some(501);
                    initial_response_status_text = Some("Not Implemented".to_string());
                    initial_completed_at = Some(
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                            as u64,
                    );
                }
                send_request_log(
                    &state,
                    &request_id,
                    created_at,
                    &method,
                    &uri,
                    pathname,
                    &route,
                    route_model,
                    &headers,
                    &fwd_headers,
                    &request_body,
                    &body,
                    auth_result.api_key_id.clone(),
                    auth_result.api_key_name.clone(),
                    if was_fallback { Some(describe_route(&initial_route)) } else { None },
                    failed_route_chain.clone(),
                    failover_reason.clone(),
                    initial_response_status,
                    initial_response_status_text.clone(),
                    initial_completed_at,
                    if was_fallback { Some(initial_route.channel_name.clone()) } else { None },
                    if was_fallback {
                        Some(initial_route.resolved_model.clone().unwrap_or_else(|| model.clone()))
                    } else {
                        None
                    },
                    retry_count,
                );
                disconnect_guard.arm();
                retry_count = 0;
                continue;
            }

            // 所有路由都不支持 Responses API:返回 501 并带明确的 JSON 错误体。
            let message = format!(
                "渠道 {} 未启用 OpenAI Responses API（responsesMode={:?}），无法转发该请求",
                route.channel_name, responses_mode
            );
            let payload = serde_json::json!({
                "error": {
                    "message": message,
                    "type": "not_implemented_error",
                    "code": "responses_not_supported",
                    "param": null,
                },
            });
            let body_text = payload.to_string();
            emit_terminal_response_log(
                &state,
                &request_id,
                created_at,
                501,
                "NOT_IMPLEMENTED",
                serde_json::json!({}),
                Some(body_text.clone()),
                body_text.len() as u64,
                route.resolved_model.clone(),
                None,
            );
            return Ok(Response::builder()
                .status(StatusCode::NOT_IMPLEMENTED)
                .header("content-type", "application/json")
                .body(Body::from(body_text))
                .unwrap_or_else(|_| Response::new(Body::empty())));
        }

        if let Some(remaining) = state.rate_limit_cooldowns.remaining(
            &route.channel_name,
            route_model,
            rate_limit_cooldown_enabled,
        ) {
            let retry_after_seconds = rate_limit_cooldown::retry_after_seconds(remaining);
            let was_fallback = attempt_index > 0;
            if advance_to_next_route(
                &mut active_routes,
                &mut attempt_index,
                &failover_policy,
                &model,
                &state,
                pathname,
                search,
                request_type.clone(),
                &stripped_path,
            ) {
                // 还有其他可用路由:真正跳过冷却中的渠道,记录 failover 轨迹。
                let label = describe_route(&route);
                if !failed_route_chain.contains(&label) {
                    failed_route_chain.push(label);
                }
                failover_reason = Some("rate_limit_cooldown".to_string());
                // 冷却跳过并没有向该上游发请求，不能伪造本次请求收到过 429；
                // 保留 failover_reason=rate_limit_cooldown 作为路由决策证据。

                send_request_log(
                    &state,
                    &request_id,
                    created_at,
                    &method,
                    &uri,
                    pathname,
                    &route,
                    route_model,
                    &headers,
                    &fwd_headers,
                    &request_body,
                    &body,
                    auth_result.api_key_id.clone(),
                    auth_result.api_key_name.clone(),
                    if was_fallback { Some(describe_route(&initial_route)) } else { None },
                    failed_route_chain.clone(),
                    failover_reason.clone(),
                    initial_response_status,
                    initial_response_status_text.clone(),
                    initial_completed_at,
                    if was_fallback { Some(initial_route.channel_name.clone()) } else { None },
                    if was_fallback {
                        Some(initial_route.resolved_model.clone().unwrap_or_else(|| model.clone()))
                    } else {
                        None
                    },
                    retry_count,
                );
                disconnect_guard.arm();
                warn!(
                    "Rate-limit cooldown active for {} ({route_model}), skipping for {retry_after_seconds}s",
                    route.channel_name
                );
                retry_count = 0;
                continue;
            }

            // 没有任何可用 fallback:所有候选渠道都在 429 冷却中。忽略当前渠道的
            // 冷却状态直接放行本次请求,让客户端拿到上游真实响应 —— 仍被限流则
            // 透传真实的 Retry-After 与错误体;已恢复则成功并自动解除冷却。
            warn!(
                "All candidate routes in rate-limit cooldown; bypassing cooldown for {} ({}) as last resort",
                route.channel_name, route_model
            );
        }

        let circuit_key = circuit_breaker::route_key(&route.channel_name, &target_url);
        let circuit_enabled = failover_policy.enabled
            && failover_policy.retry_on_network_error
            && failover_policy.circuit_breaker_enabled;
        match state
            .circuit_breaker
            .admit(&circuit_key, circuit_enabled, failover_policy.circuit_breaker_cooldown_ms)
            .await
        {
            Admission::Allow => {}
            Admission::HalfOpenProbe => {
                info!("Circuit half-open probe for {}", route.channel_name);
            }
            Admission::SkipOpen => {
                let label = describe_route(&route);
                if !failed_route_chain.contains(&label) {
                    failed_route_chain.push(label);
                }
                failover_reason = Some("circuit_open".to_string());
                if initial_response_status.is_none() {
                    initial_response_status = Some(503);
                    initial_response_status_text = Some("Circuit Open".to_string());
                    initial_completed_at = Some(
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                            as u64,
                    );
                }

                let is_fallback = attempt_index > 0;
                send_request_log(
                    &state,
                    &request_id,
                    created_at,
                    &method,
                    &uri,
                    pathname,
                    &route,
                    route.resolved_model.as_deref().unwrap_or(&model),
                    &headers,
                    &fwd_headers,
                    &request_body,
                    &body,
                    auth_result.api_key_id.clone(),
                    auth_result.api_key_name.clone(),
                    if is_fallback { Some(describe_route(&initial_route)) } else { None },
                    failed_route_chain.clone(),
                    failover_reason.clone(),
                    initial_response_status,
                    initial_response_status_text.clone(),
                    initial_completed_at,
                    if is_fallback { Some(initial_route.channel_name.clone()) } else { None },
                    if is_fallback {
                        Some(initial_route.resolved_model.clone().unwrap_or_else(|| model.clone()))
                    } else {
                        None
                    },
                    retry_count,
                );
                warn!("Circuit open, skipping {}", route.channel_name);

                if advance_to_next_route(
                    &mut active_routes,
                    &mut attempt_index,
                    &failover_policy,
                    &model,
                    &state,
                    pathname,
                    search,
                    request_type.clone(),
                    &stripped_path,
                ) {
                    retry_count = 0;
                    continue;
                }

                if let Some(cached) = last_upstream_error.as_ref() {
                    // 本渠道熔断跳过,但链上曾收到过真实 HTTP 响应:回传它。
                    return Ok(return_cached_upstream_error(
                        &state,
                        &request_id,
                        created_at,
                        cached,
                        route.resolved_model.clone(),
                    ));
                }
                emit_terminal_response_log(
                    &state,
                    &request_id,
                    created_at,
                    503,
                    "CIRCUIT_OPEN",
                    serde_json::json!({}),
                    Some(format!("上游路由熔断中: {}", route.channel_name)),
                    0,
                    None,
                    None,
                );
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        }

        let upstream_method =
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

        let http_client = state.upstream_http_client().await;
        let mut upstream_req =
            http_client.request(upstream_method, &target_url).headers(fwd_headers.clone());

        if !request_body.is_empty() {
            upstream_req = upstream_req.body(request_body.clone());
        }

        let timeout_ms = select_first_byte_timeout(&route, pathname, &state);
        let timeout_dur = std::time::Duration::from_millis(timeout_ms);

        info!("Proxying {} {} → {} ({})", method, pathname, route.target_url, route.channel_name,);

        // Failover perspective for this attempt (matches TS `index.ts`):
        // a fallback route (attempt_index > 0) records the initial route as failover_from.
        let is_fallback = attempt_index > 0;
        let failover_from = if is_fallback { Some(describe_route(&initial_route)) } else { None };
        let original_route_prefix =
            if is_fallback { Some(initial_route.channel_name.clone()) } else { None };
        let original_request_model = if is_fallback {
            Some(initial_route.resolved_model.clone().unwrap_or_else(|| model.clone()))
        } else {
            None
        };
        let request_model_for_log = route.resolved_model.as_deref().unwrap_or(&model);

        // Fire-and-forget: persist the request before waiting for the upstream response.
        // Axum drops the handler future when the client disconnects. If a non-streaming
        // upstream has not produced response headers yet, logging after `send().await`
        // would therefore lose the request entirely (for example, a client that cancels
        // after 120s while the upstream only responds after 300s).
        send_request_log(
            &state,
            &request_id,
            created_at,
            &method,
            &uri,
            pathname,
            &route,
            request_model_for_log,
            &headers,
            &fwd_headers,
            &request_body,
            &body,
            auth_result.api_key_id.clone(),
            auth_result.api_key_name.clone(),
            failover_from.clone(),
            failed_route_chain.clone(),
            failover_reason.clone(),
            initial_response_status,
            initial_response_status_text.clone(),
            initial_completed_at,
            original_route_prefix,
            original_request_model,
            retry_count,
        );
        disconnect_guard.arm();

        let t_send = Instant::now();
        let upstream_result = tokio::time::timeout(timeout_dur, upstream_req.send()).await;
        let t_ttfb = t_send.elapsed();

        match upstream_result {
            Ok(Ok(mut upstream_resp)) => {
                state.circuit_breaker.record_connect_success(&circuit_key).await;
                let status = upstream_resp.status().as_u16();
                if initial_response_status.is_none() {
                    initial_response_status = Some(status);
                    initial_response_status_text =
                        upstream_resp.status().canonical_reason().map(ToString::to_string);
                    initial_completed_at = Some(
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                            as u64,
                    );
                }
                if (200..400).contains(&status) {
                    state.rate_limit_cooldowns.record_success(&route.channel_name, route_model);
                    let is_sse_val = upstream_resp
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|ct| ct.contains("text/event-stream"));
                    let resp_hdrs = upstream_resp.headers().clone();

                    let result = build_response(
                        upstream_resp,
                        &route,
                        converting_responses,
                        &model,
                        0,
                        created_at,
                    )
                    .await;
                    let t_total = t_total.elapsed();

                    let rh = serde_json::to_value(
                        resp_hdrs
                            .iter()
                            .map(|(k, v)| {
                                (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())
                            })
                            .collect::<std::collections::HashMap<_, _>>(),
                    )
                    .unwrap_or_default();

                    let result = match result {
                        Ok(r) => r,
                        Err(BuildResponseError::Status(status)) => return Err(status),
                        Err(BuildResponseError::UpstreamDisconnected(error)) => {
                            emit_terminal_response_log(
                                &state,
                                &request_id,
                                created_at,
                                502,
                                "Upstream Disconnected",
                                rh,
                                Some(format!("读取上游响应时连接中断: {error}")),
                                0,
                                route.resolved_model.clone(),
                                Some("upstream"),
                            );
                            return Err(StatusCode::BAD_GATEWAY);
                        }
                    };

                    let usage = result.usage.clone();
                    let body_bytes = result.body_bytes;
                    let body_content = result.body_content.clone();
                    let resp_model = route.resolved_model.clone();
                    let resp_hdrs = rh.clone();
                    let ttfb_ms = t_ttfb.as_millis() as u64;
                    let first_chunk = created_at.checked_add(ttfb_ms);
                    let stop_reason = body_content.as_deref().and_then(extract_stop_reason);
                    {
                        let ipc = state.ipc.clone();
                        let rid = request_id.clone();
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let (input_tokens, output_tokens, total_tokens) =
                            extract_token_counts(&usage);
                        let cache = extract_cache_tokens(&usage);
                        let rm = resp_model.clone();
                        tokio::spawn(async move {
                            ipc.send(RustToTsMessage::ResponseLog {
                                request_id: rid.clone(),
                                response_status: status,
                                response_status_text: "OK".to_string(),
                                response_headers: rh,
                                response_body_bytes: body_bytes,
                                first_chunk_at: first_chunk,
                                first_token_at: first_chunk,
                                completed_at: Some(now),
                                has_streaming_content: is_sse_val,
                                response_model: rm,
                                stop_reason,
                                input_tokens,
                                output_tokens,
                                total_tokens,
                                cache_creation_input_tokens: cache.cache_creation,
                                cache_read_input_tokens: cache.cache_read,
                                cached_input_tokens: cache.cached,
                                response_payload: body_content,
                                disconnect_source: None,
                                disconnected_at: None,
                            });
                        });
                    }
                    // SSE stream observer: wait for stream to complete, then send supplemental log with usage data
                    if let Some(handle) = result.sse_observer {
                        let ipc = state.ipc.clone();
                        let rid = request_id.clone();
                        let hdrs = resp_hdrs.clone();
                        tokio::spawn(async move {
                            handle.notify.notified().await;
                            if let Ok(mut guard) = handle.observer.try_lock()
                                && let Some(obs) = guard.take()
                            {
                                let usage = obs.parse_usage();
                                let body_text = obs.body_text();
                                let disconnect_source =
                                    obs.disconnect_source.map(ToString::to_string);
                                let disconnected_at = obs.disconnected_at_ms;
                                let (final_status, final_status_text) = match obs.disconnect_source
                                {
                                    Some("client") => {
                                        (CLIENT_CLOSED_REQUEST_STATUS, "Client Closed Request")
                                    }
                                    Some("upstream") => (502, "Upstream Disconnected"),
                                    _ => (200, "OK"),
                                };
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                ipc.send(RustToTsMessage::ResponseLog {
                                    request_id: rid,
                                    response_status: final_status,
                                    response_status_text: final_status_text.to_string(),
                                    response_headers: hdrs,
                                    response_body_bytes: obs.total_bytes,
                                    first_chunk_at: obs.first_chunk_at_ms,
                                    // Real first-token time when detected; fall back to first
                                    // chunk (TTFB) for formats we can't classify, so the metric
                                    // never regresses to blank for the rewrite/conversion paths.
                                    first_token_at: obs.first_token_at_ms.or(obs.first_chunk_at_ms),
                                    completed_at: disconnected_at.or(Some(now)),
                                    has_streaming_content: true,
                                    response_model: resp_model,
                                    stop_reason: usage.stop_reason,
                                    input_tokens: usage.input_tokens,
                                    output_tokens: usage.output_tokens,
                                    total_tokens: usage.total_tokens,
                                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                                    cache_read_input_tokens: usage.cache_read_input_tokens,
                                    cached_input_tokens: usage.cached_input_tokens,
                                    response_payload: Some(body_text),
                                    disconnect_source,
                                    disconnected_at,
                                });
                            }
                        });
                    }
                    info!(
                        "{} {} → {} ({}): ttfb={:.0}ms total={:.0}ms status={}",
                        method,
                        pathname,
                        route.channel_name,
                        if is_retry { format!("retry {retry_count}") } else { "ok".into() },
                        t_ttfb.as_secs_f64() * 1000.0,
                        t_total.as_secs_f64() * 1000.0,
                        status,
                    );
                    let mut response = result.response;
                    if let Some(permit) = concurrency_permit.take() {
                        *response.body_mut() = Body::new(ReleaseConcurrencyBody {
                            inner: std::mem::take(response.body_mut()),
                            _permit: permit,
                        });
                    }
                    return Ok(response);
                }

                if status == 429 {
                    let retry_after = upstream_resp
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok());
                    let cooldown = state.rate_limit_cooldowns.record_429(
                        &route.channel_name,
                        route_model,
                        retry_after,
                        rate_limit_cooldown_enabled,
                    );
                    if !cooldown.is_zero() {
                        let retry_after_seconds =
                            rate_limit_cooldown::retry_after_seconds(cooldown);
                        if let Ok(value) =
                            reqwest::header::HeaderValue::from_str(&retry_after_seconds.to_string())
                        {
                            upstream_resp.headers_mut().insert("retry-after", value);
                        }
                        warn!(
                            "Rate limited by {} ({route_model}); cooling down for {retry_after_seconds}s",
                            route.channel_name
                        );
                    }
                }

                // TS behavior: only retry/fallback for explicitly retryable status codes.
                // Non-retryable (e.g., 404, 401) are returned directly to the client.
                let trigger = FailoverTrigger::Status(status);
                {
                    let label = describe_route(&route);
                    if !failed_route_chain.contains(&label) {
                        failed_route_chain.push(label);
                    }
                    failover_reason = Some(describe_trigger(&trigger));
                }
                if failover_policy.enabled
                    && failover::should_trigger_failover(&failover_policy, &trigger)
                {
                    // 即将丢弃该错误响应去重试/回退:先缓存快照。若后续渠道全部失败
                    // (网络错误/超时/冷却耗尽),回传这个链上最后的真实 HTTP 响应,
                    // 而不是网关自造的 502/504。
                    let captured = CachedUpstreamError::capture(upstream_resp).await;
                    if status == 429
                        && let Ok(cached) = captured.as_ref()
                    {
                        emit_initial_rate_limit_snapshot(
                            &state,
                            &request_id,
                            &route,
                            &target_url,
                            request_model_for_log,
                            &fwd_headers,
                            &request_body,
                            cached,
                        );
                    }
                    if failover::should_retry_same_route(&trigger)
                        && retry_count < failover_policy.retry_attempts
                    {
                        if let Ok(cached) = captured {
                            last_upstream_error = Some(cached);
                        }
                        retry_count += 1;
                        warn!(
                            "Status {status}, retrying same route ({retry_count}/{})",
                            failover_policy.retry_attempts
                        );
                        continue;
                    }
                    if advance_to_next_route(
                        &mut active_routes,
                        &mut attempt_index,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                        &stripped_path,
                    ) {
                        if let Ok(cached) = captured {
                            last_upstream_error = Some(cached);
                        }
                        retry_count = 0;
                        continue;
                    }

                    // 上游非 2xx 且无可用 failover：透传(缓存的)最后一个上游错误响应。
                    return match captured {
                        Ok(cached) => Ok(return_cached_upstream_error(
                            &state,
                            &request_id,
                            created_at,
                            &cached,
                            route.resolved_model.clone(),
                        )),
                        Err(error) => {
                            emit_terminal_response_log(
                                &state,
                                &request_id,
                                created_at,
                                502,
                                "Upstream Disconnected",
                                serde_json::json!({}),
                                Some(format!("读取上游错误响应时连接中断: {error}")),
                                0,
                                route.resolved_model.clone(),
                                Some("upstream"),
                            );
                            Err(StatusCode::BAD_GATEWAY)
                        }
                    };
                }

                // 非 failover 错误状态码(如 404、401)直接透传；透传前先补发响应日志，
                // 否则日志页只能看到请求发起、看不到结束时间与上游错误体。
                let err_headers = serde_json::to_value(
                    upstream_resp
                        .headers()
                        .iter()
                        .map(|(k, v)| {
                            (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())
                        })
                        .collect::<std::collections::HashMap<_, _>>(),
                )
                .unwrap_or_default();
                return match build_response(upstream_resp, &route, false, "", 0, created_at).await {
                    Ok(rw) => {
                        emit_terminal_response_log(
                            &state,
                            &request_id,
                            created_at,
                            status,
                            "ERROR",
                            err_headers,
                            rw.body_content.clone(),
                            rw.body_bytes,
                            route.resolved_model.clone(),
                            None,
                        );
                        Ok(rw.response)
                    }
                    Err(BuildResponseError::Status(status)) => Err(status),
                    Err(BuildResponseError::UpstreamDisconnected(error)) => {
                        emit_terminal_response_log(
                            &state,
                            &request_id,
                            created_at,
                            502,
                            "Upstream Disconnected",
                            err_headers,
                            Some(format!("读取上游错误响应时连接中断: {error}")),
                            0,
                            route.resolved_model.clone(),
                            Some("upstream"),
                        );
                        Err(StatusCode::BAD_GATEWAY)
                    }
                };
            }
            Ok(Err(e)) => {
                let is_connect_error = e.is_connect();
                let is_timeout = e.is_timeout();
                if initial_response_status.is_none() {
                    initial_response_status = Some(if is_timeout { 504 } else { 502 });
                    initial_response_status_text = Some(
                        if is_timeout { "Gateway Timeout" } else { "Bad Gateway" }.to_string(),
                    );
                    initial_completed_at = Some(
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                            as u64,
                    );
                }
                let trigger = if is_connect_error {
                    FailoverTrigger::ConnectError(e.to_string())
                } else if is_timeout {
                    FailoverTrigger::Timeout
                } else {
                    FailoverTrigger::NetworkError(e.to_string())
                };
                if is_connect_error {
                    let opened = state
                        .circuit_breaker
                        .record_connect_failure(
                            &circuit_key,
                            circuit_enabled,
                            failover_policy.circuit_breaker_failure_threshold,
                        )
                        .await;
                    if opened {
                        warn!("Circuit opened for {} after connect failure", route.channel_name);
                    }
                }
                {
                    let label = describe_route(&route);
                    if !failed_route_chain.contains(&label) {
                        failed_route_chain.push(label);
                    }
                    failover_reason = Some(describe_trigger(&trigger));
                }

                if failover_policy.enabled
                    && failover::should_trigger_failover(&failover_policy, &trigger)
                {
                    if failover::should_retry_same_route(&trigger)
                        && retry_count < failover_policy.retry_attempts
                    {
                        retry_count += 1;
                        warn!(
                            "{} — retrying ({}/{})",
                            trigger.kind_str(),
                            retry_count,
                            failover_policy.retry_attempts
                        );
                        continue;
                    }
                    if advance_to_next_route(
                        &mut active_routes,
                        &mut attempt_index,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                        &stripped_path,
                    ) {
                        retry_count = 0;
                        continue;
                    }
                }

                warn!("Upstream error: {e}");
                if let Some(cached) = last_upstream_error.as_ref() {
                    // 本渠道只出了网络错误,但链上曾收到过真实 HTTP 响应:回传它。
                    return Ok(return_cached_upstream_error(
                        &state,
                        &request_id,
                        created_at,
                        cached,
                        route.resolved_model.clone(),
                    ));
                }
                emit_terminal_response_log(
                    &state,
                    &request_id,
                    created_at,
                    502,
                    "BAD_GATEWAY",
                    serde_json::json!({}),
                    Some(format!("连接上游失败: {e}")),
                    0,
                    None,
                    Some("upstream"),
                );
                return Err(StatusCode::BAD_GATEWAY);
            }
            Err(_) => {
                // Timeout on first byte
                if initial_response_status.is_none() {
                    initial_response_status = Some(504);
                    initial_response_status_text = Some("Gateway Timeout".to_string());
                    initial_completed_at = Some(
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                            as u64,
                    );
                }
                let trigger = FailoverTrigger::Timeout;
                {
                    let label = describe_route(&route);
                    if !failed_route_chain.contains(&label) {
                        failed_route_chain.push(label);
                    }
                    failover_reason = Some(describe_trigger(&trigger));
                }
                if failover_policy.enabled
                    && failover::should_trigger_failover(&failover_policy, &trigger)
                {
                    if retry_count < failover_policy.retry_attempts {
                        retry_count += 1;
                        warn!(
                            "Timeout — retrying ({}/{})",
                            retry_count, failover_policy.retry_attempts
                        );
                        continue;
                    }
                    if advance_to_next_route(
                        &mut active_routes,
                        &mut attempt_index,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                        &stripped_path,
                    ) {
                        retry_count = 0;
                        continue;
                    }
                }
                warn!("Upstream timeout");
                if let Some(cached) = last_upstream_error.as_ref() {
                    // 本渠道首字节超时,但链上曾收到过真实 HTTP 响应:回传它。
                    return Ok(return_cached_upstream_error(
                        &state,
                        &request_id,
                        created_at,
                        cached,
                        route.resolved_model.clone(),
                    ));
                }
                emit_terminal_response_log(
                    &state,
                    &request_id,
                    created_at,
                    504,
                    "GATEWAY_TIMEOUT",
                    serde_json::json!({}),
                    Some("上游响应超时".to_string()),
                    0,
                    None,
                    None,
                );
                return Err(StatusCode::GATEWAY_TIMEOUT);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn advance_to_next_route(
    active_routes: &mut Vec<RouteResult>,
    attempt_index: &mut usize,
    policy: &crate::config::GatewayFailoverPolicy,
    model: &str,
    state: &AppState,
    pathname: &str,
    search: &str,
    request_type: crate::config::UpstreamType,
    stripped_path: &str,
) -> bool {
    if *attempt_index + 1 < active_routes.len()
        || try_add_fallbacks(
            active_routes,
            policy,
            model,
            state,
            pathname,
            search,
            request_type,
            stripped_path,
        )
    {
        *attempt_index += 1;
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn try_add_fallbacks(
    active_routes: &mut Vec<RouteResult>,
    policy: &crate::config::GatewayFailoverPolicy,
    model: &str,
    state: &AppState,
    pathname: &str,
    search: &str,
    request_type: crate::config::UpstreamType,
    stripped_path: &str,
) -> bool {
    use crate::config::{ModelFallbackMode, RoutingVisibility};

    if policy.max_fallback_attempts == 0 {
        return false;
    }
    let max_fallbacks = policy.max_fallback_attempts as usize;
    let already_added = active_routes.len().saturating_sub(1);
    if already_added >= max_fallbacks {
        return false;
    }
    let remaining_fallbacks = max_fallbacks - already_added;

    let rt = state.routing.try_read().expect("Routing lock");
    let existing: HashSet<_> = active_routes.iter().map(|r| r.channel_name.clone()).collect();

    // 1) Virtual model / same-model fallback: resolve by model name
    let all_routes = routing::resolve_routes_by_model(
        pathname,
        search,
        model,
        Some(request_type.clone()),
        &rt.providers,
        &rt.aliases,
    );
    let mut candidates: Vec<RouteResult> =
        all_routes.into_iter().filter(|r| !existing.contains(&r.channel_name)).collect();

    // 2) Custom per-model fallback models
    for fallback_model in &failover::get_custom_model_fallbacks(policy, model) {
        for r in routing::resolve_routes_by_model(
            pathname,
            search,
            fallback_model,
            Some(request_type.clone()),
            &rt.providers,
            &rt.aliases,
        ) {
            if !existing.contains(&r.channel_name) {
                candidates.push(r);
            }
        }
    }

    // 3) Site policy: any_model — still constrained to the request's provider type:
    // an Anthropic-format request must never be forwarded to an OpenAI upstream (and
    // vice versa); the body format is incompatible, so such forwarding is always invalid.
    if policy.model_fallback_mode == ModelFallbackMode::AnyModel {
        for (name, entry) in rt.providers.iter() {
            if entry.enabled
                && entry.upstream_type == request_type
                && entry.routing_visibility.as_ref() != Some(&RoutingVisibility::ExplicitOnly)
                && !existing.contains(name)
            {
                // Reuse the canonical builder so the type-forced prefix is stripped and
                // the path/search are normalized identically to the initial + same-model
                // fallback paths (avoids `.../anthropic/anthropic/...` doubling).
                candidates.push(routing::build_route_result(name, entry, pathname, search));
            }
        }
    }
    drop(rt);

    // Deduplicate and enforce maxFallbackAttempts
    let mut seen: HashSet<String> = existing;
    let mut added = 0;
    let mut new_routes = Vec::new();
    for r in candidates {
        let key = r.channel_name.clone();
        let candidate_model = rate_limit_cooldown::route_model(r.resolved_model.as_deref(), model);
        let cooling_down = state
            .rate_limit_cooldowns
            .remaining(
                &r.channel_name,
                candidate_model,
                policy.enabled && policy.retry_on_status_codes.contains(&429),
            )
            .is_some();
        // Responses API 请求:fallback 候选必须支持(native/chat_compat)Responses 端点。
        // 显式 disabled(或未知模式)的候选转发过去只会拿到网关自造的 501,对本次
        // 请求形态不可用,直接排除 —— 让冷却/失败的初始渠道把真实错误透传给客户端。
        let responses_unsupported = responses::is_responses_request(stripped_path, &r.target_url)
            && !matches!(
                r.responses_mode,
                Some(crate::config::OpenAiResponsesMode::Native)
                    | Some(crate::config::OpenAiResponsesMode::ChatCompat)
            );
        if !cooling_down
            && !responses_unsupported
            && !seen.contains(&key)
            && added < remaining_fallbacks
        {
            seen.insert(key);
            new_routes.push(r);
            added += 1;
        }
    }

    if !new_routes.is_empty() {
        active_routes.extend(new_routes);
        true
    } else {
        false
    }
}

struct ResponseWithUsage {
    response: Response,
    usage: Option<serde_json::Value>,
    body_bytes: u64,
    body_content: Option<String>,
    sse_observer: Option<SseObserverHandle>,
}

enum BuildResponseError {
    Status(StatusCode),
    UpstreamDisconnected(String),
}

async fn build_response(
    upstream_resp: reqwest::Response,
    route: &RouteResult,
    converting_responses: bool,
    responses_model: &str,
    _idle_timeout_ms: u64,
    created_at: u64,
) -> Result<ResponseWithUsage, BuildResponseError> {
    let status = StatusCode::from_u16(upstream_resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let upstream_headers = upstream_resp.headers().clone();

    let mut response_builder = Response::builder().status(status);
    let hop_by_hop = hop_by_hop_set();

    let is_sse = upstream_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    for (key, value) in upstream_headers.iter() {
        let lower = key.as_str().to_lowercase();
        if hop_by_hop.contains(&lower) {
            continue;
        }
        if let Ok(name) = HeaderName::from_bytes(key.as_str().as_bytes())
            && let Ok(val) = HeaderValue::from_bytes(value.as_bytes())
        {
            response_builder = response_builder.header(name, val);
        }
    }

    if is_sse {
        response_builder =
            response_builder.header("cache-control", "no-cache").header("x-accel-buffering", "no");
    }

    // Model name rewriting: if this is an alias route (has virtual_model) and
    // should hide the real model (return_real_model is false), rewrite the
    // upstream model name back to the alias in the response body.
    let rewriter = if let (Some(from), Some(to)) =
        (route.resolved_model.as_deref(), route.virtual_model.as_deref())
    {
        if !route.return_real_model && from != to { ModelRewriter::new(from, to) } else { None }
    } else {
        None
    };

    if converting_responses && !is_sse {
        // Non-streaming chat completions → Responses
        let body_bytes = upstream_resp
            .bytes()
            .await
            .map_err(|error| BuildResponseError::UpstreamDisconnected(error.to_string()))?;
        let len = body_bytes.len() as u64;
        let converted = crate::responses::convert_chat_to_responses(&body_bytes)
            .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        let body_str = String::from_utf8_lossy(&converted).to_string();
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(converted))
            .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        return Ok(ResponseWithUsage {
            response,
            usage: None,
            body_bytes: len,
            body_content: Some(body_str),
            sse_observer: None,
        });
    }

    if let Some(rw) = rewriter {
        if converting_responses && is_sse {
            let observer_handle = SseObserverHandle::new(created_at);
            let raw_stream = upstream_resp.bytes_stream();
            let conv = ChatSseToResponsesSse::new(responses_model);
            let sse_stream = ResponsesSseStream::new(raw_stream, conv);
            let observed = ObservingSseStream::new(sse_stream, observer_handle.clone());
            let response = response_builder
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(observed))
                .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: Some(observer_handle),
            });
        }

        if is_sse {
            let observer_handle = SseObserverHandle::new(created_at);
            let raw_stream = upstream_resp.bytes_stream();
            let body_stream = ModelRewriteStream::new(raw_stream, rw);
            let observed = ObservingSseStream::new(body_stream, observer_handle.clone());
            let response = response_builder
                .body(Body::from_stream(observed))
                .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: Some(observer_handle),
            });
        }

        // Non-SSE with model rewriter: read full body, rewrite, extract usage
        let body_bytes = upstream_resp
            .bytes()
            .await
            .map_err(|error| BuildResponseError::UpstreamDisconnected(error.to_string()))?;
        let len = body_bytes.len() as u64;
        let rewritten = rw.rewrite_chunk(&String::from_utf8_lossy(&body_bytes));
        let usage = extract_usage(&rewritten);
        let response = response_builder
            .body(Body::from(rewritten.clone()))
            .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(ResponseWithUsage {
            response,
            usage,
            body_bytes: len,
            body_content: Some(rewritten),
            sse_observer: None,
        })
    } else {
        if converting_responses && is_sse {
            let observer_handle = SseObserverHandle::new(created_at);
            let raw_stream = upstream_resp.bytes_stream();
            let conv = ChatSseToResponsesSse::new(responses_model);
            let sse_stream = ResponsesSseStream::new(raw_stream, conv);
            let observed = ObservingSseStream::new(sse_stream, observer_handle.clone());
            let response = response_builder
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(observed))
                .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: Some(observer_handle),
            });
        }

        if is_sse {
            let observer_handle = SseObserverHandle::new(created_at);
            let body_stream =
                ObservingSseStream::new(upstream_resp.bytes_stream(), observer_handle.clone());
            let response = response_builder
                .body(Body::from_stream(body_stream))
                .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: Some(observer_handle),
            });
        }

        // Non-SSE: read full body, extract usage
        let body_bytes = upstream_resp
            .bytes()
            .await
            .map_err(|error| BuildResponseError::UpstreamDisconnected(error.to_string()))?;
        let len = body_bytes.len() as u64;
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();
        let usage = extract_usage(&body_str);
        let response = response_builder
            .body(Body::from(body_bytes.to_vec()))
            .map_err(|_| BuildResponseError::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(ResponseWithUsage {
            response,
            usage,
            body_bytes: len,
            body_content: Some(body_str),
            sse_observer: None,
        })
    }
}

fn extract_usage(body: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v.get("usage").cloned())
}

fn extract_stop_reason(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("stop_reason").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

struct CacheTokens {
    cache_creation: Option<u32>,
    cache_read: Option<u32>,
    cached: Option<u32>,
}

fn extract_cache_tokens(usage: &Option<serde_json::Value>) -> CacheTokens {
    match usage {
        Some(u) => CacheTokens {
            cache_creation: u
                .get("cache_creation_input_tokens")
                .or(u.get("cacheCreationInputTokens"))
                .and_then(|v| v.as_u64().map(|n| n as u32)),
            cache_read: u
                .get("cache_read_input_tokens")
                .or(u.get("cacheReadInputTokens"))
                .and_then(|v| v.as_u64().map(|n| n as u32)),
            cached: u
                .get("cached_tokens")
                .or(u.get("cachedTokens"))
                .and_then(|v| v.as_u64().map(|n| n as u32)),
        },
        None => CacheTokens { cache_creation: None, cache_read: None, cached: None },
    }
}

fn extract_token_counts(
    usage: &Option<serde_json::Value>,
) -> (Option<u32>, Option<u32>, Option<u32>) {
    match usage {
        Some(u) => (
            u.get("prompt_tokens")
                .or(u.get("input_tokens"))
                .and_then(|v| v.as_u64().map(|n| n as u32)),
            u.get("completion_tokens")
                .or(u.get("output_tokens"))
                .and_then(|v| v.as_u64().map(|n| n as u32)),
            u.get("total_tokens").and_then(|v| v.as_u64().map(|n| n as u32)),
        ),
        None => (None, None, None),
    }
}

/// A stream wrapper that applies ModelRewriter to each chunk with a safe tail buffer.
struct ModelRewriteStream<S> {
    inner: S,
    rewriter: ModelRewriter,
    buffer: String,
    safe_tail: usize,
    drained: bool,
}

impl<S> ModelRewriteStream<S> {
    fn new(inner: S, rewriter: ModelRewriter) -> Self {
        Self { inner, rewriter, buffer: String::new(), safe_tail: 64, drained: false }
    }
}

impl<S> futures::Stream for ModelRewriteStream<S>
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.drained {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    self.buffer.push_str(&text);
                    if self.buffer.len() > self.safe_tail {
                        let flush_end_raw = self.buffer.len() - self.safe_tail;
                        let flush_end = self.buffer.floor_char_boundary(flush_end_raw);
                        let flush_part = self.buffer[..flush_end].to_string();
                        self.buffer = self.buffer[flush_end..].to_string();
                        return Poll::Ready(Some(Ok(Bytes::from(
                            self.rewriter.rewrite_chunk(&flush_part),
                        ))));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(axum::Error::new(e))));
                }
                Poll::Ready(None) => {
                    self.drained = true;
                    if !self.buffer.is_empty() {
                        let rewritten = self.rewriter.rewrite_chunk(&self.buffer);
                        return Poll::Ready(Some(Ok(Bytes::from(rewritten))));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct ResponsesSseStream<S> {
    inner: S,
    converter: ChatSseToResponsesSse,
    pending: std::vec::IntoIter<String>,
}

impl<S> ResponsesSseStream<S> {
    fn new(inner: S, converter: ChatSseToResponsesSse) -> Self {
        Self { inner, converter, pending: Vec::new().into_iter() }
    }
}

impl<S> futures::Stream for ResponsesSseStream<S>
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.pending.next() {
                return Poll::Ready(Some(Ok(Bytes::from(event))));
            }

            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let events = self.converter.feed(&chunk);
                    self.pending = events.into_iter();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(axum::Error::new(e))));
                }
                Poll::Ready(None) => {
                    let events = self.converter.finish();
                    if events.is_empty() {
                        return Poll::Ready(None);
                    }
                    self.pending = events.into_iter();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn resolve_route(
    pathname: &str,
    search: &str,
    model: &str,
    forced_type: Option<crate::config::UpstreamType>,
    providers: &std::collections::HashMap<String, crate::config::ConfigEntry>,
    aliases: &std::collections::HashMap<String, crate::config::AliasTarget>,
) -> Option<RouteResult> {
    if let Some(route) = routing::resolve_explicit_route(pathname, search, providers) {
        return Some(route);
    }
    let routes =
        routing::resolve_routes_by_model(pathname, search, model, forced_type, providers, aliases);
    routes.into_iter().next()
}

fn extract_model_from_body(body: &[u8]) -> String {
    if body.is_empty() {
        return String::new();
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("model")?.as_str().map(String::from))
        .unwrap_or_default()
}

fn select_first_byte_timeout(_route: &RouteResult, pathname: &str, state: &AppState) -> u64 {
    let rt = state.routing.try_read().expect("Routing lock");
    if pathname.contains("/images/generations") {
        rt.timeouts.image_first_byte_timeout_ms
    } else {
        rt.timeouts.stream_first_byte_timeout_ms
    }
}

/// Human-readable route label for failover chain logs: `channel` or `channel (model)`.
/// Matches TS `describeRoute` in `index.ts`.
fn describe_route(route: &RouteResult) -> String {
    match &route.resolved_model {
        Some(m) if !m.is_empty() => format!("{} ({})", route.channel_name, m),
        _ => route.channel_name.clone(),
    }
}

/// One-line failover trigger description for the request log.
fn describe_trigger(trigger: &failover::FailoverTrigger) -> String {
    match trigger {
        failover::FailoverTrigger::ConnectError(msg) => format!("connect_error: {msg}"),
        failover::FailoverTrigger::Status(status) => format!("HTTP {status}"),
        failover::FailoverTrigger::Timeout => "timeout".to_string(),
        failover::FailoverTrigger::NetworkError(msg) => format!("network_error: {msg}"),
    }
}

/// 链上最后一个真实收到的上游错误响应快照。failover 链路最终失败(后续渠道网络
/// 错误/超时/全部冷却耗尽)时回传它,保证客户端拿到真实的上游 HTTP 错误(状态码、
/// 头、含 Retry-After 的错误体),而不是网关自造的 502/504。
struct CachedUpstreamError {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl CachedUpstreamError {
    /// 消费上游错误响应并读全量 body 做快照。能触发 failover 的都是错误状态码,
    /// 响应体不会是大体积 SSE 流,直接读全量即可;读取失败说明连接中断。
    async fn capture(upstream_resp: reqwest::Response) -> Result<Self, String> {
        let status = upstream_resp.status();
        let headers = upstream_resp.headers().clone();
        let body = upstream_resp.bytes().await.map_err(|e| e.to_string())?;
        Ok(Self { status, headers, body })
    }

    fn headers_json(&self) -> Value {
        serde_json::to_value(
            self.headers
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect::<std::collections::HashMap<_, _>>(),
        )
        .unwrap_or_default()
    }

    fn body_content(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// 把快照透传给客户端(过滤 hop-by-hop 头,body 定长无需再写 content-length)。
    fn to_response(&self) -> Response {
        let mut builder = Response::builder().status(self.status);
        for (key, value) in self.headers.iter() {
            if hop_by_hop_set().contains(key.as_str()) {
                continue;
            }
            if let Ok(name) = HeaderName::from_bytes(key.as_str().as_bytes())
                && let Ok(val) = HeaderValue::from_bytes(value.as_bytes())
            {
                builder = builder.header(name, val);
            }
        }
        builder.body(Body::from(self.body.clone())).unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

/// 回传缓存的上游错误响应,并补发终端响应日志(日志页才能看到结束时间与错误体)。
fn return_cached_upstream_error(
    state: &AppState,
    request_id: &str,
    created_at: u64,
    cached: &CachedUpstreamError,
    response_model: Option<String>,
) -> Response {
    emit_terminal_response_log(
        state,
        request_id,
        created_at,
        cached.status.as_u16(),
        "ERROR",
        cached.headers_json(),
        Some(cached.body_content()),
        cached.body.len() as u64,
        response_model,
        None,
    );
    cached.to_response()
}

/// 429 会在自动故障转移时被消费；单独透过 IPC 保存，不能让最终 200 的响应覆盖它。
#[expect(
    clippy::too_many_arguments,
    reason = "Snapshot captures the full upstream attempt context"
)]
fn emit_initial_rate_limit_snapshot(
    state: &AppState,
    request_id: &str,
    route: &RouteResult,
    target_url: &str,
    request_model: &str,
    forward_headers: &reqwest::header::HeaderMap,
    forwarded_body: &[u8],
    response: &CachedUpstreamError,
) {
    let headers = serde_json::to_value(
        forward_headers
            .iter()
            .map(|(key, value)| {
                (key.as_str().to_string(), value.to_str().unwrap_or("").to_string())
            })
            .collect::<std::collections::HashMap<_, _>>(),
    )
    .unwrap_or_default();
    state.ipc.send(RustToTsMessage::InitialRateLimitSnapshot {
        request_id: request_id.to_string(),
        route_prefix: route.channel_name.clone(),
        target_url: target_url.to_string(),
        request_model: request_model.to_string(),
        forwarded_payload: (!forwarded_body.is_empty())
            .then(|| String::from_utf8_lossy(forwarded_body).to_string()),
        forward_headers: headers,
        response_headers: response.headers_json(),
        response_payload: Some(response.body_content()),
        response_payload_truncated: false,
    });
}

#[allow(clippy::too_many_arguments)]
fn send_request_log(
    state: &AppState,
    request_id: &str,
    created_at: u64,
    method: &Method,
    uri: &Uri,
    pathname: &str,
    route: &RouteResult,
    model: &str,
    headers: &HeaderMap,
    fwd_headers: &reqwest::header::HeaderMap,
    forwarded_body: &[u8],
    original_body: &[u8],
    api_key_id: Option<String>,
    api_key_name: Option<String>,
    failover_from: Option<String>,
    failover_chain: Vec<String>,
    failover_reason: Option<String>,
    initial_response_status: Option<u16>,
    initial_response_status_text: Option<String>,
    initial_completed_at: Option<u64>,
    original_route_prefix: Option<String>,
    original_request_model: Option<String>,
    retry_attempt: u32,
) {
    let rid = request_id.to_string();
    let m = method.to_string();
    let p = pathname.to_string();
    let fu = uri.to_string();
    let tu = route.target_url.clone();
    let rp = route.channel_name.clone();
    let rm = model.to_string();
    let ut = format!("{:?}", route.upstream_type).to_lowercase();
    let oh = serde_json::to_value(
        headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect::<std::collections::HashMap<_, _>>(),
    )
    .unwrap_or_default();
    let fh = serde_json::to_value(
        fwd_headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect::<std::collections::HashMap<_, _>>(),
    )
    .unwrap_or_default();
    let fp = if forwarded_body.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(forwarded_body).to_string())
    };
    let op = if original_body.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(original_body).to_string())
    };

    // `send` 仅写入无界 mpsc 队列，不会阻塞请求；同步入队可保证多次 retry/fallback
    // 的 request_log 顺序稳定，避免旧 attempt 覆盖新 attempt 的路由与首次状态。
    state.ipc.send(RustToTsMessage::RequestLog {
        request_id: rid,
        created_at,
        method: m,
        route_prefix: rp,
        upstream_type: ut,
        path: p,
        url: fu,
        target_url: tu,
        request_model: rm,
        original_payload: op,
        forwarded_payload: fp,
        original_headers: oh,
        forward_headers: fh,
        api_key_id,
        api_key_name,
        source_request_type: "chat_completion".to_string(),
        failover_from,
        failover_chain,
        failover_reason,
        initial_response_status,
        initial_response_status_text,
        initial_completed_at,
        original_route_prefix,
        original_request_model,
        retry_attempt,
    });
}

/// 终态响应日志（错误路径补发）：上游非 2xx / 网络错误 / 超时 / 路由耗尽时调用，
/// 与成功路径的 ResponseLog 同构，保证日志页能看到结束时间、状态码与（如有）上游
/// 错误体，便于排查 400/409/429 等失败请求。成功路径仍走内联的精细 timing 发送。
#[allow(clippy::too_many_arguments)]
fn emit_terminal_response_log(
    state: &AppState,
    request_id: &str,
    created_at: u64,
    status: u16,
    status_text: &str,
    response_headers: serde_json::Value,
    body_content: Option<String>,
    body_bytes: u64,
    response_model: Option<String>,
    disconnect_source: Option<&str>,
) {
    let usage = body_content.as_deref().and_then(|b| {
        serde_json::from_str::<serde_json::Value>(b).ok().and_then(|v| v.get("usage").cloned())
    });
    let (input_tokens, output_tokens, total_tokens) = extract_token_counts(&usage);
    let cache = extract_cache_tokens(&usage);
    let stop_reason = body_content.as_deref().and_then(extract_stop_reason);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;

    let ipc = state.ipc.clone();
    let rid = request_id.to_string();
    let stext = status_text.to_string();
    let disconnect_source = disconnect_source.map(ToString::to_string);
    let disconnected_at = disconnect_source.as_ref().map(|_| now);
    tokio::spawn(async move {
        ipc.send(RustToTsMessage::ResponseLog {
            request_id: rid,
            response_status: status,
            response_status_text: stext,
            response_headers,
            response_body_bytes: body_bytes,
            first_chunk_at: Some(created_at),
            first_token_at: Some(created_at),
            completed_at: Some(now),
            has_streaming_content: false,
            response_model,
            stop_reason,
            input_tokens,
            output_tokens,
            total_tokens,
            cache_creation_input_tokens: cache.cache_creation,
            cache_read_input_tokens: cache.cache_read,
            cached_input_tokens: cache.cached,
            response_payload: body_content,
            disconnect_source,
            disconnected_at,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn client_disconnect_guard_emits_499_with_source_and_timestamp() {
        let (sender, mut receiver) = crate::ipc::IpcSender::test_channel();
        let guard = ClientDisconnectLogGuard::new(Arc::new(sender), "request-1".to_string());
        guard.arm();
        drop(guard);

        let message = receiver.recv().await.expect("client disconnect log");
        match message {
            RustToTsMessage::ResponseLog {
                request_id,
                response_status,
                disconnect_source,
                disconnected_at,
                completed_at,
                ..
            } => {
                assert_eq!(request_id, "request-1");
                assert_eq!(response_status, CLIENT_CLOSED_REQUEST_STATUS);
                assert_eq!(disconnect_source.as_deref(), Some("client"));
                assert!(disconnected_at.is_some());
                assert_eq!(completed_at, disconnected_at);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn disarmed_client_disconnect_guard_emits_nothing() {
        let (sender, mut receiver) = crate::ipc::IpcSender::test_channel();
        let guard = ClientDisconnectLogGuard::new(Arc::new(sender), "request-2".to_string());
        guard.arm();
        guard.disarm();
        drop(guard);

        assert!(receiver.try_recv().is_err());
    }

    fn failover_test_state() -> AppState {
        use crate::app_state::RoutingTable;
        use crate::config::{
            ConfigEntry, GatewayFailoverPolicy, ModelConfig, ModelFallbackMode,
            OpenAiResponsesMode, SyncConfigPayload, UpstreamType,
        };

        let entry =
            |base: &str, model: &str, responses_mode: Option<OpenAiResponsesMode>| ConfigEntry {
                upstream_type: UpstreamType::OpenAI,
                target_base_url: base.to_string(),
                system_prompt: None,
                auth: None,
                models: Some(vec![ModelConfig {
                    model: model.to_string(),
                    context: None,
                    extra: serde_json::Map::new(),
                }]),
                priority: 0,
                enabled: true,
                routing_visibility: None,
                responses_mode,
                extra_fields: None,
                provider_uuid: None,
                auto_sync_models: false,
                claude_code_compat: false,
            };

        let mut routing = RoutingTable::from_payload(SyncConfigPayload::default());
        routing.providers = std::collections::HashMap::from([
            (
                "primary".to_string(),
                entry("http://primary.example/v1", "gpt-5", Some(OpenAiResponsesMode::Native)),
            ),
            (
                "chat-only".to_string(),
                entry("http://chat-only.example/v1", "gpt-4", Some(OpenAiResponsesMode::Disabled)),
            ),
            (
                "compat".to_string(),
                entry("http://compat.example/v1", "gpt-4o", Some(OpenAiResponsesMode::ChatCompat)),
            ),
        ]);
        routing.failover = GatewayFailoverPolicy {
            enabled: true,
            retry_attempts: 0,
            model_fallback_mode: ModelFallbackMode::AnyModel,
            max_fallback_attempts: 5,
            custom_model_fallbacks: vec![],
            retry_on_timeout: true,
            retry_on_network_error: true,
            retry_on_status_codes: vec![429],
            retry_on_status_ranges: vec![],
            circuit_breaker_enabled: true,
            circuit_breaker_failure_threshold: 3,
            circuit_breaker_cooldown_ms: 10_000,
        };

        let (ipc, _rx) = crate::ipc::IpcSender::test_channel();
        AppState::new(routing, ipc)
    }

    fn resolve_initial(state: &AppState, model: &str) -> RouteResult {
        let rt = state.routing.try_read().expect("Routing lock");
        routing::resolve_routes_by_model(
            "/v1/responses",
            "",
            model,
            Some(crate::config::UpstreamType::OpenAI),
            &rt.providers,
            &rt.aliases,
        )
        .pop()
        .expect("initial route")
    }

    /// /v1/responses 请求的 fallback 候选必须排除 responsesMode=disabled 的渠道,
    /// 否则唯一可用渠道冷却/失败时会把请求转发给只会 501 的渠道。
    #[test]
    fn responses_fallback_excludes_disabled_channels() {
        let state = failover_test_state();
        let policy = state.routing.try_read().expect("Routing lock").failover.clone();
        let mut active_routes = vec![resolve_initial(&state, "gpt-5")];

        let added = try_add_fallbacks(
            &mut active_routes,
            &policy,
            "gpt-5",
            &state,
            "/v1/responses",
            "",
            crate::config::UpstreamType::OpenAI,
            "/v1/responses",
        );

        assert!(added);
        let channels: Vec<&str> = active_routes.iter().map(|r| r.channel_name.as_str()).collect();
        assert!(
            !channels.contains(&"chat-only"),
            "disabled channel must be excluded: {channels:?}"
        );
        assert!(channels.contains(&"compat"), "chat_compat channel should be usable: {channels:?}");
    }

    /// 非 Responses 端点的请求不受 responsesMode 过滤限制,disabled 渠道仍可作为 fallback。
    #[test]
    fn chat_fallback_keeps_disabled_channels() {
        let state = failover_test_state();
        let policy = state.routing.try_read().expect("Routing lock").failover.clone();
        let mut active_routes = vec![resolve_initial(&state, "gpt-5")];

        let added = try_add_fallbacks(
            &mut active_routes,
            &policy,
            "gpt-5",
            &state,
            "/v1/chat/completions",
            "",
            crate::config::UpstreamType::OpenAI,
            "/v1/chat/completions",
        );

        assert!(added);
        let channels: Vec<&str> = active_routes.iter().map(|r| r.channel_name.as_str()).collect();
        assert!(
            channels.contains(&"chat-only"),
            "chat requests should keep disabled-responses channel: {channels:?}"
        );
    }

    /// 全部候选都不可用(disabled 被排除、可用渠道冷却中)时不再添加 fallback,
    /// 调用方应忽略冷却放行最后一个渠道,而不是返回网关自造的 429。
    #[test]
    fn responses_fallback_returns_false_when_only_cooldown_free_channel_is_disabled() {
        let state = failover_test_state();
        // 把两个支持 Responses 的渠道全部打入 429 冷却(any_model 候选不改写模型名,
        // 冷却按请求模型 gpt-5 查询)
        state.rate_limit_cooldowns.record_429("primary", "gpt-5", Some("30"), true);
        state.rate_limit_cooldowns.record_429("compat", "gpt-5", Some("30"), true);

        let policy = state.routing.try_read().expect("Routing lock").failover.clone();
        let mut active_routes = vec![resolve_initial(&state, "gpt-5")];

        let added = try_add_fallbacks(
            &mut active_routes,
            &policy,
            "gpt-5",
            &state,
            "/v1/responses",
            "",
            crate::config::UpstreamType::OpenAI,
            "/v1/responses",
        );

        assert!(!added, "no usable fallback should be added");
        assert_eq!(active_routes.len(), 1);
    }
}
