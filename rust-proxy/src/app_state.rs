use crate::config::{
    AliasTarget, ApiKeyInfo, ConfigEntry, GatewayFailoverPolicy, GatewayTimeoutSettings,
};
use crate::ipc::IpcSender;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

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
    config_synced_notify: Arc<Notify>,
    pub gateway_admin_key: Arc<String>,
    pub ipc: Arc<IpcSender>,
    /// 连通性测试中已知「客户端不传 stream 时上游强制流式」的 (channel, model) 集合。
    /// 内存级，进程重启失效。首次探测到后记下，后续该组合直接走流式、跳过必失败的非流式尝试。
    pub test_requires_stream: Arc<RwLock<HashSet<String>>>,
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
            config_synced_notify: Arc::new(Notify::new()),
            gateway_admin_key: Arc::new(gateway_admin_key),
            ipc: Arc::new(ipc),
            test_requires_stream: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub async fn mark_config_synced(&self) {
        *self.config_synced.write().await = true;
        self.config_synced_notify.notify_waiters();
    }

    pub async fn wait_for_config_sync(&self) {
        wait_for_config_sync_state(&self.config_synced, &self.config_synced_notify).await;
    }
}

async fn wait_for_config_sync_state(config_synced: &RwLock<bool>, config_synced_notify: &Notify) {
    if *config_synced.read().await {
        return;
    }

    loop {
        let notified = config_synced_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if *config_synced.read().await {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn config_sync_waiter_unblocks_after_notification() {
        let synced = Arc::new(RwLock::new(false));
        let notify = Arc::new(Notify::new());
        let update_synced = synced.clone();
        let update_notify = notify.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            *update_synced.write().await = true;
            update_notify.notify_waiters();
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_config_sync_state(&synced, &notify),
        )
        .await
        .expect("config sync waiter should unblock");
    }

    #[tokio::test]
    async fn config_sync_waiter_returns_immediately_when_already_synced() {
        let synced = RwLock::new(true);
        let notify = Notify::new();

        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            wait_for_config_sync_state(&synced, &notify),
        )
        .await
        .expect("already-synced waiter should return immediately");
    }
}
