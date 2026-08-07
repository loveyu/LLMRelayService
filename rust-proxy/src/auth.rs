use crate::config::ApiKeyInfo;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AuthResult {
    #[expect(dead_code)]
    pub api_key_id: Option<String>,
    #[expect(dead_code)]
    pub api_key_name: Option<String>,
    #[expect(dead_code)]
    pub quota_exhausted: bool,
    pub allowed_models: Vec<String>,
}

pub fn authenticate(
    headers: &axum::http::HeaderMap,
    _route: &crate::routing::RouteResult,
    gateway_admin_key: &str,
    api_keys: &HashMap<String, ApiKeyInfo>,
) -> Result<AuthResult, (axum::http::StatusCode, String)> {
    let credentials = extract_credentials(headers);

    if credentials.is_empty() {
        return Err((axum::http::StatusCode::UNAUTHORIZED, "Missing API key".to_string()));
    }

    if !gateway_admin_key.is_empty() {
        for cred in &credentials {
            if cred == gateway_admin_key {
                return Ok(AuthResult {
                    api_key_id: None,
                    api_key_name: Some("gateway_admin".to_string()),
                    quota_exhausted: false,
                    allowed_models: vec![],
                });
            }
        }
    }

    for cred in &credentials {
        let hash = sha256_hex(cred);
        if let Some(key_info) = api_keys.get(&hash) {
            if key_info.quota_exhausted {
                return Err((
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    format!("API key quota exhausted: {}", key_info.name),
                ));
            }
            return Ok(AuthResult {
                api_key_id: Some(key_info.id.clone()),
                api_key_name: Some(key_info.name.clone()),
                quota_exhausted: false,
                allowed_models: key_info.allowed_models.clone(),
            });
        }
    }

    Err((axum::http::StatusCode::UNAUTHORIZED, "Invalid API key".to_string()))
}

/// Match model name against allowlist patterns.
/// Supports exact match and suffix wildcard (e.g. "claude-*").
pub fn is_model_allowed(model: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|p| {
        if let Some(prefix) = p.strip_suffix('*') { model.starts_with(prefix) } else { model == p }
    })
}

fn extract_credentials(headers: &axum::http::HeaderMap) -> Vec<String> {
    let mut creds = Vec::new();

    if let Some(val) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        creds.push(val.trim().to_string());
    }

    if let Some(val) = headers.get("authorization").and_then(|v| v.to_str().ok())
        && let Some(token) = val.strip_prefix("Bearer ") {
            creds.push(token.trim().to_string());
        }

    creds
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}
