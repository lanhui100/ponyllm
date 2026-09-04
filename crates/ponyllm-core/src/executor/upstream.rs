use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use crate::error::{CoreError, GatewayErrorKind, Result};
use crate::pool::{ApiKeyEntry, KeyPool, PoolErrorType};
use crate::telemetry::{GatewayEvent, StageTimings};

/// One upstream attempt outcome inside an executor retry loop.
///
/// Emitted for every try (key selection, header build, HTTP status, network),
/// so the gateway can record per-attempt forensic frames instead of only the
/// aggregated `last_error` string.
#[derive(Debug, Clone)]
pub struct AttemptEvent {
    pub provider: String,
    pub key_id: String,
    pub attempt: u32,
    pub status_code: Option<u16>,
    pub kind: GatewayErrorKind,
    /// Short human-readable summary (safe for the frame `error` field).
    pub summary: String,
    /// Upstream response body when available (goes to `response_snippet`).
    pub detail: Option<String>,
    pub latency: Duration,
}

/// Opt-in observer for [`UpstreamExecutor`] attempts.
/// Kept as a plain sync callback to avoid holding locks across `.await`.
pub type AttemptObserver = Arc<dyn Fn(AttemptEvent) + Send + Sync>;

/// Single-append event sink for the observability pipeline.
/// The closure captures the bus + request context; the executor only supplies
/// the [`GatewayEvent`]. Replaces per-call-site collector writes.
pub type EventSink = Arc<dyn Fn(GatewayEvent) + Send + Sync>;

/// Request-scoped context carried by the sink: request start for elapsed math,
/// a shared stage-timings slot filled as the attempt progresses.
#[derive(Clone)]
pub struct EventSinkCtx {
    pub request_id: String,
    pub endpoint: String,
    pub provider: String,
    pub start: Instant,
    pub stages: Arc<Mutex<StageTimings>>,
    pub request_snippet: Option<String>,
}

impl EventSinkCtx {
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

#[derive(Clone)]
pub struct UpstreamExecutor {
    pub pool: Arc<KeyPool>,
    pub client: reqwest::Client,
    pub max_retries: usize,
    observer_provider: Option<String>,
    observer: Option<AttemptObserver>,
    sink_ctx: Option<EventSinkCtx>,
    sink: Option<EventSink>,
}

impl std::fmt::Debug for UpstreamExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamExecutor")
            .field("pool", &self.pool)
            .field("max_retries", &self.max_retries)
            .field("observer_provider", &self.observer_provider)
            .field("has_observer", &self.observer.is_some())
            .finish()
    }
}

impl UpstreamExecutor {
    pub fn new(pool: Arc<KeyPool>, max_retries: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        Self {
            pool,
            client,
            max_retries,
            observer_provider: None,
            observer: None,
            sink_ctx: None,
            sink: None,
        }
    }

    /// Attach an opt-in event sink. Emits `KeySelected`, `UpstreamHeaders`
    /// and `UpstreamAttemptFailed` on the same paths as the attempt observer.
    pub fn with_event_sink(mut self, ctx: EventSinkCtx, sink: EventSink) -> Self {
        self.sink_ctx = Some(ctx);
        self.sink = Some(sink);
        self
    }

