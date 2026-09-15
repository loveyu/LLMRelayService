use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

pub const BASE_COOLDOWN: Duration = Duration::from_secs(30);
pub const MIN_COOLDOWN: Duration = Duration::from_secs(1);
pub const MAX_COOLDOWN: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CooldownKey {
    channel: String,
    model: String,
}

#[derive(Debug, Clone, Copy)]
struct CooldownEntry {
    consecutive_429s: u32,
    until: Instant,
    duration: Duration,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CooldownSnapshot {
    pub channel: String,
    pub model: String,
    pub consecutive_429s: u32,
    pub remaining_ms: u64,
    pub cooldown_ms: u64,
    pub expires_at: u64,
}

#[derive(Default)]
pub struct RateLimitCooldowns {
    entries: Mutex<HashMap<CooldownKey, CooldownEntry>>,
}

impl RateLimitCooldowns {
    pub fn remaining(&self, channel: &str, model: &str, enabled: bool) -> Option<Duration> {
        let key = cooldown_key(channel, model);
        let mut entries = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !enabled {
            entries.remove(&key);
            return None;
        }

        let entry = entries.get(&key)?;
        entry.until.checked_duration_since(Instant::now())
    }

    pub fn record_429(
        &self,
        channel: &str,
        model: &str,
        retry_after: Option<&str>,
        enabled: bool,
    ) -> Duration {
        self.record_429_at(channel, model, retry_after, enabled, Instant::now(), SystemTime::now())
    }

    fn record_429_at(
        &self,
        channel: &str,
        model: &str,
        retry_after: Option<&str>,
        enabled: bool,
        now: Instant,
        wall_now: SystemTime,
    ) -> Duration {
        let key = cooldown_key(channel, model);
        let mut entries = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !enabled {
            entries.remove(&key);
            return Duration::ZERO;
        }

        let consecutive_429s =
            entries.get(&key).map_or(1, |entry| entry.consecutive_429s.saturating_add(1));
        let duration = parse_retry_after(retry_after, wall_now)
            .unwrap_or_else(|| jittered_exponential_cooldown(&key, consecutive_429s, wall_now));
        entries.insert(key, CooldownEntry { consecutive_429s, until: now + duration, duration });
        duration
    }

    pub fn record_success(&self, channel: &str, model: &str) {
        let key = cooldown_key(channel, model);
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&key);
    }

    pub fn list(&self) -> Vec<CooldownSnapshot> {
        let now = Instant::now();
        let wall_now = SystemTime::now();
        let entries = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut result: Vec<_> = entries
            .iter()
            .filter_map(|(key, entry)| {
                let remaining = entry.until.checked_duration_since(now)?;
                let expires_at =
                    wall_now.checked_add(remaining)?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
                Some(CooldownSnapshot {
                    channel: key.channel.clone(),
                    model: key.model.clone(),
                    consecutive_429s: entry.consecutive_429s,
                    remaining_ms: duration_ms(remaining),
                    cooldown_ms: duration_ms(entry.duration),
                    expires_at: expires_at.as_millis().min(u128::from(u64::MAX)) as u64,
                })
            })
            .collect();
        result.sort_by_key(|entry| entry.expires_at);
        result
    }

    pub fn clear(&self, channel: Option<&str>, model: Option<&str>) -> usize {
        let mut entries = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = entries.len();
        entries.retain(|key, _| {
            let channel_matches = channel.is_none_or(|expected| expected == key.channel);
            let model_matches = model.is_none_or(|expected| expected == key.model);
            !(channel_matches && model_matches)
        });
        before.saturating_sub(entries.len())
    }
}

pub fn route_model<'a>(resolved_model: Option<&'a str>, requested_model: &'a str) -> &'a str {
    resolved_model.filter(|model| !model.is_empty()).unwrap_or(requested_model)
}

fn cooldown_key(channel: &str, model: &str) -> CooldownKey {
    CooldownKey { channel: channel.to_string(), model: model.to_string() }
}

