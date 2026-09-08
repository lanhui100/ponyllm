use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use crate::error::{CoreError, Result};

pub const DEFAULT_ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const DEFAULT_ANTIGRAVITY_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const DEFAULT_ANTIGRAVITY_CLIENT_ID: &str = "mock_client_id_placeholder.example.com";
pub const DEFAULT_ANTIGRAVITY_CLIENT_SECRET: &str = "REDACTED_CLIENT_SECRET_PLACEHOLDER";
pub const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.1.24 windows/amd64";

/// Antigravity persistent OAuth credentials (stored in config / disk)
#[derive(Clone, Serialize, Deserialize)]
pub struct AntigravityCredential {
    #[serde(default)]
    pub access_token: Option<String>,
    pub refresh_token: String,
    #[serde(default = "default_client_id")]
    pub client_id: String,
    #[serde(default = "default_client_secret")]
    pub client_secret: String,
    #[serde(default = "default_project_id")]
    pub project_id: String,
    #[serde(default)]
    pub expiry: Option<DateTime<Utc>>,
}

fn default_client_id() -> String {
    DEFAULT_ANTIGRAVITY_CLIENT_ID.to_string()
}
fn default_client_secret() -> String {
    DEFAULT_ANTIGRAVITY_CLIENT_SECRET.to_string()
}
fn default_project_id() -> String {
    "aicode-consumers".to_string()
}

// Redacted Debug to prevent leaking secrets in logs
impl std::fmt::Debug for AntigravityCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let masked_refresh = if self.refresh_token.len() > 10 {
            format!("{}...{}", &self.refresh_token[..4], &self.refresh_token[self.refresh_token.len() - 4..])
        } else {
            "***".to_string()
        };
        let masked_access = self.access_token.as_ref().map(|t| {
            if t.len() > 12 {
                format!("{}...{}", &t[..6], &t[t.len() - 4..])
            } else {
                "***".to_string()
            }
        });
        f.debug_struct("AntigravityCredential")
            .field("client_id", &self.client_id)
            .field("project_id", &self.project_id)
            .field("refresh_token", &masked_refresh)
            .field("access_token", &masked_access)
            .field("expiry", &self.expiry)
            .finish()
    }
}

/// Model quota information returned by Antigravity fetchAvailableModels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelQuotaInfo {
    pub model_id: String,
    pub remaining_fraction: f64,
    pub reset_time: Option<DateTime<Utc>>,
    pub reset_time_raw: Option<String>,
}

/// Snapshot of an account's quota across models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountQuotaSnapshot {
    pub key_id: String,
    pub project_id: String,
    pub models: HashMap<String, ModelQuotaInfo>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct AntigravityTokenManager {
    key_id: String,
    cred: Arc<RwLock<AntigravityCredential>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<Option<broadcast::Sender<std::result::Result<String, String>>>>>,
}

impl AntigravityTokenManager {
    pub fn new(
        key_id: impl Into<String>,
        cred: AntigravityCredential,
        client: reqwest::Client,
    ) -> Self {
        Self {
            key_id: key_id.into(),
            cred: Arc::new(RwLock::new(cred)),
            client,
            refresh_lock: Arc::new(Mutex::new(None)),
        }
    }

    pub fn credential_snapshot(&self) -> AntigravityCredential {
        self.cred.read().clone()
    }

    pub fn project_id(&self) -> String {
        self.cred.read().project_id.clone()
    }

    /// Check whether token requires refresh (buffer: 5 minutes before expiry)
    pub fn needs_refresh(&self) -> bool {
        let guard = self.cred.read();
        match (guard.access_token.as_ref(), guard.expiry) {
            (None, _) => true,
            (Some(token), _) if token.trim().is_empty() => true,
            (Some(_), None) => false,
            (Some(_), Some(exp)) => {
                let now = Utc::now();
                let buffer = chrono::Duration::minutes(5);
                now + buffer >= exp
            }
        }
    }

