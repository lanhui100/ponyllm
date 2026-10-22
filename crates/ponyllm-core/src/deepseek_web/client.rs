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

use crate::deepseek_web::pow::{
    encode_pow_response_header, solve_pow, PowChallenge, PowError, PowResponse,
};
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

/// Uploaded file record (wave-10 H-V01, R-VIS-04): parsed from
/// `data.biz_data` of `POST /api/v0/file/upload_file`:
/// `{id, status, file_name, file_size, model_kind, is_image, audit_result}`.
/// `status` starts as `FileParseStatus::Pending` — completion is the
/// `fetch_files`/`wait_file_ready` concern (H-V02/03).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UploadedFile {
    pub id: String,
    pub status: FileParseStatus,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub model_kind: String,
    #[serde(default)]
    pub is_image: bool,
    #[serde(default)]
    pub audit_result: String,
}

/// File parse status returned by `GET /api/v0/file/fetch_files` (H-V02).
/// Real-wire strings: `PENDING`, `SUCCESS` (gap report, 2026-10-22); any
/// other string is tolerated as `Unknown` (contract §11.1 — unknown states
/// keep polling until timeout). `Failed` carries the raw wire string for
/// explicit error surfacing (R-VIS-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileParseStatus {
    Pending,
    Success,
    Failed(String),
    Unknown(String),
}

impl serde::Serialize for FileParseStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s: &str = match self {
            FileParseStatus::Pending => "PENDING",
            FileParseStatus::Success => "SUCCESS",
            FileParseStatus::Failed(raw) => raw,
            FileParseStatus::Unknown(raw) => raw,
        };
        serializer.serialize_str(s)
    }
}

impl<'de> serde::Deserialize<'de> for FileParseStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.to_ascii_uppercase().as_str() {
            "PENDING" => FileParseStatus::Pending,
            "SUCCESS" | "COMPLETED" | "FINISHED" | "READY" => FileParseStatus::Success,
            "FAILED" | "ERROR" => FileParseStatus::Failed(raw),
            _ => FileParseStatus::Unknown(raw),
        })
    }
}