fn parse_retry_after(value: Option<&str>, now: SystemTime) -> Option<Duration> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    let requested = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        httpdate::parse_http_date(value).ok()?.duration_since(now).unwrap_or(Duration::ZERO)
    };
    Some(requested.clamp(MIN_COOLDOWN, MAX_COOLDOWN))
}

fn jittered_exponential_cooldown(
    key: &CooldownKey,
    consecutive_429s: u32,
    wall_now: SystemTime,
) -> Duration {
    let exponent = consecutive_429s.saturating_sub(1).min(31);
    let base_ms =
        duration_ms(BASE_COOLDOWN).saturating_mul(1_u64 << exponent).min(duration_ms(MAX_COOLDOWN));

    // 稳定地落在 80%..=120%：避免一批并发请求同时结束冷却造成惊群。
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    consecutive_429s.hash(&mut hasher);
    wall_now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let jitter_per_mille = 800 + hasher.finish() % 401;
    let jittered_ms = base_ms.saturating_mul(jitter_per_mille) / 1000;
    Duration::from_millis(jittered_ms.clamp(duration_ms(MIN_COOLDOWN), duration_ms(MAX_COOLDOWN)))
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub fn retry_after_seconds(duration: Duration) -> u64 {
    duration_ms(duration).div_ceil(1000).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_clamped_to_five_minutes() {
        let cooldowns = RateLimitCooldowns::default();
        let duration = cooldowns.record_429_at(
            "primary",
            "gpt-5",
            Some("3600"),
            true,
            Instant::now(),
            SystemTime::now(),
        );
        assert_eq!(duration, MAX_COOLDOWN);
    }

    #[test]
    fn retry_after_http_date_is_supported_and_clamped() {
        let cooldowns = RateLimitCooldowns::default();
        let wall_now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let retry_at = httpdate::fmt_http_date(wall_now + Duration::from_secs(600));
        let duration = cooldowns.record_429_at(
            "primary",
            "gpt-5",
            Some(&retry_at),
            true,
            Instant::now(),
            wall_now,
        );
        assert_eq!(duration, MAX_COOLDOWN);
    }

    #[test]
    fn missing_retry_after_uses_jittered_exponential_backoff() {
        let cooldowns = RateLimitCooldowns::default();
        let now = Instant::now();
        let wall_now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let expected_bases = [30_u64, 60, 120, 240, 300, 300];

        for (index, base_seconds) in expected_bases.into_iter().enumerate() {
            let duration = cooldowns.record_429_at(
                "primary",
                "gpt-5",
                None,
                true,
                now,
                wall_now + Duration::from_nanos(index as u64),
            );
            let lower = Duration::from_millis(base_seconds * 800);
            let upper = Duration::from_millis((base_seconds * 1200).min(300_000));
            assert!(duration >= lower, "attempt {} below jitter range", index + 1);
            assert!(duration <= upper, "attempt {} above jitter range", index + 1);
        }
    }

    #[test]
    fn success_resets_consecutive_count() {
        let cooldowns = RateLimitCooldowns::default();
        cooldowns.record_429("primary", "gpt-5", Some("10"), true);
        cooldowns.record_429("primary", "gpt-5", Some("10"), true);
        cooldowns.record_success("primary", "gpt-5");
        cooldowns.record_429("primary", "gpt-5", Some("10"), true);

        let snapshots = cooldowns.list();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].consecutive_429s, 1);
    }

    #[test]
    fn cooldown_is_scoped_by_channel_and_model() {
        let cooldowns = RateLimitCooldowns::default();
        cooldowns.record_429("primary", "gpt-5", Some("30"), true);

        assert!(cooldowns.remaining("primary", "gpt-5", true).is_some());
        assert!(cooldowns.remaining("primary", "gpt-4", true).is_none());
        assert!(cooldowns.remaining("secondary", "gpt-5", true).is_none());
    }
}
