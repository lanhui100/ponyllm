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
        if let Some(existing_idx) = keys.iter().position(|k| k.id == entry.id) {
            keys[existing_idx] = Arc::new(entry);
        } else {
            keys.push(Arc::new(entry));
        }
        // Sort keys primarily by priority (ascending: 1, 2, 3...)
        keys.sort_by_key(|k| k.priority);
    }

    pub fn get_key_status(&self, key_id: &str) -> Option<KeyState> {
        let keys = self.keys.read();
        keys.iter().find(|k| k.id == key_id).map(|k| k.current_state())
    }

    /// Snapshot of all keys for admin observability (WEB-03): id/priority/weight
    /// plus effective state. Read-only; never exposes the raw key material.
    pub fn list_keys(&self) -> Vec<(String, u32, u32, KeyState)> {        let keys = self.keys.read();
        keys.iter()
            .map(|k| (k.id.clone(), k.priority, k.weight, k.current_state()))
            .collect()
    }

    /// Read-only snapshot of key entries (e.g. for project-id peeking).
    /// Clones only the `Arc`s; counters and state stay live.
    pub fn snapshot_keys(&self) -> Vec<Arc<ApiKeyEntry>> {
        self.keys.read().clone()
    }

    /// Select the next active, healthy key according to configured routing strategy
    pub fn select_key(&self) -> Result<Arc<ApiKeyEntry>> {
        self.select_key_excluding(&[])
    }

    /// Select the next active, healthy key excluding already attempted keys in current request
    pub fn select_key_excluding(&self, excluded_key_ids: &[String]) -> Result<Arc<ApiKeyEntry>> {
        let keys = self.keys.read();
        let active_keys: Vec<Arc<ApiKeyEntry>> = keys
            .iter()
            .filter(|k| k.current_state() == KeyState::Active && !excluded_key_ids.iter().any(|ex| ex == &k.id))
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
        // Singleton passthrough (B2): a lone key stays Active through
        // up to 2 transient 429 retries. Sustained failures (>=2 consecutive)
        // MUST cool down to prevent hammering the upstream without backoff.
        if keys.len() == 1 {
            if let PoolErrorType::RateLimit { retry_after } = &error {
                if retry_after.map(|d| d <= std::time::Duration::from_secs(60)).unwrap_or(true) {
                    if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
                        if entry.stats.consecutive_failures.load(Ordering::Relaxed) < 2 {
                            entry.record_transient_failure();
                            return;
                        }
                    }
                }
            }
        }
        let Some(entry) = keys.iter().find(|k| k.id == key_id).cloned() else {
            return;
        };

        let is_policy_violation = matches!(error, PoolErrorType::PolicyViolation);
        let violations = if is_policy_violation {
            entry.stats.policy_violations.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            0
        };

        // Mass-disable circuit breaker: permanently isolating a key
        // while it would leave <=50% of the pool alive is downgraded to a
        // 5-minute cooling.
        // HOWEVER, a key that triggers PolicyViolation for a second time (violations >= 2)
        // is confirmed dead and must be permanently disabled to avoid infinite oscillation loops.
        let permanent = is_policy_violation
            || (matches!(error, PoolErrorType::AuthInvalid) && entry.is_antigravity());

        if permanent && violations < 2 && self.would_break_floor_locked(&keys, key_id) {
            tracing::warn!(
                provider = %self.provider,
                key_id = %key_id,
                error = ?error,
                violations = violations,
                "mass-disable breaker tripped: downgrading permanent isolate to 5m cooling"
            );
            entry.record_failure(PoolErrorType::RateLimit {
                retry_after: Some(std::time::Duration::from_secs(300)),
            });
            return;
        }
        entry.record_failure(error);
    }

    /// True when permanently isolating `key_id` would leave at most half of
    /// the pool alive. Single-key pools are exempt (no floor to protect).
    fn would_break_floor_locked(&self, keys: &[Arc<ApiKeyEntry>], key_id: &str) -> bool {
        if keys.len() < 2 {
            return false;
        }
        let alive = keys
            .iter()
            .filter(|k| k.id == key_id || k.current_state() != KeyState::Disabled)
            .count();
        alive.saturating_sub(1) * 2 <= keys.len()
    }

    /// Count active healthy keys
    pub fn active_key_count(&self) -> usize {
        let keys = self.keys.read();
        keys.iter().filter(|k| k.current_state() == KeyState::Active).count()
    }

    /// Earliest unlock across cooling keys, for honest Retry-After.
    pub fn earliest_unlock(&self) -> Option<std::time::Duration> {
        let keys = self.keys.read();
        keys.iter().filter_map(|k| k.cooldown_remaining()).min()
    }

    /// Total keys in pool
    pub fn total_key_count(&self) -> usize {
        self.keys.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_key_deduplication_upsert() {
        let pool = KeyPool::new("test-provider", RoutingStrategy::RoundRobin);
        pool.add_key(ApiKeyEntry::new("k1", "token-v1", 1, 10));
        assert_eq!(pool.total_key_count(), 1);

        // Add key with same ID but different token and priority
        pool.add_key(ApiKeyEntry::new("k1", "token-v2", 2, 20));
        assert_eq!(pool.total_key_count(), 1);

        let keys = pool.snapshot_keys();
        assert_eq!(keys[0].id, "k1");
        assert_eq!(keys[0].api_key, "token-v2");
        assert_eq!(keys[0].priority, 2);
        assert_eq!(keys[0].weight, 20);
    }
}
