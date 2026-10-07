use ponyllm_core::pool::{
    default_cached_price, default_input_price, default_output_price, BillingMode,
    GatewayRoutingStrategy, ModelThinkingSpec, ModelTier, PricingConfig, PricingMode,
    PricingPeriod, UpstreamProtocol,
};
use ponyllm_protocol::common::ReasoningEffort;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use ponyllm_core::telemetry::FlightRecorder;
use serde::{Deserialize, Serialize};

use crate::commercial::CommercialConfig;

/// Re-export the canonical short-window rate-limits type (defined in
/// `ponyllm-core::pool` so the scheduler can consume it without a dependency
/// cycle; `ponyllm-config` owns the TOML surface and resolution helpers).
pub use ponyllm_core::pool::RateLimits;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub gateway: GatewaySection,
    #[serde(default)]
    pub providers: HashMap<String, ProviderSection>,
    /// Opt-in commercial profile (`[commercial]`, Stage 1 scaffolding).
    /// Absent in old TOMLs deserializes to `CommercialConfig::default()` with
    /// `enabled = false` (zero migration). Nothing consumes it at runtime yet;
    /// paid inference stays hard-disabled until Stage 2. Callers must gate on
    /// `commercial.validate()` before enabling. See `crate::commercial` for the
    /// fail-closed rules and the no-persisted-secret contract.
    #[serde(default)]
    pub commercial: CommercialConfig,
    /// Monotonic version bumped on every successful admin `ConfigStore::save`
    /// (WEB-03: strategy PUT echoes it; WEB-06 builds If-Match on it). Old
    /// TOMLs without the key deserialize as 0 and are migrated on next save.
    #[serde(default)]
    pub config_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySection {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_retries")]
    pub max_retries: usize,
    #[serde(default = "default_capacity")]
    pub flight_recorder_capacity: usize,
    /// Total wall-clock budget for one upstream call, in seconds (default 1200 =
    /// 20 minutes). Long-thinking streams routinely exceed the legacy 120s
    /// budget; the gateway replaces it with TTFB + tail-stall detection, so a
    /// genuinely dead stream still fails fast instead of pinning the
    /// connection for the whole budget.
    #[serde(default = "default_upstream_timeout_secs")]
    pub upstream_timeout_secs: u64,
    /// Optional TTFB (Time to First Byte / response headers) budget in seconds for upstream calls.
    /// Defaults to None (resolves to 90s). Setting to `Some(0)` disables the TTFB timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_ttfb_timeout_secs: Option<u64>,
    /// Request-level wall-clock budget (seconds) for the pre-commit empty-STOP
    /// transparent retry phase. None resolves to 75s; Some(0) disables the bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_stop_total_timeout_secs: Option<u64>,
    #[serde(default = "default_api_key")]
    pub api_key: String,
    #[serde(default)]
    pub default_strategy: GatewayRoutingStrategy,
    #[serde(default = "default_request_body_limit")]
    pub request_body_limit: usize,
    /// Optional default outbound HTTP proxy for upstream providers (e.g. "http://127.0.0.1:8899").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Whether to inherit system environment proxies (`http_proxy`/`https_proxy`).
    /// Defaults to `false` to isolate gateway from host terminal proxy pollution.
    #[serde(default)]
    pub use_system_proxy: bool,
    /// Whether `serve` mounts the web console (`web/dist`) under `/app/*`.
    /// Defaults to `true`; `--no-web` CLI flag forces `false` (WEB-01).
    #[serde(default = "default_web_enabled")]
    pub web_enabled: bool,
    /// Directory served as the web console SPA. Relative to the serve working
    /// directory; absolute paths preferred for services (WEB-01).
    #[serde(default = "default_web_dist_dir")]
    pub web_dist_dir: String,
    /// Whether admin write operations (CUD and dial-test) are enabled (WEB-06).
    /// Defaults to `false` for security; must be explicitly enabled.
    #[serde(default = "default_admin_write_enabled")]
    pub admin_write_enabled: bool,
    /// Telemetry snapshot file for dashboard persistence. Empty = derive
    /// `<config-dir>/telemetry-snapshot.json` in serve; set explicit path to override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_snapshot_path: Option<String>,
    /// Auth compatibility mode (P0, task-20; contract `.agents/notes/auth-eval.md` §3.1):
    /// `legacy-only` (current behavior) | `dual` (default, old token fully
    /// privileged) | `strict` (legacy token rejected). Missing field in old
    /// configs deserializes to `dual` (zero-migration); unknown values
    /// fail-fast at parse time (never silently fall back).
    #[serde(default = "default_auth_compat")]
    pub auth_compat: AuthCompat,
    /// Scoped gateway keys (P1, task-21; contract `.agents/notes/auth-eval.md` §3.2).
    /// Empty = legacy single `api_key` (auto-mapped to `admin`, zero migration).
    /// Only password hashes are persisted, never plaintext.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gateway_keys: Vec<GatewayKeyEntry>,
    /// Whether background auto-refresh & keepalive for Antigravity accounts is enabled.
    /// Defaults to `true` to keep standby accounts from expiring (Google 180-day rule).
    #[serde(default = "default_antigravity_auto_refresh")]
    pub antigravity_auto_refresh: bool,
    /// Background interval in seconds for Antigravity auto-refresh (default 86400 = 24h).
    #[serde(default = "default_antigravity_refresh_interval_secs")]
    pub antigravity_refresh_interval_secs: u64,
    /// Whether a quota-exhaustion failure on one provider may transparently fail
    /// over to another provider carrying the same model (legacy HA behavior).
    /// Default `false`: quota exhaustion surfaces `insufficient_quota` instead of
    /// silently consuming a second provider's quota. Transient faults
    /// (network / 5xx / TTFB / timeout) always keep cross-provider failover.
    #[serde(default)]
    pub cross_provider_quota_failover: bool,
    /// Explicit authentication mode (Phase-2 F1): `secured` (default,
    /// fail-closed — credentials always required) | `open` (explicit opt-in).
    /// The legacy implicit open-on-empty-key behavior is removed.
    #[serde(default = "default_auth_mode")]
    pub auth_mode: AuthMode,
    /// F2 (VULN-01): sliding-window length for failed-auth counting (seconds).
    #[serde(default = "default_auth_fail_window_secs")]
    pub auth_fail_window_secs: u64,
    /// F2 (VULN-01): failed-auth budget per (client IP, key prefix) per window;
    /// exceeding it locks the pair for `auth_lockout_secs` (tiered backoff).
    #[serde(default = "default_auth_fail_limit")]
    pub auth_fail_limit: u32,
    /// F2 (VULN-01): base lockout duration after the budget is exceeded
    /// (escalates 900s → 3600s → 14400s on repeated lockouts).
    #[serde(default = "default_auth_lockout_secs")]
    pub auth_lockout_secs: u64,
    /// F4 (VULN-02): admin IP fence. CIDR list (e.g. `203.0.113.0/24,10.0.0.0/8`);
    /// non-empty → `/api/admin/*` requires the resolved client IP inside the
    /// list, otherwise 404 (fail-closed). `PONYLLM_ADMIN_IP_ALLOWLIST` env
    /// (comma-separated) overrides at app build time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_ip_allowlist: Vec<String>,
    /// F3 (VULN-12): exact proxy IPs trusted to append `X-Forwarded-For`
    /// (e.g. EdgeOne回源网段). `PONYLLM_TRUSTED_PROXIES` env overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
    /// Phase-3 (VULN-05): HttpOnly cookie admin sessions (`PONYLLM_ADMIN_SESSION_ENABLED=1`
    /// env also enables at app build time). Default `false`: session routes
    /// are absent and cookie auth is off — behavior identical to pre-session.
    #[serde(default = "default_admin_session_enabled")]
    pub admin_session_enabled: bool,
    /// Phase-3: session TTL in seconds (default 28800 = 8h, sliding refresh on
    /// every validated use). `PONYLLM_ADMIN_SESSION_TTL_SECS` overrides (test hook).
    #[serde(default = "default_admin_session_ttl_secs")]
    pub admin_session_ttl_secs: u64,
}

