use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Allow,
    HalfOpenProbe,
    SkipOpen,
}

#[derive(Debug, Clone, Copy)]
enum CircuitState {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen { probe_started_at: Instant },
}

/// 仅针对 TCP/DNS/TLS 建连失败的轻量熔断器。
///
/// key 由渠道名与上游 origin 组成：同一渠道的不同 API 路径共享故障状态，渠道改址后
/// 自动使用新 key，不会被旧地址的熔断状态误伤。
#[derive(Default)]
pub struct CircuitBreaker {
    states: Mutex<HashMap<String, CircuitState>>,
}

impl CircuitBreaker {
    pub async fn admit(&self, key: &str, enabled: bool, cooldown_ms: u64) -> Admission {
        self.admit_at(key, enabled, Duration::from_millis(cooldown_ms), Instant::now()).await
    }

    async fn admit_at(
        &self,
        key: &str,
        enabled: bool,
        cooldown: Duration,
        now: Instant,
    ) -> Admission {
        let mut states = self.states.lock().await;
        if !enabled {
            states.remove(key);
            return Admission::Allow;
        }

        match states.get(key).copied() {
            Some(CircuitState::Open { opened_at })
                if now.saturating_duration_since(opened_at) >= cooldown =>
            {
                states.insert(key.to_string(), CircuitState::HalfOpen { probe_started_at: now });
                Admission::HalfOpenProbe
            }
            Some(CircuitState::Open { .. }) => Admission::SkipOpen,
            Some(CircuitState::HalfOpen { probe_started_at })
                if now.saturating_duration_since(probe_started_at) >= cooldown =>
            {
                // 防止探测请求被客户端取消后永久卡在 half-open；一个冷却周期后允许新探测。
                states.insert(key.to_string(), CircuitState::HalfOpen { probe_started_at: now });
                Admission::HalfOpenProbe
            }
            Some(CircuitState::HalfOpen { .. }) => Admission::SkipOpen,
            Some(CircuitState::Closed { .. }) | None => Admission::Allow,
        }
    }

    /// 记录一次建连失败，返回本次失败是否让熔断器进入 open。
    pub async fn record_connect_failure(
        &self,
        key: &str,
        enabled: bool,
        failure_threshold: u32,
    ) -> bool {
        self.record_connect_failure_at(key, enabled, failure_threshold, Instant::now()).await
    }

    async fn record_connect_failure_at(
        &self,
        key: &str,
        enabled: bool,
        failure_threshold: u32,
        now: Instant,
    ) -> bool {
        if !enabled {
            return false;
        }

        let threshold = failure_threshold.max(1);
        let mut states = self.states.lock().await;
        let next_failures = match states.get(key).copied() {
            Some(CircuitState::Closed { consecutive_failures }) => consecutive_failures + 1,
            Some(CircuitState::Open { .. }) | Some(CircuitState::HalfOpen { .. }) => threshold,
            None => 1,
        };

        if next_failures >= threshold {
            states.insert(key.to_string(), CircuitState::Open { opened_at: now });
            true
        } else {
            states.insert(
                key.to_string(),
                CircuitState::Closed { consecutive_failures: next_failures },
            );
            false
        }
    }

    /// 只要已经拿到上游响应头，就说明连接链路恢复；HTTP 错误状态不属于连接故障。
    pub async fn record_connect_success(&self, key: &str) {
        self.states.lock().await.remove(key);
    }
}

pub fn route_key(channel_name: &str, target_url: &str) -> String {
    let origin = reqwest::Url::parse(target_url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|_| target_url.to_string());
    format!("{channel_name}|{origin}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_after_threshold_and_allows_one_probe_after_cooldown() {
        let breaker = CircuitBreaker::default();
        let key = "primary|http://10.0.0.1:5588";
        let started = Instant::now();

        assert_eq!(
            breaker.admit_at(key, true, Duration::from_secs(30), started).await,
            Admission::Allow
        );
        assert!(!breaker.record_connect_failure_at(key, true, 2, started).await);
        assert!(
            breaker.record_connect_failure_at(key, true, 2, started + Duration::from_secs(1)).await
        );
        assert_eq!(
            breaker
                .admit_at(key, true, Duration::from_secs(30), started + Duration::from_secs(10))
                .await,
            Admission::SkipOpen
        );
        assert_eq!(
            breaker
                .admit_at(key, true, Duration::from_secs(30), started + Duration::from_secs(31))
                .await,
            Admission::HalfOpenProbe
        );
        assert_eq!(
            breaker
                .admit_at(key, true, Duration::from_secs(30), started + Duration::from_secs(32))
                .await,
            Admission::SkipOpen
        );
    }

    #[tokio::test]
    async fn successful_half_open_probe_closes_the_circuit() {
        let breaker = CircuitBreaker::default();
        let key = "primary|http://10.0.0.1:5588";
        let started = Instant::now();

        assert!(breaker.record_connect_failure_at(key, true, 1, started).await);
        assert_eq!(
            breaker
                .admit_at(key, true, Duration::from_secs(30), started + Duration::from_secs(31))
                .await,
            Admission::HalfOpenProbe
        );
        breaker.record_connect_success(key).await;
        assert_eq!(
            breaker
                .admit_at(key, true, Duration::from_secs(30), started + Duration::from_secs(32))
                .await,
            Admission::Allow
        );
    }

    #[test]
    fn route_key_groups_paths_by_channel_and_origin() {
        assert_eq!(
            route_key("primary", "http://10.0.0.1:5588/v1/messages"),
            route_key("primary", "http://10.0.0.1:5588/v1/chat/completions")
        );
        assert_ne!(
            route_key("primary", "http://10.0.0.1:5588/v1/messages"),
            route_key("secondary", "http://10.0.0.1:5588/v1/messages")
        );
    }
}