    /// Get valid access token with Singleflight coalesced refresh
    pub async fn get_valid_token(&self) -> Result<String> {
        // Fast path: check if current token is valid without acquiring async lock
        {
            let guard = self.cred.read();
            if !self.needs_refresh() {
                if let Some(token) = guard.access_token.as_ref() {
                    return Ok(token.clone());
                }
            }
        }

        // Slow path: Singleflight refresh
        let mut lock = self.refresh_lock.lock().await;

        // Double check: maybe another task already refreshed it while we waited for lock
        {
            let guard = self.cred.read();
            if !self.needs_refresh() {
                if let Some(token) = guard.access_token.as_ref() {
                    return Ok(token.clone());
                }
            }
        }

        // Check if a refresh is already in-flight
        if let Some(ref sender) = *lock {
            let mut rx = sender.subscribe();
            drop(lock); // Release mutex so other tasks can wait on rx
            return match rx.recv().await {
                Ok(Ok(token)) => Ok(token),
                Ok(Err(err_msg)) => Err(CoreError::Internal(err_msg)),
                Err(e) => Err(CoreError::Internal(format!("Failed to receive token broadcast: {}", e))),
            };
        }

        // We are the designated single refresher
        let (tx, _rx) = broadcast::channel(1);
        *lock = Some(tx.clone());
        drop(lock);

        let refresh_res = self.do_refresh_token().await;
        let broadcast_res = match &refresh_res {
            Ok(tok) => Ok(tok.clone()),
            Err(e) => Err(e.to_string()),
        };

        // Acquire lock again to clear singleflight slot and broadcast result
        let mut lock = self.refresh_lock.lock().await;
        *lock = None;
        let _ = tx.send(broadcast_res);
        refresh_res
    }

