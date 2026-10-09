use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Managed user entry in PonyLLM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserEntry {
    /// Unique user identifier (e.g. `user_alice` or `usr-1234`).
    pub id: String,
    /// Optional human-readable name or email.
    #[serde(default)]
    pub name: String,
    /// Whether this user is active and allowed to run inference. Default `true`.
    #[serde(default = "default_user_enabled")]
    pub enabled: bool,
    /// Optional list of model names or patterns permitted for this user.
    /// `None` = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_models: Option<Vec<String>>,
    /// Optional max token budget across this user's lifetime (prompt + completion).
    /// `None` = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Creation timestamp (UNIX seconds).
    #[serde(default = "default_user_created_at")]
    pub created_at: i64,
    /// Web login identity (B001 contract, zero migration): unique username when
    /// set; `None` = legacy pure-quota entity, cannot log in via username/password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// PBKDF2 password hash in PHC format `$pbkdf2-sha256$i=<iters>$<salt>$<hash>`;
    /// `None` = no login credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// Access role: `admin` | `user` (serde lowercase). Default `user`.
    #[serde(default, skip_serializing_if = "is_default_user_role")]
    pub role: UserRole,
    /// Token version: bumping invalidates all previously issued JWTs
    /// (password change / revocation). Default `0`.
    #[serde(default, skip_serializing_if = "is_zero_token_version")]
    pub token_version: u64,
}

/// User access role for the Web management plane (B001 contract).
///
/// Serialized lowercase on the wire (`admin` | `user`); deserialization of an
/// unknown value fails fast. Default is `User` (zero migration for legacy
/// entries that predate the field).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    /// Regular user: self-service over own data and tokens only.
    #[default]
    User,
    /// Administrator: global user/token governance.
    Admin,
}

fn is_default_user_role(role: &UserRole) -> bool {
    *role == UserRole::User
}

fn is_zero_token_version(v: &u64) -> bool {
    *v == 0
}

fn default_user_enabled() -> bool {
    true
}

fn default_user_created_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UserCheckError {
    #[error("user '{user_id}' does not exist")]
    UserNotFound { user_id: String },
    #[error("user '{user_id}' is disabled")]
    UserDisabled { user_id: String },
    #[error("model '{model}' is not allowed for user '{user_id}'")]
    ModelNotAllowed { user_id: String, model: String },
    #[error("user '{user_id}' quota exhausted: used {used_tokens} >= max {max_tokens}")]
    QuotaExhausted {
        user_id: String,
        used_tokens: u64,
        max_tokens: u64,
    },
}

#[derive(Debug)]
struct UserRuntimeState {
    entry: UserEntry,
    used_tokens: AtomicU64,
}

/// Runtime quota and access tracker for users.
#[derive(Debug, Clone, Default)]
pub struct UserQuotaTracker {
    users: Arc<DashMap<String, Arc<UserRuntimeState>>>,
}

impl UserQuotaTracker {
    pub fn new() -> Self {
        Self {
            users: Arc::new(DashMap::new()),
        }
    }

    /// Insert or update a user entry. Preserves existing used_tokens counter.
    pub fn upsert_user(&self, entry: UserEntry) {
        let user_id = entry.id.clone();
        let current_used = if let Some(existing) = self.users.get(&user_id) {
            existing.used_tokens.load(Ordering::Relaxed)
        } else {
            0
        };
        let state = Arc::new(UserRuntimeState {
            entry,
            used_tokens: AtomicU64::new(current_used),
        });
        self.users.insert(user_id, state);
    }

    /// Remove a user.
    pub fn remove_user(&self, user_id: &str) -> Option<UserEntry> {
        self.users
            .remove(user_id)
            .map(|(_, state)| state.entry.clone())
    }

    /// Get user entry if exists.
    pub fn get_user(&self, user_id: &str) -> Option<UserEntry> {
        self.users.get(user_id).map(|u| u.entry.clone())
    }

    /// List all configured users with their current used tokens.
    pub fn list_users(&self) -> Vec<(UserEntry, u64)> {
        self.users
            .iter()
            .map(|item| {
                let entry = item.value().entry.clone();
                let used = item.value().used_tokens.load(Ordering::Relaxed);
                (entry, used)
            })
            .collect()
    }

    /// Reset token usage for a user.
    pub fn reset_usage(&self, user_id: &str) -> bool {
        if let Some(user) = self.users.get(user_id) {
            user.used_tokens.store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Check if a user is permitted to use the specified model and has quota remaining.
    pub fn check_access(&self, user_id: &str, model: &str) -> Result<(), UserCheckError> {
        let user = self
            .users
            .get(user_id)
            .ok_or_else(|| UserCheckError::UserNotFound {
                user_id: user_id.to_string(),
            })?;

        if !user.entry.enabled {
            return Err(UserCheckError::UserDisabled {
                user_id: user_id.to_string(),
            });
        }

        // Check model permissions
        if let Some(allowed_models) = &user.entry.allowed_models {
            let allowed = allowed_models.iter().any(|pattern| {
                if pattern == "*" || pattern == model {
                    return true;
                }
                if let Some(prefix) = pattern.strip_suffix('*') {
                    if model.starts_with(prefix) {
                        return true;
                    }
                }
                false
            });

            if !allowed {
                return Err(UserCheckError::ModelNotAllowed {
                    user_id: user_id.to_string(),
                    model: model.to_string(),
                });
            }
        }

        // Check token quota
        if let Some(max_tokens) = user.entry.max_tokens {
            let used = user.used_tokens.load(Ordering::Relaxed);
            if used >= max_tokens {
                return Err(UserCheckError::QuotaExhausted {
                    user_id: user_id.to_string(),
                    used_tokens: used,
                    max_tokens,
                });
            }
        }

        Ok(())
    }

    /// Record token usage for a user.
    pub fn record_tokens(&self, user_id: &str, tokens: u64) {
        if let Some(user) = self.users.get(user_id) {
            user.used_tokens.fetch_add(tokens, Ordering::Relaxed);
        }
    }

    /// Get current token usage for a user.
    pub fn get_used_tokens(&self, user_id: &str) -> u64 {
        self.users
            .get(user_id)
            .map(|u| u.used_tokens.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}
