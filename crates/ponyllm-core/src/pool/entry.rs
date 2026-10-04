use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use crate::pool::antigravity::{AntigravityTokenManager, QuotaSummaryGroup};

/// Process-wide jitter counter: mixed with wall-clock nanos so concurrent
/// instances and synchronized retries desynchronize (B1). Not cryptographic,
/// only backoff decorrelation.
static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ceiling for any single cooldown: long enough to cover weekly quota windows
/// (Antigravity exposes both 5h and weekly buckets), short enough to bound a
/// garbled or hostile upstream reset hint.
const MAX_COOLDOWN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Freeze window for upstream account/product-eligibility 403s (e.g. Antigravity
/// "Your current account is not eligible for Gemini Code Assist for
/// individuals"): the account cannot use the product until its upstream status
/// changes. Longer than any quota window so the pool routes around the account
/// for days instead of hammering it every 60s (the pre-freeze behavior that
/// exhausted whole pools mid-request and interrupted agent runs).
const ELIGIBILITY_FREEZE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

fn backoff_jitter_millis(spread: u64) -> u64 {
    let n = JITTER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Knuth multiplicative mix of counter and clock, then bound.
    let mixed = n
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(nanos.rotate_left(17));
    (mixed >> 11) % spread.max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Active,
    CoolingDown,
    Disabled,
}

#[derive(Debug, Clone)]
pub enum PoolErrorType {
    RateLimit { retry_after: Option<Duration> },
    /// Quota exhaustion cools the key down (never permanently disables):
    /// real quota recovers at `resetTime`, fake 429-style throttling clears
    /// on its own. `retry_after` defaults to a conservative 15 minutes when
    /// the upstream gave no explicit signal.
    QuotaExhausted { retry_after: Option<Duration> },
    AuthInvalid { reason: Option<String> },
    PolicyViolation,
    /// Google 账号需要人工验证（403 VALIDATION_REQUIRED）：永久隔离，
    /// 等人工完成验证/重新授权后恢复；绝不能靠等待额度窗口自动恢复。
    AccountValidationRequired,
    /// 上游账号/产品资格类 403（如 Antigravity Gemini Code Assist
    /// "Your current account is not eligible for ..."）：账号当前无该产品资格，
    /// 既非瞬态也非永久——长冷冻（数日）后由调度自动路由到池内其它账号，
    /// 请求继续、agent 不中断；账号不摘除、不永久禁用。
    AccountEligibility { reason: Option<String> },
    ServerError,
    NetworkError,
}

/// Why a key is cooling down. The quota boundary guard (bugfix 2026-10-02)
/// needs this to distinguish "account quota exhausted" (must not cross to a
/// second provider carrying the same model) from transient rate-limit or
/// server faults (may cross) when a pool has no schedulable key left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownReason {
    /// Account/model quota or balance exhausted (402 / balance-wording 429 /
    /// balance-wording 403 / antigravity quota frames).
    Quota,
    /// Sliding-window rate limit (RPM/TPM/concurrency 429).
    RateLimit,
    /// Server/network fault or an operator-administered cooldown.
    Server,
    /// Upstream account/product-eligibility rejection (403 "not eligible for").
    /// The account is frozen for days; only an upstream status change revives it.
    Eligibility,
}

impl CooldownReason {
    /// Stable wire name for admin/observability surfaces. The web pool matrix
    /// keys off `"eligibility"` to render a long-frozen account in red.
    pub fn as_str(&self) -> &'static str {
        match self {
            CooldownReason::Quota => "quota",
            CooldownReason::RateLimit => "rate_limit",
            CooldownReason::Server => "server",
            CooldownReason::Eligibility => "eligibility",
        }
    }
}

/// Model family a request belongs to, for Antigravity quota-group aware
/// scheduling. Antigravity exposes *group* buckets (e.g. "Gemini Models" vs
/// "Claude and GPT models"): a key whose Gemini weekly bucket is exhausted can
/// still serve Claude/GPT traffic, so exhaustion must be judged per family
/// instead of per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaFamily {
    Gemini,
    ThirdParty,
}

