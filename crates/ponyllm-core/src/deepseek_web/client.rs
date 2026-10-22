//! chat.deepseek.com Web Provider HTTP client (wave-9 contract
//! DW-S-*/DW-Q-*/DW-E-*/DW-G-*, H-04..H-08). Evidence: REA §2/§3/§4/§6.
//!
//! Compliance notes implemented here (contract §7):
//! * no guest mode, no captcha/slider/fingerprint bypass (DW-G-01, DW-S-05);
//! * WAF markers `x-amzn-waf-action: captcha` / `cf-mitigated: challenge`
//!   fail explicitly as `ClientError::CaptchaWall` — never retried, never
//!   bypassed (DW-G-02);
//! * HTTP 429 backs off at most ONCE for the server-declared Retry-After,
//!   then fails explicitly (DW-G-03);
//! * PoW is solved exactly once per request by the caller (DW-P-07); this
//!   client never stores, replays or pre-batches answers (no cache field).

use crate::deepseek_web::pow::{encode_pow_response_header, PowChallenge, PowResponse};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default base URL of the chat.deepseek.com web provider (H-04).
pub const DEFAULT_BASE_URL: &str = "https://chat.deepseek.com";

/// `x-client-bundle-id` (REA §2).
pub const CLIENT_BUNDLE_ID: &str = "com.deepseek.chat";
/// `x-client-platform` (REA §2).
pub const CLIENT_PLATFORM: &str = "web";
/// `x-client-version` (REA evidence line 5: version 2.5.0, commit 6c40ca01).
pub const CLIENT_VERSION: &str = "2.5.0";
/// `x-client-locale` (REA §2 default).
pub const CLIENT_LOCALE: &str = "en_US";
/// `x-client-timezone-offset` (REA §2).
pub const CLIENT_TIMEZONE_OFFSET: &str = "0";

/// Non-streaming call total-timeout budget (NFR baseline
/// `external_call_timeout_ms: 3000`). Applied per short request only;
/// streaming TTFB/total stay with the existing executor guards
/// (`DEFAULT_UPSTREAM_TTFB_TIMEOUT`/`TOTAL_TIMEOUT`, T4) — NON-GOAL to tune.
const SHORT_CALL_TIMEOUT: Duration = Duration::from_millis(3000);

/// Session created via `POST /api/v0/chat_session/create` (REA §4):
/// `chat_session_id = biz_data.chat_session.id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `POST /api/v0/chat/completion` request body (contract DW-Q-01, REA §3).
/// Wire keys/types are frozen; serde defaults per DW-Q-02.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub chat_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
    pub model_type: String,
    pub prompt: String,
    #[serde(default)]
    pub ref_file_ids: Vec<String>,
    #[serde(default)]
    pub thinking_enabled: bool,
    #[serde(default)]
    pub search_enabled: bool,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default)]
    pub preempt: bool,
}

/// Error envelope of chat.deepseek.com APIs (REA §6):
/// `{biz_code, biz_msg, biz_data}`; `biz_code != 0` is failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorEnvelope {
    pub biz_code: i64,
    pub biz_msg: String,
    #[serde(default)]
    pub biz_data: Option<serde_json::Value>,
}

/// DeepSeek Web client (H-04..H-08, contract DW-S-*/DW-E-*/DW-G-*).
///
/// `token` is the user-held web session token (`Authorization: Bearer`,
/// REA §2); `device_id` is a locally generated UUID for the `x-device-id`
/// header — never a fabricated fingerprint (DW-G-01). No shared mutable
/// state: the `reqwest::Client` connection pool is owned by the instance
/// (NFR `concurrency_lock`: zero new locks).
///
/// Manual `Debug` (L3B finding): the struct holds the raw web token; a derived
/// `Debug` would print it verbatim into logs/flight records. Only non-sensitive
/// fields are emitted (token redacted, mirroring `ApiKeyEntry`'s Debug guard).
#[derive(Clone)]
pub struct DeepSeekWebClient {
    pub http: reqwest::Client,
    pub base_url: String,
    pub token: Option<String>,
    pub device_id: String,
}

