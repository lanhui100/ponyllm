use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{json, Value};
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
    /// Client-requested model (raw virtual name); survives into attempt frames.
    pub model: Option<String>,
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
    systemone: bool,
    /// Resolved short-window budget (provider default merged with the model
    /// override) plumbed by the routes. `None` = budget dimension disabled
    /// (legacy unlimited behavior). Feeds both the budget-filtered key
    /// selection and the window-exhaustion wait/Retry-After.
    rate_limits: Option<crate::pool::RateLimits>,
    /// Optional TTFB timeout budget for upstream calls.
    /// `Some(duration)` enforces response headers arrive within `duration`.
    /// `None` disables TTFB guard (call bounded only by total timeout).
    ttfb_timeout: Option<Duration>,
    /// Cross-call key exclusion (R2): key ids the outer empty-STOP retry loop
    /// already tried. Pre-seeds `attempted_keys` in the stream executors so a
    /// Priority pool cannot re-select the same key across retries.
    excluded_keys: Vec<String>,
    /// Optional preferred/pinned key id: when specified, key selection attempts
    /// to lock to this specific key (e.g. for per-account in-place retries).
    pinned_key: Option<String>,
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
pub fn summarize_attempt_failures(kinds: &[GatewayErrorKind]) -> String {
    if kinds.is_empty() {
        return String::new();
    }
    let mut network_timeout = 0;
    let mut quota_exhausted = 0;
    let mut rate_limited = 0;
    let mut auth_invalid = 0;
    let mut lock_contention = 0;
    let mut other = 0;

    for k in kinds {
        match k {
            GatewayErrorKind::UpstreamUnavailable => network_timeout += 1,
            GatewayErrorKind::LockContention => lock_contention += 1,
            GatewayErrorKind::QuotaExhausted => quota_exhausted += 1,
            GatewayErrorKind::RateLimitExceeded { .. } => rate_limited += 1,
            GatewayErrorKind::AuthInvalid => auth_invalid += 1,
            _ => other += 1,
        }
    }

    let mut parts = Vec::new();
    if network_timeout > 0 {
        parts.push(format!("{} timeout/network", network_timeout));
    }
    if lock_contention > 0 {
        parts.push(format!("{} lock busy/contention", lock_contention));
    }
    if quota_exhausted > 0 {
        parts.push(format!("{} quota exhausted", quota_exhausted));
    }
    if rate_limited > 0 {
        parts.push(format!("{} rate limited", rate_limited));
    }
    if auth_invalid > 0 {
        parts.push(format!("{} auth invalid", auth_invalid));
    }
    if other > 0 {
        parts.push(format!("{} other error", other));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" (failures: {})", parts.join(", "))
    }
}

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
///
/// For OpenCode Zen targets, upstream strict regex validator requires `ses_` + exactly 26 chars!
pub fn new_upstream_session_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("ses_{}", &raw[..26])
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

/// Upper bound for the transparent pool-wait on full window exhaustion.
///
/// Far smaller than the downstream DSH stream idle timeout (~300s), so a
/// gateway that holds the request while the pool's per-minute window breathes
/// never trips the client's stream timeout. Beyond this bound the gateway
/// gives up and answers 429 + honest Retry-After instead.
pub const DEFAULT_POOL_WAIT_MAX: Duration = Duration::from_secs(90);

/// Pool-level failover backoff for **multi-key** pools: after a 429 records
/// an error on one key, pause briefly before switching to the next key so a
/// per-minute window shared across the account's keys is not swept in
/// milliseconds (the observed 17-30× amplification on sense/deepseek-v4-flash).
///
/// Bounded to `min(earliest_unlock, 2s)` — a healthy candidate is still
/// reached in ~2s while the window breathes, and a key that unlocks sooner is
/// only paused until it does. Singleton pools keep their own
/// [`transient_retry_delay`] semantics and return `None` here.
fn pool_failover_backoff(pool: &KeyPool) -> Option<Duration> {
    if pool.total_key_count() <= 1 {
        return None;
    }
    Some(
        pool.earliest_unlock()
            .map(|d| d.min(Duration::from_secs(2)))
            .unwrap_or(Duration::from_millis(1200)),
    )
}

/// Per-attempt short-window meter guard: admits the attempt into the key's
/// window AND in-flight/concurrency slot at construction, then releases the
/// slot and settles token usage exactly once at drop, whatever the exit path
/// (success return / failover continue / terminal error).
///
/// Request/token accounting is split to close the RPM TOCTOU: the request is
/// counted at admission (`record_attempt(0)`, so a concurrent select on the
/// same key immediately sees the consumed RPM slot) while `tokens` is filled
/// by the JSON success path from the upstream `usage` and settled via
/// [`ShortWindowMeter::add_tokens`] (token-only, no extra request). The stream
/// path has no usage at this layer (routes record tokens via `record_tokens`
/// into the long-window tracker), so it stays `tokens = 0` — RPM and
/// concurrency are still accurately accounted for every attempt.
struct AttemptMeterGuard<'a> {
    meter: &'a crate::pool::meter::ShortWindowMeter,
    /// Token usage reported by THIS attempt (0 = request-only accounting).
    tokens: u64,
}

impl<'a> AttemptMeterGuard<'a> {
    /// Admit one attempt: count the request into the window now (RPM slot
    /// consumed immediately — no admission-to-settlement gap) and take the
    /// key's in-flight/concurrency slot.
    fn admit(meter: &'a crate::pool::meter::ShortWindowMeter) -> Self {
        meter.record_attempt(0);
        meter.in_flight_inc();
        Self { meter, tokens: 0 }
    }
}

impl Drop for AttemptMeterGuard<'_> {
    fn drop(&mut self) {
        self.meter.in_flight_dec();
        if self.tokens > 0 {
            self.meter.add_tokens(self.tokens);
        }
    }
}

/// Best-effort token count from an upstream JSON success body, across the
/// common wire shapes (OpenAI chat/responses `usage.total_tokens`,
/// Anthropic `usage.input_tokens`+`output_tokens`, prompt/completion split).
/// `0` when the shape is unrecognized or absent.
fn extract_response_tokens(body: &Value) -> u64 {
    let Some(usage) = body.get("usage") else {
        return 0;
    };
    if let Some(t) = usage.get("total_tokens").and_then(Value::as_u64) {
        return t;
    }
    let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
        _ => None,
    };
    if let Some(t) = sum(
        usage.get("input_tokens").and_then(Value::as_u64),
        usage.get("output_tokens").and_then(Value::as_u64),
    ) {
        return t;
    }
    if let Some(t) = sum(
        usage.get("prompt_tokens").and_then(Value::as_u64),
        usage.get("completion_tokens").and_then(Value::as_u64),
    ) {
        return t;
    }
    0
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

/// Google account-verification拦截签名（403 VALIDATION_REQUIRED）：账号本身
/// 需要人工验证（"Verify your account to continue"），与额度耗尽、ToS 封号
/// 都不是同一信号。公开给管理面探测路径复用，保证"请求失败"与"拨测失败"
/// 对同一 body 判定一致。
const ACCOUNT_VALIDATION_SIGNATURES: &[&str] = &[
    "validation_required",
    "validationrequired",
    "verify your account",
];

pub fn is_account_validation_required(err_body: &str) -> bool {
    let lower = err_body.to_lowercase();
    ACCOUNT_VALIDATION_SIGNATURES
        .iter()
        .any(|sig| lower.contains(sig))
}

/// Google 账号/产品资格阻塞签名（403 "not eligible for"，reason=RESTRICTED_AGE）：
/// 账号当前被上游判定无某产品资格（观测例：Gemini Code Assist "must be 18 years
/// old or older"）。刻意收窄到**账号级实体锚点**——只有"your (current) account is
/// not eligible"或上游自己的 `restricted_age` 状态码才算数；模型/项目级或
/// prompt 级 "not eligible" 措辞（无账号实体）若误判会把整个账号冻 3 天，
/// 让其它可服务模型停摆。与 VALIDATION_REQUIRED（需人工验证）不同：资格问题
/// 没有"完成验证"动作，只能等上游状态变化，故走长冷冻而非永久隔离。
const ACCOUNT_ELIGIBILITY_SIGNATURES: &[&str] = &[
    "your current account is not eligible for",
    "your account is not eligible for",
    "your current account is not eligible",
    "restricted_age",
];

pub fn is_account_eligibility_revoked(err_body: &str) -> bool {
    let lower = err_body.to_lowercase();
    ACCOUNT_ELIGIBILITY_SIGNATURES
        .iter()
        .any(|sig| lower.contains(sig))
}

/// Classify a 403 body into (gateway kind, pool action).
///
/// - Exact ToS death signature → permanent `PolicyViolation` isolate
///   (still guarded by the pool mass-disable breaker).
/// - Account/product-eligibility signature (`"not eligible for"` /
///   `RESTRICTED_AGE`, account-anchored) → long `AccountEligibility` freeze:
///   the account is frozen for days and the pool routes around it (see
///   2026-10-04 upstream-eligibility-403-freeze ADR). Checked BEFORE the
///   validation branch: Antigravity sometimes returns the same "not eligible"
///   condition with `"status":"VALIDATION_REQUIRED"` (observed externally),
///   and freezing is self-healing while permanent isolation is not.
/// - Account-validation signature (`VALIDATION_REQUIRED`) → permanent
///   `AccountValidationRequired` isolate: needs human verification, never
///   auto-recovers by waiting for a quota window.
/// - Quota wording → `QuotaExhausted` kind for honest downstream errors,
///   but only a cooldown on the pool: real quota recovers at resetTime
///   and throttling clears on its own (P0-2).
/// - Unknown 403 → 60s cooling + warning. A new Google wording, locale
///   variant, or WAF flap must never burn a credential on first sight.
pub fn classify_forbidden(
    err_body: &str,
    retry_after: Option<Duration>,
) -> (GatewayErrorKind, PoolErrorType) {
    let lower = err_body.to_lowercase();
    if is_tos_account_death(&lower) {
        (GatewayErrorKind::AuthInvalid, PoolErrorType::PolicyViolation)
    } else if is_account_eligibility_revoked(err_body) {
        // Account/product-eligibility 403 (observed: Gemini Code Assist
        // "Your current account is not eligible for ..." on Antigravity,
        // reason=RESTRICTED_AGE). Not a 60s blip (that hammered the account
        // every minute and exhausted whole pools mid-request) and not a
        // permanent isolate: freeze the account for days so scheduling routes
        // around it and the agent run keeps going on the next available key.
        (
            GatewayErrorKind::AuthInvalid,
            PoolErrorType::AccountEligibility {
                reason: Some(err_body.to_string()),
            },
        )
    } else if is_account_validation_required(err_body) {
        (
            GatewayErrorKind::AuthInvalid,
            PoolErrorType::AccountValidationRequired,
        )
    } else if is_zen_free_tier_gate_body(err_body) {
        // OpenCode zen free-tier gate (`FreeTierError`): only the official
        // client may use `*-free` models, so a proxy replay that trips this
        // gate is not a transient blip — cool as quota (honest
        // `quota_exhausted` signal, boundary guard stops failover).
        (
            GatewayErrorKind::QuotaExhausted,
            PoolErrorType::QuotaExhausted {
                retry_after: retry_after.or(Some(Duration::from_secs(900))),
            },
        )
    } else if body_has_rate_limit_signal(err_body) {
        // Transient rate-limit signal wins over quota wording, mirroring the
        // 429 path: Sense/商汤 mislabels RPM/TPM rejections as
        // `type: "quota_exceeded_error"` even on 403, and treating those as
        // account quota exhaustion would cool the key for ~15 minutes and
        // shut down pool failover for a window that recovers on its own.
        (
            GatewayErrorKind::RateLimitExceeded { retry_after },
            PoolErrorType::RateLimit { retry_after },
        )
    } else if is_balance_exhausted_body(err_body) {
        // Billing terminal (balance/credit/budget wording) is quota-shaped:
        // it never recovers by waiting for a sliding window.
        (
            GatewayErrorKind::QuotaExhausted,
            PoolErrorType::QuotaExhausted {
                retry_after: retry_after.or(Some(Duration::from_secs(900))),
            },
        )
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

/// Classify a probe-obtained upstream failure into the SAME pool action the
/// request path would apply for *authoritative account signals*, so backend
/// probes (keepalive quota refresh, admin refresh/dial-test) freeze an
/// eligibility-rejected account just like a live request would.
///
/// Deliberately narrower than the request path:
/// - Only **deterministic hard signals** are acted on: `AccountEligibility`
///   (long freeze), `AccountValidationRequired` / `PolicyViolation` (isolate).
/// - Soft signals (quota/rate-limit cooldowns) and the unknown-403 60s
///   fallback return `None`: a probe targeting `fetchAvailableModels` must
///   never cool a healthy key over a WAF/HTML/scope 403 or a quota-shaped
///   blip — probes are observability, not the traffic path (the observed
///   2026-10-04 `quota_probe_failed` was exactly such a transport misread).
pub fn classify_probe_failure(status: u16, body: &str) -> Option<PoolErrorType> {
    if status != 403 {
        return None;
    }
    let (_, pool_err) = classify_forbidden(body, None);
    match &pool_err {
        PoolErrorType::AccountEligibility { .. }
        | PoolErrorType::AccountValidationRequired
        | PoolErrorType::PolicyViolation => Some(pool_err),
        _ => None,
    }
}

/// Parse the human-readable reset hint Google embeds in quota bodies,
/// e.g. `"Resets in 15h21m26s."`. Compact `<d>d<hh>h<mm>m<ss>s` groups are
/// accepted in any combination/order; a bare number without a unit is not a
/// match (it is some other "resets in 5 ..." sentence), and only the compact
/// ASCII form is recognised — word forms ("2 days") and full-width digits are
/// intentionally not parsed, so callers fall back to the conservative default
/// instead of guessing.
pub fn parse_reset_duration(body: &str) -> Option<Duration> {
    let lower = body.to_ascii_lowercase();
    let marker = "resets in";
    let start = lower.find(marker)? + marker.len();
    let tail = &lower[start..];
    let mut chars = tail.chars().peekable();
    let mut total_secs: u64 = 0;
    let mut matched = false;
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let mut digits = String::new();
        while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
            digits.push(chars.next().expect("peeked digit"));
        }
        if digits.is_empty() {
            break;
        }
        let unit = match chars.peek().copied() {
            Some('d') => 86_400u64,
            Some('h') => 3_600,
            Some('m') => 60,
            Some('s') => 1,
            // Number not followed by a unit: not the Google reset shape.
            _ => break,
        };
        chars.next();
        // A garbled later group invalidates the whole hint: a partial value
        // (e.g. "1h999999999999999999999s") must not become a real cooldown.
        let value: u64 = digits.parse().ok()?;
        total_secs = total_secs.saturating_add(value.saturating_mul(unit));
        matched = true;
    }
    if matched && total_secs > 0 {
        Some(Duration::from_secs(total_secs))
    } else {
        None
    }
}

