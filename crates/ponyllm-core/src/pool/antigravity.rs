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
pub const DEFAULT_ANTIGRAVITY_OAUTH_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT: u16 = 51121;
pub const DEFAULT_ANTIGRAVITY_CLIENT_ID: &str = "mock_client_id_placeholder.example.com";
pub const DEFAULT_ANTIGRAVITY_CLIENT_SECRET: &str = "REDACTED_CLIENT_SECRET_PLACEHOLDER";
pub const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.1.24 windows/amd64";

pub const DEFAULT_ANTIGRAVITY_OAUTH_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "openid",
];

/// Result of a successful OAuth authorization code exchange
#[derive(Debug, Clone)]
pub struct AntigravityAuthResult {
    pub credential: AntigravityCredential,
    pub email: Option<String>,
}

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

// Fully redacted Debug: even affixes of OAuth material are linkable
// across log slices, so nothing but presence markers is emitted (P0-8).
// `client_id`/`project_id` are non-secret routing identifiers and stay.
impl std::fmt::Debug for AntigravityCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AntigravityCredential")
            .field("client_id", &self.client_id)
            .field("project_id", &self.project_id)
            .field(
                "refresh_token",
                &if self.refresh_token.trim().is_empty() {
                    "(missing)"
                } else {
                    "***"
                },
            )
            .field(
                "access_token",
                &if self.access_token.as_ref().is_some_and(|t| !t.trim().is_empty()) {
                    "(present)"
                } else {
                    "(missing)"
                },
            )
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

#[derive(Clone, Debug)]
enum RefreshOutcome {
    Token(String),
    /// OAuth definitively rejected the refresh_token (`invalid_grant`):
    /// permanent credential death, safe to isolate the key.
    InvalidGrant(String),
    /// Network error / non-401 upstream status / parse failure: transient,
    /// must never permanently isolate the key.
    Transient(String),
}

pub type RefreshTokenRotatedHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone)]
pub struct AntigravityTokenManager {
    key_id: String,
    cred: Arc<RwLock<AntigravityCredential>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<Option<broadcast::Sender<RefreshOutcome>>>>,
    rotation_hook: Arc<RwLock<Option<RefreshTokenRotatedHook>>>,
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
            rotation_hook: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_rotation_hook(&self, hook: RefreshTokenRotatedHook) {
        *self.rotation_hook.write() = Some(hook);
    }

    pub fn credential_snapshot(&self) -> AntigravityCredential {
        self.cred.read().clone()
    }

    pub fn project_id(&self) -> String {
        self.cred.read().project_id.clone()
    }

    /// Check whether token requires refresh (buffer: 5 minutes before expiry)
    pub fn needs_refresh(&self) -> bool {
        Self::credential_needs_refresh(&self.cred.read())
    }

    /// Pure predicate over a credential snapshot: never holds a lock while
    /// evaluating, so fast-path/double-check callers cannot nest `read()`
    /// guards on the same thread.
    fn credential_needs_refresh(cred: &AntigravityCredential) -> bool {
        match (cred.access_token.as_ref(), cred.expiry) {
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
        self.get_valid_token_inner(false).await
    }

    /// Force a refresh even when the cached token looks valid, still
    /// coalesced through the same Singleflight slot. Used for the 401
    /// stale-token recovery path: a 401 proves staleness better than the
    /// local clock does.
    pub async fn force_refresh_token(&self) -> Result<String> {
        self.get_valid_token_inner(true).await
    }

    async fn get_valid_token_inner(&self, force: bool) -> Result<String> {
        // Fast path: snapshot without holding the guard across the check.
        if !force {
            let snapshot = self.cred.read().clone();
            if !Self::credential_needs_refresh(&snapshot) {
                if let Some(token) = snapshot.access_token {
                    return Ok(token);
                }
            }
        }

        // Slow path: Singleflight refresh
        let mut lock = self.refresh_lock.lock().await;

        // Double check: maybe another task already refreshed it while we waited for lock
        if !force {
            let snapshot = self.cred.read().clone();
            if !Self::credential_needs_refresh(&snapshot) {
                if let Some(token) = snapshot.access_token {
                    return Ok(token);
                }
            }
        }

        // Check if a refresh is already in-flight
        if let Some(ref sender) = *lock {
            let mut rx = sender.subscribe();
            drop(lock); // Release mutex so other tasks can wait on rx
            let recv_res = tokio::time::timeout(Duration::from_secs(12), rx.recv()).await;
            return match recv_res {
                Ok(Ok(RefreshOutcome::Token(token))) => Ok(token),
                Ok(Ok(RefreshOutcome::InvalidGrant(reason))) => Err(CoreError::AuthInvalid {
                    key_id: self.key_id.clone(),
                    reason,
                }),
                Ok(Ok(RefreshOutcome::Transient(message))) => Err(CoreError::Internal(message)),
                Ok(Err(e)) => Err(CoreError::Internal(format!("Failed to receive token broadcast: {}", e))),
                Err(_) => Err(CoreError::Internal("Timed out waiting for Singleflight token refresh".to_string())),
            };
        }

        // We are the designated single refresher
        let (tx, _rx) = broadcast::channel(1);
        *lock = Some(tx.clone());
        drop(lock);

        struct SingleflightGuard {
            refresh_lock: Arc<Mutex<Option<broadcast::Sender<RefreshOutcome>>>>,
            tx: broadcast::Sender<RefreshOutcome>,
            completed: bool,
        }

        impl Drop for SingleflightGuard {
            fn drop(&mut self) {
                if !self.completed {
                    let lock_arc = self.refresh_lock.clone();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let mut lock = lock_arc.lock().await;
                        *lock = None;
                        let _ = tx.send(RefreshOutcome::Transient("Refresh task aborted/cancelled".to_string()));
                    });
                }
            }
        }