    /// Attach an opt-in per-attempt observer. `new` behavior is unchanged.
    pub fn with_attempt_observer(
        mut self,
        provider: impl Into<String>,
        observer: AttemptObserver,
    ) -> Self {
        self.observer_provider = Some(provider.into());
        self.observer = Some(observer);
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_attempt(
        &self,
        key_id: &str,
        attempt: u32,
        status_code: Option<u16>,
        kind: GatewayErrorKind,
        summary: String,
        detail: Option<String>,
        latency: Duration,
    ) {
        if let (Some(provider), Some(observer)) = (self.observer_provider.as_deref(), self.observer.as_ref()) {
            observer(AttemptEvent {
                provider: provider.to_string(),
                key_id: key_id.to_string(),
                attempt,
                status_code,
                kind,
                summary,
                detail,
                latency,
            });
        }
    }

    fn emit_sink(&self, event: GatewayEvent) {
        if let Some(sink) = self.sink.as_ref() {
            sink(event);
        }
    }

    /// Emit to both the legacy attempt observer and the event sink.
    /// State attaches only one of them; the sink path carries
    /// `failover`/`kind_name` computed once via `GatewayErrorKind`.
    /// 7-arg shape mirrors the established `emit_attempt` reporter below.
    #[allow(clippy::too_many_arguments)]
    fn emit_both(
        &self,
        key_id: &str,
        attempt: u32,
        status_code: Option<u16>,
        kind: GatewayErrorKind,
        summary: String,
        detail: Option<String>,
        latency: Duration,
    ) {
        self.emit_attempt(
            key_id,
            attempt,
            status_code,
            kind.clone(),
            summary.clone(),
            detail.clone(),
            latency,
        );
        self.emit_failure(key_id, attempt, status_code, &kind, &summary, detail, latency);
    }

    /// Sink half of [`Self::emit_both`]: same established shape.
    #[allow(clippy::too_many_arguments)]
    fn emit_failure(
        &self,
        key_id: &str,
        attempt: u32,
        status_code: Option<u16>,
        kind: &GatewayErrorKind,
        summary: &str,
        detail: Option<String>,
        latency: Duration,
    ) {
        self.emit_sink(GatewayEvent::UpstreamAttemptFailed {
            key_id: key_id.to_string(),
            attempt,
            status_code,
            kind: kind.kind_name().to_string(),
            failover: kind.triggers_failover(),
            summary: summary.to_string(),
            detail,
            latency_ms: latency.as_secs_f64() * 1000.0,
            request_snippet: self
                .sink_ctx
                .as_ref()
                .and_then(|c| c.request_snippet.clone()),
        });
    }

    fn emit_headers(&self, key_id: &str, attempt: u32, ttfb: Duration) {
        let ttfb_ms = ttfb.as_secs_f64() * 1000.0;
        if let Some(ctx) = self.sink_ctx.as_ref() {
            ctx.stages.lock().upstream_ttfb_ms = Some(ttfb_ms);
        }
        self.emit_sink(GatewayEvent::UpstreamHeaders {
            key_id: key_id.to_string(),
            attempt,
            ttfb_ms,
        });
    }

    fn emit_key_selected(&self, key_id: &str, select_time: Duration) {
        self.emit_sink(GatewayEvent::KeySelected {
            key_id: key_id.to_string(),
            select_ms: select_time.as_secs_f64() * 1000.0,
        });
    }

    fn build_headers(&self, key: &ApiKeyEntry) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let clean_key = key.api_key.trim();
        if clean_key.is_empty() {
            return Err(CoreError::Internal(format!("API key for '{}' is empty", key.id)));
        }

        // Bearer header for OpenAI/DeepSeek
        let bearer_val = HeaderValue::from_str(&format!("Bearer {}", clean_key))
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert(AUTHORIZATION, bearer_val);

        // x-api-key header for Anthropic
        let x_api_val = HeaderValue::from_str(clean_key)
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert("x-api-key", x_api_val);

        Ok(headers)
    }

    /// Execute a JSON request with transparent automatic failover before response body starts
    pub async fn execute_json_request(&self, url: &str, body: &Value) -> Result<Value> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;