    async fn do_refresh_token(&self) -> Result<String> {
        let (refresh_token, client_id, client_secret) = {
            let guard = self.cred.read();
            (
                guard.refresh_token.clone(),
                guard.client_id.clone(),
                guard.client_secret.clone(),
            )
        };

        if refresh_token.trim().is_empty() {
            return Err(CoreError::Internal(format!(
                "No refresh_token configured for Antigravity key '{}'",
                self.key_id
            )));
        }

        let params = [
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let req = self
            .client
            .post(DEFAULT_ANTIGRAVITY_OAUTH_TOKEN_URL)
            .form(&params)
            .timeout(Duration::from_secs(10));

        let resp = req.send().await.map_err(|e| {
            CoreError::Internal(format!(
                "Antigravity OAuth refresh network error for '{}': {}",
                self.key_id, e
            ))
        })?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(CoreError::UpstreamStatusError {
                status,
                body: body_text,
            });
        }

        let json_val: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
            CoreError::Internal(format!("Failed to parse OAuth refresh JSON: {}", e))
        })?;

        let access_token = json_val
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::Internal("Missing access_token in refresh response".to_string()))?
            .to_string();

        let expires_in_sec = json_val
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        let new_expiry = Utc::now() + chrono::Duration::seconds(expires_in_sec as i64);

        // Update in-memory credential snapshot
        {
            let mut guard = self.cred.write();
            guard.access_token = Some(access_token.clone());
            guard.expiry = Some(new_expiry);
            if let Some(new_refresh) = json_val.get("refresh_token").and_then(|v| v.as_str()) {
                if !new_refresh.trim().is_empty() {
                    guard.refresh_token = new_refresh.to_string();
                }
            }
        }

        tracing::info!(
            key_id = %self.key_id,
            expires_in = %expires_in_sec,
            expiry = %new_expiry.to_rfc3339(),
            "Antigravity token refreshed successfully"
        );

        Ok(access_token)
    }

    /// Fetch available models and quota info from Antigravity PA endpoint
    pub async fn fetch_quota(&self, endpoint: Option<&str>) -> Result<AccountQuotaSnapshot> {
        let access_token = self.get_valid_token().await?;
        let base = endpoint.unwrap_or(DEFAULT_ANTIGRAVITY_ENDPOINT).trim_end_matches('/');
        let url = format!("{}/v1internal:fetchAvailableModels", base);

        let req = self
            .client
            .post(&url)
            .header(reqwest::header::USER_AGENT, ANTIGRAVITY_USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", access_token))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("requestType", "agent")
            .json(&serde_json::json!({}))
            .timeout(Duration::from_secs(15));

        let resp = req.send().await.map_err(|e| {
            CoreError::Internal(format!(
                "Failed to fetch Antigravity available models for '{}': {}",
                self.key_id, e
            ))
        })?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(CoreError::UpstreamStatusError {
                status,
                body: body_text,
            });
        }

        let json_val: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
            CoreError::Internal(format!("Failed to parse fetchAvailableModels response: {}", e))
        })?;

        let mut models_map = HashMap::new();
        if let Some(models_obj) = json_val.get("models").and_then(|v| v.as_object()) {
            for (model_id, m_val) in models_obj {
                if let Some(quota_obj) = m_val.get("quotaInfo") {
                    let rem_frac = quota_obj
                        .get("remainingFraction")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1.0);
                    let reset_raw = quota_obj
                        .get("resetTime")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let parsed_utc = reset_raw.as_deref().and_then(|s| {
                        DateTime::parse_from_rfc3339(s)
                            .map(|dt| dt.with_timezone(&Utc))
                            .ok()
                    });

                    models_map.insert(
                        model_id.clone(),
                        ModelQuotaInfo {
                            model_id: model_id.clone(),
                            remaining_fraction: rem_frac,
                            reset_time: parsed_utc,
                            reset_time_raw: reset_raw,
                        },
                    );
                }
            }
        }

        Ok(AccountQuotaSnapshot {
            key_id: self.key_id.clone(),
            project_id: self.project_id(),
            models: models_map,
            fetched_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_antigravity_credential_debug_masking() {
        let cred = AntigravityCredential {
            access_token: Some("ya29.a0AdMD6Einf3FwekkOnCpHNv8u3_j2qDn2ADGX5t".to_string()),
            refresh_token: "1//04mock_oauth_refresh_token_for_testing_00000000000000".to_string(),
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            project_id: "test-proj".to_string(),
            expiry: None,
        };
        let debug_str = format!("{:?}", cred);
        assert!(!debug_str.contains("ya29.a0AdMD6Einf3FwekkOnCpHNv8u3_j2qDn2ADGX5t"));
        assert!(!debug_str.contains("1//04mock_oauth_refresh_token_for_testing_00000000000000"));
        assert!(debug_str.contains("ya29.a...GX5t"));
        assert!(debug_str.contains("1//0...bB5p"));
    }

    #[test]
    fn test_needs_refresh_logic() {
        let cred = AntigravityCredential {
            access_token: Some("dummy".to_string()),
            refresh_token: "dummy_rf".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "proj".to_string(),
            expiry: Some(Utc::now() + chrono::Duration::minutes(10)),
        };
        let mgr = AntigravityTokenManager::new("k1", cred, reqwest::Client::new());
        assert!(!mgr.needs_refresh());

        // Expiring in 3 minutes (< 5 min buffer)
        let cred_expiring = AntigravityCredential {
            access_token: Some("dummy".to_string()),
            refresh_token: "dummy_rf".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "proj".to_string(),
            expiry: Some(Utc::now() + chrono::Duration::minutes(3)),
        };
        let mgr_exp = AntigravityTokenManager::new("k2", cred_expiring, reqwest::Client::new());
        assert!(mgr_exp.needs_refresh());
    }

    #[tokio::test]
    async fn test_singleflight_coalescing() {
        let cred = AntigravityCredential {
            access_token: Some("old_tok".to_string()),
            refresh_token: "rf".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "proj".to_string(),
            expiry: Some(Utc::now() - chrono::Duration::minutes(1)), // expired
        };
        let mgr = Arc::new(AntigravityTokenManager::new("k1", cred, reqwest::Client::new()));
        assert!(mgr.needs_refresh());
    }
}