/// Scoped gateway credential (P1): one entry per issued key.
///
/// `key_hash` is `sha256_hex(salt + "::" + plaintext)`; `salt` is per-entry
/// random hex. The plaintext is shown ONCE at issuance and never stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayKeyEntry {
    /// Stable identifier (e.g. `agent-ci-1`).
    pub id: String,
    /// Key scope: `admin` | `inference` | `readonly` (see `KeyScope`).
    pub scope: KeyScope,
    /// Plaintext prefix for operator identification (e.g. `sk-pony-admin-`);
    /// full key = `{prefix}{random}`.
    pub prefix: String,
    /// Random hex salt for the stored hash.
    pub salt: String,
    /// SHA-256 hex of `salt + "::" + plaintext`.
    pub key_hash: String,
    /// Optional UNIX expiry seconds; `None` = never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Legacy flag, kept for deserializing entries written before the
    /// 2026-09-21 hard-delete semantic (nothing writes `true` anymore;
    /// deletion removes the entry outright). Still enforced by
    /// `authenticate` so a stale on-disk `revoked = true` keeps failing closed.
    #[serde(default)]
    pub revoked: bool,
    /// Last 4 chars of the plaintext, persisted at issuance for operator
    /// identification (task-27; contract `web-users-api.md` §1). The server
    /// stores only hashes and cannot recompute this — entries issued before
    /// this field deserialize to `"****"`.
    #[serde(default = "default_gateway_key_last4")]
    pub last4: String,
}

/// Gateway key scope (P1; contract §3.2 frozen names).
///
/// Three machine scopes only; `operator` stays a human-login role (Deferred A)
/// and is never issued as a machine key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum KeyScope {
    /// Full power (legacy single `api_key` maps here).
    #[default]
    Admin,
    /// Inference + quota + telemetry summaries (agent/skill keys).
    Inference,
    /// Admin reads + quota + telemetry summaries (no inference).
    Readonly,
}

impl KeyScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Inference => "inference",
            Self::Readonly => "readonly",
        }
    }

    /// Plaintext prefix identifying the scope (contract §3.2 frozen).
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Admin => "sk-pony-admin-",
            Self::Inference => "sk-pony-infer-",
            Self::Readonly => "sk-pony-read-",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "sk-pony-admin-" => Some(Self::Admin),
            "sk-pony-infer-" => Some(Self::Inference),
            "sk-pony-read-" => Some(Self::Readonly),
            _ => None,
        }
    }
}

/// Default `last4` for entries issued before the field existed (task-27).
fn default_gateway_key_last4() -> String {
    "****".to_string()
}

/// Gateway authentication mode (Phase-2 F1, VULN-17 fail-closed).
///
/// `secured` (default): a credential is ALWAYS required. `open` is explicit
/// opt-in only — the legacy "empty `api_key` implies open" behavior is gone.
/// A Secured-start gateway also refuses runtime reloads that would flip it
/// open (empty key + no scoped keys), see the `ponyllm-server` reload guard.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Secured,
    Open,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Secured => "secured",
            Self::Open => "open",
        }
    }
}

fn default_auth_mode() -> AuthMode {
    AuthMode::Secured
}

fn default_auth_fail_window_secs() -> u64 {
    60
}

fn default_auth_fail_limit() -> u32 {
    30
}

fn default_auth_lockout_secs() -> u64 {
    900
}

fn default_admin_session_enabled() -> bool {
    false
}

fn default_admin_session_ttl_secs() -> u64 {
    28800
}

/// Compute the stored hash for a scoped gateway key (P1): never store plaintext.
pub fn hash_gateway_key(salt: &str, plaintext: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b"::");
    h.update(plaintext.as_bytes());
    format!("{:x}", h.finalize())
}

/// Generate a new scoped gateway key (P1): returns `(plaintext, entry)`.
/// Plaintext is shown ONCE at issuance; only `entry` (hash) is persisted.
pub fn generate_scoped_gateway_key(
    id: impl Into<String>,
    scope: KeyScope,
) -> (String, GatewayKeyEntry) {
    use sha2::{Digest, Sha256};
    let raw = uuid::Uuid::new_v4().simple().to_string();
    let plaintext = format!("{}{}", scope.prefix(), raw);
    let salt_src = uuid::Uuid::new_v4().simple().to_string();
    let mut s = Sha256::new();
    s.update(salt_src.as_bytes());
    let salt = format!("{:x}", s.finalize())[..32].to_string();
    let key_hash = hash_gateway_key(&salt, &plaintext);
    let last4 = plaintext
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    (
        plaintext,
        GatewayKeyEntry {
            id: id.into(),
            scope,
            prefix: scope.prefix().to_string(),
            salt,
            key_hash,
            expires_at: None,
            revoked: false,
            last4,
        },
    )
}

/// Gateway auth compatibility mode (P0 auth hardening switch).
///
/// Serialized lowercase on the wire (`legacy-only | dual | strict`); unknown
/// values are rejected at deserialization (fail-fast, never default).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthCompat {
    /// Current behavior: single token, bare token accepted.
    #[serde(rename = "legacy-only")]
    LegacyOnly,
    /// Default: legacy fully privileged (bare token accepted + deprecated-auth
    /// tagging reserved for P1 telemetry).
    #[default]
    Dual,
    /// Legacy token rejected (401 with re-issue guidance); bare token rejected.
    Strict,
}

fn default_auth_compat() -> AuthCompat {
    AuthCompat::Dual
}

/// Fail-fast guard for bind/auth combinations (P0):
/// open mode (empty/`none` key) on a non-loopback bind is refused at startup.
/// Returns `Err` with a human-readable reason when the combination is unsafe.
pub fn validate_bind_auth_combo(
    bind: &str,
    api_key: &str,
    auth_compat: AuthCompat,
) -> Result<(), String> {
    let _ = auth_compat;
    let trimmed = api_key.trim();
    let open = trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none");
    if !open {
        return Ok(());
    }
    let host = bind
        .split_once(':')
        .map(|(h, _)| h.trim())
        .unwrap_or(bind.trim());
    let loopback = host.eq_ignore_ascii_case("127.0.0.1")
        || host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "[::1]";
    if loopback {
        return Ok(());
    }
    Err(format!(
        "拒绝启动：开放模式（空 api_key）禁止绑定非环回地址 '{}'（auth_compat={:?}）。请设置网关 api_key，或改绑 127.0.0.1。",
        bind, auth_compat
    ))
}

/// Weak-key guard for CLI `auth <KEY>` (P0): rejects well-known weak secrets
/// before they are persisted. Returns `Err` with a human-readable reason.
pub fn validate_gateway_key_strength(key: &str) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.len() < 16 {
        return Err(format!(
            "拒绝落盘：网关口令长度 {} < 16（弱口令）。请用 `ponyllm auth --rotate` 生成随机 Key。",
            trimmed.len()
        ));
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Err(
            "拒绝落盘：网关口令为纯数字（弱口令）。请用 `ponyllm auth --rotate` 生成随机 Key。"
                .to_string(),
        );
    }
    const BLOCKLIST: &[&str] = &[
        "123456", "password", "qwerty", "admin", "letmein", "ponyllm", "changeme", "secret",
    ];
    let lower = trimmed.to_ascii_lowercase();
    // Substring matching only applies to short keys (<24 chars): a long key
    // containing e.g. "123456" as a random hex run still carries full entropy
    // (uuid-derived suffixes hit this with ~1e-6 probability otherwise).
    // Exact blocklist equality is rejected at any length.
    let stripped: String = lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if BLOCKLIST.iter().any(|w| stripped == *w) {
        return Err(
            "拒绝落盘：网关口令为常见弱口令（弱口令）。请用 `ponyllm auth --rotate` 生成随机 Key。"
                .to_string(),
        );
    }
    if trimmed.len() < 24 && BLOCKLIST.iter().any(|w| lower.contains(w)) {
        return Err("拒绝落盘：网关口令命中常见弱口令（弱口令）。请用 `ponyllm auth --rotate` 生成随机 Key。".to_string());
    }
    Ok(())
}

fn default_admin_write_enabled() -> bool {
    false
}

fn default_antigravity_auto_refresh() -> bool {
    true
}

fn default_antigravity_refresh_interval_secs() -> u64 {
    86400
}

pub fn default_request_body_limit() -> usize {
    128 * 1024 * 1024 // 128MB
}

fn default_bind() -> String {
    "127.0.0.1:8080".to_string()
}
fn default_retries() -> usize {
    3
}
fn default_capacity() -> usize {
    200
}

/// Default total upstream budget: 20 minutes (see [`GatewaySection::upstream_timeout_secs`]).
pub fn default_upstream_timeout_secs() -> u64 {
    1200
}

