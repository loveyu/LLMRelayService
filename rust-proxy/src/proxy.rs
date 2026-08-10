use crate::app_state::AppState;
use crate::auth;
use crate::failover::{self, FailoverTrigger};
use crate::ipc::RustToTsMessage;
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

    loop {
        if attempt_index >= active_routes.len() {
            warn!("All routes exhausted for {}", pathname);
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
            );
            return Err(StatusCode::BAD_GATEWAY);
        }

        let route = active_routes[attempt_index].clone();
        let is_retry = attempt_index == 0 && retry_count > 0;

        let t_total = Instant::now();

        // ── OpenAI Responses API handling ──────────────────────────────
        let is_responses = responses::is_responses_endpoint(pathname);
        let responses_mode = route.responses_mode.as_ref();
        let mut converting_responses = false;

        if is_responses {
            match responses_mode {
                Some(crate::config::OpenAiResponsesMode::Native) => {
                    // pass through unchanged
                }
                Some(crate::config::OpenAiResponsesMode::Disabled) | None => {
                    warn!(
                        "Responses API endpoint hit but responsesMode is {:?}, returning 501",
                        responses_mode
                    );
                    return Err(StatusCode::NOT_IMPLEMENTED);
                }
                Some(crate::config::OpenAiResponsesMode::ChatCompat) => {
                    converting_responses = true;
                }
            }
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

        let upstream_method =
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

        let mut upstream_req =
            state.http_client.request(upstream_method, &target_url).headers(fwd_headers.clone());

        if !request_body.is_empty() {
            upstream_req = upstream_req.body(request_body.clone());
        }

        let timeout_ms = select_first_byte_timeout(&route, pathname, &state);
        let timeout_dur = std::time::Duration::from_millis(timeout_ms);

        info!("Proxying {} {} → {} ({})", method, pathname, route.target_url, route.channel_name,);

        let t_send = Instant::now();
        let upstream_result = tokio::time::timeout(timeout_dur, upstream_req.send()).await;
        let t_ttfb = t_send.elapsed();

        // Failover perspective for this attempt (matches TS `index.ts`):
        // a fallback route (attempt_index > 0) records the initial route as failover_from.
        let is_fallback = attempt_index > 0;
        let failover_from = if is_fallback { Some(describe_route(&initial_route)) } else { None };

        // Fire-and-forget: send request log to TS via IPC
        send_request_log(
            &state,
            &request_id,
            created_at,
            &method,
            &uri,
            pathname,
            &route,
            &model,
            &headers,
            &fwd_headers,
            &request_body,
            &body,
            auth_result.api_key_id.clone(),
            auth_result.api_key_name.clone(),
            failover_from.clone(),
            failed_route_chain.clone(),
            failover_reason.clone(),
            failover_from.clone(),
            if is_fallback { Some(model.clone()) } else { None },
            retry_count,
        );

        match upstream_result {
            Ok(Ok(upstream_resp)) => {
                let status = upstream_resp.status().as_u16();
                if (200..400).contains(&status) {
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
                        Err(status) => return Err(status),
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
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                ipc.send(RustToTsMessage::ResponseLog {
                                    request_id: rid,
                                    response_status: 200,
                                    response_status_text: "OK".to_string(),
                                    response_headers: hdrs,
                                    response_body_bytes: obs.total_bytes,
                                    first_chunk_at: obs.first_chunk_at_ms,
                                    first_token_at: obs.first_token_at_ms,
                                    completed_at: Some(now),
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
                    return Ok(result.response);
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
                    if retry_count < failover_policy.retry_attempts {
                        retry_count += 1;
                        warn!(
                            "Status {status}, retrying same route ({retry_count}/{})",
                            failover_policy.retry_attempts
                        );
                        continue;
                    }
                    if try_add_fallbacks(
                        &mut active_routes,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                    ) {
                        attempt_index += 1;
                        retry_count = 0;
                        continue;
                    }
                }

                // 上游非 2xx 且无可用 failover：透传错误响应给客户端前，先补发响应日志，
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
                        );
                        Ok(rw.response)
                    }
                    Err(s) => Err(s),
                };
            }
            Ok(Err(e)) => {
                let is_timeout = e.is_timeout();
                let trigger = if is_timeout {
                    FailoverTrigger::Timeout
                } else {
                    FailoverTrigger::NetworkError(e.to_string())
                };
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
                            "{} — retrying ({}/{})",
                            trigger.kind_str(),
                            retry_count,
                            failover_policy.retry_attempts
                        );
                        continue;
                    }
                    if try_add_fallbacks(
                        &mut active_routes,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                    ) {
                        attempt_index += 1;
                        retry_count = 0;
                        continue;
                    }
                }

                warn!("Upstream error: {e}");
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
                );
                return Err(StatusCode::BAD_GATEWAY);
            }
            Err(_) => {
                // Timeout on first byte
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
                    if try_add_fallbacks(
                        &mut active_routes,
                        &failover_policy,
                        &model,
                        &state,
                        pathname,
                        search,
                        request_type.clone(),
                    ) {
                        attempt_index += 1;
                        retry_count = 0;
                        continue;
                    }
                }
                warn!("Upstream timeout");
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
                );
                return Err(StatusCode::GATEWAY_TIMEOUT);
            }
        }
    }
}