impl std::fmt::Debug for DeepSeekWebClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekWebClient")
            .field("base_url", &self.base_url)
            .field("device_id", &self.device_id)
            .field(
                "token",
                &self.token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl DeepSeekWebClient {
    /// Build a client for a user-held web session token (H-04, DW-S-03).
    pub fn new(token: Option<String>, device_id: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            token,
            device_id,
        }
    }

    /// Request builder for `{base}{path}` with the client header group
    /// (REA §2 / DW-S-03). Authorization is added only when a token exists —
    /// no forged credentials (DW-G-05).
    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("x-client-bundle-id", CLIENT_BUNDLE_ID)
            .header("x-client-platform", CLIENT_PLATFORM)
            .header("x-client-version", CLIENT_VERSION)
            .header("x-client-locale", CLIENT_LOCALE)
            .header("x-client-timezone-offset", CLIENT_TIMEZONE_OFFSET)
            .header("x-device-id", &self.device_id)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = &self.token {
            builder = builder.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder
    }

    /// Send a non-streaming request under the 3000 ms budget with the
    /// bounded 429 Retry-After backoff (DW-G-03). A 429 without Retry-After
    /// or a second 429 after the wait fails immediately — no infinite
    /// retries, no exponential spin. WAF markers fail explicitly.
    async fn send_short(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ClientError> {
        let mut attempts = 0;
        loop {
            let builder = builder.try_clone().ok_or_else(|| {
                ClientError::Other("short-call request builder not cloneable".to_string())
            })?;
            let resp = builder
                .timeout(SHORT_CALL_TIMEOUT)
                .send()
                .await
                .map_err(|e| ClientError::Network(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = retry_after_duration(&resp);
                // Bounded: exactly one wait for the server-declared value,
                // then any subsequent 429 is a hard error.
                if attempts == 0 {
                    if let Some(wait) = retry_after {
                        tokio::time::sleep(wait).await;
                        attempts += 1;
                        continue;
                    }
                }
                return Err(ClientError::RateLimited(retry_after));
            }
            check_waf_headers(&resp)?;
            if status.is_success() {
                return Ok(resp);
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_api_error(status.as_u16(), &body));
        }
    }

    /// `POST /api/v0/chat/create_pow_challenge` → `biz_data.challenge`
    /// (H-05, DW-S-01 / DW-P-01). Body is exactly `{"target_path": "<path>"}`.
    pub async fn create_pow_challenge(
        &self,
        target_path: &str,
    ) -> Result<PowChallenge, ClientError> {
        let builder = self
            .request("/api/v0/chat/create_pow_challenge")
            .json(&serde_json::json!({ "target_path": target_path }));
        let resp = self.send_short(builder).await?;
        let body = resp.text().await.map_err(|e| ClientError::Network(e.to_string()))?;
        parse_challenge_response(&body)
    }

    /// `POST /api/v0/chat_session/create` with empty body → `biz_data.chat_session`
    /// (H-06, DW-S-02).
    pub async fn create_chat_session(&self) -> Result<ChatSession, ClientError> {
        let builder = self
            .request("/api/v0/chat_session/create")
            .json(&serde_json::json!({}));
        let resp = self.send_short(builder).await?;
        let body = resp.text().await.map_err(|e| ClientError::Network(e.to_string()))?;
        parse_session_response(&body)
    }

    /// `POST /api/v0/chat/completion` with client headers and the
    /// `X-DS-PoW-Response` header (H-07, DW-S-03/04). Returns the raw SSE
    /// response body for `deepseek_web::sse` parsing; WAF headers per DW-G-02,
    /// HTTP 429 per DW-E-01/DW-G-03 (bounded single backoff).
    ///
    /// No streaming timeout is applied here on purpose: first-byte and total
    /// duration are governed by the existing executor guards
    /// (`DEFAULT_UPSTREAM_TTFB_TIMEOUT`/`TOTAL_TIMEOUT`, contract §8, NON-GOAL).
    pub async fn stream_completion(
        &self,
        req: &CompletionRequest,
        pow: &PowResponse,
    ) -> Result<reqwest::Response, ClientError> {
        let mut attempts = 0;
        loop {
            let builder = self
                .request("/api/v0/chat/completion")
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header("X-DS-PoW-Response", encode_pow_response_header(pow))
                .json(req);
            let resp = builder.send().await.map_err(|e| ClientError::Network(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = retry_after_duration(&resp);
                if attempts == 0 {
                    if let Some(wait) = retry_after {
                        tokio::time::sleep(wait).await;
                        attempts += 1;
                        continue;
                    }
                }
                return Err(ClientError::RateLimited(retry_after));
            }
            check_waf_headers(&resp)?;
            if status.is_success() {
                return Ok(resp);
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_api_error(status.as_u16(), &body));
        }
    }
}

/// Challenge parse: envelope may be flat `{"biz_code",…}` (exactly what a
/// mock upstream and the bundle's `.json.data` shape imply) or wrapped in a
/// `data` key; challenge keys may be snake_case (server) or camelCase (the
/// frozen `PowChallenge` serde names / contract assertions).
fn parse_challenge_response(body: &str) -> Result<PowChallenge, ClientError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ClientError::Other(format!("challenge response is not JSON: {e}")))?;
    let data = value.get("data").unwrap_or(&value);
    let biz_code = data.get("biz_code").and_then(|v| v.as_i64()).unwrap_or(0);
    let biz_msg = data
        .get("biz_msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if biz_code != 0 {
        return Err(biz_code_error(biz_code, biz_msg));
    }
    let challenge = data
        .get("biz_data")
        .and_then(|b| b.get("challenge"))
        .ok_or_else(|| ClientError::Other("challenge response missing biz_data.challenge".to_string()))?;
    Ok(PowChallenge {
        algorithm: challenge
            .get("algorithm")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClientError::Other("challenge missing algorithm".to_string()))?
            .to_string(),
        challenge: challenge
            .get("challenge")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClientError::Other("challenge missing challenge".to_string()))?
            .to_string(),
        salt: challenge
            .get("salt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClientError::Other("challenge missing salt".to_string()))?
            .to_string(),
        difficulty: challenge
            .get("difficulty")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ClientError::Other("challenge missing difficulty".to_string()))?,
        signature: challenge
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClientError::Other("challenge missing signature".to_string()))?
            .to_string(),
        expire_at: challenge
            .get("expireAt")
            .or_else(|| challenge.get("expire_at"))
            .and_then(|v| v.as_i64()),
        expire_after: challenge
            .get("expireAfter")
            .or_else(|| challenge.get("expire_after"))
            .and_then(|v| v.as_u64()),
    })
}

