use crate::config::ConcurrencyRuleConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct ConcurrencyLimits {
    active: Arc<Mutex<HashMap<String, usize>>>,
}

pub struct ConcurrencyPermit {
    rule_id: String,
    active: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        let Ok(mut active) = self.active.lock() else { return };
        let Some(value) = active.get_mut(&self.rule_id) else { return };
        *value = value.saturating_sub(1);
        if *value == 0 {
            active.remove(&self.rule_id);
        }
    }
}

impl ConcurrencyLimits {
    pub fn try_acquire(&self, rule_id: &str, max_concurrency: usize) -> Option<ConcurrencyPermit> {
        if max_concurrency == 0 {
            return None;
        }
        let Ok(mut active) = self.active.lock() else {
            return None;
        };
        let value = active.entry(rule_id.to_string()).or_default();
        if *value >= max_concurrency {
            return None;
        }
        *value += 1;
        Some(ConcurrencyPermit { rule_id: rule_id.to_string(), active: Arc::clone(&self.active) })
    }

    pub fn snapshot(
        &self,
        rules: &HashMap<String, ConcurrencyRuleConfig>,
    ) -> Vec<ConcurrencyRuleRuntime> {
        let active = self.active.lock().ok();
        let mut result: Vec<_> = rules
            .values()
            .map(|rule| {
                let active_requests =
                    active.as_ref().and_then(|value| value.get(&rule.id)).copied().unwrap_or(0);
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyRuleRuntime {
    pub id: String,
    pub name: String,
    pub max_concurrency: usize,
    pub active_requests: usize,
    pub available_slots: usize,
}
