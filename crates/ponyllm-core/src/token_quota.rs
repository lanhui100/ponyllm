//! Token-level quota tracker (B001): in-memory, non-persistent, isomorphic to
//! [`crate::user::UserQuotaTracker`] but keyed by **token id**
//! (`GatewayKeyEntry.id` / `sk-pony-*` key id).
//!
//! Quota limits are NOT stored here: the caller passes the limit at check time
//! (`check_quota(key_id, quota_limit)`), mirroring how `UserEntry.max_tokens`
//! stays in config while the tracker only accounts usage.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// One token's runtime accounting state.
#[derive(Debug)]
struct TokenRuntimeState {
    used_tokens: AtomicU64,
}

/// Concurrent quota-usage tracker keyed by token id.
///
/// `upsert` preserves the existing used-token counter (same semantics as
/// `UserQuotaTracker::upsert_user`); `remove` deletes the row outright.
#[derive(Debug, Clone, Default)]
pub struct TokenQuotaTracker {
    tokens: Arc<DashMap<String, Arc<TokenRuntimeState>>>,
}

/// Token quota check errors (B001 contract, frozen).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenQuotaError {
    /// Unknown token id (fail-closed: a quota check on a missing token is an
    /// error, never an implicit pass).
    #[error("token '{key_id}' does not exist")]
    TokenNotFound { key_id: String },
    /// `used_tokens >= quota_limit`.
    #[error("token '{key_id}' quota exhausted: used {used_tokens} >= limit {quota_limit}")]
    QuotaExhausted {
        key_id: String,
        used_tokens: u64,
        quota_limit: u64,
    },
}

impl TokenQuotaTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(DashMap::new()),
        }
    }

    /// Insert or refresh a token's accounting row, preserving any existing
    /// used-count.
    pub fn upsert(&self, key_id: impl Into<String>) {
        let key_id = key_id.into();
        let current_used = self
            .tokens
            .get(&key_id)
            .map(|existing| existing.used_tokens.load(Ordering::Relaxed))
            .unwrap_or(0);
        let state = Arc::new(TokenRuntimeState {
            used_tokens: AtomicU64::new(current_used),
        });
        self.tokens.insert(key_id, state);
    }

    /// Remove a token's accounting row. Returns the removed key id, or `None`
    /// when the token was not tracked.
    pub fn remove(&self, key_id: &str) -> Option<String> {
        self.tokens.remove(key_id).map(|(id, _)| id)
    }

    /// Add `tokens` to the used counter for `key_id` (no-op when untracked).
    pub fn record_tokens(&self, key_id: &str, tokens: u64) {
        if let Some(state) = self.tokens.get(key_id) {
            state.used_tokens.fetch_add(tokens, Ordering::Relaxed);
        }
    }

    /// Current used-token count for `key_id` (0 when untracked).
    pub fn get_used_tokens(&self, key_id: &str) -> u64 {
        self.tokens
            .get(key_id)
            .map(|state| state.used_tokens.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Zero the used counter for `key_id`. Returns `false` when untracked.
    pub fn reset_usage(&self, key_id: &str) -> bool {
        if let Some(state) = self.tokens.get(key_id) {
            state.used_tokens.store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Fail-closed quota gate: `Err(TokenNotFound)` for an untracked token;
    /// `Err(QuotaExhausted)` when `used >= quota_limit`; `Ok(())` otherwise.
    /// `quota_limit = None` means unrestricted (no quota gate).
    pub fn check_quota(
        &self,
        key_id: &str,
        quota_limit: Option<u64>,
    ) -> Result<(), TokenQuotaError> {
        let state = self
            .tokens
            .get(key_id)
            .ok_or_else(|| TokenQuotaError::TokenNotFound {
                key_id: key_id.to_string(),
            })?;
        if let Some(quota_limit) = quota_limit {
            let used = state.used_tokens.load(Ordering::Relaxed);
            if used >= quota_limit {
                return Err(TokenQuotaError::QuotaExhausted {
                    key_id: key_id.to_string(),
                    used_tokens: used,
                    quota_limit,
                });
            }
        }
        Ok(())
    }
}