/// Classify a client-requested model into an Antigravity quota family.
/// `None` = unknown family: the key-selection filter stays permissive (today's
/// behavior), so non-Antigravity providers and unclassified models are never
/// accidentally starved.
pub fn classify_quota_family(model: &str) -> Option<QuotaFamily> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gemini") {
        Some(QuotaFamily::Gemini)
    } else if m.starts_with("claude") || m.starts_with("gpt") || m.contains("oss") {
        Some(QuotaFamily::ThirdParty)
    } else {
        None
    }
}

/// Conservative reset horizon when an exhausted quota bucket advertises no
/// `reset_time`: far enough to stop the 429 storm, short enough that a stale
/// verdict self-expires and never over-blocks past a real recovery.
const DEFAULT_EXHAUSTED_RESET_FALLBACK_HOURS: i64 = 6;

fn group_matches_family(name: &str, family: QuotaFamily) -> bool {
    let n = name.to_ascii_lowercase();
    match family {
        QuotaFamily::Gemini => n.contains("gemini"),
        QuotaFamily::ThirdParty => {
            n.contains("claude") || n.contains("gpt") || n.contains("3p") || n.contains("third")
        }
    }
}

/// Canonical ledger key for a family, used by the request-path 429 writeback
/// (`set_family_quota_exhausted`) which has no upstream group display_name.
fn family_group_key(family: QuotaFamily) -> &'static str {
    match family {
        QuotaFamily::Gemini => "Gemini Models",
        QuotaFamily::ThirdParty => "Claude and GPT models",
    }
}

#[derive(Debug)]
pub struct KeyStats {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub consecutive_failures: AtomicUsize,
    pub policy_violations: AtomicUsize,
    pub cooldown_until: RwLock<Option<Instant>>,
    /// Wall-clock mirror of `cooldown_until`, so observability surfaces
    /// (admin API / Web badge) can render the advertised reset time without
    /// reverse-engineering a monotonic clock.
    pub cooldown_reset_at: RwLock<Option<SystemTime>>,
    /// Why this key is cooling down, when it is. The quota boundary guard
    /// (bugfix 2026-10-02) uses this to tell "account quota exhausted" apart
    /// from transient rate-limit / server faults when a pool has no
    /// schedulable key left.
    pub cooldown_reason: RwLock<Option<CooldownReason>>,
    pub disabled_reason: RwLock<Option<String>>,
    /// Human-readable reason for a *hard cooldown* that is not a permanent
    /// disable — currently the upstream account/product-eligibility message.
    /// Kept separate from `disabled_reason` because `current_state()` treats
    /// a non-None `disabled_reason` as permanently Disabled, while an
    /// eligibility freeze must stay `CoolingDown` and auto-recover when the
    /// cooldown expires.
    pub error_reason: RwLock<Option<String>>,
}

impl Default for KeyStats {
    fn default() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            consecutive_failures: AtomicUsize::new(0),
            policy_violations: AtomicUsize::new(0),
            cooldown_until: RwLock::new(None),
            cooldown_reset_at: RwLock::new(None),
            cooldown_reason: RwLock::new(None),
            disabled_reason: RwLock::new(None),
            error_reason: RwLock::new(None),
        }
    }
}

#[derive(Clone)]
pub enum KeyAuth {
    Static(String),
    Antigravity(Arc<AntigravityTokenManager>),
}

impl std::fmt::Debug for KeyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(key) => {
                let masked = if key.len() > 8 {
                    format!("{}...{}", &key[..4], &key[key.len() - 4..])
                } else {
                    "***".to_string()
                };
                write!(f, "Static({})", masked)
            }
            Self::Antigravity(_) => write!(f, "Antigravity(TokenManager)"),
        }
    }
}

pub struct ApiKeyEntry {
    pub id: String,
    pub api_key: String,
    pub auth: KeyAuth,
    pub account_id: Option<String>,
    pub priority: u32,
    pub weight: u32,
    pub stats: KeyStats,
    pub usage_tracker: Arc<crate::pool::usage::KeyUsageTracker>,
    /// Per-key 60s short-window meter (requests/tokens + in-flight
    /// concurrency), consumed by the pool scheduler for budget filtering and
    /// by the executor for attempt/success accounting (M1/M2, ADR
    /// `2026-09-30-unified-quota-metering-governance-kernel`).
    pub short_meter: Arc<crate::pool::meter::ShortWindowMeter>,
    /// Antigravity quota-group exhaustion ledger: group display_name → wall-clock
    /// reset of its (exhausted) weekly bucket. Kept separate from the key-level
    /// cooldown because exhaustion is *family-scoped*: a key with Gemini weekly
    /// exhausted can still serve Claude/GPT and must stay schedulable for those
    /// requests (ADR `2026-10-04-antigravity-group-quota-aware-scheduling`).
    pub quota_group_exhausted: Arc<RwLock<HashMap<String, DateTime<Utc>>>>,
}