/// Whether a failure body carries a transient per-minute rate-limit signal:
/// `rate_limit` / `rate limit`, the `rpm`/`tpm`/`qps` tokens, per-minute or
/// per-second phrasing, or concurrency limits.
///
/// Shared by the 429 and 403 classifiers: these signals must never be read as
/// account quota exhaustion, even when the upstream provider errantly labels
/// the rejection `type: "quota_exceeded_error"` (observed on Sense/商汤 RPM
/// limits). `_` is kept inside a token so `rpm_user`/`corp_tpm` identifiers
/// stay intact, while `/` splits `tpm/rpm` into two rate-limit words.
pub fn body_has_rate_limit_signal(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("requests per minute")
        || lower.contains("tokens per minute")
        || lower.contains("queries per second")
        || lower.contains("concurrency")
        || lower
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| matches!(word, "rpm" | "tpm" | "qps"))
}

/// Whether a 429 body is account/model quota exhaustion rather than a
/// transient rate limit. Google's cloudcode-pa returns
/// `"status": "RESOURCE_EXHAUSTED"` / `reason: "QUOTA_EXHAUSTED"` with
/// `"Individual quota reached. ... Resets in 15h21m26s."`, and treating that
/// as a 3-second rate-limit blip is what keeps hammering a closed window.
///
/// Deliberately narrow: per-minute TPM/RPM bodies ("TPM quota exceeded") carry
/// neither the reason code nor a long reset window and must keep the short
/// rate-limit path. Only a quota body with an advertised reset of 5+ minutes
/// is treated as a windowed quota even without the explicit reason code.
///
/// Transient rate-limit signals are detected FIRST via
/// [`body_has_rate_limit_signal`], because upstreams such as Sense/商汤
/// mislabel an RPM rejection as `type: "quota_exceeded_error"` while the
/// `message` only says `"rpm exhausted"`.
pub fn is_quota_exhausted_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    if body_has_rate_limit_signal(body) {
        return false;
    }

    lower.contains("quota_exhausted")
        || lower.contains("individual quota")
        || (lower.contains("quota")
            && parse_reset_duration(body)
                .map(|d| d >= Duration::from_secs(300))
                .unwrap_or(false))
}

/// OpenCode zen free-tier *usage-limit* body (`FreeUsageLimitError`, 429 from
/// the Console/proxy). Despite the `"Rate limit exceeded"` message, this is a
/// windowed *usage quota* (recovers at the Console window reset), not a
/// transient per-request rate limit. Classifying it as `RateLimit` was what
/// escalated every zen key into 5m→2h cooldowns and surfaced a misleading
/// gateway-side `rate_limit_exceeded` to clients while the upstream had only
/// closed its free window. Matched by the upstream's own type name so generic
/// `"rate limit"` wording on other upstreams is untouched.
pub fn is_zen_free_usage_limit_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("freeusagelimiterror") || lower.contains("free usage limit")
}

/// OpenCode zen free-tier *gate* rejection (`FreeTierError`, 403): the Console
/// only serves `*-free` models to requests that look like the official
/// OpenCode client. A proxied replay that trips this gate does not recover by
/// waiting for a window — cool it as quota so the pool surfaces an honest
/// `quota_exhausted` signal (and the quota boundary guard stops cross-provider
/// failover) instead of cycling generic unknown-403 60s rate-limit cooldowns.
pub fn is_zen_free_tier_gate_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("freetiererror") || lower.contains("free tier can only be used")
}

/// Classify a 429 into (gateway kind, pool action).
///
/// - OpenCode zen free usage-limit (`FreeUsageLimitError`) → `QuotaExhausted`
///   even though the message says "Rate limit exceeded": the free window is
///   closed, and the quota boundary guard must stop cross-provider failover.
/// - Quota wording → `QuotaExhausted` cooling for the advertised reset (body
///   `Resets in ...` first, then the `Retry-After` header, then the pool's
///   conservative default). No same-key transient retry: the window is closed.
/// - Anything else → `RateLimit`, preserving the legacy short
///   exponential/jittered cooldown and the singleton transient retry.
fn classify_too_many_requests(
    err_body: &str,
    retry_after: Option<Duration>,
) -> (GatewayErrorKind, PoolErrorType) {
    if is_zen_free_usage_limit_body(err_body) {
        return (
            GatewayErrorKind::QuotaExhausted,
            PoolErrorType::QuotaExhausted { retry_after },
        );
    }
    let body_reset = parse_reset_duration(err_body);
    if is_quota_exhausted_body(err_body) {
        (
            GatewayErrorKind::QuotaExhausted,
            PoolErrorType::QuotaExhausted {
                retry_after: body_reset.or(retry_after),
            },
        )
    } else {
        let retry_after = retry_after.or(body_reset);
        (
            GatewayErrorKind::RateLimitExceeded { retry_after },
            PoolErrorType::RateLimit { retry_after },
        )
    }
}

/// Whether a failure body (429/402/403) means the *account balance* is gone
/// rather than a time-window being closed. Balance exhaustion never recovers
/// by waiting for a sliding window, so the transparent pool-wait must be
/// suppressed: fail fast with the existing classification.
///
/// Deliberately broad token matching ("balance"/"credit"/"budget"/"payment
/// required") — these only run against upstream error bodies, where a hit is
/// far more likely a billing signal than an innocent identifier.
pub fn is_balance_exhausted_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("balance")
        || lower.contains("credit")
        || lower.contains("budget")
        || lower.contains("payment required")
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

/// Whether this routed target hits the zen free tier, whose Console gate
/// rejects non-stream upstream bodies outright (`FreeTierError` even with
/// the tool gate satisfied — verified 2026-09-19). Routes must force an
/// upstream stream for these targets and aggregate the SSE downstream.
/// Chat/Responses upstream protocols only: the aggregation collectors cover
/// those two wire shapes.
pub fn zen_free_tier_forces_upstream_stream(provider_name: &str, target_url: &str, physical_model: &str) -> bool {
    is_opencode_zen_target(provider_name, target_url) && physical_model.ends_with("-free")
}

/// OpenCode zen free-tier body gate: since 2026-09-17 the Console rejects
/// `*-free` models with `FreeTierError` unless the request body itself
/// carries opencode's agent tool set. Wire probe (2026-09-19): the full
/// 12-name tool list passes on `/chat/completions`, `/responses` and
/// `/messages`; header/UA/TLS replication alone does not; a single-name
/// subset does not; tool schemas are irrelevant (stub objects pass).
pub const OPENCODE_ZEN_TOOL_NAMES: [&str; 12] = [
    "bash", "edit", "glob", "google_search", "grep", "read", "skill", "task", "todowrite",
    "webfetch", "websearch", "write",
];

/// Wire shapes the zen free-tier gate accepts for the tool list, selected by
/// the upstream endpoint path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZenToolWire {
    /// `{type:"function",function:{name,...}}` — `/chat/completions`.
    OpenAiChat,
    /// `{type:"function",name,...}` — `/responses`.
    OpenAiResponses,
    /// `{name,input_schema,...}` — `/messages`.
    AnthropicMessages,
}

fn zen_tool_wire(url: &str) -> ZenToolWire {
    let url = url.to_ascii_lowercase();
    if url.contains("/responses") {
        ZenToolWire::OpenAiResponses
    } else if url.contains("/messages") {
        ZenToolWire::AnthropicMessages
    } else {
        ZenToolWire::OpenAiChat
    }
}

/// Whether the upstream body targets a zen free model (`-free` suffix).
/// Paid zen models keep the historical no-injection wire.
fn zen_body_requests_free_model(body: &Value) -> bool {
    body.get("model")
        .and_then(|v| v.as_str())
        .map(|m| m.ends_with("-free"))
        .unwrap_or(false)
}

/// Tool names already present in the body, across all three wire shapes
/// (OpenAI chat nests the name under `function`, Responses and Anthropic
/// carry a flat `name`).
fn zen_existing_tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    t.get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| {
                            t.get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                        })
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn zen_missing_tool_names(body: &Value) -> Vec<&'static str> {
    let existing = zen_existing_tool_names(body);
    OPENCODE_ZEN_TOOL_NAMES
        .iter()
        .filter(|n| !existing.iter().any(|e| e == *n))
        .copied()
        .collect()
}

fn zen_stub_tool(name: &str, wire: ZenToolWire) -> Value {
    match wire {
        ZenToolWire::OpenAiChat => json!({
            "type": "function",
            "function": {"name": name, "description": "opencode agent tool", "parameters": {"type": "object", "properties": {}}}
        }),
        ZenToolWire::OpenAiResponses => json!({
            "type": "function",
            "name": name,
            "description": "opencode agent tool",
            "parameters": {"type": "object", "properties": {}}
        }),
        ZenToolWire::AnthropicMessages => json!({
            "name": name,
            "description": "opencode agent tool",
            "input_schema": {"type": "object", "properties": {}}
        }),
    }
}

/// Default total wall-clock budget for one upstream call: 20 minutes.
/// Long-thinking streams routinely exceed the legacy 120s budget; the
/// gateway complements it with TTFB + tail-stall detection so a genuinely
/// dead stream still fails fast instead of pinning the connection for the
/// whole budget.
pub const DEFAULT_UPSTREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(1200);

/// TTFB guard: the upstream must deliver response headers within this budget
/// or the attempt is judged dead (TRANSPORT) and failover kicks in. Far
/// smaller than the total budget because a healthy provider starts emitting
/// headers in seconds, while the *body* may legitimately stream for 20 min.
/// Sanitize a proxy URL for safe logging and display, redacting any username/password.
pub fn sanitize_proxy_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.contains("://") {
        if let Ok(mut url) = reqwest::Url::parse(trimmed) {
            if url.password().is_some() {
                let _ = url.set_password(Some("***"));
            }
            if !url.username().is_empty() {
                let _ = url.set_username("***");
            }
            return url.to_string();
        }
    }
    if let Some((_userinfo, host)) = trimmed.split_once('@') {
        format!("***@{}", host)
    } else {
        trimmed.to_string()
    }
}