fn try_add_fallbacks(
    active_routes: &mut Vec<RouteResult>,
    policy: &crate::config::GatewayFailoverPolicy,
    model: &str,
    state: &AppState,
    pathname: &str,
    search: &str,
    request_type: crate::config::UpstreamType,
) -> bool {
    use crate::config::{ModelFallbackMode, RoutingVisibility};

    if policy.max_fallback_attempts == 0 {
        return false;
    }
    let max_fallbacks = policy.max_fallback_attempts as usize;

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
        if !seen.contains(&key) && added < max_fallbacks {
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

async fn build_response(
    upstream_resp: reqwest::Response,
    route: &RouteResult,
    converting_responses: bool,
    responses_model: &str,
    _idle_timeout_ms: u64,
    created_at: u64,
) -> Result<ResponseWithUsage, StatusCode> {
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
        let body_bytes = upstream_resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let len = body_bytes.len() as u64;
        let converted = crate::responses::convert_chat_to_responses(&body_bytes)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let body_str = String::from_utf8_lossy(&converted).to_string();
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(converted))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
            let raw_stream = upstream_resp.bytes_stream();
            let conv = ChatSseToResponsesSse::new(responses_model);
            let sse_stream = ResponsesSseStream::new(raw_stream, conv);
            let response = response_builder
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(sse_stream))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: None,
            });
        }

        if is_sse {
            let raw_stream = upstream_resp.bytes_stream();
            let body_stream = ModelRewriteStream::new(raw_stream, rw);
            let response = response_builder
                .body(Body::from_stream(body_stream))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: None,
            });
        }

        // Non-SSE with model rewriter: read full body, rewrite, extract usage
        let body_bytes = upstream_resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let len = body_bytes.len() as u64;
        let rewritten = rw.rewrite_chunk(&String::from_utf8_lossy(&body_bytes));
        let usage = extract_usage(&rewritten);
        let response = response_builder
            .body(Body::from(rewritten.clone()))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(ResponseWithUsage {
            response,
            usage,
            body_bytes: len,
            body_content: Some(rewritten),
            sse_observer: None,
        })
    } else {
        if converting_responses && is_sse {
            let raw_stream = upstream_resp.bytes_stream();
            let conv = ChatSseToResponsesSse::new(responses_model);
            let sse_stream = ResponsesSseStream::new(raw_stream, conv);
            let response = response_builder
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(sse_stream))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: None,
            });
        }

        if is_sse {
            let observer_handle = SseObserverHandle::new(created_at);
            let body_stream =
                ObservingSseStream::new(upstream_resp.bytes_stream(), observer_handle.clone());
            let response = response_builder
                .body(Body::from_stream(body_stream))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(ResponseWithUsage {
                response,
                usage: None,
                body_bytes: 0,
                body_content: None,
                sse_observer: Some(observer_handle),
            });
        }

        // Non-SSE: read full body, extract usage
        let body_bytes = upstream_resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let len = body_bytes.len() as u64;
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();
        let usage = extract_usage(&body_str);
        let response = response_builder
            .body(Body::from(body_bytes.to_vec()))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
        failover::FailoverTrigger::Status(status) => format!("HTTP {status}"),
        failover::FailoverTrigger::Timeout => "timeout".to_string(),
        failover::FailoverTrigger::NetworkError(msg) => format!("network_error: {msg}"),
    }
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
    original_route_prefix: Option<String>,
    original_request_model: Option<String>,
    retry_attempt: u32,
) {
    let ipc = state.ipc.clone();
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

    tokio::spawn(async move {
        ipc.send(RustToTsMessage::RequestLog {
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
            original_route_prefix,
            original_request_model,
            retry_attempt,
        });
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
        });
    });
}