/// Range guard shared by gateway/provider/model timeout overrides.
/// Accepts `60..=1800` seconds so a 0/1 typo cannot pin connections for
/// seconds or kill long streams instantly.
pub fn validate_upstream_timeout_secs(v: u64, label: &str) -> Result<(), String> {
    if (60..=1800).contains(&v) {
        Ok(())
    } else {
        Err(format!(
            "{} 必须在 60~1800 秒之间（收到 {}），防止误配导致长流被秒杀或死连占资源",
            label, v
        ))
    }
}
fn default_web_enabled() -> bool {
    true
}
fn default_web_dist_dir() -> String {
    "web/dist".to_string()
}
pub fn generate_secure_api_key() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("sk-pony-{}", raw)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayAuthAction {
    Show,
    Rotate,
    Set(String),
    MisdirectedList,
    MisdirectedAgy,
}

pub fn parse_gateway_auth_action(custom_key: Option<&str>, rotate: bool) -> GatewayAuthAction {
    if let Some(k) = custom_key {
        let trimmed = k.trim();
        if trimmed.eq_ignore_ascii_case("list") {
            return GatewayAuthAction::MisdirectedList;
        }
        if trimmed.eq_ignore_ascii_case("agy") || trimmed.eq_ignore_ascii_case("antigravity") {
            return GatewayAuthAction::MisdirectedAgy;
        }
        if trimmed.eq_ignore_ascii_case("show") || trimmed.eq_ignore_ascii_case("get") {
            return GatewayAuthAction::Show;
        }
        if trimmed.eq_ignore_ascii_case("rotate")
            || trimmed.eq_ignore_ascii_case("gen")
            || trimmed.eq_ignore_ascii_case("generate")
        {
            return GatewayAuthAction::Rotate;
        }
        if !trimmed.is_empty() {
            return GatewayAuthAction::Set(trimmed.to_string());
        }
    }
    if rotate {
        GatewayAuthAction::Rotate
    } else {
        GatewayAuthAction::Show
    }
}

fn default_api_key() -> String {
    generate_secure_api_key()
}

impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            max_retries: default_retries(),
            flight_recorder_capacity: default_capacity(),
            upstream_timeout_secs: default_upstream_timeout_secs(),
            upstream_ttfb_timeout_secs: None,
            empty_stop_total_timeout_secs: None,
            api_key: default_api_key(),
            default_strategy: GatewayRoutingStrategy::Economy,
            request_body_limit: default_request_body_limit(),
            proxy: None,
            use_system_proxy: false,
            web_enabled: true,
            web_dist_dir: default_web_dist_dir(),
            admin_write_enabled: false,
            telemetry_snapshot_path: None,
            auth_compat: default_auth_compat(),
            gateway_keys: Vec::new(),
            antigravity_auto_refresh: default_antigravity_auto_refresh(),
            antigravity_refresh_interval_secs: default_antigravity_refresh_interval_secs(),
            cross_provider_quota_failover: false,
            auth_mode: default_auth_mode(),
            auth_fail_window_secs: default_auth_fail_window_secs(),
            auth_fail_limit: default_auth_fail_limit(),
            auth_lockout_secs: default_auth_lockout_secs(),
            admin_ip_allowlist: Vec::new(),
            trusted_proxies: Vec::new(),
            admin_session_enabled: default_admin_session_enabled(),
            admin_session_ttl_secs: default_admin_session_ttl_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    pub name: String,
    #[serde(default)]
    pub tier: ModelTier,
    /// Explicit routing preference for this model under this provider: a larger
    /// value ranks this candidate ahead of same-named models of other providers
    /// (and ahead of hot-cache / strategy scores). `None` (default) = no
    /// preference, treated as 0, so legacy configs keep their exact behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_mode: Option<BillingMode>,
    #[serde(default = "default_context_window")]
    pub context_window: String,
    #[serde(default = "default_max_output")]
    pub max_output: String,
    #[serde(default = "default_modalities")]
    pub input_types: Vec<String>,
    #[serde(default = "default_modalities")]
    pub output_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price: Option<f64>,
    /// Optional pricing mode override: uniform or peak_valley
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_mode: Option<PricingMode>,
    /// Optional peak-valley / time-of-use periods for this model
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_periods: Vec<PricingPeriod>,
    /// Optional display name for consoles (cosmetic only; routing always uses `name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional default sampling temperature for this model (0.0–2.0).
    /// Applied only when the inbound request omits `temperature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Optional default nucleus sampling cutoff for this model (0.0–1.0).
    /// Applied only when the inbound request omits `top_p`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<UpstreamProtocol>,
    /// Optional custom base_url override for this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_default: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_max: Option<ReasoningEffort>,
    /// Optional outbound HTTP proxy override for this model (e.g. "http://127.0.0.1:8899", "direct", or "none").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Optional total upstream timeout override for this model (seconds, 60~1800).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Optional short-window rate limits for this model (per-key RPM/TPM
    /// sliding-window budget + concurrency cap), overriding the provider-level
    /// default field-by-field. `None` inherits the provider default (or stays
    /// unlimited). See [`RateLimits`] for field semantics. Quotas are account
    /// level; the model config is the configuration source for them (ADR
    /// `2026-09-30-unified-quota-metering-governance-kernel`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimits>,
    /// Optional ordered model fallbacks when this model fails upstream or exhausts its providers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<String>,
}

pub fn default_context_window() -> String {
    "1M".to_string()
}
pub fn default_max_output() -> String {
    "32K".to_string()
}
pub fn default_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            tier: ModelTier::Standard,
            priority: None,
            billing_mode: None,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
            input_price: None,
            cached_price: None,
            output_price: None,
            pricing_mode: None,
            pricing_periods: Vec::new(),
            display_name: None,
            temperature: None,
            top_p: None,
            protocol: None,
            base_url: None,
            thinking_default: None,
            thinking_max: None,
            proxy: None,
            timeout_secs: None,
            rate_limits: None,
            fallbacks: Vec::new(),
        }
    }
}

