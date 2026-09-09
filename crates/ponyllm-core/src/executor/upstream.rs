use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use crate::error::{CoreError, GatewayErrorKind, Result};
use crate::pool::{ApiKeyEntry, KeyPool, KeyState, PoolErrorType};
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
    /// Session id forwarded as `x-opencode-session` (+ affinity aliases).
    /// Resolved once per gateway request so every key retry shares it.
    session_id: String,
    /// Client label forwarded as `x-opencode-client`.
    client_label: String,
    /// Zen scope gate (see [`is_opencode_zen_target`]): only zen targets
    /// get the session headers. Defaults to off so non-zen upstreams keep
    /// byte-identical wire headers to before.
    opencode_zen: bool,
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

/// Gateway-owned User-Agent advertised to upstreams (not a generic SDK name).
/// OpenCode Go requires callers to identify with their own agent string for
/// abuse monitoring; the reqwest default would be flagged as generic.
pub fn ponyllm_user_agent() -> String {
    format!("ponyllm/{}", env!("CARGO_PKG_VERSION"))
}

/// Downstream session headers accepted as the upstream `x-opencode-session`
/// source, in priority order. `opencode` itself sends `x-opencode-session`
/// for opencode providers and `x-session-affinity`/`X-Session-Id` otherwise;
/// `x-pony-session` lets non-opencode coding tools pin a stable conversation.
pub const SESSION_HEADER_PRIORITY: &[&str] = &[
    "x-opencode-session",
    "x-pony-session",
    "x-session-affinity",
    "x-session-id",
];

/// Generate a gateway-side session id used when the downstream client sent
/// none. Passes the upstream `MissingSessionID` gate; per-conversation
/// stability still requires the downstream to send one of
/// [`SESSION_HEADER_PRIORITY`].
pub fn new_upstream_session_id() -> String {
    format!("ponyllm-{}", uuid::Uuid::new_v4().simple())
}

fn clean_session_value(value: &HeaderValue) -> Option<String> {
    let trimmed = value.to_str().ok()?.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return None;
    }
    Some(trimmed.to_string())
}

/// Transient-retry backoff for **singleton** pools only: when the sole key
/// survives `record_error` still `Active` (singleton 429 via
/// `record_transient_failure`, or sub-threshold 5xx/network), allow the same
/// key to be retried within this request instead of failing with
/// `NoAvailableKey` after one attempt.
///
/// Multi-key pools always fail over to the next key (see
/// `test_executor_fails_over_on_server_error_without_immediate_cooldown`);
/// retrying the same Active key there would starve healthy candidates.
fn transient_retry_delay(
    pool: &KeyPool,
    key_id: &str,
    attempt: usize,
    max_attempts: usize,
    retry_after: Option<Duration>,
) -> Option<Duration> {
    if attempt + 1 >= max_attempts {
        return None;
    }
    if pool.total_key_count() != 1 {
        return None;
    }
    if pool.get_key_status(key_id) != Some(KeyState::Active) {
        return None;
    }
    let base = retry_after.map(|d| d.min(Duration::from_secs(5)));
    Some(base.unwrap_or(Duration::from_millis(1200)))
}

/// Whether a terminal 400 body looks like Google's transient geo-gate
/// (`FAILED_PRECONDITION: User location is not supported`) rather than a
/// genuine caller error. Sampling shows these arrive in time-windowed storms
/// affecting every client of the egress IP equally (reference gateway fails
/// identically), then clear on their own — while genuine 400s (empty
/// messages, bad schema) never match this signature.
pub fn is_transient_geo_gate(status_code: u16, err_body: &str) -> bool {
    if status_code != 400 {
        return false;
    }
    let lower = err_body.to_ascii_lowercase();
    lower.contains("failed_precondition")
        && (lower.contains("location is not supported")
            || lower.contains("unsupported_location")
            || lower.contains("unsupported location"))
}

/// Exact-match Google account-death signatures for 403 bodies (matched
/// against the lowercased body). Deliberately narrow: bare "violation" /
/// "suspended" substrings also match prompt safety rejections and other
/// recoverable 403s, and every permanent isolate burns a credential (P0-1).
const TOS_ACCOUNT_DEATH_SIGNATURES: &[&str] = &[
    "terms_of_service_violation",
    "terms of service violation",
    "violated terms of service",
    "account_suspended",
    "account suspended",
    "consumer_suspended",
    "consumer suspended",
];

