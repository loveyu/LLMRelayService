use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::app_state::AppState;
use crate::rate_limit_cooldown::MAX_COOLDOWN;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearCooldownRequest {
    channel: Option<String>,
    model: Option<String>,
}

pub async fn list_cooldowns_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !crate::provider_test::is_admin(&headers, &state.gateway_admin_key) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "未授权" }))).into_response();
    }

    Json(json!({
        "cooldowns": state.rate_limit_cooldowns.list(),
        "maxCooldownMs": MAX_COOLDOWN.as_millis() as u64,
    }))
    .into_response()
}

pub async fn clear_cooldowns_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ClearCooldownRequest>,
) -> Response {
    if !crate::provider_test::is_admin(&headers, &state.gateway_admin_key) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "未授权" }))).into_response();
    }

    let cleared = state.rate_limit_cooldowns.clear(body.channel.as_deref(), body.model.as_deref());
    Json(json!({ "ok": true, "cleared": cleared })).into_response()
}
