use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;
use crate::error::{CoreError, Result};
use super::entry::{ApiKeyEntry, KeyState, PoolErrorType};
use super::strategy::RoutingStrategy;

#[derive(Debug)]
pub struct KeyPool {
    pub provider: String,
    pub strategy: RoutingStrategy,
    keys: RwLock<Vec<Arc<ApiKeyEntry>>>,
    rr_counter: AtomicUsize,
}

impl KeyPool {
    pub fn new(provider: impl Into<String>, strategy: RoutingStrategy) -> Self {
        Self {
            provider: provider.into(),
            strategy,
            keys: RwLock::new(Vec::new()),
            rr_counter: AtomicUsize::new(0),
        }
    }

    pub fn add_key(&self, entry: ApiKeyEntry) {
        let mut keys = self.keys.write();
        keys.push(Arc::new(entry));
        // Sort keys primarily by priority (ascending: 1, 2, 3...)
        keys.sort_by_key(|k| k.priority);
    }

    pub fn get_key_status(&self, key_id: &str) -> Option<KeyState> {
        let keys = self.keys.read();
        keys.iter().find(|k| k.id == key_id).map(|k| k.current_state())
    }

    /// Select the next active, healthy key according to configured routing strategy
    pub fn select_key(&self) -> Result<Arc<ApiKeyEntry>> {
        let keys = self.keys.read();
        let active_keys: Vec<Arc<ApiKeyEntry>> = keys
            .iter()
            .filter(|k| k.current_state() == KeyState::Active)
            .cloned()
            .collect();

        if active_keys.is_empty() {
            return Err(CoreError::NoAvailableKey(self.provider.clone()));
        }

        match self.strategy {
            RoutingStrategy::Priority => {
                // Return the lowest priority number (highest priority) available
                let mut sorted = active_keys;
                sorted.sort_by_key(|k| k.priority);
                Ok(sorted[0].clone())
            }
            RoutingStrategy::RoundRobin => {
                let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % active_keys.len();
                Ok(active_keys[idx].clone())
            }
            RoutingStrategy::WeightedRoundRobin => {
                // Weighted selection based on weight field
                let total_weight: u32 = active_keys.iter().map(|k| k.weight.max(1)).sum();
                if total_weight == 0 {
                    let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % active_keys.len();
                    return Ok(active_keys[idx].clone());
                }
                let count = self.rr_counter.fetch_add(1, Ordering::Relaxed) as u32 % total_weight;
                let mut acc = 0;
                for k in &active_keys {
                    acc += k.weight.max(1);
                    if count < acc {
                        return Ok(k.clone());
                    }
                }
                Ok(active_keys[0].clone())
            }
        }
    }

    /// Record a successful request on a key
    pub fn record_success(&self, key_id: &str) {
        let keys = self.keys.read();
        if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
            entry.record_success();
        }
    }

    /// Record an error on a key
    pub fn record_error(&self, key_id: &str, error: PoolErrorType) {
        let keys = self.keys.read();
        if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
            entry.record_failure(error);
        }
    }
}