pub const DEFAULT_UPSTREAM_TTFB_TIMEOUT: Duration = Duration::from_secs(90);

/// Create an optimized, connection-pooled HTTP client for upstream LLM providers.
/// Enables TCP nodelay, Keep-Alive probing, and idle connection reuse to minimize TTFT.
/// By default, disables system environment proxies (`http_proxy`/`https_proxy`) to isolate
/// the gateway from ambient terminal proxy environments.
pub fn create_upstream_http_client() -> reqwest::Client {
    create_upstream_http_client_with_timeout(None, false, DEFAULT_UPSTREAM_TOTAL_TIMEOUT)
}

/// Create an upstream HTTP client with optional explicit proxy URL and system proxy inheritance flag (returns Result).
pub fn try_create_upstream_http_client_with_options(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
) -> std::result::Result<reqwest::Client, String> {
    try_create_upstream_http_client_with_timeout(proxy_url, use_system_proxy, DEFAULT_UPSTREAM_TOTAL_TIMEOUT)
}

/// Variant with an explicit total-budget override (per-gateway/provider/model).
pub fn try_create_upstream_http_client_with_timeout(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
    total_timeout: Duration,
) -> std::result::Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(total_timeout)
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        // VULN-07/F6: the data plane must never follow upstream redirects —
        // an attacker-controllable public base_url that passes the egress
        // gate could otherwise 302 (or rebind) to a metadata/private target
        // AFTER the check. Redirects surface as errors, same as probes.
        .redirect(reqwest::redirect::Policy::none());

    if !use_system_proxy {
        builder = builder.no_proxy();
    }

    if let Some(proxy_str) = proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            let proxy = reqwest::Proxy::all(trimmed)
                .map_err(|e| format!("无法解析代理地址 '{}': {}", sanitize_proxy_url(trimmed), e))?;
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
    create_upstream_http_client_with_timeout(proxy_url, use_system_proxy, DEFAULT_UPSTREAM_TOTAL_TIMEOUT)
}

/// Non-fallible variant with an explicit total-budget override.
pub fn create_upstream_http_client_with_timeout(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
    total_timeout: Duration,
) -> reqwest::Client {
    match try_create_upstream_http_client_with_timeout(proxy_url, use_system_proxy, total_timeout) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to create configured proxy client, falling back to default");
            // VULN-07/F6: the fallback client must carry the same redirect
            // discipline as the primary builder.
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default()
        }
    }
}