/// One entry of `GET /api/v0/file/fetch_files` (H-V02, R-VIS-04): the parse
/// state of a previously uploaded file. Secondary fields tolerate absence so
/// unknown fetch shapes do not fail the whole poll.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub id: String,
    pub status: FileParseStatus,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub is_image: bool,
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
    /// (REA §2 / DW-S-03) and `application/json` (JSON callers). Authorization
    /// is added only when a token exists — no forged credentials (DW-G-05).
    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        self.request_method(reqwest::Method::POST, path)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
    }

    /// Generalized request builder (wave-10: GET for `fetch_files`, multipart
    /// POST for `upload_file`).
    ///
    /// **No content-type is set here.** reqwest's `header()` APPENDS rather
    /// than replaces, so a pre-set `application/json` would remain first on
    /// the wire next to the multipart boundary header and servers (incl. the
    /// real chat.deepseek.com upload endpoint) honor the first value → 400.
    /// JSON callers set `application/json` via [`Self::request`]; multipart
    /// callers rely on reqwest's `multipart/form-data; boundary=…`.
    fn request_method(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .header("x-client-bundle-id", CLIENT_BUNDLE_ID)
            .header("x-client-platform", CLIENT_PLATFORM)
            .header("x-client-version", CLIENT_VERSION)
            .header("x-client-locale", CLIENT_LOCALE)
            .header("x-client-timezone-offset", CLIENT_TIMEZONE_OFFSET)
            .header("x-device-id", &self.device_id)
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
            match handle_short_response(resp, attempts == 0).await {
                ShortResponse::Retry(wait) => {
                    tokio::time::sleep(wait).await;
                    attempts += 1;
                }
                ShortResponse::Final(outcome) => return outcome,
            }
        }
    }

    /// Like [`Self::send_short`] but re-creates the request builder per
    /// attempt via `make` — required for multipart uploads whose builders are
    /// not clonable (reqwest cannot clone a multipart body). The bounded 429
    /// Retry-After backoff and WAF checks behave identically.
    async fn send_short_rebuild<F>(
        &self,
        make: F,
    ) -> Result<reqwest::Response, ClientError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut attempts = 0;
        loop {
            let resp = make()
                .timeout(SHORT_CALL_TIMEOUT)
                .send()
                .await
                .map_err(|e| ClientError::Network(e.to_string()))?;
            match handle_short_response(resp, attempts == 0).await {
                ShortResponse::Retry(wait) => {
                    tokio::time::sleep(wait).await;
                    attempts += 1;
                }
                ShortResponse::Final(outcome) => return outcome,
            }
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

    /// Upload a file as an image (wave-10 H-V01, R-VIS-03/04).
    ///
    /// Honest single PoW solve per request (wave-9 DW-P-07, R-VIS-15 — no
    /// replay, no pre-batch): fresh `create_pow_challenge(target_path =
    /// "/api/v0/file/upload_file")` → `solve_pow` → multipart `POST
    /// {base}/api/v0/file/upload_file` with field name `file`, filename and
    /// content-type, plus the `X-DS-PoW-Response` header. Parses
    /// `data.biz_data.{id,status,file_name,file_size,model_kind,is_image,
    /// audit_result}`; non-zero `biz_code` maps via wave-9 H-08. Bytes are
    /// owned and dropped after upload (no disk, no cache).
    pub async fn upload_file(
        &self,
        file_name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadedFile, ClientError> {
        const UPLOAD_TARGET: &str = "/api/v0/file/upload_file";
        let challenge = self.create_pow_challenge(UPLOAD_TARGET).await?;
        let pow = solve_pow(&challenge, UPLOAD_TARGET).map_err(pow_error_to_client)?;
        // Pre-validate the mime string once (the only fallible multipart
        // construction step) before entering the retry loop.
        reqwest::multipart::Part::bytes(Vec::new())
            .mime_str(mime)
            .map_err(|e| ClientError::Other(format!("invalid mime {mime}: {e}")))?;
        // multipart builders are not clonable — rebuild per attempt from the
        // owned bytes (the bounded 429 backoff may clone the payload once at
        // most; ≤100MB per contract R-VIS-09).
        let resp = self
            .send_short_rebuild(|| {
                let part = reqwest::multipart::Part::bytes(bytes.clone())
                    .file_name(file_name.to_string())
                    .mime_str(mime)
                    .expect("mime pre-validated");
                let form = reqwest::multipart::Form::new().part("file", part);
                self.request_method(reqwest::Method::POST, UPLOAD_TARGET)
                    .header("X-DS-PoW-Response", encode_pow_response_header(&pow))
                    .multipart(form)
            })
            .await?;
        let body = resp.text().await.map_err(|e| ClientError::Network(e.to_string()))?;
        parse_upload_response(&body)
    }

    /// Fetch parse status of previously uploaded files (wave-10 H-V02,
    /// R-VIS-04): `GET {base}/api/v0/file/fetch_files?file_ids=a,b`.
    /// Pure query — no upload side effect. Empty `ids` returns `Ok(vec![])`
    /// without any request. Unknown status strings are tolerated
    /// (`FileParseStatus::Unknown`).
    pub async fn fetch_files(&self, ids: &[String]) -> Result<Vec<FileInfo>, ClientError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let file_ids = ids.join(",");
        // Raw query append keeps the literal comma form (`?file_ids=a,b` per
        // the upstream contract); ids are internally generated safe
        // identifiers, so no percent-encoding is required here.
        let builder = self
            .request_method(
                reqwest::Method::GET,
                &format!("/api/v0/file/fetch_files?file_ids={file_ids}"),
            );
        let resp = self.send_short(builder).await?;
        let body = resp.text().await.map_err(|e| ClientError::Network(e.to_string()))?;
        parse_fetch_response(&body)
    }

    /// Poll `fetch_files(&[id])` until the file reaches a terminal parse
    /// state (wave-10 H-V03, R-VIS-04/14).
    ///
    /// Polls every 1s; `PENDING`/`Unknown` keep polling; `Success` and
    /// `Failed` are terminal and returned as-is (the caller turns `Failed`
    /// into an explicit error with reason — R-VIS-13). The deadline is the
    /// passed `timeout`, hard-capped at 60s (contract R-VIS-14 default); a
    /// timeout yields an explicit error — never a silent half-finished file.
    /// The loop is request-scoped: no background task, no resident timer.
    pub async fn wait_file_ready(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<FileInfo, ClientError> {
        const MAX_WAIT_SECS: u64 = 60;
        let timeout = timeout.min(std::time::Duration::from_secs(MAX_WAIT_SECS));
        let deadline = std::time::Instant::now() + timeout;
        let mut last_status: Option<FileParseStatus> = None;
        loop {
            let files = self.fetch_files(&[id.to_string()]).await?;
            if let Some(info) = files.into_iter().find(|f| f.id == id) {
                last_status = Some(info.status.clone());
                match &info.status {
                    FileParseStatus::Pending | FileParseStatus::Unknown(_) => {}
                    FileParseStatus::Success | FileParseStatus::Failed(_) => return Ok(info),
                }
            }
            if std::time::Instant::now() >= deadline {
                let last = match &last_status {
                    Some(FileParseStatus::Unknown(raw)) => raw.clone(),
                    Some(FileParseStatus::Failed(raw)) => raw.clone(),
                    Some(s) => format!("{s:?}"),
                    None => "none".to_string(),
                };
                return Err(ClientError::Other(format!(
                    "file {id} not ready within {}s (last status: {last})",
                    timeout.as_secs()
                )));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            tokio::time::sleep(remaining.min(std::time::Duration::from_secs(1))).await;
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

/// Upload parse (wave-10 H-V01, R-VIS-04): envelope
/// `{"code":0,"data":{"biz_code":0,"biz_data":{…}}}`; non-zero `biz_code`
/// maps via wave-9 H-08.
fn parse_upload_response(body: &str) -> Result<UploadedFile, ClientError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ClientError::Other(format!("upload response is not JSON: {e}")))?;
    let data = value.get("data").unwrap_or(&value);
    let biz_code = data.get("biz_code").and_then(|v| v.as_i64()).unwrap_or(0);
    if biz_code != 0 {
        return Err(classify_api_error(200, body));
    }
    let biz_data = data
        .get("biz_data")
        .ok_or_else(|| ClientError::Other("upload response missing data.biz_data".to_string()))?;
    serde_json::from_value(biz_data.clone())
        .map_err(|e| ClientError::Other(format!("upload biz_data unparseable: {e}")))
}

/// Fetch parse (wave-10 H-V02, R-VIS-04): the file list may sit under any of
/// `data.biz_data.file_infos` / `data.biz_data.files` / `data.file_infos` /
/// `data.files` / `files` / `file_infos` (wire shape is only partially
/// documented); each entry deserializes tolerantly (secondary fields
/// default). Non-zero `biz_code` maps via wave-9 H-08.
fn parse_fetch_response(body: &str) -> Result<Vec<FileInfo>, ClientError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ClientError::Other(format!("fetch_files response is not JSON: {e}")))?;
    let data = value.get("data").unwrap_or(&value);
    if let Some(code) = data.get("biz_code").and_then(|v| v.as_i64()) {
        if code != 0 {
            return Err(classify_api_error(200, body));
        }
    }
    let array = ["file_infos", "files", "items", "list"]
        .iter()
        .find_map(|key| {
            data.get("biz_data")
                .and_then(|b| b.get(*key))
                .or_else(|| data.get(*key))
                .and_then(|v| v.as_array())
        })
        .ok_or_else(|| ClientError::Other("fetch_files response missing file list".to_string()))?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        out.push(serde_json::from_value(item.clone()).map_err(|e| {
            ClientError::Other(format!("fetch_files entry unparseable: {e}"))
        })?);
    }
    Ok(out)
}