fn is_tos_account_death(lower_body: &str) -> bool {
    TOS_ACCOUNT_DEATH_SIGNATURES
        .iter()
        .any(|sig| lower_body.contains(sig))
}

/// Classify a 403 body into (gateway kind, pool action).
///
/// - Exact ToS death signature → permanent `PolicyViolation` isolate
///   (still guarded by the pool mass-disable breaker).
/// - Quota wording → `QuotaExhausted` kind for honest downstream errors,
///   but only a cooldown on the pool: real quota recovers at resetTime
///   and throttling clears on its own (P0-2).
/// - Unknown 403 → 60s cooling + warning. A new Google wording, locale
///   variant, or WAF flap must never burn a credential on first sight.
fn classify_forbidden(
    err_body: &str,
    retry_after: Option<Duration>,
) -> (GatewayErrorKind, PoolErrorType) {
    let lower = err_body.to_lowercase();
    if is_tos_account_death(&lower) {
        (GatewayErrorKind::AuthInvalid, PoolErrorType::PolicyViolation)
    } else if lower.contains("quota")
        || lower.contains("#3501")
        || lower.contains("resource_exhausted")
        || lower.contains("quota_exceeded")
    {
        (
            GatewayErrorKind::QuotaExhausted,
            PoolErrorType::QuotaExhausted {
                retry_after: retry_after.or(Some(Duration::from_secs(900))),
            },
        )
    } else if lower.contains("#1008") || lower.contains("unsupported_location") {
        (
            GatewayErrorKind::RateLimitExceeded {
                retry_after: Some(Duration::from_secs(300)),
            },
            PoolErrorType::RateLimit {
                retry_after: Some(Duration::from_secs(300)),
            },
        )
    } else {
        tracing::warn!(
            body_preview = %err_body.chars().take(300).collect::<String>(),
            "unknown 403 body: cooling 60s instead of permanent isolate"
        );
        (
            GatewayErrorKind::UpstreamUnavailable,
            PoolErrorType::RateLimit {
                retry_after: Some(Duration::from_secs(60)),
            },
        )
    }
}

/// Outcome of the stale-token recovery attempt for an Antigravity 401.
enum StaleTokenRecovery {
    /// Forced refresh healed the token: caller must retry the same key.
    RetrySameKey,
    /// Recovery ran and already recorded the pool outcome: caller only
    /// sets `last_kind`, no further recording.
    Recorded(GatewayErrorKind),
    /// Not applicable (static key, or this key already refreshed once
    /// this request): caller runs the legacy path.
    Passthrough,
}
/// One bounded same-key retry for a transient geo-gate on a **singleton**
/// pool: the gate is egress/account-level, so failing over is pointless and
/// there is no other key anyway. Fires at most once per request (`attempt ==
/// 0`) with a ~2.5s backoff — enough for sub-10s blips, short enough that
/// multi-minute storms still surface fast. Never records pool state (the key
/// is healthy; cf. reference gateways that auto-ban credentials on these).
fn geo_gate_retry_delay(attempt: usize, pool: &KeyPool) -> Option<Duration> {
    if attempt != 0 || pool.total_key_count() != 1 {
        return None;
    }
    Some(Duration::from_millis(2500))
}

/// Resolve the upstream session id: first valid downstream session header,
/// else a freshly generated gateway-side id.
pub fn resolve_upstream_session(downstream: &HeaderMap) -> String {
    for name in SESSION_HEADER_PRIORITY {
        if let Some(value) = downstream.get(*name).and_then(clean_session_value) {
            return value;
        }
    }
    new_upstream_session_id()
}

/// Resolve the upstream client label: downstream `x-opencode-client` when
/// present, else the gateway's own name.
pub fn resolve_upstream_client(downstream: &HeaderMap) -> String {
    downstream
        .get("x-opencode-client")
        .and_then(clean_session_value)
        .unwrap_or_else(|| "ponyllm".to_string())
}

/// Scope gate: only opencode **zen** endpoints receive the session-header
/// treatment. Go endpoints tolerate off-client use under a different
/// contract, and every other upstream must never see opencode-specific
/// headers. Either signal matches: an `opencode*` provider name (covers
/// direct `https://opencode.ai/zen/v1` configs) or an `opencode` URL
/// segment (covers `.../opencode/zen/v1` forward proxies); a `/go/`
/// path segment always opts out.
pub fn is_opencode_zen_target(provider_name: &str, target_url: &str) -> bool {
    let provider = provider_name.to_ascii_lowercase();
    let url = target_url.to_ascii_lowercase();
    if !(provider.starts_with("opencode") || url.contains("opencode")) {
        return false;
    }
    !url.contains("/go/")
}