impl ModelConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tier: ModelTier::Standard,
            priority: None,
            billing_mode: None,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
            input_price: None,
            cached_price: None,
            output_price: None,
            pricing_mode: None,
            pricing_periods: Vec::new(),
            display_name: None,
            temperature: None,
            top_p: None,
            protocol: None,
            base_url: None,
            thinking_default: None,
            thinking_max: None,
            proxy: None,
            timeout_secs: None,
            rate_limits: None,
            fallbacks: Vec::new(),
        }
    }

    pub fn thinking_spec(&self) -> ModelThinkingSpec {
        let inferred = ModelThinkingSpec::infer_from_model_name(&self.name);
        let default_effort = self.thinking_default.unwrap_or(inferred.default_effort);
        let max_effort = self.thinking_max.unwrap_or(inferred.max_effort);
        ModelThinkingSpec::new(default_effort, max_effort)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSection {
    pub base_url: String,
    pub default_model: String,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub billing_mode: BillingMode,
    #[serde(default = "default_input_price")]
    pub input_price: f64,
    #[serde(default = "default_cached_price")]
    pub cached_price: f64,
    #[serde(default = "default_output_price")]
    pub output_price: f64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub keys: Vec<KeySection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_protocol: Option<UpstreamProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_url: Option<String>,
    /// Optional explicit outbound HTTP proxy for this provider (e.g. "http://127.0.0.1:8899").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Optional total upstream timeout override for this provider (seconds, 60~1800).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Optional TTFB (Time to First Byte / response headers) budget override in seconds for this provider.
    /// Defaults to None (inherits gateway `upstream_ttfb_timeout_secs` or 90s).
    /// Explicitly setting to `Some(0)` disables the TTFB timeout for this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_timeout_secs: Option<u64>,
    /// Optional provider-level default short-window rate limits, inherited by
    /// every model that does not set its own override (see
    /// [`ProviderSection::effective_rate_limits`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimits>,
    /// Egress pool (contract `2026-10-07-egress-pool-contract`): list of exit
    /// shapes for per-attempt exit-IP rotation. Each entry is `direct`/`none`/
    /// empty (the gateway node's own exit IP) or a forward-proxy URL
    /// (`http(s)://host:port` / `socks5://...` — one distinct exit IP per
    /// proxy). An explicit pool REPLACES the provider `proxy` semantics;
    /// empty = legacy single-`proxy` behavior (zero migration).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub egress_pool: Vec<String>,
    /// Egress rotation strategy: `round_robin` (default) | `priority`.
    /// Only meaningful when `egress_pool` is non-empty.
    #[serde(default = "default_egress_strategy")]
    pub egress_strategy: String,
}

impl ProviderSection {
    pub fn pricing(&self) -> PricingConfig {
        PricingConfig {
            mode: PricingMode::Uniform,
            input_price: self.input_price,
            cached_price: self.cached_price,
            output_price: self.output_price,
            pricing_periods: Vec::new(),
        }
    }

    pub fn is_free(&self) -> bool {
        self.pricing().is_free()
    }

    pub fn get_model_pricing(&self, model_name: &str) -> PricingConfig {
        let default_pricing = self.pricing();
        if let Some(cfg) = self.model_configs.iter().find(|m| m.name == model_name) {
            let in_p = cfg.input_price.unwrap_or(default_pricing.input_price);
            let out_p = cfg.output_price.unwrap_or(default_pricing.output_price);
            let ca_p = if let Some(custom_cached) = cfg.cached_price {
                custom_cached
            } else if in_p < 1e-6 {
                0.0
            } else if cfg.input_price.is_some() {
                let ratio = if default_pricing.input_price > 1e-6 {
                    (default_pricing.cached_price / default_pricing.input_price).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                (in_p * ratio).min(in_p)
            } else {
                default_pricing.cached_price.min(in_p)
            };

            let mode = cfg.pricing_mode.unwrap_or(PricingMode::Uniform);
            let periods = cfg.pricing_periods.clone();

            PricingConfig {
                mode,
                input_price: in_p,
                cached_price: ca_p,
                output_price: out_p,
                pricing_periods: periods,
            }
        } else {
            default_pricing
        }
    }

    pub fn get_model_billing_mode(&self, model_name: &str) -> BillingMode {
        self.model_configs
            .iter()
            .find(|m| m.name == model_name)
            .and_then(|m| m.billing_mode)
            .unwrap_or(self.billing_mode)
    }

    pub fn get_model_config(&self, model_name: &str) -> ModelConfig {
        if let Some(cfg) = self.model_configs.iter().find(|m| m.name == model_name) {
            return cfg.clone();
        }
        ModelConfig::new(model_name)
    }

    /// Effective short-window rate limits for a model: the model-level
    /// override wins field-by-field over the provider-level default; `None`
    /// when neither is configured (unlimited). Window/cached-accounting
    /// defaults (`60s` / `count_cached = true`) are applied at scheduling
    /// time via [`RateLimits::window_secs_effective`] and
    /// [`RateLimits::count_cached_effective`].
    pub fn effective_rate_limits(&self, model_name: &str) -> Option<RateLimits> {
        let model_limits = self
            .model_configs
            .iter()
            .find(|m| m.name == model_name)
            .and_then(|m| m.rate_limits);
        RateLimits::resolve(self.rate_limits.as_ref(), model_limits.as_ref())
    }

    pub fn upsert_model_config(&mut self, cfg: ModelConfig) {
        if let Some(existing) = self.model_configs.iter_mut().find(|m| m.name == cfg.name) {
            *existing = cfg.clone();
        } else {
            self.model_configs.push(cfg.clone());
        }

        if cfg.name != self.default_model && !self.models.contains(&cfg.name) {
            self.models.push(cfg.name);
        }
    }

    pub fn list_all_models(&self) -> Vec<ModelConfig> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if !self.default_model.is_empty() {
            result.push(self.get_model_config(&self.default_model));
            seen.insert(self.default_model.clone());
        }

        for m in &self.models {
            if !seen.contains(m) {
                result.push(self.get_model_config(m));
                seen.insert(m.clone());
            }
        }

        for mc in &self.model_configs {
            if !seen.contains(&mc.name) {
                result.push(mc.clone());
                seen.insert(mc.name.clone());
            }
        }

        result
    }
}

fn default_strategy() -> String {
    "priority".to_string()
}

/// Default `egress_strategy` when the field is absent from the TOML
/// (contract C1/C2: `round_robin`).
pub fn default_egress_strategy() -> String {
    "round_robin".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySection {
    pub id: String,
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

impl KeySection {
    pub fn new(
        id: impl Into<String>,
        api_key: impl Into<String>,
        priority: u32,
        weight: u32,
    ) -> Self {
        Self {
            id: id.into(),
            api_key: api_key.into(),
            account_id: None,
            priority,
            weight,
        }
    }

    /// Return the explicit account_id if configured, otherwise fallback to id (self-contained account boundary).
    pub fn effective_account_id(&self) -> &str {
        self.account_id.as_deref().unwrap_or(&self.id)
    }
}

fn default_priority() -> u32 {
    1
}
fn default_weight() -> u32 {
    10
}

/// Strict validation for provider add inputs, shared by `provider add` and the
/// wizard so both reject the same malformed values before touching disk.
pub fn validate_provider_fields(
    base_url: &str,
    default_model: &str,
    strategy: &str,
    billing_mode: &str,
    chat_url: Option<&str>,
    responses_url: Option<&str>,
    messages_url: Option<&str>,
) -> Result<(), String> {
    let url_ok = |u: &str| {
        let t = u.trim();
        (t.starts_with("http://") || t.starts_with("https://"))
            && t.len() > "https://".len()
            && !t.contains(char::is_whitespace)
    };
    if !url_ok(base_url) {
        return Err(format!(
            "无效的 Base URL '{}': 必须以 http:// 或 https:// 开头且不含空白",
            base_url
        ));
    }
    if default_model.trim().is_empty() {
        return Err("默认模型名称不能为空".to_string());
    }
    match strategy.trim().to_ascii_lowercase().as_str() {
        "priority" | "round_robin" | "weighted" => {}
        _ => {
            return Err(format!(
                "无效的调度策略 '{}': 仅支持 priority, round_robin, weighted",
                strategy
            ))
        }
    }
    match billing_mode.trim().to_ascii_lowercase().as_str() {
        "metered" | "plan" | "free" => {}
        _ => {
            return Err(format!(
                "无效的计费模式 '{}': 仅支持 metered, plan, free",
                billing_mode
            ))
        }
    }
    for (label, url) in [
        ("--chat-url", chat_url),
        ("--responses-url", responses_url),
        ("--messages-url", messages_url),
    ] {
        if let Some(u) = url {
            if !url_ok(u) {
                return Err(format!(
                    "无效的 {} '{}': 必须以 http:// 或 https:// 开头且不含空白",
                    label, u
                ));
            }
        }
    }
    Ok(())
}

/// Strict validation for per-model pricing overrides (USD per 1M tokens).
/// `None` inherits the provider baseline; provided values must be finite and >= 0.
pub fn validate_model_pricing(
    input_price: Option<f64>,
    cached_price: Option<f64>,
    output_price: Option<f64>,
) -> Result<(), String> {
    for (label, v) in [
        ("input_price", input_price),
        ("cached_price", cached_price),
        ("output_price", output_price),
    ] {
        if let Some(p) = v {
            if p.is_nan() || p.is_infinite() || p < 0.0 {
                return Err(format!(
                    "模型价格 {} 必须为大于等于 0 的合法数值，输入: {}",
                    label, p
                ));
            }
        }
    }
    Ok(())
}

/// Strict validation for time-of-use pricing periods.
pub fn validate_pricing_periods(periods: &[PricingPeriod]) -> Result<(), String> {
    for p in periods {
        if p.input_price.is_nan() || p.input_price < 0.0 {
            return Err(format!("时段 {} 输入价格必须为 >= 0 的数值", p.name));
        }
        if p.cached_price.is_nan() || p.cached_price < 0.0 {
            return Err(format!("时段 {} 缓存命中价格必须为 >= 0 的数值", p.name));
        }
        if p.output_price.is_nan() || p.output_price < 0.0 {
            return Err(format!("时段 {} 输出价格必须为 >= 0 的数值", p.name));
        }
    }
    Ok(())
}

/// Strict validation for per-model default sampling parameters.
/// `None` means "no override, keep the request value".
pub fn validate_model_sampling(temperature: Option<f32>, top_p: Option<f32>) -> Result<(), String> {
    if let Some(t) = temperature {
        if !t.is_finite() || t < 0.0 || t > 2.0 {
            return Err(format!(
                "模型默认 temperature 必须在 0.0–2.0 之间，输入: {}",
                t
            ));
        }
    }
    if let Some(p) = top_p {
        if !p.is_finite() || p < 0.0 || p > 1.0 {
            return Err(format!("模型默认 top_p 必须在 0.0–1.0 之间，输入: {}", p));
        }
    }
    Ok(())
}

impl ConfigFile {
    /// Resolves the effective TTFB timeout for a given provider.
    /// Priority:
    /// 1. Provider-level `ttfb_timeout_secs`: Some(0) => None (disabled), Some(s) => Some(s)
    /// 2. Gateway-level `upstream_ttfb_timeout_secs`: Some(0) => None (disabled), Some(s) => Some(s)
    /// 3. Global default: 90 seconds (Some(Duration::from_secs(90)))
    pub fn effective_ttfb_timeout(&self, provider_name: &str) -> Option<std::time::Duration> {
        if let Some(prov) = self.providers.get(provider_name) {
            if let Some(secs) = prov.ttfb_timeout_secs {
                return if secs == 0 {
                    None
                } else {
                    Some(std::time::Duration::from_secs(secs))
                };
            }
        }
        if let Some(secs) = self.gateway.upstream_ttfb_timeout_secs {
            if secs == 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(secs))
            }
        } else {
            Some(ponyllm_core::DEFAULT_UPSTREAM_TTFB_TIMEOUT)
        }
    }

    pub fn resolve_path(path: Option<&str>) -> std::path::PathBuf {
        ponyllm_core::resolve_config_path(path.map(Path::new))
    }

    pub fn load_or_default(path: Option<&str>) -> Result<Self, Box<dyn std::error::Error>> {
        let resolved = Self::resolve_path(path);
        if resolved.exists() {
            let content = fs::read_to_string(&resolved)?;
            let cfg: ConfigFile = toml::from_str(&content)?;
            cfg.commercial.validate()?;
            Ok(cfg)
        } else if let Some(p) = path {
            Err(format!(
                "指定的配置文件 '{}' 不存在，请检查路径或执行 'ponyllm init' 生成配置",
                p
            )
            .into())
        } else {
            let content = generate_sample_config();
            let cfg: ConfigFile = toml::from_str(content)?;
            cfg.commercial.validate()?;
            Ok(cfg)
        }
    }

    /// Save configuration atomically (write to temp file, sync, then rename)
    pub fn save_to_path(&self, path: &str) -> std::io::Result<()> {
        if let Err(e) = self.commercial.validate() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("commercial configuration validation failed: {}", e),
            ));
        }
        #[cfg(unix)]
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let target_path = Path::new(path);
        let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }

        // Write-before-backup (WEB-06): if target exists, backup to <path>.bak
        if target_path.exists() {
            let backup_path = target_path.with_extension("toml.bak");
            let _ = fs::copy(target_path, &backup_path);
            // Backups hold the same key material (P0-8): restrict to owner.
            #[cfg(unix)]
            {
                let _ = fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o600));
            }
        }

        let temp_file_name = format!(
            ".{}.tmp.{}.{}",
            target_path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("ponyllm"),
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        let temp_path = parent.join(temp_file_name);

        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            opts.mode(0o600);

            let mut file = opts.open(&temp_path)?;
            #[cfg(unix)]
            let _ = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600));

            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        if let Err(e) = fs::rename(&temp_path, target_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(e);
        }

        #[cfg(unix)]
        let _ = fs::set_permissions(target_path, fs::Permissions::from_mode(0o600));

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_provider_full(
        &mut self,
        name: &str,
        base_url: &str,
        default_model: &str,
        strategy: &str,
        billing_mode: BillingMode,
        input_price: f64,
        cached_price: f64,
        output_price: f64,
    ) {
        let entry = self
            .providers
            .entry(name.to_string())
            .or_insert_with(|| ProviderSection {
                base_url: base_url.to_string(),
                default_model: default_model.to_string(),
                strategy: strategy.to_string(),
                billing_mode,
                input_price,
                cached_price,
                output_price,
                models: Vec::new(),
                model_configs: Vec::new(),
                keys: Vec::new(),
                default_protocol: None,
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
                ttfb_timeout_secs: None,
                rate_limits: None,
                egress_pool: Vec::new(),
                egress_strategy: default_egress_strategy(),
            });
        entry.base_url = base_url.to_string();
        entry.default_model = default_model.to_string();
        entry.strategy = strategy.to_string();
        entry.billing_mode = billing_mode;
        entry.input_price = input_price;
        entry.cached_price = cached_price;
        entry.output_price = output_price;
    }

    pub fn add_provider(
        &mut self,
        name: &str,
        base_url: &str,
        default_model: &str,
        strategy: &str,
    ) {
        self.add_provider_full(
            name,
            base_url,
            default_model,
            strategy,
            BillingMode::Metered,
            default_input_price(),
            default_cached_price(),
            default_output_price(),
        );
    }

    pub fn update_provider(
        &mut self,
        name: &str,
        base_url: &str,
        default_model: &str,
        strategy: &str,
    ) -> Result<(), String> {
        let p = self
            .providers
            .get_mut(name)
            .ok_or_else(|| format!("提供商 '{}' 不存在", name))?;
        p.base_url = base_url.to_string();
        p.default_model = default_model.to_string();
        p.strategy = strategy.to_string();
        Ok(())
    }

    pub fn add_model(&mut self, provider: &str, model: &str) -> Result<(), String> {
        let p = self.providers.get_mut(provider).ok_or_else(|| {
            format!(
                "提供商 '{}' 不存在，请先使用 'ponyllm provider add' 添加",
                provider
            )
        })?;
        if !p.models.contains(&model.to_string()) && p.default_model != model {
            p.models.push(model.to_string());
        }
        Ok(())
    }

    pub fn upsert_model_config(
        &mut self,
        provider: &str,
        model_cfg: ModelConfig,
    ) -> Result<(), String> {
        let p = self
            .providers
            .get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        p.upsert_model_config(model_cfg);
        Ok(())
    }

    pub fn remove_model(&mut self, provider: &str, model: &str) -> Result<bool, String> {
        let p = self
            .providers
            .get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        if p.default_model == model {
            return Err(format!(
                "无法直接删除默认主模型 '{}'。若要删除，请先指定其他模型为默认主模型",
                model
            ));
        }
        let len_models_before = p.models.len();
        p.models.retain(|m| m != model);
        let len_cfgs_before = p.model_configs.len();
        p.model_configs.retain(|m| m.name != model);
        Ok(p.models.len() < len_models_before || p.model_configs.len() < len_cfgs_before)
    }

    pub fn set_default_model(&mut self, provider: &str, model: &str) -> Result<(), String> {
        let p = self
            .providers
            .get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        let old_default = std::mem::replace(&mut p.default_model, model.to_string());
        if !old_default.is_empty() && old_default != model && !p.models.contains(&old_default) {
            p.models.push(old_default);
        }
        p.models.retain(|m| m != model);
        Ok(())
    }

    pub fn remove_provider(&mut self, name: &str) -> bool {
        self.providers.remove(name).is_some()
    }

    pub fn add_key(
        &mut self,
        provider: &str,
        id: &str,
        api_key: &str,
        priority: u32,
        weight: u32,
    ) -> Result<(), String> {
        let p = self.providers.get_mut(provider).ok_or_else(|| {
            format!(
                "提供商 '{}' 不存在，请先使用 'ponyllm provider add' 添加",
                provider
            )
        })?;

        if let Some(existing) = p.keys.iter_mut().find(|k| k.id == id) {
            existing.api_key = api_key.to_string();
            existing.priority = priority;
            existing.weight = weight;
        } else {
            p.keys.push(KeySection {
                id: id.to_string(),
                api_key: api_key.to_string(),
                account_id: None,
                priority,
                weight,
            });
        }
        Ok(())
    }

    pub fn remove_key(&mut self, provider: &str, id: &str) -> Result<bool, String> {
        let p = self
            .providers
            .get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        let len_before = p.keys.len();
        p.keys.retain(|k| k.id != id);
        Ok(p.keys.len() < len_before)
    }

    pub fn mask_key(api_key: &str) -> String {
        FlightRecorder::sanitize_key(api_key)
    }
}