/// Probe-only variant of the upstream client (H2 red-team B2): same proxy
/// semantics as [`create_upstream_http_client_with_options`] (so the probe
/// shares the data plane's egress IP — P0-7 consistency), but with NO
/// redirect following and short timeouts. An attacker-controlled public
/// `base_url` that passes the egress gate must not 302 to a
/// metadata/private target afterwards; redirect attempts surface as errors
/// instead of being followed.
pub fn create_probe_http_client_with_options(proxy_url: Option<&str>) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none());
    let builder = match proxy_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(trimmed) => match reqwest::Proxy::all(trimmed) {
            Ok(proxy) => {
                let proxy =
                    proxy.no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1"));
                builder.proxy(proxy)
            }
            Err(e) => {
                tracing::warn!(error = %e, proxy = %sanitize_proxy_url(trimmed), "Invalid probe proxy, probing direct");
                builder.no_proxy()
            }
        },
        None => builder.no_proxy(),
    };
    builder.build().unwrap_or_default()
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
            systemone: false,
            rate_limits: None,
            ttfb_timeout: Some(DEFAULT_UPSTREAM_TTFB_TIMEOUT),
            excluded_keys: Vec::new(),
            pinned_key: None,
        }
    }

    /// Pin key selection to a specific key id (e.g. for single-account in-place retry).
    pub fn with_pinned_key(mut self, key_id: Option<String>) -> Self {
        self.pinned_key = key_id;
        self
    }

    /// Pre-exclude key ids from selection (R2): the outer Antigravity
    /// empty-STOP retry loop passes keys it already tried so this executor's
    /// stream calls fail over to fresh keys instead of re-selecting them.
    pub fn with_excluded_keys(mut self, excluded: &[String]) -> Self {
        self.excluded_keys = excluded.to_vec();
        self
    }

    /// Adopt the resolved short-window budget (provider + model `rate_limits`)
    /// for budget-filtered key selection and window-exhaustion handling.
    /// `None` keeps the legacy unlimited behavior.
    pub fn with_rate_limits(mut self, limits: Option<crate::pool::RateLimits>) -> Self {
        self.rate_limits = limits;
        self
    }

    /// Opt into the opencode zen session treatment for this executor.
    /// Routes compute the flag with [`is_opencode_zen_target`] from the
    /// resolved provider + target URL; everything else stays untouched.
    pub fn with_opencode_zen(mut self, enabled: bool) -> Self {
        self.opencode_zen = enabled;
        self
    }

    pub fn with_systemone(mut self, enabled: bool) -> Self {
        self.systemone = enabled;
        self
    }

    /// Adopt an explicit TTFB timeout budget for upstream calls.
    /// `Some(duration)` enforces that response headers arrive within `duration`.
    /// `None` disables the TTFB guard (upstream call only bounded by total timeout).
    pub fn with_ttfb_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.ttfb_timeout = timeout;
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

    /// Send one upstream request guarded by the TTFB budget: the response
    /// headers must arrive within [`self.ttfb_timeout`] or the
    /// attempt is judged dead. The reqwest client itself still owns the
    /// larger *total* budget (20 min default) that covers the whole body
    /// stream; this guard only cuts the "provider never answered" case short
    /// so failover happens in seconds, not minutes. If `self.ttfb_timeout` is None,
    /// the TTFB guard is disabled and the request is only bounded by the client total timeout.
    async fn send_guarded(
        &self,
        req: reqwest::RequestBuilder,
    ) -> std::result::Result<reqwest::Response, String> {
        if let Some(timeout) = self.ttfb_timeout {
            match tokio::time::timeout(timeout, req.send()).await {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_elapsed) => Err(format!(
                    "upstream TTFB timeout after {:?} (no response headers)",
                    timeout
                )),
            }
        } else {
            match req.send().await {
                Ok(r) => Ok(r),
                Err(e) => Err(e.to_string()),
            }
        }
    }

    /// Transparent pool-wait hook for full-pool exhaustion.
    ///
    /// Called when `select_key_excluding_with_limits` can no longer produce a
    /// key. The transparent hold is **budget-driven only**: with no
    /// `rate_limits` configured (`None`, legacy), a full-pool `NoAvailableKey`
    /// is pure cooldown and must fail fast so cross-provider failover is not
    /// delayed by a hold of up to [`DEFAULT_POOL_WAIT_MAX`]. With a budget
    /// configured, when the exhaustion is *window-shaped* (per-minute/quota/
    /// cooldown — recoverable by waiting; see
    /// [`KeyPool::exhausted_by_window_with_limits`]) and not balance/auth,
    /// hold the request for at most [`DEFAULT_POOL_WAIT_MAX`] until the pool's
    /// earliest refill/unlock ([`KeyPool::window_refill_in_with_limits`]), then
    /// allow exactly one fresh rescue pass (`attempted_keys` cleared,
    /// `pool_wait_done` set so the loop cannot spin). When the needed wait
    /// exceeds the cap — or the exhaustion is not window-shaped (all keys
    /// permanently disabled) — return `false` so the caller fails immediately
    /// with 429 + honest Retry-After.
    ///
    /// Returns `true` when the caller should `continue` the retry loop after
    /// a bounded sleep.
    async fn maybe_window_wait(
        &self,
        pool_wait_done: &mut bool,
        attempted_keys: &mut Vec<String>,
        balance_exhausted: bool,
    ) -> bool {
        if balance_exhausted || *pool_wait_done {
            return false;
        }
        // Only budget-driven exhaustion is waitable. With `rate_limits = None`
        // (pure cooldown / unconfigured budget) keep the legacy fail-fast:
        // a hold here would delay cross-provider failover up to
        // DEFAULT_POOL_WAIT_MAX for a state that only cooldown expiry fixes.
        if self.rate_limits.is_none() {
            return false;
        }
        // Only window/cooldown exhaustion is waitable; an all-disabled
        // (auth) pool reports false here and fails immediately.
        if !self
            .pool
            .exhausted_by_window_with_limits(self.rate_limits.as_ref())
        {
            return false;
        }
        let hold = self
            .pool
            .window_refill_in_with_limits(self.rate_limits.as_ref())
            .or_else(|| self.pool.earliest_unlock());
        let Some(hold) = hold else {
            // No key can ever refill (all disabled): fail immediately.
            return false;
        };
        if hold > DEFAULT_POOL_WAIT_MAX {
            return false;
        }
        tracing::info!(
            provider = %self.pool.provider,
            hold_ms = hold.as_millis(),
            wait_max_ms = DEFAULT_POOL_WAIT_MAX.as_millis(),
            "pool window-exhausted: transparent wait then single rescue retry"
        );
        tokio::time::sleep(hold).await;
        *pool_wait_done = true;
        attempted_keys.clear();
        attempted_keys.extend(self.excluded_keys.clone());
        true
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
            let mut st = ctx.stages.lock();
            st.upstream_ttfb_ms = Some(ttfb_ms);
            // Default upstream_ttft_ms to upstream_ttfb_ms until first stream token arrives
            if st.upstream_ttft_ms.is_none() {
                st.upstream_ttft_ms = Some(ttfb_ms);
            }
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
            Err(CoreError::RefreshSkipped { .. }) => {
                // Another replica holds the refresh serialization lock: the
                // token is being refreshed (and persisted) right now.
                // Invalidate our stale in-memory token so subsequent calls don't reuse it.
                // Do NOT retry the same key immediately in this request with the stale token
                // (which would 401 again and falsely trip the Passthrough AuthInvalid death penalty),
                // and do NOT cool this healthy key. Fail over to other candidate keys with LockContention.
                mgr.invalidate_token();
                tracing::warn!(key_id = %key.id, "Antigravity forced refresh skipped (lock held by another replica); failing over to next candidate key");
                StaleTokenRecovery::Recorded(GatewayErrorKind::LockContention)
            }
            Err(CoreError::AuthInvalid { reason, .. }) => {
                // refresh_token burned (invalid_grant): permanent isolate,
                // still guarded by the pool mass-disable breaker.
                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid { reason: Some(reason.clone()) });
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

    /// Zen free-tier body gate: append opencode's agent tool set to
    /// `*-free` model requests so the Console free-tier check passes.
    /// Downstream tools are preserved; only the missing opencode names are
    /// appended, so the patch is idempotent across per-key retries. No-op
    /// outside the zen scope, for paid models, and once the tool set is
    /// already complete.
    fn inject_zen_free_tier_tools<'a>(
        &self,
        url: &str,
        body: std::borrow::Cow<'a, Value>,
    ) -> std::borrow::Cow<'a, Value> {
        // System One is already a native structured-decision wire and must stay
        // byte-for-byte opaque; the chat/responses free-tier tool gate does not
        // apply to `/systemone`.
        if !self.opencode_zen
            || url.to_ascii_lowercase().contains("/systemone")
            || !zen_body_requests_free_model(body.as_ref())
        {
            return body;
        }
        let missing = zen_missing_tool_names(body.as_ref());
        if missing.is_empty() {
            return body;
        }
        let wire = zen_tool_wire(url);
        let mut patched = body.into_owned();
        let Some(obj) = patched.as_object_mut() else {
            return std::borrow::Cow::Owned(patched);
        };
        let tools = obj.entry("tools".to_string()).or_insert_with(|| Value::Array(Vec::new()));
        if let Some(arr) = tools.as_array_mut() {
            for name in missing {
                arr.push(zen_stub_tool(name, wire));
            }
        }
        if wire == ZenToolWire::OpenAiChat && !obj.contains_key("tool_choice") {
            obj.insert("tool_choice".to_string(), Value::String("auto".to_string()));
        }
        std::borrow::Cow::Owned(patched)
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

        // systemone only needs Bearer + Zen client headers; do not send
        // Anthropic x-api-key or anthropic-version to a custom endpoint.
        if self.systemone {
            let bearer_val = HeaderValue::from_str(&format!("Bearer {}", clean_key))
                .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
            headers.insert(AUTHORIZATION, bearer_val);
        } else {
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            let bearer_val = HeaderValue::from_str(&format!("Bearer {}", clean_key))
                .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
            headers.insert(AUTHORIZATION, bearer_val);
            let x_api_val = HeaderValue::from_str(clean_key)
                .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
            headers.insert("x-api-key", x_api_val);
        }


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
            // OpenCode zen endpoints require an official OpenCode client User-Agent
            // (FreeTierError otherwise for free models, e.g. >=1.17.0).
            let ua_str = "opencode/1.18.31 (Linux; x64)";
            let ua_val = HeaderValue::from_static(ua_str);
            headers.insert(USER_AGENT, ua_val);
        }

        Ok(headers)
    }

    /// Execute a JSON request with transparent automatic failover before response body starts
    pub async fn execute_json_request(&self, url: &str, body: &Value) -> Result<Value> {
        let (val, _key_id) = self.execute_json_request_with_key(url, body).await?;
        Ok(val)
    }

    /// Execute a JSON request with transparent automatic failover and return (response_val, winning_key_id)
    pub async fn execute_json_request_with_key(&self, url: &str, body: &Value) -> Result<(Value, String)> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;
        let mut attempted_keys = Vec::new();
        let mut attempt_kinds = Vec::new();
        // Antigravity keys already force-refreshed once this request (P0-3
        // stale-token recovery): a second 401 on the same key is genuine.
        let mut refreshed_keys: Vec<String> = Vec::new();
        // 402 / balance-wording upstream body: waiting for a window cannot
        // fix an empty balance, so full-pool exhaustion must fail fast.
        let mut balance_exhausted = false;
        // Transparent-wait guard: at most one bounded hold on full pool
        // window exhaustion, so the rescue retry cannot spin forever.
        let mut pool_wait_done = false;

        let max_attempts = self.max_retries.max(self.pool.total_key_count()).max(1);

        for attempt in 0..max_attempts {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let affinity_seed = body.get("messages")
                .and_then(|m| m.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("content").and_then(|c| c.as_str()))
                .and_then(crate::pool::hot_cache::PrefixFingerprint::compute)
                .map(|fp| fp.as_u64());
            // Antigravity quota-group aware selection: the requested model's
            // family decides which quota group (Gemini vs Claude/GPT) must have
            // headroom for this key to be schedulable.
            let quota_family = body
                .get("model")
                .and_then(|m| m.as_str())
                .and_then(crate::pool::entry::classify_quota_family);
            let key = match self.pool.select_key_with_affinity_for_family(affinity_seed, &attempted_keys, self.rate_limits.as_ref(), quota_family) {
                Ok(k) => k,
                Err(e) => {
                    // Full-pool exhaustion: when window-shaped (per-minute/quota,
                    // not balance/auth-disabled), transparently wait up to
                    // DEFAULT_POOL_WAIT_MAX then retry the pool once.
                    if self
                        .maybe_window_wait(&mut pool_wait_done, &mut attempted_keys, balance_exhausted)
                        .await
                    {
                        continue;
                    }
                    // Honest failure kind after the rescue wait (review
                    // 2026-10-04): family/quota boundaries surface as
                    // QuotaExhausted; short-window budget exhaustion as
                    // RateLimitExceeded with the real refill hint — never the
                    // misleading generic Internal that downstream reads as
                    // "gateway did attempt upstream" (it did not, for these).
                    if attempt > 0 {
                        if self.pool.any_key_quota_cooldown() || self.pool.any_key_family_exhausted_any() {
                            last_kind = GatewayErrorKind::QuotaExhausted;
                        } else if self.pool.exhausted_by_window_with_limits(self.rate_limits.as_ref()) {
                            last_kind = GatewayErrorKind::RateLimitExceeded {
                                retry_after: self.pool.window_refill_in_with_limits(self.rate_limits.as_ref()),
                            };
                        }
                    }
                    // First-attempt pool exhaustion surfaces structurally so
                    // callers never string-match on the aggregated message.
                    if attempt == 0 {
                        self.emit_both("", attempt_idx, None, e.kind(), e.to_string(), None, attempt_start.elapsed());
                        return Err(e);
                    }
                    self.emit_both("", attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    let aggregated_error = format!("{}{}", summarize_attempt_failures(&attempt_kinds), if last_error.is_empty() { String::new() } else { format!(": {}", last_error) });
                    return Err(CoreError::AllRetriesFailed {
                        retries: attempt,
                        attempted_keys,
                        last_error: aggregated_error,
                        kind: last_kind,
                    });
                }
            };

            attempted_keys.push(key.id.clone());
            self.emit_key_selected(&key.id, select_start.elapsed());

            // Short-window metering: admit this attempt now — the request is
            // counted into the window immediately (RPM slot visible to the
            // next select; closes the admission-to-settlement TOCTOU) and the
            // key's in-flight/concurrency slot is taken. The drop-guard
            // releases the slot and settles `tokens` exactly once at the end
            // of this iteration (success return / failover continue / error).
            let mut attempt_meter = AttemptMeterGuard::admit(key.meter());

            let effective_body = self.inject_zen_free_tier_tools(url, Self::prepare_effective_body(&key, body));
            let headers = match self.build_headers(&key, Some(effective_body.as_ref())).await {
                Ok(h) => h,
                Err(e) => {
                    // Antigravity token-resolution failures carry their own
                    // kind: dead credentials isolate, transient refresh
                    // faults only cool (P0-3).
                    // RefreshSkipped means cross-replica lock is currently held by another replica:
                    // do NOT cool this key as the key credential is healthy and being updated.
                    if !matches!(&e, CoreError::RefreshSkipped { .. }) {
                        let pool_err = match &e {
                            CoreError::AuthInvalid { reason, .. } => PoolErrorType::AuthInvalid { reason: Some(reason.clone()) },
                            _ if key.is_antigravity() => PoolErrorType::NetworkError,
                            _ => PoolErrorType::AuthInvalid { reason: None },
                        };
                        self.pool.record_error(&key.id, pool_err);
                    }
                    last_error = e.to_string();
                    last_kind = e.kind();
                    attempt_kinds.push(last_kind.clone());
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(effective_body.as_ref());

            let resp = match self.send_guarded(req).await {
                Ok(r) => r,
                Err(err_str) => {
                    last_error = format!("Network error with {}: {}", key.id, err_str);
                    last_kind = GatewayErrorKind::UpstreamUnavailable;
                    attempt_kinds.push(last_kind.clone());
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                    if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                        attempted_keys.retain(|id| id != &key.id);
                        self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };
            {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        self.emit_headers(&key.id, attempt_idx, attempt_start.elapsed());
                        const MAX_UPSTREAM_JSON_BYTES: usize = 4 * 1024 * 1024;
                        let bytes = resp.bytes().await?;
                        if bytes.len() > MAX_UPSTREAM_JSON_BYTES {
                            return Err(CoreError::Internal("upstream JSON response exceeds 4 MiB".to_string()));
                        }
                        let json_val: Value = serde_json::from_slice(&bytes)?;
                        // Report usage into the short-window meter (TPM budget).
                        attempt_meter.tokens = extract_response_tokens(&json_val);
                        return Ok((json_val, key.id.clone()));
                    }

                    // Handle failover status codes
                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    // Error bodies are diagnostic only; cap before converting to
                    // String so a hostile upstream cannot amplify memory/logs.
                    const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;
                    let err_bytes = resp.bytes().await.unwrap_or_default();
                    let err_body = String::from_utf8_lossy(
                        &err_bytes[..err_bytes.len().min(MAX_UPSTREAM_ERROR_BYTES)],
                    )
                    .to_string();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        // Balance-wording 429 (billing, not window): sets the
                        // fail-fast flag; the classification below is kept.
                        let balance_wording = is_balance_exhausted_body(&err_body);
                        if balance_wording {
                            balance_exhausted = true;
                        }
                        let (kind, pool_err) = classify_too_many_requests(&err_body, retry_after);
                        // Balance-wording 429 means the account balance is
                        // gone, not a sliding window closing: classify as
                        // quota so the boundary guard stops cross-provider
                        // failover instead of draining a second provider
                        // (bugfix 2026-10-02; mirrors the 403 balance path).
                        let (kind, pool_err) = if balance_wording {
                            (
                                GatewayErrorKind::QuotaExhausted,
                                PoolErrorType::QuotaExhausted { retry_after },
                            )
                        } else {
                            (kind, pool_err)
                        };
                        last_kind = kind;
                        attempt_kinds.push(last_kind.clone());
                        let transient_retry_after = match &pool_err {
                            PoolErrorType::RateLimit { retry_after } => *retry_after,
                            _ => None,
                        };
                        let is_quota = matches!(&pool_err, PoolErrorType::QuotaExhausted { .. });
                        let quota_reset = match &pool_err {
                            PoolErrorType::QuotaExhausted { retry_after } => *retry_after,
                            _ => None,
                        };
                        self.pool.record_error(&key.id, pool_err);
                        // Family-scoped 429 writeback: an upstream quota reset
                        // records the family group's exhaustion immediately, so
                        // the pre-exclusion ledger self-heals between keepalive
                        // refreshes instead of waiting for the next probe
                        // (ADR 2026-10-04-antigravity-group-quota-aware-scheduling).
                        if key.is_antigravity() {
                            if let (Some(fam), Some(reset)) = (quota_family, quota_reset) {
                                let reset_at = chrono::Utc::now()
                                    + chrono::Duration::from_std(reset)
                                        .unwrap_or(chrono::Duration::hours(6));
                                key.set_family_quota_exhausted(fam, reset_at);
                            }
                        }
                        // Quota exhaustion closes the window: never retry the
                        // same key in-request, let the pool fail over / fail fast.
                        if !is_quota {
                            if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, transient_retry_after) {
                                attempted_keys.retain(|id| id != &key.id);
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
                        // Pool-level failover backoff: multi-key pools pause
                        // briefly before switching keys so a per-minute window
                        // shared across the account's keys is not swept in
                        // milliseconds (eliminates the observed 17-30× 429
                        // amplification on sense/deepseek-v4-flash).
                        if attempt + 1 < max_attempts {
                            if let Some(delay) = pool_failover_backoff(&self.pool) {
                                tokio::time::sleep(delay).await;
                            }
                        }
                    } else if status_code == 401 {
                        match self.recover_stale_antigravity_token(&key, &mut refreshed_keys).await {
                            StaleTokenRecovery::RetrySameKey => {
                                attempted_keys.retain(|id| id != &key.id);
                                last_kind = GatewayErrorKind::AuthInvalid;
                                attempt_kinds.push(last_kind.clone());
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                continue;
                            }
                            StaleTokenRecovery::Recorded(kind) => {
                                last_kind = kind;
                                attempt_kinds.push(last_kind.clone());
                            }
                            StaleTokenRecovery::Passthrough => {
                                last_kind = GatewayErrorKind::AuthInvalid;
                                attempt_kinds.push(last_kind.clone());
                                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid { reason: None });
                            }
                        }
                    } else if status_code == 403 {
                        // Balance-wording 403 (billing, not window): fail-fast
                        // flag only; the classification below is kept.
                        if is_balance_exhausted_body(&err_body) {
                            balance_exhausted = true;
                        }
                        let (kind, pool_err) = classify_forbidden(&err_body, retry_after);
                        last_kind = kind;
                        attempt_kinds.push(last_kind.clone());
                        self.pool.record_error(&key.id, pool_err);
                    } else if status_code == 402 {
                        // 402 Payment Required is balance exhaustion by
                        // definition: never transparent-wait on it.
                        balance_exhausted = true;
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        attempt_kinds.push(last_kind.clone());
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted { retry_after });
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        attempt_kinds.push(last_kind.clone());
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
        }

        let aggregated_error = format!("{}{}", summarize_attempt_failures(&attempt_kinds), if last_error.is_empty() { String::new() } else { format!(": {}", last_error) });
        Err(CoreError::AllRetriesFailed {
            retries: max_attempts,
            attempted_keys,
            last_error: aggregated_error,
            kind: last_kind,
        })
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded.
    /// Returns the response and the exact Instant when the winning attempt started.
    pub async fn execute_stream_request_with_timing(&self, url: &str, body: &Value) -> Result<(reqwest::Response, Instant)> {
        let (resp, instant, _key_id) = self.execute_stream_request_with_timing_and_key(url, body).await?;
        Ok((resp, instant))
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded.
    /// Returns (response, attempt_start_instant, winning_key_id).
    pub async fn execute_stream_request_with_timing_and_key(&self, url: &str, body: &Value) -> Result<(reqwest::Response, Instant, String)> {
        let mut last_error = String::new();
        let mut last_kind = GatewayErrorKind::Internal;
        // R2: pre-seed with the outer retry loop's already-tried keys so a
        // Priority pool cannot re-select the same key across retries.
        let mut attempted_keys = self.excluded_keys.clone();
        let mut attempt_kinds = Vec::new();
        // Antigravity keys already force-refreshed once this request (P0-3
        // stale-token recovery): a second 401 on the same key is genuine.
        let mut refreshed_keys: Vec<String> = Vec::new();
        // 402 / balance-wording upstream body: waiting for a window cannot
        // fix an empty balance, so full-pool exhaustion must fail fast.
        let mut balance_exhausted = false;
        // Transparent-wait guard: at most one bounded hold on full pool
        // window exhaustion, so the rescue retry cannot spin forever.
        let mut pool_wait_done = false;

        let max_attempts = self.max_retries.max(self.pool.total_key_count()).max(1);

        for attempt in 0..max_attempts {
            let attempt_start = Instant::now();
            let attempt_idx = attempt as u32;
            let select_start = Instant::now();
            let affinity_seed = body.get("messages")
                .and_then(|m| m.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("content").and_then(|c| c.as_str()))
                .and_then(crate::pool::hot_cache::PrefixFingerprint::compute)
                .map(|fp| fp.as_u64());
            // Antigravity quota-group aware selection: the requested model's
            // family decides which quota group (Gemini vs Claude/GPT) must have
            // headroom for this key to be schedulable.
            let quota_family = body
                .get("model")
                .and_then(|m| m.as_str())
                .and_then(crate::pool::entry::classify_quota_family);
            let pinned_candidate = if let Some(ref pk) = self.pinned_key {
                if !attempted_keys.iter().any(|ex| ex == pk) {
                    self.pool.snapshot_keys().into_iter().find(|k| {
                        k.id == *pk
                            && k.current_state() == crate::pool::entry::KeyState::Active
                            && KeyPool::budget_ok(k, self.rate_limits.as_ref())
                            && !k.quota_group_exhausted_for(quota_family, chrono::Utc::now())
                    })
                } else {
                    None
                }
            } else {
                None
            };
            let key = match pinned_candidate {
                Some(pk) => pk,
                None => match self.pool.select_key_with_affinity_for_family(affinity_seed, &attempted_keys, self.rate_limits.as_ref(), quota_family) {
                    Ok(k) => k,
                    Err(e) => {
                        // Full-pool exhaustion: when window-shaped (per-minute/quota,
                        // not balance/auth-disabled), transparently wait up to
                        // DEFAULT_POOL_WAIT_MAX then retry the pool once.
                        if self
                            .maybe_window_wait(&mut pool_wait_done, &mut attempted_keys, balance_exhausted)
                            .await
                        {
                            continue;
                        }
                        // Honest failure kind after the rescue wait (review
                        // 2026-10-04): family/quota boundaries surface as
                        // QuotaExhausted; short-window budget exhaustion as
                        // RateLimitExceeded with the real refill hint — never the
                        // misleading generic Internal that downstream reads as
                        // "gateway did attempt upstream" (it did not, for these).
                        if attempt > 0 {
                            if self.pool.any_key_quota_cooldown() || self.pool.any_key_family_exhausted_any() {
                                last_kind = GatewayErrorKind::QuotaExhausted;
                            } else if self.pool.exhausted_by_window_with_limits(self.rate_limits.as_ref()) {
                                last_kind = GatewayErrorKind::RateLimitExceeded {
                                    retry_after: self.pool.window_refill_in_with_limits(self.rate_limits.as_ref()),
                                };
                            }
                        }
                        // First-attempt pool exhaustion surfaces structurally so
                        // callers never string-match on the aggregated message.
                        if attempt == 0 {
                            self.emit_both("", attempt_idx, None, e.kind(), e.to_string(), None, attempt_start.elapsed());
                            return Err(e);
                        }
                        self.emit_both("", attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                        let aggregated_error = format!("{}{}", summarize_attempt_failures(&attempt_kinds), if last_error.is_empty() { String::new() } else { format!(": {}", last_error) });
                        return Err(CoreError::AllRetriesFailed {
                            retries: attempt,
                            attempted_keys,
                            last_error: aggregated_error,
                            kind: last_kind,
                        });
                    }
                },
            };

            attempted_keys.push(key.id.clone());
            self.emit_key_selected(&key.id, select_start.elapsed());

            // Short-window metering: admit this attempt now — the request is
            // counted into the window immediately and the key's
            // in-flight/concurrency slot is taken; the drop-guard releases
            // the slot exactly once at the end of this iteration. The stream
            // path has no usage here, so `tokens` stays 0 (known limitation).
            let _attempt_meter = AttemptMeterGuard::admit(key.meter());

            let effective_body = self.inject_zen_free_tier_tools(url, Self::prepare_effective_body(&key, body));
            let headers = match self.build_headers(&key, Some(effective_body.as_ref())).await {
                Ok(h) => h,
                Err(e) => {
                    // Antigravity token-resolution failures carry their own
                    // kind: dead credentials isolate, transient refresh
                    // faults only cool (P0-3).
                    // RefreshSkipped means cross-replica lock is currently held by another replica:
                    // do NOT cool this key as the key credential is healthy and being updated.
                    if !matches!(&e, CoreError::RefreshSkipped { .. }) {
                        let pool_err = match &e {
                            CoreError::AuthInvalid { reason, .. } => PoolErrorType::AuthInvalid { reason: Some(reason.clone()) },
                            _ if key.is_antigravity() => PoolErrorType::NetworkError,
                            _ => PoolErrorType::AuthInvalid { reason: None },
                        };
                        self.pool.record_error(&key.id, pool_err);
                    }
                    last_error = e.to_string();
                    last_kind = e.kind();
                    attempt_kinds.push(last_kind.clone());
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(effective_body.as_ref());

            let resp = match self.send_guarded(req).await {
                Ok(r) => r,
                Err(err_str) => {
                    last_error = format!("Network error with {}: {}", key.id, err_str);
                    last_kind = GatewayErrorKind::UpstreamUnavailable;
                    attempt_kinds.push(last_kind.clone());
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                    if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, None) {
                        attempted_keys.retain(|id| id != &key.id);
                        self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    self.emit_both(&key.id, attempt_idx, None, last_kind.clone(), last_error.clone(), None, attempt_start.elapsed());
                    continue;
                }
            };
            {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        self.emit_headers(&key.id, attempt_idx, attempt_start.elapsed());
                        return Ok((resp, attempt_start, key.id.clone()));
                    }

                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    // Error bodies are diagnostic only; cap before converting to
                    // String so a hostile upstream cannot amplify memory/logs.
                    const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;
                    let err_bytes = resp.bytes().await.unwrap_or_default();
                    let err_body = String::from_utf8_lossy(
                        &err_bytes[..err_bytes.len().min(MAX_UPSTREAM_ERROR_BYTES)],
                    )
                    .to_string();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        // Balance-wording 429 (billing, not window): sets the
                        // fail-fast flag; the classification below is kept.
                        let balance_wording = is_balance_exhausted_body(&err_body);
                        if balance_wording {
                            balance_exhausted = true;
                        }
                        let (kind, pool_err) = classify_too_many_requests(&err_body, retry_after);
                        // Balance-wording 429 means the account balance is
                        // gone, not a sliding window closing: classify as
                        // quota so the boundary guard stops cross-provider
                        // failover instead of draining a second provider
                        // (bugfix 2026-10-02; mirrors the 403 balance path).
                        let (kind, pool_err) = if balance_wording {
                            (
                                GatewayErrorKind::QuotaExhausted,
                                PoolErrorType::QuotaExhausted { retry_after },
                            )
                        } else {
                            (kind, pool_err)
                        };
                        last_kind = kind;
                        attempt_kinds.push(last_kind.clone());
                        let transient_retry_after = match &pool_err {
                            PoolErrorType::RateLimit { retry_after } => *retry_after,
                            _ => None,
                        };
                        let is_quota = matches!(&pool_err, PoolErrorType::QuotaExhausted { .. });
                        let quota_reset = match &pool_err {
                            PoolErrorType::QuotaExhausted { retry_after } => *retry_after,
                            _ => None,
                        };
                        self.pool.record_error(&key.id, pool_err);
                        // Family-scoped 429 writeback: an upstream quota reset
                        // records the family group's exhaustion immediately, so
                        // the pre-exclusion ledger self-heals between keepalive
                        // refreshes instead of waiting for the next probe
                        // (ADR 2026-10-04-antigravity-group-quota-aware-scheduling).
                        if key.is_antigravity() {
                            if let (Some(fam), Some(reset)) = (quota_family, quota_reset) {
                                let reset_at = chrono::Utc::now()
                                    + chrono::Duration::from_std(reset)
                                        .unwrap_or(chrono::Duration::hours(6));
                                key.set_family_quota_exhausted(fam, reset_at);
                            }
                        }
                        // Quota exhaustion closes the window: never retry the
                        // same key in-request, let the pool fail over / fail fast.
                        if !is_quota {
                            if let Some(delay) = transient_retry_delay(&self.pool, &key.id, attempt, max_attempts, transient_retry_after) {
                                attempted_keys.retain(|id| id != &key.id);
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
                        // Pool-level failover backoff: multi-key pools pause
                        // briefly before switching keys so a per-minute window
                        // shared across the account's keys is not swept in
                        // milliseconds (eliminates the observed 17-30× 429
                        // amplification on sense/deepseek-v4-flash).
                        if attempt + 1 < max_attempts {
                            if let Some(delay) = pool_failover_backoff(&self.pool) {
                                tokio::time::sleep(delay).await;
                            }
                        }
                    } else if status_code == 401 {
                        match self.recover_stale_antigravity_token(&key, &mut refreshed_keys).await {
                            StaleTokenRecovery::RetrySameKey => {
                                attempted_keys.retain(|id| id != &key.id);
                                last_kind = GatewayErrorKind::AuthInvalid;
                                attempt_kinds.push(last_kind.clone());
                                self.emit_both(&key.id, attempt_idx, Some(status_code), last_kind.clone(), last_error.clone(), Some(err_body), attempt_start.elapsed());
                                continue;
                            }
                            StaleTokenRecovery::Recorded(kind) => {
                                last_kind = kind;
                                attempt_kinds.push(last_kind.clone());
                            }
                            StaleTokenRecovery::Passthrough => {
                                last_kind = GatewayErrorKind::AuthInvalid;
                                attempt_kinds.push(last_kind.clone());
                                self.pool.record_error(&key.id, PoolErrorType::AuthInvalid { reason: None });
                            }
                        }
                    } else if status_code == 403 {
                        // Balance-wording 403 (billing, not window): fail-fast
                        // flag only; the classification below is kept.
                        if is_balance_exhausted_body(&err_body) {
                            balance_exhausted = true;
                        }
                        let (kind, pool_err) = classify_forbidden(&err_body, retry_after);
                        last_kind = kind;
                        attempt_kinds.push(last_kind.clone());
                        self.pool.record_error(&key.id, pool_err);
                    } else if status_code == 402 {
                        // 402 Payment Required is balance exhaustion by
                        // definition: never transparent-wait on it.
                        balance_exhausted = true;
                        last_kind = GatewayErrorKind::QuotaExhausted;
                        attempt_kinds.push(last_kind.clone());
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted { retry_after });
                    } else if status.is_server_error() {
                        last_kind = GatewayErrorKind::UpstreamUnavailable;
                        attempt_kinds.push(last_kind.clone());
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
        }

        let aggregated_error = format!("{}{}", summarize_attempt_failures(&attempt_kinds), if last_error.is_empty() { String::new() } else { format!(": {}", last_error) });
        Err(CoreError::AllRetriesFailed {
            retries: max_attempts,
            attempted_keys,
            last_error: aggregated_error,
            kind: last_kind,
        })
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded
    pub async fn execute_stream_request(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        self.execute_stream_request_with_timing(url, body).await.map(|(resp, _)| resp)
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
                !matches!(pool_err, PoolErrorType::PolicyViolation | PoolErrorType::AuthInvalid { .. }),
                "body: {}",
                body
            );
            assert_ne!(kind, GatewayErrorKind::AuthInvalid, "body: {}", body);
        }
    }

    #[test]
    fn forbidden_validation_required_isolates_for_human_verification() {
        // Exact shape from the reported failure: HTTP 403 VALIDATION_REQUIRED
        // is an account-level human-verification gate, not quota exhaustion.
        let body = r#"{"error": {"code": 403, "message": "Verify your account to continue.", "status": "PERMISSION_DENIED"}}"#;
        let (kind, pool_err) = classify_forbidden(body, None);
        assert_eq!(kind, GatewayErrorKind::AuthInvalid, "body: {}", body);
        assert!(
            matches!(pool_err, PoolErrorType::AccountValidationRequired),
            "body: {}",
            body
        );
        assert!(is_account_validation_required(body));
        // Pool-level effect: permanently isolated with a verification hint,
        // never a quota-window cooldown.
        let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        entry.record_failure(pool_err);
        assert_eq!(entry.current_state(), KeyState::Disabled);
        assert!(
            entry
                .disabled_reason()
                .unwrap_or_default()
                .contains("VALIDATION_REQUIRED"),
            "disabled reason must name the verification gate"
        );
    }

    #[test]
    fn forbidden_eligibility_freezes_for_days_and_keeps_pool_alive() {
        // Observed 2026-10-04 on Antigravity: chat completions for
        // gemini-3.8-flash-high returned this 403 body. It must freeze the
        // account for days (not a 60s blip that hammered the account every
        // minute) while leaving the rest of the pool schedulable.
        let body = r#"{
            "error": {
              "code": 403,
              "message": "Your current account is not eligible for Gemini Code Assist for individuals. To use Gemini Code Assist for individuals you must be 18 years old or older. If you think you are receiving this message in error, please ensure you have verified your age and try to log in again.",
              "status": "PERMISSION_DENIED"
            }
        }"#;
        assert!(is_account_eligibility_revoked(body), "body must be detected: {body}");
        let (kind, pool_err) = classify_forbidden(body, None);
        assert_eq!(kind, GatewayErrorKind::AuthInvalid, "body: {body}");
        match &pool_err {
            PoolErrorType::AccountEligibility { reason } => {
                let reason = reason.as_deref().unwrap_or_default();
                assert!(
                    reason.contains("not eligible"),
                    "reason must carry the upstream message, got: {reason}"
                );
            }
            other => panic!("expected AccountEligibility freeze, got {other:?}"),
        }
        // Pool-level effect: days-long cooling (not disabled, not 60s), with
        // the eligibility reason exposed for the admin/web red badge.
        let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        entry.record_failure(pool_err);
        assert_eq!(entry.current_state(), KeyState::CoolingDown);
        assert_eq!(
            entry.cooldown_reason(),
            Some(crate::pool::entry::CooldownReason::Eligibility)
        );
        let remaining = entry.cooldown_remaining().expect("must still be cooling");
        assert!(
            remaining >= Duration::from_secs(3 * 24 * 60 * 60) - Duration::from_secs(60),
            "freeze must be ~3 days, got {remaining:?}"
        );
        assert!(
            entry
                .error_reason()
                .unwrap_or_default()
                .contains("not eligible"),
            "error_reason must expose the upstream message"
        );
    }

    #[test]
    fn probe_failure_classifies_403_eligibility_but_ignores_transport_blips() {
        // 探针路径与请求路径共用"确定性硬信号"分类：资格 403 → 给出长冷冻
        // 动作，由调用方 pool.record_error 落地（keepalive / 管理面刷新 / 拨测）。
        assert!(matches!(
            classify_probe_failure(
                403,
                "Your current account is not eligible for Gemini Code Assist for individuals."
            ),
            Some(PoolErrorType::AccountEligibility { .. })
        ));
        // 网络超时（status=0 / 5xx / 429）→ None：探针不得把瞬时抽风
        // 冻成账号问题（2026-10-04 的 quota_probe_failed 正是网络超时误伤）。
        assert!(classify_probe_failure(0, "").is_none());
        assert!(classify_probe_failure(503, "upstream oops").is_none());
        assert!(classify_probe_failure(429, "rate limit").is_none());
        // 软信号（quota 措辞）与 unknown-403 兜底 → None：探针只认确定性
        // 硬信号，绝不因 WAF/HTML/scope 类 403 给健康 key 套冷却。
        assert!(classify_probe_failure(403, "RESOURCE_EXHAUSTED #3501 quota exceeded").is_none());
        assert!(classify_probe_failure(403, "<html>Forbidden</html>").is_none());
        // 其它权威 403 签名同样被探针路径采纳，与请求路径一致。
        assert!(matches!(
            classify_probe_failure(403, "Verify your account to continue."),
            Some(PoolErrorType::AccountValidationRequired)
        ));
        assert!(matches!(
            classify_probe_failure(403, "ACCOUNT_SUSPENDED terms of service violation"),
            Some(PoolErrorType::PolicyViolation)
        ));
    }

    #[test]
    fn eligibility_403_with_validation_reason_still_freezes_not_isolates() {
        // Antigravity 同一"not eligible"条件有两种 body 形态（观测：
        // PERMISSION_DENIED；外部证据：VALIDATION_REQUIRED）。资格语义必须
        // 优先——冷冻 3 天可自愈，永久隔离不可逆（ADR 决策）。
        for body in [
            r#"{"error":{"code":403,"message":"Your current account is not eligible for Gemini Code Assist for individuals.","status":"VALIDATION_REQUIRED"}}"#,
            r#"{"error":{"code":403,"message":"Your current account is not eligible for Gemini Code Assist for individuals.","status":"PERMISSION_DENIED","reason":"RESTRICTED_AGE"}}"#,
        ] {
            let (kind, pool_err) = classify_forbidden(body, None);
            assert_eq!(kind, GatewayErrorKind::AuthInvalid, "body: {body}");
            assert!(
                matches!(pool_err, PoolErrorType::AccountEligibility { .. }),
                "eligibility must win over validation wording, got {pool_err:?} (body: {body})"
            );
            let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
            entry.record_failure(pool_err);
            assert_eq!(
                entry.current_state(),
                KeyState::CoolingDown,
                "eligibility freeze must NOT permanently isolate (body: {body})"
            );
        }
    }

    #[test]
    fn model_level_not_eligible_wording_must_not_freeze_whole_account() {
        // 无账号实体锚点的 "not eligible"（模型/项目级措辞）不得冻结整个
        // 账号 3 天——Antigravity 池按账号共享凭证，误冻会停摆其它可服务模型。
        for body in [
            "Your project is not eligible for gemini-2.5-pro",
            "Model gemini-2.5-pro is not eligible for this request",
            "not eligible for gemini code assist",
        ] {
            assert!(
                !is_account_eligibility_revoked(body),
                "model-level wording must NOT match account eligibility: {body}"
            );
            let (_, pool_err) = classify_forbidden(body, None);
            assert!(
                !matches!(pool_err, PoolErrorType::AccountEligibility { .. }),
                "must not freeze the account, got {pool_err:?} (body: {body})"
            );
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
    fn zen_free_tier_gate_403_is_quota_not_unknown_403() {
        // OpenCode zen `FreeTierError` (403): only the official client may use
        // `*-free` models. A proxied replay tripping this gate must cool as
        // quota (honest `quota_exhausted` + boundary guard) instead of cycling
        // the generic unknown-403 60s rate-limit cooldown forever.
        for body in [
            r#"{"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free tier can only be used from within OpenCode"}}"#,
            r#"{"type":"error","error":{"type":"FreeTierError","message":"Error from provider (Console): OpenCode's free tier can only be used from within OpenCode"}}"#,
        ] {
            assert!(is_zen_free_tier_gate_body(body), "body must be detected: {body}");
            let (kind, pool_err) = classify_forbidden(body, None);
            assert_eq!(kind, GatewayErrorKind::QuotaExhausted, "body: {body}");
            match pool_err {
                PoolErrorType::QuotaExhausted { retry_after } => {
                    assert_eq!(retry_after, Some(Duration::from_secs(900)));
                }
                other => panic!("expected quota cooldown, got {:?} (body: {body})", other),
            }
        }
        // Pool-level effect: cooling with Quota reason, never disabled.
        let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        let (_, pool_err) = classify_forbidden(
            r#"{"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free tier can only be used from within OpenCode"}}"#,
            None,
        );
        entry.record_failure(pool_err);
        assert_eq!(entry.current_state(), KeyState::CoolingDown);
        assert_eq!(
            entry.cooldown_reason(),
            Some(crate::pool::entry::CooldownReason::Quota)
        );
    }

    #[test]
    fn zen_free_usage_limit_429_is_quota_not_rate_limit() {
        // Observed 2026-10-03 from the opencode zen proxy: the Console free
        // window closed and every zen key returned this body. The message says
        // "Rate limit exceeded", but the type `FreeUsageLimitError` means a
        // windowed usage quota — it must classify as QuotaExhausted so
        // `pool_quota_exhausted` reclassifies a fully-cooled pool to
        // `quota_exhausted` (clients see an honest quota signal instead of a
        // misleading gateway-side `rate_limit_exceeded`).
        for body in [
            r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#,
            r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Error from provider (Console): Rate limit exceeded. Please try again later."}}"#,
        ] {
            assert!(is_zen_free_usage_limit_body(body), "body must be detected: {body}");
            let (kind, pool_err) = classify_too_many_requests(body, None);
            assert_eq!(kind, GatewayErrorKind::QuotaExhausted, "body: {body}");
            assert!(
                matches!(pool_err, PoolErrorType::QuotaExhausted { .. }),
                "got {:?} (body: {body})",
                pool_err
            );
        }
        // Pool-level effect: Quota cooldown reason (feeds `any_key_quota_cooldown`).
        let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        let (_, pool_err) = classify_too_many_requests(
            r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#,
            None,
        );
        entry.record_failure(pool_err);
        assert_eq!(entry.current_state(), KeyState::CoolingDown);
        assert_eq!(
            entry.cooldown_reason(),
            Some(crate::pool::entry::CooldownReason::Quota)
        );
    }

    #[test]
    fn forbidden_sense_rate_limit_signal_beats_quota_wording() {
        // Sense/商汤 labels RPM rejections `type: "quota_exceeded_error"` even
        // on 403 (same mislabel as the 429 path): the rate-limit signal must
        // win, otherwise the key is cooled for ~15 minutes and failover is
        // shut down for a self-recovering window.
        for body in [
            r#"{"error":{"message":"rpm exhausted","type":"quota_exceeded_error","code":"8"}}"#,
            r#"{"error":{"message":"inference exceeds tpm/rpm limit","type":"quota_exceeded_error","code":"8"}}"#,
            r#"{"error":{"message":"requests per minute exceeded","type":"quota_exceeded_error"}}"#,
        ] {
            let (kind, pool_err) = classify_forbidden(body, None);
            assert!(
                matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
                "403 Sense rate-limit body must stay RateLimitExceeded, got {:?} (body: {body})",
                kind
            );
            assert!(
                matches!(pool_err, PoolErrorType::RateLimit { .. }),
                "got {:?} (body: {body})",
                pool_err
            );
        }
    }

    #[test]
    fn forbidden_balance_wording_is_terminal_quota() {
        // Billing wording on a 403 is account-balance exhaustion, not a
        // transient window: classify as quota (never a 60s rate-limit blip).
        for body in [
            r#"{"error":{"message":"insufficient balance","type":"insufficient_balance"}}"#,
            "your account balance is exhausted",
            r#"{"error":{"message":"out of credits","type":"billing_error"}}"#,
        ] {
            let (kind, pool_err) = classify_forbidden(body, None);
            assert_eq!(kind, GatewayErrorKind::QuotaExhausted, "body: {body}");
            assert!(
                matches!(pool_err, PoolErrorType::QuotaExhausted { .. }),
                "got {:?} (body: {body})",
                pool_err
            );
        }
    }

    #[test]
    fn quota_429_body_cools_for_advertised_reset() {
        // Exact shape observed from cloudcode-pa (Antigravity): HTTP 429,
        // RESOURCE_EXHAUSTED / QUOTA_EXHAUSTED, reset embedded in the message.
        let body = r#"{
          "error": {
            "code": 429,
            "message": "Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 15h21m26s.",
            "status": "RESOURCE_EXHAUSTED",
            "details": [
              {
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": "QUOTA_EXHAUSTED",
                "domain": "cloudcode-pa.googleapis.com",
                "metadata": {"uiMessage": "true", "model": "gemini-3.8-flash-high"}
              }
            ]
          }
        }"#;
        let expected = Duration::from_secs(15 * 3600 + 21 * 60 + 26);
        assert_eq!(parse_reset_duration(body), Some(expected));

        let (kind, pool_err) = classify_too_many_requests(body, None);
        assert_eq!(kind, GatewayErrorKind::QuotaExhausted);
        match pool_err {
            PoolErrorType::QuotaExhausted { retry_after } => {
                assert_eq!(retry_after, Some(expected));
            }
            other => panic!("expected quota cooldown, got {:?}", other),
        }
    }

    #[test]
    fn quota_reset_prefers_body_over_shorter_header() {
        // Without an explicit reset the pool falls back to its conservative
        // default; with one, the advertised window must win over Retry-After.
        let (kind, pool_err) = classify_too_many_requests("QUOTA_EXHAUSTED", None);
        assert_eq!(kind, GatewayErrorKind::QuotaExhausted);
        match pool_err {
            PoolErrorType::QuotaExhausted { retry_after } => assert_eq!(retry_after, None),
            other => panic!("expected quota cooldown, got {:?}", other),
        }

        let body = "Individual quota reached. Resets in 2h0m0s.";
        match classify_too_many_requests(body, Some(Duration::from_secs(60))).1 {
            PoolErrorType::QuotaExhausted { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(7200)));
            }
            other => panic!("expected quota cooldown, got {:?}", other),
        }
    }

    #[test]
    fn plain_429_stays_rate_limit_not_quota() {
        let header = Some(Duration::from_secs(5));
        let (kind, pool_err) = classify_too_many_requests(
            r#"{"error": {"message": "Rate limit exceeded"}}"#,
            header,
        );
        assert_eq!(
            kind,
            GatewayErrorKind::RateLimitExceeded { retry_after: header }
        );
        assert!(matches!(pool_err, PoolErrorType::RateLimit { .. }));

        // Per-minute TPM quota wording is a transient rate limit, not a
        // windowed account quota: keep the short path.
        let (kind, pool_err) = classify_too_many_requests(
            r#"{"error": {"message": "TPM quota exceeded"}}"#,
            Some(Duration::from_secs(10)),
        );
        assert!(
            matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
            "got {:?}",
            kind
        );
        assert!(matches!(pool_err, PoolErrorType::RateLimit { .. }));

        // Sense (商汤) returns `type: "quota_exceeded_error"` even when only RPM is exhausted.
        // It must be classified as a RateLimitExceeded, allowing pool failover / retry,
        // rather than shutting down the key as QuotaExhausted.
        let sense_rpm_body = r#"{"error":{"message":"rpm exhausted","type":"quota_exceeded_error","code":"8"}}"#;
        let (kind, pool_err) = classify_too_many_requests(sense_rpm_body, None);
        assert!(
            matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
            "Sense RPM error must be classified as RateLimitExceeded, got {:?}",
            kind
        );
        assert!(matches!(pool_err, PoolErrorType::RateLimit { .. }));

        let sense_tpm_body = r#"{"error":{"message":"inference exceeds tpm/rpm limit","type":"rate_limit_error","code":"429003"}}"#;
        let (kind, pool_err) = classify_too_many_requests(sense_tpm_body, None);
        assert!(
            matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
            "Sense TPM/RPM error must be classified as RateLimitExceeded, got {:?}",
            kind
        );
        assert!(matches!(pool_err, PoolErrorType::RateLimit { .. }));
    }

    #[test]
    fn rate_limit_signal_wins_over_quota_wording() {
        // A mixed body that carries both a transient rate-limit signal and
        // quota wording must keep the rate-limit path: the rate-limit signal
        // is the actionable, recoverable interpretation (anti-hammering),
        // whereas shutting the key down would block failover.
        let mixed = r#"{"error":{"message":"quota_exhausted but rpm reached","type":"quota_exceeded_error","code":"8"}}"#;
        let (kind, pool_err) = classify_too_many_requests(mixed, None);
        assert!(
            matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
            "mixed quota+rpm body must classify as RateLimitExceeded, got {:?}",
            kind
        );
        assert!(matches!(pool_err, PoolErrorType::RateLimit { .. }));
    }

    #[test]
    fn identifier_like_rpm_user_stays_genuine_quota() {
        // `_` is kept inside tokens, so an identifier such as `rpm_user` must
        // NOT be read as an `rpm` rate-limit signal: a genuine account quota
        // message mentioning such an identifier stays on the quota path.
        let genuine = r#"{"error":{"message":"quota_exhausted for account rpm_user","type":"insufficient_balance","code":"402"}}"#;
        assert!(
            is_quota_exhausted_body(genuine),
            "identifier `rpm_user` must not suppress a genuine quota signal"
        );
    }

    #[test]
    fn qps_and_concurrency_bodies_stay_rate_limit() {
        let qps = r#"{"error":{"message":"qps exceeded","type":"rate_limit_error"}}"#;
        assert!(!is_quota_exhausted_body(qps), "qps body must be a rate limit");

        let concurrency = r#"{"error":{"message":"concurrency limit reached","type":"rate_limit_error"}}"#;
        assert!(
            !is_quota_exhausted_body(concurrency),
            "concurrency body must be a rate limit"
        );

        let spaced = r#"{"error":{"message":"Rate limit reached","type":"rate_limit_error"}}"#;
        assert!(
            !is_quota_exhausted_body(spaced),
            "space-variant `rate limit` must be a rate limit"
        );
    }

    #[test]
    fn huge_malformed_body_never_panics_and_stays_bounded() {
        // The call site truncates upstream bodies to 64 KiB, but the matcher
        // must still behave on arbitrarily long / malformed input without
        // panicking or blowing up (defense in depth for hostile upstreams).
        let mut huge = String::with_capacity(1 << 20);
        huge.push_str("{\"error\":{\"message\":\"");
        for _ in 0..((1 << 20) - 128) {
            huge.push('x');
        }
        huge.push_str("rpm exhausted");
        huge.push_str("\"}}");
        assert!(
            !is_quota_exhausted_body(&huge),
            "huge body ending in rpm signal must be a rate limit"
        );

        let mut huge_quota = String::with_capacity(1 << 20);
        huge_quota.push_str("{\"error\":{\"message\":\"");
        for _ in 0..((1 << 20) - 128) {
            huge_quota.push('x');
        }
        huge_quota.push_str("QUOTA_EXHAUSTED resets in 15h");
        huge_quota.push_str("\"}}");
        assert!(
            is_quota_exhausted_body(&huge_quota),
            "huge body ending in explicit QUOTA_EXHAUSTED must stay quota"
        );
    }

    #[test]
    fn quota_wording_with_long_reset_counts_as_quota() {
        // No QUOTA_EXHAUSTED reason code, but a 15m+ advertised window: still a
        // windowed quota and must not be knocked every few seconds.
        let (kind, pool_err) =
            classify_too_many_requests("Quota reached for this account. Resets in 15m0s.", None);
        assert_eq!(kind, GatewayErrorKind::QuotaExhausted);
        match pool_err {
            PoolErrorType::QuotaExhausted { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(900)));
            }
            other => panic!("expected quota cooldown, got {:?}", other),
        }

        // Short window stays on the exact-duration rate-limit path.
        let (kind, _) =
            classify_too_many_requests("Quota reached for this account. Resets in 4m0s.", None);
        assert!(
            matches!(kind, GatewayErrorKind::RateLimitExceeded { .. }),
            "got {:?}",
            kind
        );
    }

    #[test]
    fn parse_reset_duration_accepts_compact_groups_and_rejects_noise() {
        assert_eq!(
            parse_reset_duration("Resets in 2d3h4m5s."),
            Some(Duration::from_secs(2 * 86400 + 3 * 3600 + 4 * 60 + 5))
        );
        assert_eq!(
            parse_reset_duration("resets in 30m"),
            Some(Duration::from_secs(1800))
        );
        assert_eq!(parse_reset_duration("no hint here"), None);
        assert_eq!(parse_reset_duration("Resets in soon"), None);
        // A bare number without a unit is not a duration.
        assert_eq!(parse_reset_duration("Resets in 5 requests"), None);
        // A garbled later group invalidates the whole hint rather than
        // yielding the valid prefix.
        assert_eq!(parse_reset_duration("Resets in 1h999999999999999999999999s"), None);
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
        assert!(generated.starts_with("ses_"), "got {}", generated);
        let blank = downstream(&[("x-opencode-session", "   ")]);
        assert!(resolve_upstream_session(&blank).starts_with("ses_"));
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
            "opencode/1.18.31 (Linux; x64)"
        );

        let pool = Arc::new(KeyPool::new("test", RoutingStrategy::RoundRobin));
        let fallback = UpstreamExecutor::new(pool, 1).with_opencode_zen(true);
        let headers = futures::executor::block_on(fallback.build_headers(&key, None)).unwrap();
        let session = headers
            .get("x-opencode-session")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(session.starts_with("ses_"), "got {}", session);
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

#[cfg(test)]
mod pool_wait_tests {
    use super::*;
    use crate::pool::{KeyPool, RateLimits, RoutingStrategy};

    #[test]
    fn extract_response_tokens_across_wire_shapes() {
        // OpenAI chat / responses: usage.total_tokens wins.
        assert_eq!(
            extract_response_tokens(&json!({"usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}})),
            12
        );
        // Anthropic messages: input + output.
        assert_eq!(
            extract_response_tokens(&json!({"usage": {"input_tokens": 3, "output_tokens": 4}})),
            7
        );
        // prompt/completion split without total.
        assert_eq!(
            extract_response_tokens(&json!({"usage": {"prompt_tokens": 10, "completion_tokens": 20}})),
            30
        );
        // Missing/absent usage -> 0 (request-only accounting).
        assert_eq!(extract_response_tokens(&json!({"choices": []})), 0);
        assert_eq!(extract_response_tokens(&json!({"usage": {}})), 0);
    }

    #[test]
    fn attempt_meter_guard_counts_request_at_admission() {
        let pool = KeyPool::new("p", RoutingStrategy::RoundRobin);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        let keys = pool.snapshot_keys();
        let meter = keys[0].meter();
        {
            // Admission counts the request into the window immediately (RPM
            // slot visible to the next select — no admission-to-settlement
            // TOCTOU) and takes the in-flight slot.
            let mut guard = AttemptMeterGuard::admit(meter);
            assert_eq!(meter.requests_in_window(), 1);
            assert_eq!(meter.in_flight(), 1);
            guard.tokens = 100;
        }
        // Drop: in-flight released; tokens settled (token-only, no extra
        // request) when > 0.
        assert_eq!(meter.in_flight(), 0);
        assert_eq!(meter.tokens_in_window(), 100);
        assert_eq!(meter.requests_in_window(), 1);
    }

    #[test]
    fn attempt_meter_guard_skips_tokens_when_zero() {
        let pool = KeyPool::new("p", RoutingStrategy::RoundRobin);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        let keys = pool.snapshot_keys();
        let meter = keys[0].meter();
        {
            // Stream path: tokens stays 0 (no usage at this layer) — the drop
            // still releases the in-flight slot but settles no tokens.
            let _guard = AttemptMeterGuard::admit(meter);
            assert_eq!(meter.requests_in_window(), 1);
            assert_eq!(meter.in_flight(), 1);
        }
        assert_eq!(meter.in_flight(), 0);
        assert_eq!(meter.tokens_in_window(), 0);
        assert_eq!(meter.requests_in_window(), 1);
    }

    #[test]
    fn balance_wording_bodies_are_fail_fast_signals() {
        // Billing language anywhere in the body marks balance exhaustion;
        // window language alone must NOT.
        for body in [
            "Insufficient balance. Please top up.",
            r#"{"error":{"message":"your account credit is exhausted","code":"402"}}"#,
            "Budget exceeded for this project",
            "Payment required to continue using the API",
            "402 payment required",
        ] {
            assert!(is_balance_exhausted_body(body), "body: {body}");
        }
        for body in [
            "TPM quota exceeded",
            "Individual quota reached. Resets in 15h21m26s.",
            "rpm exhausted",
        ] {
            assert!(!is_balance_exhausted_body(body), "body: {body}");
        }
    }

    #[test]
    fn pool_failover_backoff_bounded_and_singleton_none() {
        // Singleton pools keep transient-retry semantics: no pool-level pause.
        let single = KeyPool::new("p", RoutingStrategy::RoundRobin);
        single.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        assert_eq!(pool_failover_backoff(&single), None);

        // Multi-key pool with no active cooldown: default ~1.2s bound.
        let multi = KeyPool::new("p", RoutingStrategy::RoundRobin);
        multi.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        multi.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        let d = pool_failover_backoff(&multi).expect("multi-key pool gets a backoff");
        assert_eq!(d, Duration::from_millis(1200));

        // A key with a long cooldown caps the pause at 2s (never waits out a
        // 15h quota reset before failing over).
        multi.set_key_cooldown("k1", Duration::from_secs(3600));
        let d = pool_failover_backoff(&multi).expect("backoff still present");
        assert_eq!(d, Duration::from_secs(2));

        // A key unlocking sooner bounds the pause to that unlock.
        multi.set_key_cooldown("k2", Duration::from_millis(500));
        let d = pool_failover_backoff(&multi).expect("backoff still present");
        assert!(d <= Duration::from_millis(500), "got {d:?}");
    }

    #[tokio::test]
    async fn window_wait_suppressed_for_balance_and_beyond_cap() {
        // Balance case: a window-shaped pool (k1 cooling) is still waitable,
        // but the 402/balance flag short-circuits to fail-fast.
        let pool = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        pool.set_key_cooldown("k1", Duration::from_secs(60));
        let executor = UpstreamExecutor::new(pool, 1);
        let mut done = false;
        let mut keys = vec!["k1".to_string()];
        assert!(!executor.maybe_window_wait(&mut done, &mut keys, true).await);
        assert!(!done);
        assert_eq!(keys, vec!["k1".to_string()]);

        // Beyond-cap case: BOTH keys cooling > cap => window-exhausted with a
        // hold that exceeds DEFAULT_POOL_WAIT_MAX -> fail fast (429).
        let pool2 = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        pool2.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        pool2.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        pool2.set_key_cooldown("k1", Duration::from_secs(3600));
        pool2.set_key_cooldown("k2", Duration::from_secs(7200));
        let executor2 = UpstreamExecutor::new(pool2, 1);
        let mut done2 = false;
        let mut keys2 = vec!["k1".to_string(), "k2".to_string()];
        assert!(!executor2.maybe_window_wait(&mut done2, &mut keys2, false).await);
        assert!(!done2);
    }

    #[tokio::test]
    async fn window_wait_holds_once_then_allows_single_rescue() {
        let pool = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        // 秒级冷却（≥3s）：避免"设冷却→断言调用"间隙在重载 runner 上超过
        // 亚秒冷却窗口、键提前回填（exhausted 退化 → maybe_window_wait 返回
        // false）导致的计时抖动（2026-09-30 macOS arm64 runner 实证一次）。
        pool.set_key_cooldown("k1", Duration::from_secs(3));
        pool.set_key_cooldown("k2", Duration::from_secs(6));
        // A budget must be configured for the window-shaped hold to apply.
        let executor = UpstreamExecutor::new(pool, 1).with_rate_limits(Some(RateLimits::default()));

        let mut done = false;
        let mut keys = vec!["k1".to_string(), "k2".to_string()];
        assert!(
            executor
                .maybe_window_wait(&mut done, &mut keys, false)
                .await
        );
        assert!(done, "guard latches after the single rescue");
        assert!(keys.is_empty(), "rescue pass clears attempted keys");

        // A second exhaustion must not wait again.
        let mut keys2 = vec!["k1".to_string()];
        assert!(
            !executor
                .maybe_window_wait(&mut done, &mut keys2, false)
                .await
        );
        assert_eq!(keys2, vec!["k1".to_string()]);
    }

    #[tokio::test]
    async fn window_wait_fails_fast_without_rate_limits() {
        // No budget configured (legacy): a full-pool pure-cooldown exhaustion
        // must fail fast — a transparent hold here would delay cross-provider
        // failover by up to DEFAULT_POOL_WAIT_MAX for a state that only
        // cooldown expiry fixes.
        let pool = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        pool.set_key_cooldown("k1", Duration::from_secs(30));
        pool.set_key_cooldown("k2", Duration::from_secs(60));
        let executor = UpstreamExecutor::new(pool, 1); // rate_limits: None
        let mut done = false;
        let mut keys = vec!["k1".to_string(), "k2".to_string()];
        assert!(
            !executor
                .maybe_window_wait(&mut done, &mut keys, false)
                .await,
            "limits=None must not hold on pure cooldown"
        );
        assert!(!done);
        assert_eq!(keys, vec!["k1".to_string(), "k2".to_string()]);
    }

    #[tokio::test]
    async fn cooled_pool_waits_but_all_disabled_fails_fast() {
        // A window-shaped pool (cooldowns, no permanent disable) waits once
        // when a budget is configured.
        let pool = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        // 秒级冷却：同上（亚秒冷却在重载 runner 上会被设-调间隙吃光，
        // 键提前回填导致断言抖失败）。
        pool.set_key_cooldown("k1", Duration::from_millis(2500));
        let executor = UpstreamExecutor::new(pool, 1).with_rate_limits(Some(RateLimits::default()));
        let mut done = false;
        let mut keys: Vec<String> = Vec::new();
        assert!(
            executor
                .maybe_window_wait(&mut done, &mut keys, false)
                .await
        );
        assert!(done);

        // All-keys-disabled (auth/verify, never refills): no wait, fail fast,
        // with or without a budget.
        let pool2 = Arc::new(KeyPool::new("p", RoutingStrategy::RoundRobin));
        let k = ApiKeyEntry::new("k1", "sk-1", 1, 10);
        k.record_failure(PoolErrorType::AuthInvalid { reason: None });
        pool2.add_key(k);
        let executor2 = UpstreamExecutor::new(pool2, 1).with_rate_limits(Some(RateLimits::default()));
        let mut done2 = false;
        let mut keys2: Vec<String> = Vec::new();
        assert!(
            !executor2
                .maybe_window_wait(&mut done2, &mut keys2, false)
                .await
        );
        assert!(!done2);
    }
}

#[cfg(test)]
mod zen_tools_tests {
    use super::*;
    use crate::pool::{KeyPool, RoutingStrategy};

    fn zen_executor() -> UpstreamExecutor {
        let pool = Arc::new(KeyPool::new("zen-provider", RoutingStrategy::RoundRobin));
        UpstreamExecutor::new(pool, 1).with_opencode_zen(true)
    }

    fn tool_names(body: &Value) -> Vec<String> {
        zen_existing_tool_names(body)
    }

    const CHAT_URL: &str = "http://127.0.0.1:8899/pony_x/opencode/zen/v1/chat/completions";
    const RESP_URL: &str = "https://opencode.ai/zen/v1/responses";
    const MSG_URL: &str = "https://opencode.ai/zen/v1/messages";

    #[test]
    fn free_model_chat_body_gets_full_tool_set() {
        let ex = zen_executor();
        let body = json!({"model": "mimo-v2.5-free", "messages": [{"role": "user", "content": "hi"}]});
        let out = ex.inject_zen_free_tier_tools(CHAT_URL, std::borrow::Cow::Borrowed(&body));
        let names = tool_names(out.as_ref());
        assert_eq!(names.len(), OPENCODE_ZEN_TOOL_NAMES.len());
        for n in OPENCODE_ZEN_TOOL_NAMES {
            assert!(names.iter().any(|e| e == n), "missing {n}");
        }
        assert_eq!(out["tool_choice"], "auto");
        // OpenAI chat wire shape: nested under "function".
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "bash");
    }

    #[test]
    fn injection_is_idempotent_and_preserves_downstream_tools() {
        let ex = zen_executor();
        let body = json!({
            "model": "muse-spark-1.3-contributor-free",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "my_tool", "parameters": {}}}],
            "tool_choice": "required"
        });
        let once = ex.inject_zen_free_tier_tools(CHAT_URL, std::borrow::Cow::Borrowed(&body));
        let names_once = tool_names(once.as_ref());
        assert_eq!(names_once.len(), OPENCODE_ZEN_TOOL_NAMES.len() + 1);
        assert!(names_once.iter().any(|e| e == "my_tool"));
        // Downstream tool_choice is never overwritten.
        assert_eq!(once["tool_choice"], "required");
        // Second pass must be a no-op (per-key retry safety).
        let twice = ex.inject_zen_free_tier_tools(CHAT_URL, once.clone());
        assert_eq!(tool_names(twice.as_ref()).len(), names_once.len());
    }

    #[test]
    fn responses_wire_uses_flat_name() {
        let ex = zen_executor();
        let body = json!({"model": "muse-spark-1.3-contributor-free", "input": "hi"});
        let out = ex.inject_zen_free_tier_tools(RESP_URL, std::borrow::Cow::Borrowed(&body));
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["name"], "bash");
        assert!(out["tools"][0].get("function").is_none());
    }

    #[test]
    fn messages_wire_uses_anthropic_schema() {
        let ex = zen_executor();
        let body = json!({"model": "mimo-v2.5-free", "messages": [], "max_tokens": 16});
        let out = ex.inject_zen_free_tier_tools(MSG_URL, std::borrow::Cow::Borrowed(&body));
        assert_eq!(out["tools"][0]["name"], "bash");
        assert!(out["tools"][0].get("input_schema").is_some());
        assert!(out["tools"][0].get("function").is_none());
    }

    #[test]
    fn paid_model_and_non_zen_scope_stay_untouched() {
        let ex = zen_executor();
        let paid = json!({"model": "glm-5.3", "messages": []});
        let out = ex.inject_zen_free_tier_tools(CHAT_URL, std::borrow::Cow::Borrowed(&paid));
        assert!(out.get("tools").is_none());

        let plain_pool = Arc::new(KeyPool::new("zen-provider", RoutingStrategy::RoundRobin));
        let plain = UpstreamExecutor::new(plain_pool, 1);
        let free = json!({"model": "mimo-v2.5-free", "messages": []});
        let out = plain.inject_zen_free_tier_tools(CHAT_URL, std::borrow::Cow::Borrowed(&free));
        assert!(out.get("tools").is_none());
    }

    #[tokio::test]
    async fn test_excluded_keys_force_fresh_key_selection() {
        // R2: the outer empty-STOP loop passes already-tried keys; the next
        // executor call must not re-select them. Priority pool would
        // otherwise pin the same key forever.
        use axum::response::IntoResponse;
        use std::sync::Arc;
        let seen_auth = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = seen_auth.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |headers: axum::http::HeaderMap, _body: String| {
                let seen = seen.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    seen.lock().unwrap().push(auth);
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({"ok": true})),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let endpoint = format!("http://{}/v1/chat/completions", addr);

        let pool = Arc::new(KeyPool::new("p", RoutingStrategy::Priority));
        pool.add_key(ApiKeyEntry::new("k1", "sk-k1", 1, 10));
        pool.add_key(ApiKeyEntry::new("k2", "sk-k2", 2, 10));
        let body = serde_json::json!({"model": "m", "messages": []});

        // Baseline: Priority always picks k1.
        let base = UpstreamExecutor::new(pool.clone(), 1);
        let (_, _, kid) = base
            .execute_stream_request_with_timing_and_key(&endpoint, &body)
            .await
            .unwrap();
        assert_eq!(kid, "k1");

        // With k1 excluded, the same Priority pool must yield k2.
        let excl = UpstreamExecutor::new(pool.clone(), 1)
            .with_excluded_keys(&["k1".to_string()]);
        let (_, _, kid2) = excl
            .execute_stream_request_with_timing_and_key(&endpoint, &body)
            .await
            .unwrap();
        assert_eq!(kid2, "k2");
        let auths = seen_auth.lock().unwrap();
        assert!(auths.iter().any(|a| a.contains("sk-k2")), "upstream must see k2: {auths:?}");
    }

    #[test]
    fn test_sanitize_proxy_url() {
        assert_eq!(
            sanitize_proxy_url("http://user:secret123@100.105.241.39:8899"),
            "http://***:***@100.105.241.39:8899/"
        );
        assert_eq!(
            sanitize_proxy_url("http://127.0.0.1:8899"),
            "http://127.0.0.1:8899/"
        );
        assert_eq!(
            sanitize_proxy_url("user:pass@host:8080"),
            "***@host:8080"
        );
    }
}