/// Create an optimized, connection-pooled HTTP client for upstream LLM providers.
/// Enables TCP nodelay, Keep-Alive probing, and idle connection reuse to minimize TTFT.
/// By default, disables system environment proxies (`http_proxy`/`https_proxy`) to isolate
/// the gateway from ambient terminal proxy environments.
pub fn create_upstream_http_client() -> reqwest::Client {
    create_upstream_http_client_with_options(None, false)
}

/// Create an upstream HTTP client with optional explicit proxy URL and system proxy inheritance flag (returns Result).
pub fn try_create_upstream_http_client_with_options(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
) -> std::result::Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32);

    if !use_system_proxy {
        builder = builder.no_proxy();
    }

    if let Some(proxy_str) = proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            let proxy = reqwest::Proxy::all(trimmed)
                .map_err(|e| format!("无法解析代理地址 '{}': {}", trimmed, e))?;
            let proxy = proxy.no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1"));
            builder = builder.proxy(proxy);
        }
    }

    builder.build().map_err(|e| format!("构建 HTTP Client 失败: {}", e))
}

pub fn create_upstream_http_client_with_options(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
) -> reqwest::Client {
    match try_create_upstream_http_client_with_options(proxy_url, use_system_proxy) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to create configured proxy client, falling back to default");
            reqwest::Client::builder().build().unwrap_or_default()
        }
    }
}