pub fn generate_sample_config() -> &'static str {
    r#"# ponyllm Unified Gateway Configuration

[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200

# 空 STOP 透明重试墙钟总预算（秒）：None→默认 75s；Some(0)→禁用墙钟
# （警告：会复活下游 DSH ~300s stream idle timeout 事故，不推荐）
empty_stop_total_timeout_secs = 75

# DeepSeek Provider (三协议合一: /v1/chat/completions, /v1/responses, /v1/messages)
[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-v4-flash"
default_protocol = "chat"
strategy = "priority"
# Anthropic Messages 协议走独立路径，其余协议由 base_url 派生
messages_url = "https://api.deepseek.com/anthropic"
keys = [
    { id = "deepseek-primary", api_key = "sk-xxxx", priority = 1, weight = 10 },
]

# OpenAI Provider
[providers.openai]
base_url = "https://api.openai.com"
default_model = "gpt-4o"
strategy = "priority"
keys = [
    { id = "openai-main", api_key = "sk-proj-xxxx", priority = 1, weight = 10 },
]

# Anthropic Provider
[providers.anthropic]
base_url = "https://api.anthropic.com"
default_model = "claude-3-7-sonnet-20250219"
strategy = "priority"
keys = [
    { id = "anthropic-1", api_key = "sk-ant-xxxx", priority = 1, weight = 10 },
]
"#
}

impl KeySection {
    pub fn is_antigravity(
        &self,
        provider_protocol: Option<UpstreamProtocol>,
        provider_name: &str,
    ) -> bool {
        if let Some(UpstreamProtocol::Antigravity) = provider_protocol {
            return true;
        }
        if provider_name.to_lowercase().contains("antigravity") {
            return true;
        }
        let trimmed = self.api_key.trim();
        if trimmed.starts_with('{') && trimmed.contains("refresh_token") {
            return true;
        }
        if trimmed.starts_with("1//") {
            return true;
        }
        false
    }

    pub fn to_antigravity_credential(
        &self,
    ) -> Result<ponyllm_core::pool::AntigravityCredential, String> {
        let trimmed = self.api_key.trim();
        if trimmed.starts_with('{') {
            serde_json::from_str::<ponyllm_core::pool::AntigravityCredential>(trimmed)
                .map_err(|e| format!("Failed to parse Antigravity credential JSON: {}", e))
        } else if trimmed.starts_with("1//") {
            Ok(ponyllm_core::pool::AntigravityCredential {
                access_token: None,
                refresh_token: trimmed.to_string(),
                client_id: ponyllm_core::pool::DEFAULT_ANTIGRAVITY_CLIENT_ID.to_string(),
                client_secret: ponyllm_core::pool::DEFAULT_ANTIGRAVITY_CLIENT_SECRET.to_string(),
                project_id: "aicode-consumers".to_string(),
                expiry: None,
            })
        } else {
            Err("Not a valid Antigravity credential (must be JSON or start with 1//)".to_string())
        }
    }

    pub fn masked_display_key(&self) -> String {
        let trimmed = self.api_key.trim();
        if trimmed.starts_with('{') {
            if let Ok(cred) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(rf) = cred.get("refresh_token").and_then(|v| v.as_str()) {
                    if rf.len() > 10 {
                        return format!("ag(1//...{})", &rf[rf.len() - 4..]);
                    }
                }
            }
            "ag(masked-json)".to_string()
        } else if trimmed.starts_with("1//") {
            if trimmed.len() > 10 {
                format!("1//...{}", &trimmed[trimmed.len() - 4..])
            } else {
                "1//***".to_string()
            }
        } else if trimmed.len() > 8 {
            format!("{}...{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
        } else {
            "***".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn test_config_atomic_save_permission_0600() {
        use std::os::unix::fs::PermissionsExt;
        let test_dir = std::env::temp_dir().join(format!("ponyllm_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&test_dir).unwrap();
        let target = test_dir.join("config.toml");
        let mut cfg = ConfigFile::load_or_default(None).unwrap();
        cfg.gateway.api_key = "test-secret-key".to_string();
        cfg.save_to_path(target.to_str().unwrap()).unwrap();

        let meta = fs::metadata(&target).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        let _ = fs::remove_dir_all(&test_dir);
        assert_eq!(mode, 0o600, "Expected file mode 0600, but got {:o}", mode);
    }

    #[test]
    fn test_scoped_key_last4_roundtrip_and_legacy_default() {
        // Issued entries persist the plaintext tail for operator identification.
        let (plain, entry) = generate_scoped_gateway_key("k1", KeyScope::Inference);
        assert_eq!(entry.last4, plain[plain.len() - 4..].to_string());
        assert_eq!(entry.prefix, KeyScope::Inference.prefix());
        // Old entries without the field deserialize to "****" (never fail).
        let legacy: GatewayKeyEntry = serde_json::from_value(serde_json::json!({
            "id": "old",
            "scope": "admin",
            "prefix": "sk-pony-admin-",
            "salt": "s",
            "key_hash": "h"
        }))
        .unwrap();
        assert_eq!(legacy.last4, "****");
        assert!(!legacy.revoked);
        assert!(legacy.expires_at.is_none());
    }

    #[test]
    fn test_model_priority_serde_roundtrip_and_legacy_default() {
        // A model with priority persists it verbatim through TOML.
        let mut cfg = ModelConfig::new("gpt-6-sol");
        cfg.priority = Some(10);
        let toml_str = toml::to_string(&cfg).unwrap();
        let back: ModelConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.priority, Some(10));

        // Legacy models without the field deserialize to `None` (treated as 0).
        let legacy_toml = "name = \"gpt-6-sol\"\ntier = \"Standard\"\n";
        let legacy: ModelConfig = toml::from_str(legacy_toml).unwrap();
        assert_eq!(legacy.priority, None);

        // `None` never leaks into the serialized form.
        let mut plain = ModelConfig::new("plain");
        plain.priority = None;
        let serialized = toml::to_string(&plain).unwrap();
        assert!(
            !serialized.contains("priority"),
            "None priority must be skipped: {}",
            serialized
        );
    }

    #[test]
    fn test_rate_limits_toml_roundtrip_and_legacy_default() {
        // A model with rate_limits persists it verbatim through TOML.
        let mut cfg = ModelConfig::new("gpt-6-sol");
        cfg.rate_limits = Some(RateLimits {
            rpm: Some(10),
            tpm: Some(2_000_000),
            window_secs: Some(60),
            concurrency: Some(4),
            count_cached: Some(false),
        });
        let toml_str = toml::to_string(&cfg).unwrap();
        let back: ModelConfig = toml::from_str(&toml_str).unwrap();
        let rl = back
            .rate_limits
            .expect("rate_limits must survive TOML roundtrip");
        assert_eq!(rl.rpm, Some(10));
        assert_eq!(rl.tpm, Some(2_000_000));
        assert_eq!(rl.window_secs, Some(60));
        assert_eq!(rl.concurrency, Some(4));
        assert_eq!(rl.count_cached, Some(false));

        // Legacy models without the field deserialize to `None` (unlimited).
        let legacy_toml = "name = \"gpt-6-sol\"\ntier = \"Standard\"\n";
        let legacy: ModelConfig = toml::from_str(legacy_toml).unwrap();
        assert_eq!(legacy.rate_limits, None);

        // Partial tables: missing fields default to `None` (per-field inherit).
        let partial: ModelConfig =
            toml::from_str("name = \"m\"\n\n[rate_limits]\nrpm = 5\n").unwrap();
        let rl = partial.rate_limits.expect("partial table still parses");
        assert_eq!(rl.rpm, Some(5));
        assert_eq!(rl.tpm, None);
        assert_eq!(rl.window_secs, None);
        assert_eq!(rl.count_cached, None);

        // `None` never leaks into the serialized form.
        let mut plain = ModelConfig::new("plain");
        plain.rate_limits = None;
        let serialized = toml::to_string(&plain).unwrap();
        assert!(
            !serialized.contains("rate_limits"),
            "None rate_limits must be skipped: {}",
            serialized
        );
    }

    #[test]
    fn test_rate_limits_zero_means_unlimited_and_window_default() {
        // 0 (and None) fold to "unlimited" for numeric budgets.
        let z = RateLimits {
            rpm: Some(0),
            tpm: Some(0),
            window_secs: None,
            concurrency: Some(0),
            count_cached: None,
        };
        assert!(z.is_unlimited());
        assert_eq!(z.rpm_effective(), None);
        assert_eq!(z.tpm_effective(), None);
        assert_eq!(z.concurrency_effective(), None);
        // Window / cached accounting defaults.
        assert_eq!(z.window_secs_effective(), RateLimits::DEFAULT_WINDOW_SECS);
        assert!(z.count_cached_effective());
        // Validation rejects a degenerate 0 window but accepts 0 budgets.
        assert!(z.validate().is_ok());
        let bad_window = RateLimits {
            window_secs: Some(0),
            ..Default::default()
        };
        assert!(bad_window.validate().is_err());
    }

    #[test]
    fn test_effective_rate_limits_model_overrides_provider_fieldwise() {
        let mut prov = ProviderSection {
            base_url: "https://api.example.com".to_string(),
            default_model: "m1".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.0,
            cached_price: 0.5,
            output_price: 2.0,
            models: vec!["m1".to_string()],
            model_configs: Vec::new(),
            keys: Vec::new(),
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
            rate_limits: Some(RateLimits {
                rpm: Some(10),
                tpm: Some(1_000_000),
                window_secs: Some(120),
                concurrency: Some(2),
                count_cached: Some(true),
            }),
            egress_pool: Vec::new(),
            egress_strategy: default_egress_strategy(),
        };
        // No model override: full provider default.
        let resolved = prov.effective_rate_limits("m1").unwrap();
        assert_eq!(resolved.rpm, Some(10));
        assert_eq!(resolved.window_secs, Some(120));

        // Model override wins field-by-field; untouched fields inherit.
        prov.model_configs.push(ModelConfig {
            name: "m1".to_string(),
            rate_limits: Some(RateLimits {
                rpm: Some(30),
                tpm: None,
                window_secs: None,
                concurrency: None,
                count_cached: Some(false),
            }),
            ..ModelConfig::new("m1")
        });
        let resolved = prov.effective_rate_limits("m1").unwrap();
        assert_eq!(resolved.rpm, Some(30), "model rpm must override provider");
        assert_eq!(resolved.tpm, Some(1_000_000), "provider tpm inherited");
        assert_eq!(resolved.window_secs, Some(120), "provider window inherited");
        assert_eq!(
            resolved.concurrency,
            Some(2),
            "provider concurrency inherited"
        );
        assert_eq!(
            resolved.count_cached,
            Some(false),
            "model count_cached overrides"
        );

        // Unknown model without any limits: None (unlimited).
        let none_prov = ProviderSection {
            base_url: String::new(),
            default_model: String::new(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: Vec::new(),
            model_configs: Vec::new(),
            keys: Vec::new(),
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
            rate_limits: None,
            egress_pool: Vec::new(),
            egress_strategy: default_egress_strategy(),
        };
        assert_eq!(none_prov.effective_rate_limits("nope"), None);
    }

    #[test]
    fn test_ttfb_timeout_resolution_and_defaults() {
        let toml_str = r#"
[gateway]
bind = "127.0.0.1:8080"

[providers.default_prov]
base_url = "https://api.example.com"
default_model = "test-model"

[providers.custom_prov]
base_url = "https://api.example.com"
default_model = "test-model"
ttfb_timeout_secs = 120

[providers.disabled_prov]
base_url = "https://api.example.com"
default_model = "test-model"
ttfb_timeout_secs = 0
"#;
        let cfg: ConfigFile = toml::from_str(toml_str).unwrap();

        // 1. Default fallback is 90s
        assert_eq!(
            cfg.effective_ttfb_timeout("default_prov"),
            Some(std::time::Duration::from_secs(90))
        );
        // Provider not explicitly in config also falls back to gateway default (90s)
        assert_eq!(
            cfg.effective_ttfb_timeout("unknown_prov"),
            Some(std::time::Duration::from_secs(90))
        );

        // 2. Provider override (120s)
        assert_eq!(
            cfg.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );

        // 3. Provider override to 0 (disabled)
        assert_eq!(cfg.effective_ttfb_timeout("disabled_prov"), None);

        // 4. Gateway override
        let mut gw_override = cfg.clone();
        gw_override.gateway.upstream_ttfb_timeout_secs = Some(45);
        assert_eq!(
            gw_override.effective_ttfb_timeout("default_prov"),
            Some(std::time::Duration::from_secs(45))
        );
        // Custom provider still overrides gateway
        assert_eq!(
            gw_override.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );

        // 5. Gateway disabled (0)
        let mut gw_disabled = cfg.clone();
        gw_disabled.gateway.upstream_ttfb_timeout_secs = Some(0);
        assert_eq!(gw_disabled.effective_ttfb_timeout("default_prov"), None);
        // Custom provider still overrides disabled gateway
        assert_eq!(
            gw_disabled.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );

        // 6. Provider=0 (disabled) explicitly overrides non-zero gateway (45s)
        assert_eq!(gw_override.effective_ttfb_timeout("disabled_prov"), None);

        // 7. TOML deserialization with [gateway] upstream_ttfb_timeout_secs and tight provider override
        let toml_gw_str = r#"
[gateway]
bind = "127.0.0.1:8080"
upstream_ttfb_timeout_secs = 60

[providers.default_prov]
base_url = "https://api.example.com"
default_model = "test-model"

[providers.tight_prov]
base_url = "https://api.example.com"
default_model = "test-model"
ttfb_timeout_secs = 10

[providers.disabled_prov]
base_url = "https://api.example.com"
default_model = "test-model"
ttfb_timeout_secs = 0
"#;
        let cfg_gw: ConfigFile = toml::from_str(toml_gw_str).unwrap();
        assert_eq!(
            cfg_gw.effective_ttfb_timeout("default_prov"),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(
            cfg_gw.effective_ttfb_timeout("tight_prov"),
            Some(std::time::Duration::from_secs(10))
        );
        assert_eq!(cfg_gw.effective_ttfb_timeout("disabled_prov"), None);
    }

    #[test]
    fn test_cross_provider_quota_failover_toml_roundtrip() {
        // Explicit `true` parses and survives a save/load round-trip.
        let explicit: ConfigFile = toml::from_str(
            r#"
[gateway]
cross_provider_quota_failover = true
"#,
        )
        .unwrap();
        assert!(explicit.gateway.cross_provider_quota_failover);
        // Old configs without the field deserialize to the new default `false`
        // (zero migration): quota exhaustion stops at the first provider.
        let legacy: ConfigFile = toml::from_str(
            r#"
[gateway]
bind = "127.0.0.1:8080"
"#,
        )
        .unwrap();
        assert!(!legacy.gateway.cross_provider_quota_failover);
    }
}

// ---------------------------------------------------------------------------
// Egress-pool entry validation (contract C3)
// ---------------------------------------------------------------------------

/// Cloud metadata endpoints that must never be reachable from an egress pool
/// entry, even if their IPs were to change.
const EGRESS_METADATA_HOSTS: &[&str] = &[
    "metadata.google.internal",
    "metadata.google.com",
    "instance-data",
    "169.254.169.254",
];

/// Operator-managed allowlist (`PONYLLM_PROBE_ALLOWLIST`): entries match the
/// exact host or any subdomain; literal IPs also match exactly and are
/// exempted from the name/IP policy — the same hatch the server-side proxy
/// guard honors, so config-side and admin-write validation agree.
fn egress_probe_allowlisted(host: &str) -> bool {
    if let Ok(list) = std::env::var("PONYLLM_PROBE_ALLOWLIST") {
        let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
        for entry in list.split(',') {
            let e = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            if !e.is_empty() && (lower == e || lower.ends_with(&format!(".{}", e))) {
                return true;
            }
        }
    }
    false
}

fn egress_blocked_name(host: &str) -> Option<&'static str> {
    if egress_probe_allowlisted(host) {
        return None;
    }
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if lower == "svc" || lower.ends_with(".svc") || lower.ends_with(".svc.cluster.local") {
        return Some("kubernetes in-cluster names (*.svc) are not allowed for egress pool entries");
    }
    if EGRESS_METADATA_HOSTS
        .iter()
        .any(|m| lower == *m || lower.ends_with(&format!(".{}", m)))
    {
        return Some("cloud metadata endpoints are not allowed for egress pool entries");
    }
    None
}

/// Shared IPv4 octet policy for egress entries (loopback handled separately by
/// the caller, mirroring `check_proxy_url_fast`).
fn egress_blocked_v4(o: [u8; 4]) -> bool {
    o[0] == 10 // private 10/8
        || (o[0] == 172 && (16..=31).contains(&o[1])) // 172.16/12
        || (o[0] == 192 && o[1] == 168) // 192.168/16
        || (o[0] == 169 && o[1] == 254) // link-local 169.254/16 (metadata)
        || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64/10 CGNAT (RFC 6598)
        || (o[0] == 198 && (18..=19).contains(&o[1])) // 198.18/15 benchmarking (RFC 2544)
        || o == [0, 0, 0, 0] // 0.0.0.0
}

/// IPv6 policy for egress entries: unspecified, IPv4-mapped forms (judged by
/// the embedded IPv4 rules), unique-local and link-local are refused; pure V6
/// loopback is allowed by the caller before this runs.
fn egress_blocked_v6(ip: &std::net::Ipv6Addr) -> bool {
    // Pure V6 unspecified judged FIRST (:: is never a valid exit).
    if ip.is_unspecified() {
        return true;
    }
    // IPv4-mapped forms (::ffff:10.0.0.1, ::127.0.0.1, …) must be judged by
    // the embedded IPv4 rules — the V6 predicates below are all false for
    // such addresses.
    if let Some(mapped) = ip.to_ipv4() {
        return egress_blocked_v4(mapped.octets());
    }
    ip.is_unique_local() // fc00::/7
        || ip.is_unicast_link_local() // fe80::/10
}

fn check_egress_proxy_url(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    // Parse with the SAME WHATWG parser the dialer uses (`reqwest::Proxy::all`
    // builds on the url crate): hex/octal IPv4 literals, fragments and
    // userinfo must be judged by the parser that will actually dial, or a
    // private/metadata host sneaks through validation (review
    // SSRF-BYPASS-EGRESS-POOL — `http://0xa000005:3128` and
    // `http://10.0.0.5:8080#@127.0.0.1` were accepted by the old manual
    // parser while the dialer resolved them to 10.0.0.5).
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| format!("invalid egress entry '{}': {}", trimmed, e))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" && scheme != "socks5" && scheme != "socks5h" {
        return Err(format!(
            "egress entry scheme must be http/https/socks5, got '{}'",
            scheme
        ));
    }
    // Port is optional exactly like the dialer: reqwest's Proxy::all dials
    // http(s) on the scheme default and socks on 1080 when the port is
    // omitted, so a port-less entry is NOT a fail-open (the URL parses and
    // Proxy::all accepts it — no silent direct fallback). What the guard must
    // refuse are entries Proxy::all cannot use: unknown scheme (above),
    // unparseable/blank host (url::Url::parse already Err'd), empty host
    // (below).
    match parsed.host() {
        None => Err(format!("egress entry '{}' has no host", trimmed)),
        Some(url::Host::Ipv4(ip)) => {
            let o = ip.octets();
            // Loopback proxy explicitly allowed (documented local pproxy shape).
            if o[0] == 127 {
                return Ok(());
            }
            if egress_blocked_v4(o) {
                return Err(format!(
                    "egress target '{}' resolves to a blocked address ({})",
                    host_display(&parsed),
                    ip
                ));
            }
            Ok(())
        }
        Some(url::Host::Ipv6(ip)) => {
            if ip.is_loopback() {
                return Ok(());
            }
            if egress_blocked_v6(&ip) {
                return Err(format!(
                    "egress target '{}' resolves to a blocked address ({})",
                    host_display(&parsed),
                    ip
                ));
            }
            Ok(())
        }
        Some(url::Host::Domain(hostname)) => {
            if egress_probe_allowlisted(hostname) {
                return Ok(());
            }
            if let Some(reason) = egress_blocked_name(hostname) {
                // Loopback proxies (local pproxy) are the documented shape;
                // the name check would otherwise reject `localhost`.
                let lower = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
                if lower != "localhost" {
                    return Err(reason.to_string());
                }
            }
            Ok(())
        }
    }
}

/// Host part of a parsed URL for error messages (domain verbatim, IPs in
/// canonical form).
fn host_display(parsed: &url::Url) -> String {
    parsed
        .host_str()
        .unwrap_or("<missing-host>")
        .to_string()
}

/// Validate one egress-pool entry (contract C3):
///
/// - `direct` / `none` / empty / whitespace = legal (gateway node's own exit);
/// - any other value must be a parseable `http(s)://` / `socks5://`/`socks5h://`
///   forward-proxy URL whose host passes the same SSRF posture as the
///   data-plane proxy guard: public or loopback targets allowed; private /
///   link-local / metadata / in-cluster names refused.
///
/// Mirrors `ponyllm-server::egress::check_proxy_url_fast` so config-side
/// validation and the admin write path agree on one policy. The server's PUT
/// handler delegates here for the pool entries.
pub fn validate_egress_entry(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("direct")
        || trimmed.eq_ignore_ascii_case("none")
    {
        return Ok(());
    }
    check_egress_proxy_url(trimmed)
}

#[cfg(test)]
mod egress_entry_tests {
    use super::*;

    #[test]
    fn accepts_direct_none_and_empty() {
        for entry in ["direct", "none", "  ", "", "DIRECT", "None"] {
            assert!(
                validate_egress_entry(entry).is_ok(),
                "entry '{entry:?}' must be legal"
            );
        }
    }

    #[test]
    fn accepts_public_and_loopback_proxies() {
        for entry in [
            "http://127.0.0.1:8899",
            "http://localhost:7890",
            "http://1.2.3.4:8899",
            "http://egress-1.example.com:8899",
            "https://egress-2.example.com:443",
            "socks5://192.0.2.10:1080",
            "socks5h://[::1]:1080",
        ] {
            assert!(
                validate_egress_entry(entry).is_ok(),
                "entry '{entry}' must be accepted"
            );
        }
    }

    #[test]
    fn rejects_bad_scheme_and_missing_host() {
        for entry in [
            "ftp://example.com:21",
            "gopher://example.com/",
            "file:///etc/passwd",
            "no-scheme-here",
            "http://",
        ] {
            assert!(
                validate_egress_entry(entry).is_err(),
                "entry '{entry}' must be refused"
            );
        }
    }

    #[test]
    fn rejects_private_linklocal_and_metadata_targets() {
        for entry in [
            "http://10.0.0.5:8080",
            "http://172.16.9.9:3128",
            "http://192.168.1.1:3128",
            "http://169.254.169.254:80",
            "http://metadata.google.internal:80",
            "http://x.svc.cluster.local:8080",
            "http://[::ffff:10.0.0.1]:8080",
            "http://[fe80::1]:8080",
        ] {
            assert!(
                validate_egress_entry(entry).is_err(),
                "entry '{entry}' must be refused"
            );
        }
    }

    /// Review `SSRF-BYPASS-EGRESS-POOL`: the old hand-rolled parser accepted
    /// these while the WHATWG dialer (reqwest::Proxy::all) resolved them to
    /// private/metadata hosts. Validation must parse with the SAME parser.
    #[test]
    fn rejects_parser_mismatch_bypass_tricks() {
        for entry in [
            "http://0xa000005:3128",           // hex-encoded 10.0.0.5
            "http://0x0a000005:3128",          // hex-encoded 10.0.0.5 (padded)
            "http://0252.0.0.1:3128",          // octal-encoded 170.0.0.1 (public) — legal
            "http://10.0.0.5:8080#@127.0.0.1", // fragment trick: host is 10.0.0.5
            "http://169.254.169.254#@127.0.0.1", // fragment trick: metadata host
            "http:// 10.0.0.5:8080",           // blank-host trick
            "http://:8080",                    // empty host
            "http://10.0.0.5:8080 ",           // trailing space (trimmed before parse)
        ] {
            let trimmed = entry.trim();
            if trimmed == "http://0252.0.0.1:3128" {
                continue; // public octal literal — legal, asserted below
            }
            assert!(
                validate_egress_entry(entry).is_err(),
                "entry '{entry}' must be refused (parser-mismatch bypass)"
            );
        }
        // Octal public literal stays legal (not a bypass).
        assert!(validate_egress_entry("http://0252.0.0.1:3128").is_ok());
        // Hex-encoded loopback is still loopback → allowed (documented shape).
        assert!(validate_egress_entry("http://0x7f000001:3128").is_ok());
        // Port-less entries stay legal EXACTLY like the dialer: reqwest
        // Proxy::all defaults http(s) ports and socks to 1080, so accepting
        // them is not a fail-open (same acceptance as the dialer — review
        // VALIDATION-FAIL-OPEN).
        assert!(validate_egress_entry("http://egress-1.example.com").is_ok());
        assert!(validate_egress_entry("socks5://192.0.2.10").is_ok());
    }
}