        for attempt in 0..self.max_retries.max(1) {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let key = match self.pool.select_key() {
                Ok(k) => k,
                Err(e) => {
                    // First-attempt pool exhaustion surfaces structurally so
                    // callers never string-match on the aggregated message.
                    if attempt == 0 {
                        self.emit_both("", attempt_idx, None, e.kind(), e.to_string(), None, attempt_start.elapsed());
                        return Err(e);
                    }
                    self.emit_both("", attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    return Err(CoreError::AllRetriesFailed {
                        retries: attempt,
                        last_error,
                        kind: last_kind,
                    });
                }
            };

            self.emit_key_selected(&key.id, select_start.elapsed());

            let headers = match self.build_headers(&key) {
                Ok(h) => h,
                Err(e) => {
                    self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    last_error = e.to_string();
                    last_kind = GatewayErrorKind::AuthInvalid;
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(body);

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        self.emit_headers(&key.id, attempt_idx, attempt_start.elapsed());
                        let json_val = resp.json::<Value>().await?;
                        return Ok(json_val);
                    }

                    // Handle failover status codes
                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    let err_body = resp.text().await.unwrap_or_default();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        last_kind = GatewayErrorKind::RateLimitExceeded { retry_after };
                        self.pool.record_error(&key.id, PoolErrorType::RateLimit { retry_after });
                    } else if status_code == 401 {
                        last_kind = GatewayErrorKind::AuthInvalid;
                        self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    } else if status_code == 402 || (status_code == 403 && err_body.to_lowercase().contains("quota")) {
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted);
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                    } else {
                        // Client error that is not retryable (e.g. 400 Bad Request)
                        self.emit_both(&key.id, attempt_idx, Some(status_code), GatewayErrorKind::ClientBadRequest, last_error.clone(), Some(err_body.clone()), attempt_start.elapsed());
                        return Err(CoreError::UpstreamStatusError {
                            status,
                            body: err_body,
                        });
                    }
                    self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                }
                Err(err) => {
                    last_error = format!("Network error with {}: {}", key.id, err);
                    last_kind = GatewayErrorKind::UpstreamUnavailable;
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: self.max_retries,
            last_error,
            kind: last_kind,
        })
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded
    pub async fn execute_stream_request(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;

        for attempt in 0..self.max_retries.max(1) {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let key = match self.pool.select_key() {
                Ok(k) => k,
                Err(e) => {
                    // First-attempt pool exhaustion surfaces structurally so
                    // callers never string-match on the aggregated message.
                    if attempt == 0 {
                        self.emit_both("", attempt_idx, None, e.kind(), e.to_string(), None, attempt_start.elapsed());
                        return Err(e);
                    }
                    self.emit_both("", attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    return Err(CoreError::AllRetriesFailed {
                        retries: attempt,
                        last_error,
                        kind: last_kind,
                    });
                }
            };

            self.emit_key_selected(&key.id, select_start.elapsed());

            let headers = match self.build_headers(&key) {
                Ok(h) => h,
                Err(e) => {
                    self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    last_error = e.to_string();
                    last_kind = GatewayErrorKind::AuthInvalid;
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(body);

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        self.emit_headers(&key.id, attempt_idx, attempt_start.elapsed());
                        return Ok(resp);
                    }

                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    let err_body = resp.text().await.unwrap_or_default();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        last_kind = GatewayErrorKind::RateLimitExceeded { retry_after };
                        self.pool.record_error(&key.id, PoolErrorType::RateLimit { retry_after });
                    } else if status_code == 401 {
                        last_kind = GatewayErrorKind::AuthInvalid;
                        self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    } else if status_code == 402 || (status_code == 403 && err_body.to_lowercase().contains("quota")) {
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted);
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                    } else {
                        self.emit_both(&key.id, attempt_idx, Some(status_code), GatewayErrorKind::ClientBadRequest, last_error.clone(), Some(err_body.clone()), attempt_start.elapsed());
                        return Err(CoreError::UpstreamStatusError {
                            status,
                            body: err_body,
                        });
                    }
                    self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                }
                Err(err) => {
                    last_error = format!("Network error with {}: {}", key.id, err);
                    last_kind = GatewayErrorKind::UpstreamUnavailable;
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: self.max_retries,
            last_error,
            kind: last_kind,
        })
    }
}