/// Detects system proxy settings dynamically without hardcoding ports.
///
/// Priority order:
/// 1. Environment variables (`HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`, `http_proxy`, `ALL_PROXY`, `all_proxy`)
/// 2. User proxy environment file (`~/.pony/proxy.env`)
/// 3. Local active proxy ports probe (`127.0.0.1` on common ports: 8899, 7890, 10808, 10809, 7897, 1080)
pub fn detect_system_proxy() -> Option<String> {
    // 1. Environment variables
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(normalize_proxy_url(trimmed));
            }
        }
    }

    // 2. ~/.pony/proxy.env
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let env_file = std::path::Path::new(&home).join(".pony").join("proxy.env");
        if env_file.is_file() {
            if let Ok(content) = std::fs::read_to_string(&env_file) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with('#') || line.is_empty() {
                        continue;
                    }
                    let stripped = line.strip_prefix("export ").unwrap_or(line).trim();
                    for prefix in [
                        "https_proxy=",
                        "HTTPS_PROXY=",
                        "http_proxy=",
                        "HTTP_PROXY=",
                        "all_proxy=",
                        "ALL_PROXY=",
                    ] {
                        if let Some(val) = stripped.strip_prefix(prefix) {
                            let clean = val.trim().trim_matches('\'').trim_matches('"').trim();
                            if !clean.is_empty() {
                                return Some(normalize_proxy_url(clean));
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Probing common local proxy ports with fast timeout (30ms)
    // Exclude 8080 (PonyLLM's default gateway port) to prevent self-looping
    const COMMON_LOCAL_PORTS: &[u16] = &[8899, 7890, 10808, 10809, 7897, 1080];
    for &port in COMMON_LOCAL_PORTS {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(30)).is_ok() {
            return Some(format!("http://127.0.0.1:{}", port));
        }
    }

    None
}


fn normalize_proxy_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("socks5://")
    {
        trimmed.to_string()
    } else {
        format!("http://{}", trimmed)
    }
}


impl UpstreamExecutor {
    pub fn new(pool: Arc<KeyPool>, max_retries: usize) -> Self {
        Self::with_client(pool, create_upstream_http_client(), max_retries)
    }

    pub fn with_client(pool: Arc<KeyPool>, client: reqwest::Client, max_retries: usize) -> Self {
        Self {
            pool,
            client,
            max_retries,
            observer_provider: None,
            observer: None,
            sink_ctx: None,
            sink: None,
            // Defense in depth: even callers that never saw downstream
            // headers still satisfy the upstream MissingSessionID gate
            // once they opt into the zen scope below.
            session_id: new_upstream_session_id(),
            client_label: "ponyllm".to_string(),
            opencode_zen: false,
        }
    }

    /// Opt into the opencode zen session treatment for this executor.
    /// Routes compute the flag with [`is_opencode_zen_target`] from the
    /// resolved provider + target URL; everything else stays untouched.
    pub fn with_opencode_zen(mut self, enabled: bool) -> Self {
        self.opencode_zen = enabled;
        self
    }

    /// Adopt downstream session identity for upstream routing/caching.
    /// Extracts `x-opencode-session` (or affinity aliases) once, so every
    /// per-key retry inside this executor reuses the same session instead of
    /// churning one id per attempt.
    pub fn with_downstream_headers(mut self, downstream: &HeaderMap) -> Self {
        self.session_id = resolve_upstream_session(downstream);
        self.client_label = resolve_upstream_client(downstream);
        self
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

    /// Stale-token recovery for an Antigravity 401 (P0-3): at most one
    /// forced refresh per key per request, then a same-key retry when the
    /// refresh heals the token. A 401 proves staleness better than the
    /// local expiry clock; killing the key without trying a refresh turns
    /// every routine token rotation into a burned credential.
    async fn recover_stale_antigravity_token(
        &self,
        key: &ApiKeyEntry,
        refreshed_keys: &mut Vec<String>,
    ) -> StaleTokenRecovery {
        if !key.is_antigravity() || refreshed_keys.contains(&key.id) {
            return StaleTokenRecovery::Passthrough;
        }
        refreshed_keys.push(key.id.clone());
        let Some(mgr) = key.antigravity_manager() else {
            return StaleTokenRecovery::Passthrough;
        };
        match mgr.force_refresh_token().await {
            Ok(_) => {
                tracing::info!(key_id = %key.id, "Antigravity 401 healed by forced refresh; retrying same key");
                StaleTokenRecovery::RetrySameKey
            }
            Err(CoreError::AuthInvalid { reason, .. }) => {
                // refresh_token burned (invalid_grant): permanent isolate,
                // still guarded by the pool mass-disable breaker.
                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                tracing::warn!(key_id = %key.id, reason = %reason, "Antigravity refresh_token dead (invalid_grant)");
                StaleTokenRecovery::Recorded(GatewayErrorKind::AuthInvalid)
            }
            Err(e) => {
                // Transient refresh failure: cool, never burn.
                self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                tracing::warn!(key_id = %key.id, error = %e, "Antigravity forced refresh transient failure");
                StaleTokenRecovery::Recorded(GatewayErrorKind::UpstreamUnavailable)
            }
        }
    }

    pub fn prepare_effective_body<'a>(key: &ApiKeyEntry, body: &'a Value) -> std::borrow::Cow<'a, Value> {
        if key.is_antigravity() {
            if let Some(target_proj) = key.antigravity_manager().map(|m| m.project_id()) {
                if let Some(obj) = body.as_object() {
                    if obj.contains_key("project") && obj.get("project").and_then(|v| v.as_str()) != Some(&target_proj) {
                        let mut patched = body.clone();
                        patched["project"] = Value::String(target_proj);
                        return std::borrow::Cow::Owned(patched);
                    }
                }
            }
        }
        std::borrow::Cow::Borrowed(body)
    }

    async fn build_headers(&self, key: &ApiKeyEntry, body: Option<&Value>) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if key.is_antigravity() {
            let token = key.resolve_token().await?;
            headers.insert(USER_AGENT, HeaderValue::from_static(crate::pool::ANTIGRAVITY_USER_AGENT));
            headers.insert("requestType", HeaderValue::from_static("agent"));
            headers.insert("x-goog-api-client", HeaderValue::from_static("gl-node/22.14.0 gdcl/1.1.24"));
            headers.insert(reqwest::header::ACCEPT, HeaderValue::from_static("text/event-stream, application/json"));
            let req_id = body
                .and_then(|b| b.get("requestId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    format!("agent/{}/{}/traj-default/1", uuid::Uuid::new_v4(), chrono::Utc::now().timestamp_millis())
                });
            if let Ok(val) = HeaderValue::from_str(&req_id) {
                headers.insert("requestId", val);
            }
            let bearer_val = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
                .map_err(|e| CoreError::Internal(format!("Invalid Antigravity bearer token for '{}': {}", key.id, e)))?;
            headers.insert(AUTHORIZATION, bearer_val);
            return Ok(headers);
        }

        let clean_key = key.api_key.trim();
        if clean_key.is_empty() {
            return Err(CoreError::Internal(format!("API key for '{}' is empty", key.id)));
        }

        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        // Bearer header for OpenAI/DeepSeek
        let bearer_val = HeaderValue::from_str(&format!("Bearer {}", clean_key))
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert(AUTHORIZATION, bearer_val);

        // x-api-key header for Anthropic
        let x_api_val = HeaderValue::from_str(clean_key)
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert("x-api-key", x_api_val);

        // OpenCode zen routing gate: `x-opencode-session` is mandatory
        // (MissingSessionID 400 otherwise). Aliases cover the native
        // session headers other coding agents send. Scoped to zen only;
        // every other upstream keeps its historical wire headers.
        if self.opencode_zen {
            let session_val = HeaderValue::from_str(&self.session_id)
                .map_err(|e| CoreError::Internal(format!("Invalid session id: {}", e)))?;
            headers.insert("x-opencode-session", session_val.clone());
            headers.insert("x-session-affinity", session_val.clone());
            headers.insert("x-session-id", session_val);
            let client_val = HeaderValue::from_str(&self.client_label)
                .map_err(|e| CoreError::Internal(format!("Invalid client label: {}", e)))?;
            headers.insert("x-opencode-client", client_val);
            // Own agent string, never a generic SDK default.
            let ua_val = HeaderValue::from_str(&ponyllm_user_agent())
                .map_err(|e| CoreError::Internal(format!("Invalid user agent: {}", e)))?;
            headers.insert(USER_AGENT, ua_val);
        }

        Ok(headers)
    }

    /// Execute a JSON request with transparent automatic failover before response body starts
    pub async fn execute_json_request(&self, url: &str, body: &Value) -> Result<Value> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;
        let mut attempted_keys = Vec::new();
        // Antigravity keys already force-refreshed once this request (P0-3
        // stale-token recovery): a second 401 on the same key is genuine.
        let mut refreshed_keys: Vec<String> = Vec::new();

        let max_attempts = self.max_retries.max(self.pool.total_key_count()).max(1);

        for attempt in 0..max_attempts {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let key = match self.pool.select_key_excluding(&attempted_keys) {
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
                        attempted_keys,
                        last_error,
                        kind: last_kind,
                    });
                }
            };

            attempted_keys.push(key.id.clone());
            self.emit_key_selected(&key.id, select_start.elapsed());

            let effective_body = Self::prepare_effective_body(&key, body);
            let headers = match self.build_headers(&key, Some(effective_body.as_ref())).await {
                Ok(h) => h,
                Err(e) => {
                    // Antigravity token-resolution failures carry their own
                    // kind: dead credentials isolate, transient refresh
                    // faults only cool (P0-3). Static keys keep the legacy
                    // fail-closed behavior.
                    let pool_err = match &e {
                        CoreError::AuthInvalid { .. } => PoolErrorType::AuthInvalid,
                        _ if key.is_antigravity() => PoolErrorType::NetworkError,
                        _ => PoolErrorType::AuthInvalid,
                    };
                    self.pool.record_error(&key.id, pool_err);
                    last_error = e.to_string();
                    last_kind = GatewayErrorKind::AuthInvalid;
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(effective_body.as_ref());

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
                        if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, retry_after) {
                            attempted_keys.retain(|id| id != &key.id);
                            self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    } else if status_code == 401 {
                        match self.recover_stale_antigravity_token(&key, &mut refreshed_keys).await {
                            StaleTokenRecovery::RetrySameKey => {
                                attempted_keys.retain(|id| id != &key.id);
                                last_kind = GatewayErrorKind::AuthInvalid;
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                continue;
                            }
                            StaleTokenRecovery::Recorded(kind) => {
                                last_kind = kind;
                            }
                            StaleTokenRecovery::Passthrough => {
                                last_kind = GatewayErrorKind::AuthInvalid;
                                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                            }
                        }
                    } else if status_code == 403 {
                        let (kind, pool_err) = classify_forbidden(&err_body, retry_after);
                        last_kind = kind;
                        self.pool.record_error(&key.id, pool_err);
                    } else if status_code == 402 {
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted { retry_after });
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                        if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                            attempted_keys.retain(|id| id != &key.id);
                            self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    } else {
                        // Client error that is not retryable (e.g. 400 Bad Request)
                        if is_transient_geo_gate(status_code, &err_body) {
                            if let Some(delay) = geo_gate_retry_delay(attempt, &self.pool) {
                                attempted_keys.retain(|id| id != &key.id);
                                self.emit_both(&key.id, attempt_idx, Some(status_code), GatewayErrorKind::ClientBadRequest, last_error.clone(), Some(err_body), attempt_start.elapsed());
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
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
                    if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                        attempted_keys.retain(|id| id != &key.id);
                        self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: max_attempts,
            attempted_keys,
            last_error,
            kind: last_kind,
        })
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded
    pub async fn execute_stream_request(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;
        let mut attempted_keys = Vec::new();
        // Antigravity keys already force-refreshed once this request (P0-3
        // stale-token recovery): a second 401 on the same key is genuine.
        let mut refreshed_keys: Vec<String> = Vec::new();

        let max_attempts = self.max_retries.max(self.pool.total_key_count()).max(1);

        for attempt in 0..max_attempts {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let key = match self.pool.select_key_excluding(&attempted_keys) {
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
                        attempted_keys,
                        last_error,
                        kind: last_kind,
                    });
                }
            };

            attempted_keys.push(key.id.clone());
            self.emit_key_selected(&key.id, select_start.elapsed());

            let effective_body = Self::prepare_effective_body(&key, body);
            let headers = match self.build_headers(&key, Some(effective_body.as_ref())).await {
                Ok(h) => h,
                Err(e) => {
                    // Antigravity token-resolution failures carry their own
                    // kind: dead credentials isolate, transient refresh
                    // faults only cool (P0-3). Static keys keep the legacy
                    // fail-closed behavior.
                    let pool_err = match &e {
                        CoreError::AuthInvalid { .. } => PoolErrorType::AuthInvalid,
                        _ if key.is_antigravity() => PoolErrorType::NetworkError,
                        _ => PoolErrorType::AuthInvalid,
                    };
                    self.pool.record_error(&key.id, pool_err);
                    last_error = e.to_string();
                    last_kind = GatewayErrorKind::AuthInvalid;
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(effective_body.as_ref());

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
                        if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, retry_after) {
                            attempted_keys.retain(|id| id != &key.id);
                            self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    } else if status_code == 401 {
                        match self.recover_stale_antigravity_token(&key, &mut refreshed_keys).await {
                            StaleTokenRecovery::RetrySameKey => {
                                attempted_keys.retain(|id| id != &key.id);
                                last_kind = GatewayErrorKind::AuthInvalid;
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                continue;
                            }
                            StaleTokenRecovery::Recorded(kind) => {
                                last_kind = kind;
                            }
                            StaleTokenRecovery::Passthrough => {
                                last_kind = GatewayErrorKind::AuthInvalid;
                                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                            }
                        }
                    } else if status_code == 403 {
                        let (kind, pool_err) = classify_forbidden(&err_body, retry_after);
                        last_kind = kind;
                        self.pool.record_error(&key.id, pool_err);
                    } else if status_code == 402 {
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted { retry_after });
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                        if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                            attempted_keys.retain(|id| id != &key.id);
                            self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    } else {
                        // Genuine 400s stay terminal; transient geo-gates earn
                        // one bounded same-key retry (see json path).
                        if is_transient_geo_gate(status_code, &err_body) {
                            if let Some(delay) = geo_gate_retry_delay(attempt, &self.pool) {
                                attempted_keys.retain(|id| id != &key.id);
                                self.emit_both(&key.id, attempt_idx, Some(status_code), GatewayErrorKind::ClientBadRequest, last_error.clone(), Some(err_body), attempt_start.elapsed());
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
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
                    if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                        attempted_keys.retain(|id| id != &key.id);
                        self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: max_attempts,
            attempted_keys,
            last_error,
            kind: last_kind,
        })
    }
}

