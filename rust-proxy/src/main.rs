mod app_state;
mod auth;
mod config;
mod failover;
mod ipc;
mod logging;
mod provider_test;
mod proxy;
mod responses;
mod routing;
mod sse_observer;
mod transform;

use crate::app_state::{AppState, RoutingTable};
use crate::config::SyncConfigPayload;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _log_guard = logging::init()?;

    info!("Starting LRS Rust Proxy Engine");

    let bridge = ipc::IpcBridge::start().await?;
    info!("IPC bridge started");
    let config_rx = bridge.config_rx;
    let ipc_sender = bridge.sender;

    let routing = RoutingTable::from_payload(SyncConfigPayload::default());
    let app_state = Arc::new(AppState::new(routing, ipc_sender));

    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    // 请求体上限：默认 100MB，可通过 RUST_PROXY_MAX_BODY_BYTES 覆盖。
    // 10MB 对带图片的请求（codex responses 里 base64 图 + 长 system prompt + 上下文）不够，会 413。
    let max_body_bytes = std::env::var("RUST_PROXY_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100 * 1024 * 1024);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/v1/models", get(models_handler))
        .route("/openai/v1/models", get(models_handler))
        .route("/anthropic/v1/models", get(models_handler))
        // 渠道连通性测试：本机 TS 控制台/OpenAPI 经此走 Rust 测试上游，TS 旧实现逐步废弃。
        .route("/admin/providers/{channel_name}/test", post(provider_test::test_provider_handler))
        .fallback(proxy::proxy_handler)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(max_body_bytes))
        // axum 的 body 提取器(Bytes/String/Json)另有 DefaultBodyLimit(默认仅 2MB),
        // 跟上面 tower-http 层是两套机制;必须同步抬高,否则大 body(图片)在提取阶段就被 413。
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(cors)
        .with_state(app_state.clone());

    // Listen for config updates from IPC
    tokio::spawn({
        let app_state = app_state.clone();
        let mut config_rx = config_rx;
        async move {
            while let Some(config) = config_rx.recv().await {
                info!(
                    "Config updated: {} providers, {} aliases, {} api keys",
                    config.providers.len(),
                    config.aliases.len(),
                    config.api_keys.len(),
                );
                let new_routing = RoutingTable::from_payload(config);
                {
                    let mut rt = app_state.routing.write().await;
                    *rt = new_routing;
                }
                app_state.mark_config_synced().await;
            }
        }
    });

    let port = std::env::var("RUST_PROXY_PORT").unwrap_or_else(|_| "3301".to_string());
    let host = std::env::var("RUST_PROXY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Rust proxy listening on {addr}");

    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;

    info!("Rust proxy shut down gracefully");
    Ok(())
}

async fn models_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let rt = state.routing.read().await;
    // Collect all unique model names from providers
    let mut model_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in rt.providers.values() {
        if entry.enabled
            && let Some(ref entry_models) = entry.models
        {
            for m in entry_models {
                model_set.insert(m.model.clone());
            }
        }
    }
    // Also collect alias names
    for alias in rt.aliases.keys() {
        model_set.insert(alias.clone());
    }
    let data: Vec<serde_json::Value> = model_set
        .into_iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "created": 1,
                "owned_by": "lrs",
            })
        })
        .collect();
    (
        axum::http::StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&serde_json::json!({
            "object": "list",
            "data": data,
        }))
        .unwrap_or_default(),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, draining connections...");
}

async fn health_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let synced = *state.config_synced.read().await;
    Json(serde_json::json!({
        "status": "ok",
        "service": "rust-proxy",
        "config_synced": synced,
    }))
}
