use crate::circuit_breaker::CircuitBreaker;
use crate::concurrency_limit::ConcurrencyLimits;
use crate::config::{
    AliasTarget, ApiKeyInfo, ConfigEntry, GatewayFailoverPolicy, GatewayTimeoutSettings,
};
use crate::ipc::IpcSender;
use crate::rate_limit_cooldown::RateLimitCooldowns;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

const UPSTREAM_TOTAL_TIMEOUT_SECS: u64 = 600;

#[derive(Clone)]
struct UpstreamHttpClient {
    client: reqwest::Client,
    connect_timeout_ms: u64,
}

fn build_http_client(connect_timeout_ms: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(connect_timeout_ms))
        .timeout(std::time::Duration::from_secs(UPSTREAM_TOTAL_TIMEOUT_SECS))
        .build()
        .expect("Failed to create HTTP client")
}

#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub providers: HashMap<String, ConfigEntry>,
    pub aliases: HashMap<String, AliasTarget>,
    #[expect(dead_code)]
    pub uuid_to_channel: HashMap<String, String>,
    pub failover: GatewayFailoverPolicy,
    pub timeouts: GatewayTimeoutSettings,
    pub api_keys: HashMap<String, ApiKeyInfo>,
    pub concurrency_rules: HashMap<String, crate::config::ConcurrencyRuleConfig>,
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
            concurrency_rules: payload
                .concurrency_rules
                .into_iter()
                .map(|rule| (rule.id.clone(), rule))
                .collect(),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub routing: Arc<RwLock<RoutingTable>>,
    http_client: Arc<RwLock<UpstreamHttpClient>>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub rate_limit_cooldowns: Arc<RateLimitCooldowns>,
    pub concurrency_limits: Arc<ConcurrencyLimits>,
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
        let connect_timeout_ms = routing.timeouts.connect_timeout_ms;
        AppState {
            routing: Arc::new(RwLock::new(routing)),
            http_client: Arc::new(RwLock::new(UpstreamHttpClient {
                client: build_http_client(connect_timeout_ms),
                connect_timeout_ms,
            })),
            circuit_breaker: Arc::new(CircuitBreaker::default()),
            rate_limit_cooldowns: Arc::new(RateLimitCooldowns::default()),
            concurrency_limits: Arc::new(ConcurrencyLimits::default()),
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

    pub async fn upstream_http_client(&self) -> reqwest::Client {
        self.http_client.read().await.client.clone()
    }

    /// reqwest 的 connect timeout 是 ClientBuilder 级设置。配置热更新时仅在值变化时
    /// 替换共享 Client；普通请求继续 clone 连接池句柄，不会按请求新建 Client。
    pub async fn update_connect_timeout(&self, connect_timeout_ms: u64) {
        if self.http_client.read().await.connect_timeout_ms == connect_timeout_ms {
            return;
        }

        let next = UpstreamHttpClient {
            client: build_http_client(connect_timeout_ms),
            connect_timeout_ms,
        };
        *self.http_client.write().await = next;
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