#[cfg(test)]
mod session_header_tests {
    use super::*;
    use crate::pool::{KeyPool, RoutingStrategy};

    fn downstream(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (k, v) in pairs {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn forbidden_exact_tos_signatures_isolate() {
        for body in [
            r#"{"error": {"code": 403, "message": "TERMS_OF_SERVICE_VIOLATION"}}"#,
            "Account suspended for violating Terms of Service",
            "CONSUMER_SUSPENDED",
        ] {
            let (kind, pool_err) = classify_forbidden(body, None);
            assert_eq!(kind, GatewayErrorKind::AuthInvalid, "body: {}", body);
            assert!(
                matches!(pool_err, PoolErrorType::PolicyViolation),
                "body: {}",
                body
            );
        }
    }

    #[test]
    fn forbidden_broad_substrings_do_not_isolate() {
        // Bare "violation"/"suspended" also match safety rejections and
        // other recoverable 403s: they must cool, never burn.
        for body in [
            "prompt violates policy for this request",
            "request suspended by content filter",
            "VIOLATION of usage policy detected in prompt",
        ] {
            let (kind, pool_err) = classify_forbidden(body, None);
            assert!(
                !matches!(pool_err, PoolErrorType::PolicyViolation | PoolErrorType::AuthInvalid),
                "body: {}",
                body
            );
            assert_ne!(kind, GatewayErrorKind::AuthInvalid, "body: {}", body);
        }
    }

    #[test]
    fn forbidden_quota_cools_instead_of_disabling() {
        let (kind, pool_err) = classify_forbidden("RESOURCE_EXHAUSTED #3501 quota exceeded", None);
        assert_eq!(kind, GatewayErrorKind::QuotaExhausted);
        match pool_err {
            PoolErrorType::QuotaExhausted { .. } => {}
            other => panic!("expected quota cooldown, got {:?}", other),
        }
        // Pool-level effect: cooling, not disabled.
        let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        entry.record_failure(pool_err);
        assert_eq!(entry.current_state(), KeyState::CoolingDown);
    }

    #[test]
    fn forbidden_unknown_body_cools_60s() {
        let (kind, pool_err) = classify_forbidden("some new google wording (403)", None);
        assert_eq!(kind, GatewayErrorKind::UpstreamUnavailable);
        match pool_err {
            PoolErrorType::RateLimit { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(60)));
            }
            other => panic!("expected 60s cooling, got {:?}", other),
        }
    }