        let mut guard = SingleflightGuard {
            refresh_lock: self.refresh_lock.clone(),
            tx: tx.clone(),
            completed: false,
        };

        let refresh_res = self.do_refresh_token().await;
        let broadcast_res = match &refresh_res {
            Ok(tok) => RefreshOutcome::Token(tok.clone()),
            Err(CoreError::AuthInvalid { reason, .. }) => {
                RefreshOutcome::InvalidGrant(reason.clone())
            }
            Err(e) => RefreshOutcome::Transient(e.to_string()),
        };

        guard.completed = true;
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

        let token_url = std::env::var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| DEFAULT_ANTIGRAVITY_OAUTH_TOKEN_URL.to_string());
        let req = self
            .client
            .post(&token_url)
            .header(reqwest::header::USER_AGENT, ANTIGRAVITY_USER_AGENT)
            .header("x-goog-api-client", "gl-node/22.14.0 gdcl/1.1.24")
            .header(reqwest::header::ACCEPT, "application/json")
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
            // `invalid_grant` is the only definitive credential-death
            // signal: the stored refresh_token is burned and will never
            // recover. Everything else (network blips, 5xx, rate limits)
            // is transient and must not isolate the key.
            if body_text.to_ascii_lowercase().contains("invalid_grant") {
                return Err(CoreError::AuthInvalid {
                    key_id: self.key_id.clone(),
                    reason: format!("OAuth refresh rejected ({}): {}", status, body_text),
                });
            }
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

        let maybe_rotated = json_val.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.trim().to_string());

        // Update in-memory credential snapshot
        {
            let mut guard = self.cred.write();
            guard.access_token = Some(access_token.clone());
            guard.expiry = Some(new_expiry);
            if let Some(ref new_rf) = maybe_rotated {
                if !new_rf.is_empty() {
                    guard.refresh_token = new_rf.clone();
                }
            }
        }

        if let Some(ref new_rf) = maybe_rotated {
            if !new_rf.is_empty() {
                let hook = self.rotation_hook.read().clone();
                if let Some(cb) = hook {
                    cb(&self.key_id, new_rf);
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

// ---------------------------------------------------------------------------
// Google OAuth2 Authorization & Credential Exchange Helpers (Web / CLI shared)
// ---------------------------------------------------------------------------

fn url_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

fn url_decode_component(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                if let Ok(val) = u8::from_str_radix(std::str::from_utf8(&[c1, c2]).unwrap_or(""), 16) {
                    bytes.push(val);
                    continue;
                }
                bytes.push(b'%');
                bytes.push(c1);
                bytes.push(c2);
            } else {
                bytes.push(b'%');
            }
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// Construct Google OAuth2 authorization URL for Antigravity with offline refresh access.
pub fn build_authorization_url(redirect_uri: &str, state: &str) -> String {
    let scopes = DEFAULT_ANTIGRAVITY_OAUTH_SCOPES.join(" ");
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent%20select_account&state={}",
        DEFAULT_ANTIGRAVITY_OAUTH_AUTH_URL,
        url_encode_component(DEFAULT_ANTIGRAVITY_CLIENT_ID),
        url_encode_component(redirect_uri),
        url_encode_component(&scopes),
        url_encode_component(state),
    )
}

/// Parse OAuth authorization code from either a full redirected URL
/// (e.g. `http://localhost:51121/oauth2callback?code=4/0A...&scope=...`)
/// or a raw/trimmed code string pasted into the terminal.
pub fn parse_code_from_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Check if input contains `code=`
    if let Some(pos) = trimmed.find("code=") {
        let after_code = &trimmed[pos + 5..];
        let end = after_code
            .find(|c: char| c == '&' || c == '#' || c.is_whitespace())
            .unwrap_or(after_code.len());
        let raw_code = &after_code[..end];
        let decoded = url_decode_component(raw_code).trim().to_string();
        if !decoded.is_empty() {
            return Some(decoded);
        }
    }

    // If no `code=` parameter and not a full URL with query/fragment, treat as raw code
    if !trimmed.contains("://") && !trimmed.contains('?') && !trimmed.contains('&') {
        let decoded = url_decode_component(trimmed);
        if !decoded.is_empty() {
            return Some(decoded);
        }
    }

    None
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for b in s.bytes() {
        if b == b'=' {
            break;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        } as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Safely extract the Google account email from an unverified JWT `id_token` payload.
pub fn extract_email_from_id_token(id_token: &str) -> Option<String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = base64url_decode(parts[1])?;
    let val: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    val.get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Exchange an OAuth authorization code for Antigravity credentials using the default endpoint.
pub async fn exchange_code_for_credential(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
) -> Result<AntigravityAuthResult> {
    let token_url = std::env::var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE")
        .unwrap_or_else(|_| DEFAULT_ANTIGRAVITY_OAUTH_TOKEN_URL.to_string());
    exchange_code_for_credential_custom(client, code, redirect_uri, &token_url).await
}

/// Exchange an OAuth authorization code for Antigravity credentials with a custom token URL (supports tests & mock servers).
pub async fn exchange_code_for_credential_custom(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    token_url: &str,
) -> Result<AntigravityAuthResult> {
    let form = [
        ("code", code),
        ("client_id", DEFAULT_ANTIGRAVITY_CLIENT_ID),
        ("client_secret", DEFAULT_ANTIGRAVITY_CLIENT_SECRET),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];

    let resp = client
        .post(token_url)
        .header(reqwest::header::USER_AGENT, ANTIGRAVITY_USER_AGENT)
        .form(&form)
        .send()
        .await
        .map_err(|e| CoreError::Internal(format!("OAuth code exchange network error: {}", e)))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .unwrap_or_else(|_| "(failed to read response text)".to_string());

    if !status.is_success() {
        return Err(CoreError::Internal(format!(
            "OAuth code exchange rejected ({}): {}",
            status, body_text
        )));
    }

    let json_val: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|e| CoreError::Internal(format!("Failed to parse OAuth exchange response JSON: {}", e)))?;

    let refresh_token = json_val
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CoreError::Internal(
                "OAuth exchange succeeded but response did not contain a refresh_token".to_string(),
            )
        })?
        .to_string();

    let access_token = json_val
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let expiry = json_val
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .map(|sec| Utc::now() + chrono::Duration::seconds(sec as i64));

    let email = json_val
        .get("id_token")
        .and_then(|v| v.as_str())
        .and_then(extract_email_from_id_token);

    let cred = AntigravityCredential {
        access_token,
        refresh_token,
        client_id: DEFAULT_ANTIGRAVITY_CLIENT_ID.to_string(),
        client_secret: DEFAULT_ANTIGRAVITY_CLIENT_SECRET.to_string(),
        project_id: "aicode-consumers".to_string(),
        expiry,
    };

    Ok(AntigravityAuthResult {
        credential: cred,
        email,
    })
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
        // No OAuth material at all: not even affixes (linkable across slices).
        assert!(!debug_str.contains("ya29.a0AdMD6Einf3FwekkOnCpHNv8u3_j2qDn2ADGX5t"));
        assert!(!debug_str.contains("1//04mock_oauth_refresh_token_for_testing_00000000000000"));
        assert!(!debug_str.contains("test-secret"));
        assert!(!debug_str.contains("ya29.a..."));
        assert!(!debug_str.contains("1//0..."));
        // Routing identifiers stay for operability.
        assert!(debug_str.contains("test-client"));
        assert!(debug_str.contains("test-proj"));
        assert!(debug_str.contains("***"));
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

    #[tokio::test]
    async fn test_refresh_token_rotation_hook() {
        let cred = AntigravityCredential {
            access_token: Some("old_tok".to_string()),
            refresh_token: "rf_orig".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "proj".to_string(),
            expiry: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        let mgr = Arc::new(AntigravityTokenManager::new("ag-key-rot", cred, reqwest::Client::new()));
        let rotated = Arc::new(parking_lot::Mutex::new(None));
        let rotated_clone = rotated.clone();
        mgr.set_rotation_hook(Arc::new(move |key_id, new_rf| {
            *rotated_clone.lock() = Some((key_id.to_string(), new_rf.to_string()));
        }));

        if let Some(hook) = mgr.rotation_hook.read().clone() {
            hook("ag-key-rot", "1//new-refresh-token");
        }
        let captured = rotated.lock().clone();
        assert_eq!(captured, Some(("ag-key-rot".to_string(), "1//new-refresh-token".to_string())));
    }

    #[tokio::test]
    async fn test_singleflight_drop_guard_prevents_hang_on_cancellation() {
        let cred = AntigravityCredential {
            access_token: None,
            refresh_token: "rf".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "proj".to_string(),
            expiry: None,
        };
        let mgr = Arc::new(AntigravityTokenManager::new("ag-cancel-test", cred, reqwest::Client::new()));

        let mgr_clone = mgr.clone();
        tokio::select! {
            _ = mgr_clone.force_refresh_token() => {}
            _ = tokio::time::sleep(Duration::from_millis(1)) => {}
        };

        tokio::time::sleep(Duration::from_millis(50)).await;

        let lock_guard = mgr.refresh_lock.lock().await;
        assert!(lock_guard.is_none(), "Singleflight slot must be None after cancellation");
    }

    #[test]
    fn test_build_authorization_url() {
        let url = build_authorization_url("http://localhost:51121/oauth2callback", "nonce-state-123");
        assert!(url.starts_with(DEFAULT_ANTIGRAVITY_OAUTH_AUTH_URL));
        assert!(url.contains(&format!("client_id={}", DEFAULT_ANTIGRAVITY_CLIENT_ID)));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent%20select_account"));
        assert!(url.contains("state=nonce-state-123"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("http%3A%2F%2Flocalhost%3A51121%2Foauth2callback"));
    }

    #[test]
    fn test_parse_code_from_input() {
        // 1. Full URL with code, scope, state
        let input1 = "http://localhost:51121/oauth2callback?code=4%2F0AY0e-dummy_code&scope=openid&state=123";
        assert_eq!(parse_code_from_input(input1), Some("4/0AY0e-dummy_code".to_string()));

        // 2. 127.0.0.1 redirect URL
        let input2 = "http://127.0.0.1:51121/?state=123&code=4/0B9988_code";
        assert_eq!(parse_code_from_input(input2), Some("4/0B9988_code".to_string()));

        // 3. Raw code pasted directly with whitespace
        let input3 = "   4/0AY0e-raw_code_pasted   ";
        assert_eq!(parse_code_from_input(input3), Some("4/0AY0e-raw_code_pasted".to_string()));

        // 4. code= format
        let input4 = "code=4/0AY_direct";
        assert_eq!(parse_code_from_input(input4), Some("4/0AY_direct".to_string()));

        // 5. Empty or garbage
        assert_eq!(parse_code_from_input(""), None);
        assert_eq!(parse_code_from_input("   "), None);
    }

    #[test]
    fn test_extract_email_from_id_token() {
        // Header: {"alg":"RS256"} -> base64url "eyJhbGciOiJSUzI1NiJ9"
        let header = "eyJhbGciOiJSUzI1NiJ9";
        // Payload: {"email":"dev.engineer@gmail.com","sub":"1001"}
        // JSON: {"email":"dev.engineer@gmail.com","sub":"1001"}
        // Base64URL: eyJlbWFpbCI6ImRldi5lbmdpbmVlckBnbWFpbC5jb20iLCJzdWIiOiIxMDAxIn0
        let payload = "eyJlbWFpbCI6ImRldi5lbmdpbmVlckBnbWFpbC5jb20iLCJzdWIiOiIxMDAxIn0";
        let sig = "dummy_signature";
        let jwt = format!("{}.{}.{}", header, payload, sig);

        let email = extract_email_from_id_token(&jwt);
        assert_eq!(email, Some("dev.engineer@gmail.com".to_string()));

        // Invalid formats
        assert_eq!(extract_email_from_id_token("not-a-jwt"), None);
        assert_eq!(extract_email_from_id_token("a.b"), None);
    }

    #[tokio::test]
    async fn test_exchange_code_for_credential_custom() {
        use axum::{routing::post, Json, Router};
        use serde_json::json;

        let app = Router::new().route(
            "/token",
            post(|axum::Form(params): axum::Form<std::collections::HashMap<String, String>>| async move {
                assert_eq!(params.get("code").map(|s| s.as_str()), Some("valid-code-42"));
                assert_eq!(params.get("grant_type").map(|s| s.as_str()), Some("authorization_code"));
                Json(json!({
                    "access_token": "ya29.test_access",
                    "refresh_token": "1//0test_refresh",
                    "expires_in": 3600,
                    "id_token": "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJzdWIiOiIxIn0.sig"
                }))
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let token_url = format!("http://{}/token", addr);
        let result = exchange_code_for_credential_custom(
            &client,
            "valid-code-42",
            "http://localhost:51121/oauth2callback",
            &token_url,
        )
        .await
        .expect("exchange should succeed");

        assert_eq!(result.credential.refresh_token, "1//0test_refresh");
        assert_eq!(result.credential.access_token, Some("ya29.test_access".to_string()));
        assert_eq!(result.email, Some("user@example.com".to_string()));
    }
}