/// Map a `solve_pow` failure onto the client error surface (wave-10 H-V01).
/// An unsolvable fresh challenge is an upstream anomaly — surfaced, never
/// silently retried or replayed.
fn pow_error_to_client(e: PowError) -> ClientError {
    match e {
        PowError::NoSolution => ClientError::Other("pow solve failed: no solution within difficulty".to_string()),
        PowError::Expired => ClientError::Other("pow challenge expired before solve".to_string()),
        PowError::Other(msg) => ClientError::Other(msg),
    }
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

/// Outcome of the shared short-call response classification
/// ([`handle_short_response`]): a bounded 429 retry (only on the first
/// attempt, with the server-declared wait) or a final result.
enum ShortResponse {
    Retry(std::time::Duration),
    Final(Result<reqwest::Response, ClientError>),
}

/// Shared post-send classification for short (non-streaming) calls
/// (DW-G-02/DW-G-03, wave-10 upload/fetch reuse):
/// * HTTP 429 with Retry-After on the FIRST attempt → `Retry(wait)` (bounded
///   single backoff; a second 429 is a hard `RateLimited`);
/// * WAF markers → explicit `CaptchaWall`, zero retries;
/// * success → the raw response;
/// * any other status → `classify_api_error`.
async fn handle_short_response(
    resp: reqwest::Response,
    first_attempt: bool,
) -> ShortResponse {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = retry_after_duration(&resp);
        if first_attempt {
            if let Some(wait) = retry_after {
                return ShortResponse::Retry(wait);
            }
        }
        return ShortResponse::Final(Err(ClientError::RateLimited(retry_after)));
    }
    if let Err(e) = check_waf_headers(&resp) {
        return ShortResponse::Final(Err(e));
    }
    if status.is_success() {
        return ShortResponse::Final(Ok(resp));
    }
    let body = resp.text().await.unwrap_or_default();
    ShortResponse::Final(Err(classify_api_error(status.as_u16(), &body)))
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