    #[test]
    fn prefers_opencode_session_over_aliases() {
        let headers = downstream(&[
            ("x-session-affinity", "aff-1"),
            ("x-opencode-session", "ses-1"),
        ]);
        assert_eq!(resolve_upstream_session(&headers), "ses-1");
    }

    #[test]
    fn falls_back_through_alias_priority() {
        let headers = downstream(&[("x-session-id", "sid-1")]);
        assert_eq!(resolve_upstream_session(&headers), "sid-1");
        let headers = downstream(&[("x-pony-session", "pony-ses-1")]);
        assert_eq!(resolve_upstream_session(&headers), "pony-ses-1");
    }

    #[test]
    fn generates_prefixed_id_when_missing_or_blank() {
        let empty = HeaderMap::new();
        let generated = resolve_upstream_session(&empty);
        assert!(generated.starts_with("ponyllm-"), "got {}", generated);
        let blank = downstream(&[("x-opencode-session", "   ")]);
        assert!(resolve_upstream_session(&blank).starts_with("ponyllm-"));
        assert_ne!(
            resolve_upstream_session(&empty),
            resolve_upstream_session(&empty)
        );
    }

    #[test]
    fn zen_scope_gating() {
        // Direct zen base.
        assert!(is_opencode_zen_target(
            "opencode-official",
            "https://opencode.ai/zen/v1/responses"
        ));
        // Forward-proxy zen base under a non-opencode provider name.
        assert!(is_opencode_zen_target(
            "pony-proxy",
            "https://access.ponyjob.top/pony_abc/opencode/zen/v1/responses"
        ));
        // Go endpoints opt out even for opencode providers.
        assert!(!is_opencode_zen_target(
            "opencode-go",
            "https://opencode.ai/zen/go/v1/responses"
        ));
        // Unrelated upstreams never match.
        assert!(!is_opencode_zen_target(
            "sense",
            "https://token.sensenova.cn/v1/chat/completions"
        ));
        assert!(!is_opencode_zen_target("bai", "https://example.com/v1"));
        // Matching is case-insensitive.
        assert!(is_opencode_zen_target(
            "OpenCode-Zen",
            "https://opencode.ai/ZEN/v1/responses"
        ));
    }

