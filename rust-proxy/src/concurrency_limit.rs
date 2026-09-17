use crate::config::ConcurrencyRuleConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct ConcurrencyLimits {
    active: Arc<Mutex<ActiveCounters>>,
}

#[derive(Default)]
struct ActiveCounters {
    rules: HashMap<String, usize>,
    channels: HashMap<String, usize>,
    models: HashMap<(String, String), usize>,
}

pub struct ConcurrencyPermit {
    rule_id: Option<String>,
    channel: String,
    model: String,
    active: Arc<Mutex<ActiveCounters>>,
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        let Ok(mut active) = self.active.lock() else { return };
        if let Some(rule_id) = self.rule_id.as_ref()
            && let Some(value) = active.rules.get_mut(rule_id)
        {
            *value = value.saturating_sub(1);
            if *value == 0 {
                active.rules.remove(rule_id);
            }
        }
        decrement(&mut active.channels, &self.channel);
        decrement(&mut active.models, &(self.channel.clone(), self.model.clone()));
    }
}

impl ConcurrencyLimits {
    pub fn try_acquire(
        &self,
        rule_id: &str,
        max_concurrency: usize,
        channel: &str,
        model: &str,
    ) -> Option<ConcurrencyPermit> {
        if max_concurrency == 0 {
            return None;
        }
        let Ok(mut active) = self.active.lock() else {
            return None;
        };
        let value = active.rules.entry(rule_id.to_string()).or_default();
        if *value >= max_concurrency {
            return None;
        }
        *value += 1;
        *active.channels.entry(channel.to_string()).or_default() += 1;
        *active.models.entry((channel.to_string(), model.to_string())).or_default() += 1;
        Some(ConcurrencyPermit {
            rule_id: Some(rule_id.to_string()),
            channel: channel.to_string(),
            model: model.to_string(),
            active: Arc::clone(&self.active),
        })
    }

    /// 未绑定规则的请求也纳入实时观测，但不参与任何上限判断。
    pub fn track(&self, channel: &str, model: &str) -> ConcurrencyPermit {
        let mut active = self.active.lock().expect("concurrency counters lock");
        *active.channels.entry(channel.to_string()).or_default() += 1;
        *active.models.entry((channel.to_string(), model.to_string())).or_default() += 1;
        ConcurrencyPermit {
            rule_id: None,
            channel: channel.to_string(),
            model: model.to_string(),
            active: Arc::clone(&self.active),
        }
    }

    pub fn snapshot(
        &self,
        rules: &HashMap<String, ConcurrencyRuleConfig>,
    ) -> Vec<ConcurrencyRuleRuntime> {
        let active = self.active.lock().ok();
        let mut result: Vec<_> = rules
            .values()
            .map(|rule| {
                let active_requests = active
                    .as_ref()
                    .and_then(|value| value.rules.get(&rule.id))
                    .copied()
                    .unwrap_or(0);
                ConcurrencyRuleRuntime {
                    id: rule.id.clone(),
                    name: rule.name.clone(),
                    max_concurrency: rule.max_concurrency,
                    active_requests,
                    available_slots: rule.max_concurrency.saturating_sub(active_requests),
                }
            })
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }
}

fn decrement<K: std::hash::Hash + Eq + Clone>(values: &mut HashMap<K, usize>, key: &K) {
    if let Some(value) = values.get_mut(key) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            values.remove(key);
        }
    }
}

impl ConcurrencyLimits {
    pub fn channel_snapshot(&self) -> Vec<ConcurrencyChannelRuntime> {
        let Ok(active) = self.active.lock() else {
            return Vec::new();
        };
        let mut by_channel: HashMap<String, Vec<ConcurrencyModelRuntime>> = HashMap::new();
        for ((channel, model), active_requests) in &active.models {
            by_channel.entry(channel.clone()).or_default().push(ConcurrencyModelRuntime {
                model: model.clone(),
                active_requests: *active_requests,
            });
        }
        let mut channels: Vec<_> = active
            .channels
            .iter()
            .map(|(channel, active_requests)| {
                let mut models = by_channel.remove(channel).unwrap_or_default();
                models.sort_by(|a, b| a.model.cmp(&b.model));
                ConcurrencyChannelRuntime {
                    channel: channel.clone(),
                    active_requests: *active_requests,
                    models,
                }
            })
            .collect();
        channels.sort_by(|a, b| a.channel.cmp(&b.channel));
        channels
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyRuleRuntime {
    pub id: String,
    pub name: String,
    pub max_concurrency: usize,
    pub active_requests: usize,
    pub available_slots: usize,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyChannelRuntime {
    pub channel: String,
    pub active_requests: usize,
    pub models: Vec<ConcurrencyModelRuntime>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyModelRuntime {
    pub model: String,
    pub active_requests: usize,
}