/// Session parse: `chat_session_id = biz_data.chat_session.id` (DW-S-02).
fn parse_session_response(body: &str) -> Result<ChatSession, ClientError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ClientError::Other(format!("session response is not JSON: {e}")))?;
    let data = value.get("data").unwrap_or(&value);
    if let Some(code) = data.get("biz_code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = data
                .get("biz_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(biz_code_error(code, msg));
        }
    }
    let session = data
        .get("biz_data")
        .and_then(|b| b.get("chat_session"))
        .ok_or_else(|| ClientError::Other("session response missing biz_data.chat_session".to_string()))?;
    Ok(ChatSession {
        id: session
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClientError::Other("chat_session missing id".to_string()))?
            .to_string(),
        title: session.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()),
        updated_at: session
            .get("updated_at")
            .or_else(|| session.get("updatedAt"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// Map a non-zero `biz_code` to the typed error (contract §6 E-*).
fn biz_code_error(biz_code: i64, biz_msg: String) -> ClientError {
    match biz_code {
        40012 => ClientError::Banned,
        40300 | 40301 => ClientError::InvalidPow,
        40002 | 40003 => ClientError::Token,
        // 40029 / 50006 and any other code: surfaced `Other` with the raw
        // business message (DW-E-06/07).
        _ => {
            if biz_msg.trim().is_empty() {
                ClientError::Other(format!("biz_code {biz_code}"))
            } else {
                ClientError::Other(biz_msg)
            }
        }
    }
}

/// Classify an upstream failure into a typed error (H-08, contract §6 DW-E-*).
/// Pure function: parses the `{biz_code, biz_msg, biz_data}` envelope (REA §6).
/// Reads no clock, no Retry-After header (the caller attaches that to
/// `RateLimited` via `retry_after_duration`).
pub fn classify_api_error(http_status: u16, body: &str) -> ClientError {
    if http_status == 429 {
        return ClientError::RateLimited(None);
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let data = value.get("data").unwrap_or(&value);
        if let Some(code) = data.get("biz_code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = data
                    .get("biz_msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return biz_code_error(code, msg);
            }
        }
    }
    ClientError::HttpStatus(http_status)
}

/// Parse the `Retry-After` response header as a duration (integer seconds,
/// the form the chat.deepseek.com API uses); `None` when absent/unparseable.
fn retry_after_duration(resp: &reqwest::Response) -> Option<Duration> {
    let raw = resp.headers().get("retry-after")?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// DW-G-02 hard boundary: upstream WAF/captcha markers fail explicitly with
/// the marker verbatim, with zero retry attempts.
fn check_waf_headers(resp: &reqwest::Response) -> Result<(), ClientError> {
    if let Some(marker) = resp
        .headers()
        .get("x-amzn-waf-action")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.eq_ignore_ascii_case("captcha"))
    {
        return Err(ClientError::CaptchaWall(marker.to_string()));
    }
    if let Some(marker) = resp
        .headers()
        .get("cf-mitigated")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.eq_ignore_ascii_case("challenge"))
    {
        return Err(ClientError::CaptchaWall(marker.to_string()));
    }
    Ok(())
}

/// Client errors (frozen). Gateway maps these to `GatewayErrorKind` per DW-E-*.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// `biz_code 40012` USER_IS_BANNED (DW-E-02).
    #[error("user is banned (40012)")]
    Banned,
    /// `biz_code 40300`/`40301` invalid PoW response (DW-E-03).
    #[error("invalid pow response (40300/40301)")]
    InvalidPow,
    /// HTTP 429 with optional Retry-After duration (DW-E-01, DW-G-03).
    #[error("rate limited, retry after {0:?}")]
    RateLimited(Option<std::time::Duration>),
    /// `biz_code 40002`/`40003` missing or invalid web token (DW-E-04).
    #[error("missing or invalid web token")]
    Token,
    /// WAF/captcha wall: `x-amzn-waf-action: captcha` / `cf-mitigated: challenge`
    /// (DW-G-02) — explicit failure, never bypassed.
    #[error("captcha/challenge wall: {0}")]
    CaptchaWall(String),
    /// Unclassified upstream HTTP status (DW-E-05/06).
    #[error("upstream http error {0}")]
    HttpStatus(u16),
    /// Network/transport failure (DW-E-05).
    #[error("network error: {0}")]
    Network(String),
    /// Any other failure; `biz_msg` is surfaced (DW-E-06/07).
    #[error("deepseek web error: {0}")]
    Other(String),
}
