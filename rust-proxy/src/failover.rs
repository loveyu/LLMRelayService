use crate::config::{FailoverStatusRange, GatewayFailoverPolicy};

/// Triggers that can cause a failover/retry.
#[derive(Debug, Clone)]
pub enum FailoverTrigger {
    ConnectError(String),
    Timeout,
    NetworkError(String),
    Status(u16),
}

impl FailoverTrigger {
    pub fn kind_str(&self) -> &'static str {
        match self {
            FailoverTrigger::ConnectError(_) => "connect_error",
            FailoverTrigger::Timeout => "timeout",
            FailoverTrigger::NetworkError(_) => "network_error",
            FailoverTrigger::Status(_) => "status",
        }
    }
}

/// Check if a failover trigger should cause a retry according to the policy.
pub fn should_trigger_failover(policy: &GatewayFailoverPolicy, trigger: &FailoverTrigger) -> bool {
    match trigger {
        FailoverTrigger::ConnectError(_) => policy.retry_on_network_error,
        FailoverTrigger::Timeout => policy.retry_on_timeout,
        FailoverTrigger::NetworkError(_) => policy.retry_on_network_error,
        FailoverTrigger::Status(status) => {
            // Check exact status codes
            if policy.retry_on_status_codes.contains(status) {
                return true;
            }
            // Check status ranges (e.g., "5xx")
            if *status >= 500
                && *status < 600
                && policy.retry_on_status_ranges.contains(&FailoverStatusRange::S5xx)
            {
                return true;
            }
            false
        }
    }
}

/// 建连失败已经证明当前地址不可达，再试同一路由只会重复等待 connect timeout。
/// 仍然允许进入 fallback；其他触发器保留全局 retryAttempts 行为。
pub fn should_retry_same_route(trigger: &FailoverTrigger) -> bool {
    !matches!(trigger, FailoverTrigger::ConnectError(_))
}

/// Get custom model fallback models for a given model from the policy.
pub fn get_custom_model_fallbacks(policy: &GatewayFailoverPolicy, model: &str) -> Vec<String> {
    policy
        .custom_model_fallbacks
        .iter()
        .find(|r| r.model == model)
        .map(|r| r.fallbacks.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_errors_skip_same_route_retry_but_remain_failover_eligible() {
        let policy = crate::config::default_failover();
        let trigger = FailoverTrigger::ConnectError("connection timed out".to_string());

        assert!(should_trigger_failover(&policy, &trigger));
        assert!(!should_retry_same_route(&trigger));
        assert!(should_retry_same_route(&FailoverTrigger::Timeout));
        assert!(should_retry_same_route(&FailoverTrigger::Status(503)));
    }
}