// Manual Debug: `#[derive(Debug)]` would print `api_key` verbatim into
// logs/dumps (P0-8). Only a short non-sensitive preview is emitted.
impl std::fmt::Debug for ApiKeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyEntry")
            .field("id", &self.id)
            .field("account_id", &self.account_id)
            .field("api_key", &crate::telemetry::FlightRecorder::sanitize_key(&self.api_key))
            .field("auth", &self.auth)
            .field("priority", &self.priority)
            .field("weight", &self.weight)
            .finish()
    }
}

impl ApiKeyEntry {
    pub fn new(id: impl Into<String>, api_key: impl Into<String>, priority: u32, weight: u32) -> Self {
        let k = api_key.into();
        Self {
            id: id.into(),
            api_key: k.clone(),
            auth: KeyAuth::Static(k),
            account_id: None,
            priority,
            weight,
            stats: KeyStats::default(),
            usage_tracker: Arc::new(crate::pool::usage::KeyUsageTracker::new()),
            short_meter: Arc::new(crate::pool::meter::ShortWindowMeter::new()),
            quota_group_exhausted: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_account_id(mut self, account_id: Option<String>) -> Self {
        self.account_id = account_id;
        self
    }

    /// Effective tenant/account identity: returns configured account_id, or falls back to key id.
    pub fn effective_account_id(&self) -> &str {
        self.account_id.as_deref().unwrap_or(&self.id)
    }

    pub fn new_antigravity(
        id: impl Into<String>,
        manager: Arc<AntigravityTokenManager>,
        priority: u32,
        weight: u32,
    ) -> Self {
        Self {
            id: id.into(),
            api_key: String::new(),
            auth: KeyAuth::Antigravity(manager),
            account_id: None,
            priority,
            weight,
            stats: KeyStats::default(),
            usage_tracker: Arc::new(crate::pool::usage::KeyUsageTracker::new()),
            short_meter: Arc::new(crate::pool::meter::ShortWindowMeter::new()),
            quota_group_exhausted: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn is_antigravity(&self) -> bool {
        matches!(self.auth, KeyAuth::Antigravity(_))
    }

    /// The key's short-window meter (shared accessor for the scheduler and
    /// the executor accounting path).
    pub fn meter(&self) -> &crate::pool::meter::ShortWindowMeter {
        self.short_meter.as_ref()
    }

    pub fn antigravity_manager(&self) -> Option<Arc<AntigravityTokenManager>> {
        match &self.auth {
            KeyAuth::Antigravity(mgr) => Some(mgr.clone()),
            _ => None,
        }
    }

    pub async fn resolve_token(&self) -> crate::error::Result<String> {
        match &self.auth {
            KeyAuth::Static(key) => Ok(key.clone()),
            KeyAuth::Antigravity(mgr) => mgr.get_valid_token().await,
        }
    }

    /// Check the current effective state of the key with fast read-path
    pub fn current_state(&self) -> KeyState {
        if self.stats.disabled_reason.read().is_some() {
            return KeyState::Disabled;
        }

        // Fast read path: avoid write-lock contention under heavy concurrent reads
        {
            let cd_read = self.stats.cooldown_until.read();
            if let Some(until) = *cd_read {
                if Instant::now() < until {
                    return KeyState::CoolingDown;
                }
            } else {
                return KeyState::Active;
            }
        }

        // Slow path: upgrade to write lock only to reset expired cooldown
        let mut cd_write = self.stats.cooldown_until.write();
        if let Some(until) = *cd_write {
            if Instant::now() >= until {
                *cd_write = None;
                *self.stats.cooldown_reset_at.write() = None;
                *self.stats.cooldown_reason.write() = None;
                *self.stats.error_reason.write() = None;
                self.stats.consecutive_failures.store(0, Ordering::SeqCst);
                KeyState::Active
            } else {
                KeyState::CoolingDown
            }
        } else {
            KeyState::Active
        }
    }

    /// Record Antigravity quota-group bucket exhaustion into the family-scoped
    /// ledger. A group is exhausted when ANY of its window buckets (weekly or
    /// 5h/individual — the incident 429 "Individual quota reached ... Resets
    /// in 3h56m" is exactly the individual class) reads `remaining_fraction
    /// <= 0.0`; the group stays blocked for that family until the farthest
    /// reset. A bucket without `reset_time` falls back to a conservative
    /// horizon so a missing field never re-opens the storm. Everything else is
    /// cleared so recovered groups become schedulable immediately.
    pub fn apply_quota_groups(&self, groups: Option<&[QuotaSummaryGroup]>, now: DateTime<Utc>) {
        let mut ledger = self.quota_group_exhausted.write();
        // Drop entries whose reset has already passed (stale verdicts).
        ledger.retain(|_, reset| *reset > now);
        match groups {
            None => {}
            Some(groups) => {
                for g in groups {
                    let mut exhausted_reset: Option<DateTime<Utc>> = None;
                    for b in &g.buckets {
                        if b.remaining_fraction <= 0.0 {
                            let reset = b.reset_time.unwrap_or_else(|| {
                                now + chrono::Duration::hours(DEFAULT_EXHAUSTED_RESET_FALLBACK_HOURS)
                            });
                            exhausted_reset = Some(match exhausted_reset {
                                Some(cur) => cur.max(reset),
                                None => reset,
                            });
                        }
                    }
                    if let Some(reset) = exhausted_reset {
                        if reset > now {
                            ledger.insert(g.display_name.clone(), reset);
                            continue;
                        }
                    }
                    ledger.remove(&g.display_name);
                }
            }
        }
    }

    /// Request-path 429 writeback (ADR
    /// `2026-10-04-antigravity-group-quota-aware-scheduling`): an upstream
    /// quota rejection for a known family records the family group's reset
    /// directly, so the pre-exclusion ledger self-heals between keepalive
    /// refreshes instead of waiting for the next probe.
    pub fn set_family_quota_exhausted(&self, family: QuotaFamily, reset_at: DateTime<Utc>) {
        if reset_at <= Utc::now() {
            return;
        }
        self.quota_group_exhausted
            .write()
            .insert(family_group_key(family).to_string(), reset_at);
    }

    /// Whether this key is currently group-exhausted for the given family:
    /// true only when a matching group has an unexpired weekly-exhaustion reset.
    /// `None` (unknown family / non-Antigravity provider) never filters.
    pub fn quota_group_exhausted_for(&self, family: Option<QuotaFamily>, now: DateTime<Utc>) -> bool {
        let Some(family) = family else { return false };
        let ledger = self.quota_group_exhausted.read();
        ledger
            .iter()
            .any(|(name, reset)| *reset > now && group_matches_family(name, family))
    }

    /// Snapshot of the group-exhaustion ledger for runtime-state inheritance
    /// across pool rebuilds (keeps the pre-computed verdict on hot reload).
    pub fn quota_group_exhaustions(&self) -> HashMap<String, DateTime<Utc>> {
        self.quota_group_exhausted.read().clone()
    }

    /// Restore a group-exhaustion ledger (from [`Self::quota_group_exhaustions`]).
    pub fn restore_quota_group_exhaustions(&self, ledger: HashMap<String, DateTime<Utc>>) {
        *self.quota_group_exhausted.write() = ledger;
    }

    /// Record a successful request
    pub fn record_success(&self) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// Record token consumption on this key
    pub fn record_tokens(&self, wall_ms: u64, prompt: u64, completion: u64, cached: u64) {
        self.usage_tracker.record_tokens(wall_ms, prompt, completion, cached);
    }

    /// Clear any active cooldown, immediately returning the key to Active state.
    pub fn clear_cooldown(&self) {
        let mut cd = self.stats.cooldown_until.write();
        *cd = None;
        *self.stats.cooldown_reset_at.write() = None;
        *self.stats.cooldown_reason.write() = None;
        *self.stats.error_reason.write() = None;
        self.stats.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// Clear disabled state, restoring key to active unless cooling down.
    pub fn clear_disabled(&self) {
        *self.stats.disabled_reason.write() = None;
        self.stats.policy_violations.store(0, Ordering::SeqCst);
        self.stats.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// Remaining cooldown, if still cooling.
    pub fn cooldown_remaining(&self) -> Option<Duration> {
        let guard = self.stats.cooldown_until.read();
        let until = (*guard)?;
        let now = Instant::now();
        if now < until {
            Some(until - now)
        } else {
            None
        }
    }

    /// Wall-clock instant the key is expected to recover, while still cooling.
    pub fn cooldown_reset_at(&self) -> Option<SystemTime> {
        let until = (*self.stats.cooldown_until.read())?;
        if Instant::now() >= until {
            return None;
        }
        *self.stats.cooldown_reset_at.read()
    }

    /// Why this key is currently cooling down, when known.
    pub fn cooldown_reason(&self) -> Option<CooldownReason> {
        let until = (*self.stats.cooldown_until.read())?;
        if Instant::now() >= until {
            return None;
        }
        *self.stats.cooldown_reason.read()
    }

    /// Concrete reason why the key is disabled, if permanently isolated.
    pub fn disabled_reason(&self) -> Option<String> {
        self.stats.disabled_reason.read().clone()
    }

    /// Raw stored hard-error message (`error_reason`), regardless of current
    /// state gating. Used by pool-level state inheritance on config rebuilds.
    pub fn raw_error_reason(&self) -> Option<String> {
        self.stats.error_reason.read().clone()
    }

    /// Human-readable reason for a *hard* non-active state: an upstream
    /// eligibility freeze (cooldown reason `Eligibility`) or a permanent
    /// disable. `None` for soft cooldowns (quota / rate-limit / server).
    pub fn error_reason(&self) -> Option<String> {
        let hard = self.current_state() == KeyState::Disabled
            || matches!(self.cooldown_reason(), Some(CooldownReason::Eligibility));
        if hard {
            if matches!(self.cooldown_reason(), Some(CooldownReason::Eligibility)) {
                self.stats.error_reason.read().clone()
            } else {
                self.disabled_reason()
            }
        } else {
            None
        }
    }

    /// Apply a cooldown, keeping the monotonic deadline and its wall-clock
    /// mirror in lockstep.
    ///
    /// A later deadline always wins: an in-flight request that fails with a
    /// short transient error must never shorten a cooldown already advertised
    /// by a quota reset, or the key starts receiving traffic again mid-window.
    ///
    /// Both fields are written inside one `cooldown_until` critical section so
    /// concurrent failures cannot install a longer deadline and then lose the
    /// matching mirror (the Web badge would under-report and hide the hint
    /// while the key still cools). Lock order stays `until -> reset`, matching
    /// `current_state()`, so there is no inversion. Durations are clamped to
    /// [`MAX_COOLDOWN`]; if either clock addition fails the whole update is
    /// skipped, so a garbled/hostile reset can neither panic nor desync.
    pub fn set_cooldown(&self, duration: Duration) {
        let duration = duration.min(MAX_COOLDOWN);
        let Some(deadline) = Instant::now().checked_add(duration) else {
            return;
        };
        let Some(reset_at) = SystemTime::now().checked_add(duration) else {
            return;
        };
        let mut cd = self.stats.cooldown_until.write();
        if let Some(existing) = *cd {
            if existing >= deadline {
                return;
            }
        }
        *self.stats.cooldown_reset_at.write() = Some(reset_at);
        *cd = Some(deadline);
    }

    /// Count a transient 429 without cooling, for singleton pools that
    /// always passthrough upstream instead of local isolation.
    pub fn record_transient_failure(&self) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.consecutive_failures.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a failed request and transition state accordingly
    pub fn record_failure(&self, err_type: PoolErrorType) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
        let consecutive = self.stats.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

        // A key already in a 3-day eligibility freeze stays frozen with its
        // calling reason: only an `AccountEligibility` (re-)proof may refresh
        // the message. Any other error type (a stray quota/rate-limit/server
        // hit racing an in-flight request or a probe) must not downgrade the
        // freeze's `cooldown_reason`/`error_reason` — otherwise the web red
        // badge loses the reason and the operator cannot tell eligibility
        // apart from a soft cooldown. The pending cooldown deadline is still
        // extended (set_cooldown keeps the max), never shortened.
        let frozen_eligibility =
            matches!(self.cooldown_reason(), Some(CooldownReason::Eligibility));

        match err_type {
            PoolErrorType::RateLimit { retry_after } => {
                let duration = match retry_after {
                    Some(d) if d >= Duration::from_secs(300) => {
                        // Long policy-driven coolings (geo-gate #1008,
                        // quota): escalate 5m -> 30m -> 2h cap across
                        // consecutive hits so a sustained storm backs off
                        // instead of knocking every 5 minutes (B3).
                        let step = consecutive.min(4).saturating_sub(1) as u32;
                        d.saturating_mul(2u32.saturating_pow(step)).min(Duration::from_secs(7200))
                    }
                    Some(d) => d,
                    None => {
                        let base_multiplier = 2u64.saturating_pow((consecutive as u32).saturating_sub(1));
                        let base_secs = (3u64.saturating_mul(base_multiplier)).min(60);
                        Duration::from_millis(base_secs * 1000 + backoff_jitter_millis(500))
                    }
                };
                self.set_cooldown(duration);
                if !frozen_eligibility {
                    *self.stats.cooldown_reason.write() = Some(CooldownReason::RateLimit);
                }
            }
            PoolErrorType::QuotaExhausted { retry_after } => {
                // Quota exhaustion is transient by nature (real quota
                // recovers at resetTime; throttling clears on its own), so
                // it cools down instead of permanently disabling the key.
                // `retry_after` carries the upstream-advertised reset when
                // available; the 15m default is a conservative fallback.
                let duration = retry_after.unwrap_or(Duration::from_secs(15 * 60));
                self.set_cooldown(duration);
                if !frozen_eligibility {
                    *self.stats.cooldown_reason.write() = Some(CooldownReason::Quota);
                }
            }
            PoolErrorType::AuthInvalid { reason } => {
                let msg = reason.unwrap_or_else(|| "Authentication failed (invalid key)".to_string());
                if !frozen_eligibility {
                    *self.stats.disabled_reason.write() = Some(msg);
                }
            }
            PoolErrorType::AccountEligibility { reason } => {
                // Account/product-eligibility rejection: not a 60s blip and not
                // a permanent isolate. Freeze for days so scheduling routes
                // around the account; the upstream status change is the only
                // thing that revives it. The (bounded) upstream message is
                // stored in `error_reason` (NOT `disabled_reason`, which would
                // flip the state to permanent Disabled) so admin/observability
                // can render the exact reason in red.
                let msg = reason.unwrap_or_else(|| {
                    "Account not eligible for the requested product (upstream 403)".to_string()
                });
                // Bound the stored reason: upstream bodies can be huge / hostile
                // (mirrors the request-path MAX_UPSTREAM_ERROR_BYTES backstop).
                let bounded = truncate_reason(&msg);
                self.set_cooldown(ELIGIBILITY_FREEZE);
                *self.stats.cooldown_reason.write() = Some(CooldownReason::Eligibility);
                *self.stats.error_reason.write() = Some(bounded);
            }
            PoolErrorType::PolicyViolation => {
                *self.stats.disabled_reason.write() = Some("Account policy violation / Terms of Service suspension (permanent isolate)".to_string());
            }
            PoolErrorType::AccountValidationRequired => {
                *self.stats.disabled_reason.write() = Some("Google account verification required (VALIDATION_REQUIRED): complete verification in the Google account, then reauthorize".to_string());
            }
            PoolErrorType::ServerError | PoolErrorType::NetworkError => {
                if consecutive >= 3 {
                    let exp = (consecutive as u32).saturating_sub(3);
                    let secs = (1u64.saturating_mul(2u64.saturating_pow(exp))).min(30);
                    self.set_cooldown(Duration::from_secs(secs));
                    if !frozen_eligibility {
                        *self.stats.cooldown_reason.write() = Some(CooldownReason::Server);
                    }
                }
            }
        }
    }
}

/// Bound a stored upstream reason (eligibility message) to a sane size for
/// memory / admin payload safety. Mirrors the request-path backstop
/// (`MAX_UPSTREAM_ERROR_BYTES`) and the web-side display truncation.
fn truncate_reason(msg: &str) -> String {
    const MAX_REASON_CHARS: usize = 2048;
    if msg.chars().count() <= MAX_REASON_CHARS {
        return msg.to_string();
    }
    let trimmed: String = msg.chars().take(MAX_REASON_CHARS).collect();
    format!("{}…(truncated)", trimmed)
}