    #[test]
    fn build_headers_carries_session_and_own_ua_for_zen() {
        let pool = Arc::new(KeyPool::new("test", RoutingStrategy::RoundRobin));
        let key = ApiKeyEntry::new("k1", "sk-test", 1, 10);
        let executor = UpstreamExecutor::new(pool, 1)
            .with_downstream_headers(&downstream(&[("x-opencode-session", "ses-keep")]))
            .with_opencode_zen(true);
        let headers = futures::executor::block_on(executor.build_headers(&key, None)).unwrap();
        assert_eq!(headers.get("x-opencode-session").unwrap(), "ses-keep");
        assert_eq!(headers.get("x-session-affinity").unwrap(), "ses-keep");
        assert_eq!(headers.get("x-session-id").unwrap(), "ses-keep");
        assert_eq!(headers.get("x-opencode-client").unwrap(), "ponyllm");
        assert_eq!(
            headers.get(USER_AGENT).unwrap().to_str().unwrap(),
            ponyllm_user_agent()
        );

        let pool = Arc::new(KeyPool::new("test", RoutingStrategy::RoundRobin));
        let fallback = UpstreamExecutor::new(pool, 1).with_opencode_zen(true);
        let headers = futures::executor::block_on(fallback.build_headers(&key, None)).unwrap();
        let session = headers
            .get("x-opencode-session")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(session.starts_with("ponyllm-"), "got {}", session);
    }

