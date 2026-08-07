use serde::{Deserialize, Serialize};

/// Matches TS `UpstreamType = 'anthropic' | 'openai'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpstreamType {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
}

/// Matches TS `RouteAuthHeader = 'x-api-key' | 'authorization'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteAuthHeader {
    #[serde(rename = "x-api-key")]
    XApiKey,
    Authorization,
}

/// Matches TS `RoutingVisibility = 'direct' | 'explicit_only'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingVisibility {
    Direct,
    #[serde(rename = "explicit_only")]
    ExplicitOnly,
}

/// Matches TS `OpenAiResponsesMode = 'native' | 'chat_compat' | 'disabled'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiResponsesMode {
    Native,
    #[serde(rename = "chat_compat")]
    ChatCompat,
    Disabled,
}

/// Matches TS `RouteAuthConfig`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteAuthConfig {
    pub header: RouteAuthHeader,
    pub value: String,
}

/// Matches TS `ModelConfig`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Matches TS `ConfigEntry` — a single provider/channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEntry {
    #[serde(default = "default_upstream_type", rename = "type")]
    pub upstream_type: UpstreamType,
    #[serde(rename = "targetBaseUrl")]
    pub target_base_url: String,
    #[serde(default, rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RouteAuthConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelConfig>>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, rename = "routingVisibility", skip_serializing_if = "Option::is_none")]
    pub routing_visibility: Option<RoutingVisibility>,
    #[serde(default, rename = "responsesMode", skip_serializing_if = "Option::is_none")]
    pub responses_mode: Option<OpenAiResponsesMode>,
    #[serde(default, rename = "extraFields", skip_serializing_if = "Option::is_none")]
    pub extra_fields: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, rename = "providerUuid", skip_serializing_if = "Option::is_none")]
    pub provider_uuid: Option<String>,
    #[serde(default, rename = "autoSyncModels")]
    pub auto_sync_models: bool,
    #[serde(default, rename = "claudeCodeCompat")]
    pub claude_code_compat: bool,
}

fn default_upstream_type() -> UpstreamType {
    UpstreamType::OpenAI
}

fn default_true() -> bool {
    true
}

/// Matches TS `ModelAliasTarget`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAliasTarget {
    pub provider: String,
    pub model: String,
}

/// Matches TS `AliasTarget` — in-memory alias entry (from TS `listModelAliases` filtered enabled)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasTarget {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<ModelAliasTarget>>,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default, rename = "returnRealModel")]
    pub return_real_model: bool,
}

/// Matches TS `ModelFallbackMode`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFallbackMode {
    Disabled,
    #[serde(rename = "same_model")]
    SameModel,
    #[serde(rename = "any_model")]
    AnyModel,
}

/// Matches TS `FailoverStatusRange = '5xx'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailoverStatusRange {
    #[serde(rename = "5xx")]
    S5xx,
}

/// Matches TS `CustomModelFallbackRule`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModelFallbackRule {
    pub model: String,
    pub fallbacks: Vec<String>,
}

/// Matches TS `GatewayFailoverPolicy`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayFailoverPolicy {
    pub enabled: bool,
    #[serde(rename = "retryAttempts")]
    pub retry_attempts: u32,
    #[serde(rename = "modelFallbackMode")]
    pub model_fallback_mode: ModelFallbackMode,
    #[serde(rename = "maxFallbackAttempts")]
    pub max_fallback_attempts: u32,
    #[serde(default, rename = "customModelFallbacks")]
    pub custom_model_fallbacks: Vec<CustomModelFallbackRule>,
    #[serde(rename = "retryOnTimeout")]
    pub retry_on_timeout: bool,
    #[serde(rename = "retryOnNetworkError")]
    pub retry_on_network_error: bool,
    #[serde(rename = "retryOnStatusCodes")]
    pub retry_on_status_codes: Vec<u16>,
    #[serde(rename = "retryOnStatusRanges")]
    pub retry_on_status_ranges: Vec<FailoverStatusRange>,
}

/// Matches TS `GatewayTimeoutSettings`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayTimeoutSettings {
    #[serde(rename = "defaultFirstByteTimeoutMs")]
    pub default_first_byte_timeout_ms: u64,
    #[serde(rename = "streamFirstByteTimeoutMs")]
    pub stream_first_byte_timeout_ms: u64,
    #[serde(rename = "imageFirstByteTimeoutMs")]
    pub image_first_byte_timeout_ms: u64,
    #[serde(rename = "responseIdleTimeoutMs")]
    pub response_idle_timeout_ms: u64,
}

/// Matches TS `AuthenticatedApiKeyInfo` (subset — what Rust needs for auth + quota)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    pub id: String,
    pub name: String,
    /// SHA-256 hex string of the key for lookup
    #[serde(rename = "keyHash")]
    pub key_hash: String,
    #[serde(default, rename = "allowedModels")]
    pub allowed_models: Vec<String>,
    #[serde(default, rename = "costQuota")]
    pub cost_quota: Option<i64>,
    #[serde(default, rename = "costUsed")]
    pub cost_used: i64,
    #[serde(rename = "quotaExhausted")]
    pub quota_exhausted: bool,
}

/// The complete configuration snapshot that TS sends to Rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfigPayload {
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ConfigEntry>,
    #[serde(default)]
    pub aliases: std::collections::HashMap<String, AliasTarget>,
    #[serde(default = "default_failover")]
    pub failover: GatewayFailoverPolicy,
    #[serde(default = "default_timeouts")]
    pub timeouts: GatewayTimeoutSettings,
    #[serde(default)]
    pub api_keys: Vec<ApiKeyInfo>,
}

impl Default for SyncConfigPayload {
    fn default() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
            aliases: std::collections::HashMap::new(),
            failover: default_failover(),
            timeouts: default_timeouts(),
            api_keys: Vec::new(),
        }
    }
}

fn default_failover() -> GatewayFailoverPolicy {
    GatewayFailoverPolicy {
        enabled: true,
        retry_attempts: 1,
        model_fallback_mode: ModelFallbackMode::SameModel,
        max_fallback_attempts: 2,
        custom_model_fallbacks: Vec::new(),
        retry_on_timeout: true,
        retry_on_network_error: true,
        retry_on_status_codes: vec![408, 429],
        retry_on_status_ranges: vec![FailoverStatusRange::S5xx],
    }
}

fn default_timeouts() -> GatewayTimeoutSettings {
    GatewayTimeoutSettings {
        default_first_byte_timeout_ms: 300_000,
        stream_first_byte_timeout_ms: 300_000,
        image_first_byte_timeout_ms: 300_000,
        response_idle_timeout_ms: 300_000,
    }
}
