use crate::config::{FailoverStatusRange, GatewayFailoverPolicy};

/// Triggers that can cause a failover/retry.
#[derive(Debug, Clone)]
pub enum FailoverTrigger {
    Timeout,
    NetworkError(String),
    Status(u16),
}

impl FailoverTrigger {
    pub fn kind_str(&self) -> &'static str {
        match self {
            FailoverTrigger::Timeout => "timeout",
            FailoverTrigger::NetworkError(_) => "network_error",
            FailoverTrigger::Status(_) => "status",
        }
    }
}

/// Check if a failover trigger should cause a retry according to the policy.
pub fn should_trigger_failover(policy: &GatewayFailoverPolicy, trigger: &FailoverTrigger) -> bool {
    match trigger {
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

/// Get custom model fallback models for a given model from the policy.
pub fn get_custom_model_fallbacks(policy: &GatewayFailoverPolicy, model: &str) -> Vec<String> {
    policy
        .custom_model_fallbacks
        .iter()
        .find(|r| r.model == model)
        .map(|r| r.fallbacks.clone())
        .unwrap_or_default()
}
