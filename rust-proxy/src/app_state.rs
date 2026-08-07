use crate::config::{
    AliasTarget, ApiKeyInfo, ConfigEntry, GatewayFailoverPolicy, GatewayTimeoutSettings,
};
use crate::ipc::IpcSender;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub providers: HashMap<String, ConfigEntry>,
    pub aliases: HashMap<String, AliasTarget>,
    #[expect(dead_code)]
    pub uuid_to_channel: HashMap<String, String>,
    pub failover: GatewayFailoverPolicy,
    pub timeouts: GatewayTimeoutSettings,
    pub api_keys: HashMap<String, ApiKeyInfo>,
}

impl RoutingTable {
    pub fn from_payload(payload: crate::config::SyncConfigPayload) -> Self {
        let mut uuid_to_channel = HashMap::new();
        for (name, entry) in &payload.providers {
            if let Some(ref uuid) = entry.provider_uuid {
                uuid_to_channel.insert(uuid.clone(), name.clone());
            }
        }

        let mut api_keys_map = HashMap::new();
        for key in &payload.api_keys {
            api_keys_map.insert(key.key_hash.clone(), key.clone());
        }

        RoutingTable {
            providers: payload.providers,
            aliases: payload.aliases,
            uuid_to_channel,
            failover: payload.failover,
            timeouts: payload.timeouts,
            api_keys: api_keys_map,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub routing: Arc<RwLock<RoutingTable>>,
    pub http_client: reqwest::Client,
    pub config_synced: Arc<RwLock<bool>>,
    pub gateway_admin_key: Arc<String>,
    pub ipc: Arc<IpcSender>,
}

impl AppState {
    pub fn new(routing: RoutingTable, ipc: IpcSender) -> Self {
        let gateway_admin_key = std::env::var("GATEWAY_API_KEY").unwrap_or_default();
        AppState {
            routing: Arc::new(RwLock::new(routing)),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("Failed to create HTTP client"),
            config_synced: Arc::new(RwLock::new(false)),
            gateway_admin_key: Arc::new(gateway_admin_key),
            ipc: Arc::new(ipc),
        }
    }
}