    #[test]
    fn build_headers_untouched_outside_zen_scope() {
        let pool = Arc::new(KeyPool::new("test", RoutingStrategy::RoundRobin));
        let key = ApiKeyEntry::new("k1", "sk-test", 1, 10);
        // Default executor: no session headers at all (historical wire shape).
        let plain = UpstreamExecutor::new(pool, 1)
            .with_downstream_headers(&downstream(&[("x-opencode-session", "ses-keep")]));
        let headers = futures::executor::block_on(plain.build_headers(&key, None)).unwrap();
        assert!(headers.get("x-opencode-session").is_none());
        assert!(headers.get("x-session-affinity").is_none());
        assert!(headers.get("x-session-id").is_none());
        assert!(headers.get("x-opencode-client").is_none());
        assert_eq!(
            headers.get(USER_AGENT).map(|v| v.to_str().unwrap()),
            None
        );
        // Auth headers still present.
        assert!(headers.get(AUTHORIZATION).is_some());
        assert!(headers.get("x-api-key").is_some());
    }

    #[test]
    fn test_antigravity_headers_and_envelope_request_id_unified() {
        let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::RoundRobin));
        let cred = crate::pool::AntigravityCredential {
            access_token: Some("fake-token-123".to_string()),
            refresh_token: "1//fake-refresh".to_string(),
            client_id: "fake-client".to_string(),
            client_secret: "fake-secret".to_string(),
            project_id: "test-proj".to_string(),
            expiry: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        };
        let tm = Arc::new(crate::pool::AntigravityTokenManager::new("ag-key-1", cred, reqwest::Client::new()));
        let key = ApiKeyEntry::new_antigravity("ag-key-1", tm, 1, 10);

        let executor = UpstreamExecutor::new(pool, 1);
        let expected_req_id = "agent/uuid-1/1700000000/traj-1/1";
        let body = serde_json::json!({
            "requestId": expected_req_id,
            "project": "test-proj"
        });
        let headers = futures::executor::block_on(executor.build_headers(&key, Some(&body))).unwrap();

        assert_eq!(headers.get("requestId").unwrap(), expected_req_id);
        assert_eq!(headers.get("requestType").unwrap(), "agent");
        assert_eq!(headers.get("x-goog-api-client").unwrap(), "gl-node/22.14.0 gdcl/1.1.24");
        assert_eq!(headers.get(USER_AGENT).unwrap(), crate::pool::ANTIGRAVITY_USER_AGENT);
        assert!(headers.get(reqwest::header::ACCEPT).is_some());
    }

    #[test]
    fn test_failover_rewrites_antigravity_project_id_for_selected_key() {
        let cred = crate::pool::AntigravityCredential {
            access_token: Some("fake-tok".to_string()),
            refresh_token: "1//rf".to_string(),
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            project_id: "project-of-key-2".to_string(),
            expiry: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        };
        let tm = Arc::new(crate::pool::AntigravityTokenManager::new("ag-k2", cred, reqwest::Client::new()));
        let key2 = ApiKeyEntry::new_antigravity("ag-k2", tm, 1, 10);

        let pre_serialized_body = serde_json::json!({
            "project": "project-of-key-1",
            "requestId": "agent/u/1/t/1"
        });

        let prepared = UpstreamExecutor::prepare_effective_body(&key2, &pre_serialized_body);
        assert_eq!(prepared["project"], "project-of-key-2");
        assert_eq!(prepared["requestId"], "agent/u/1/t/1");
    }
}
