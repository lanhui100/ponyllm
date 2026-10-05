use std::collections::HashMap;
use ponyllm_core::pool::{
    default_cached_price, default_input_price, default_output_price, BillingMode,
    GatewayRoutingStrategy, ModelTier, ModelThinkingSpec, PricingConfig, PricingMode, PricingPeriod,
    RateLimits, UpstreamProtocol,
};
use ponyllm_protocol::common::ReasoningEffort;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSpec {
    pub name: String,
    #[serde(default)]
    pub tier: ModelTier,
    /// Explicit routing preference for this model under this provider; mirrors
    /// `ModelConfig::priority` (larger = preferred first, `None` = 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    #[serde(default = "default_context_window")]
    pub context_window: String,
    #[serde(default = "default_max_output")]
    pub max_output: String,
    #[serde(default = "default_modalities")]
    pub input_types: Vec<String>,
    #[serde(default = "default_modalities")]
    pub output_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_mode: Option<BillingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_mode: Option<PricingMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_periods: Vec<PricingPeriod>,
    /// Cosmetic display name for consoles; routing always uses `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Default sampling temperature for this model (applied when the request omits it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Default nucleus sampling cutoff for this model (applied when the request omits it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Native wire protocol of this model. `None` inherits the provider default.
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
    /// `None` inherits the provider default proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Optional total upstream timeout override for this model (seconds, 60~1800).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Optional short-window rate limits for this model (per-key RPM/TPM
    /// sliding-window budget + concurrency cap). Mirrors the disk-level
    /// `ModelConfig::rate_limits` on the runtime side so the executor can
    /// resolve effective limits without re-parsing the config file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimits>,
    /// Optional ordered model fallbacks when this model fails upstream or exhausts its providers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<String>,
}

impl ModelSpec {
    pub fn thinking_spec(&self) -> ModelThinkingSpec {
        let inferred = ModelThinkingSpec::infer_from_model_name(&self.name);
        let default_effort = self.thinking_default.unwrap_or(inferred.default_effort);
        let max_effort = self.thinking_max.unwrap_or(inferred.max_effort);
        ModelThinkingSpec::new(default_effort, max_effort)
    }
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

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            tier: ModelTier::Standard,
            priority: None,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
            billing_mode: None,
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


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
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
    #[serde(default)]
    pub model_specs: Vec<ModelSpec>,
    /// Default native wire protocol for models of this provider.
    /// `None` keeps the legacy URL heuristic so old configs migrate with zero changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_protocol: Option<UpstreamProtocol>,
    /// Per-protocol endpoint base overrides. `None` entries derive from `base_url`.
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
    /// Optional TTFB budget override in seconds for this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_timeout_secs: Option<u64>,
    /// Optional provider-level default short-window rate limits, inherited by
    /// every model without its own override (runtime mirror of
    /// `ponyllm-config::ProviderSection::rate_limits`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimits>,
}

fn default_strategy() -> String {
    "priority".to_string()
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            default_model: String::new(),
            strategy: default_strategy(),
            billing_mode: BillingMode::default(),
            input_price: default_input_price(),
            cached_price: default_cached_price(),
            output_price: default_output_price(),
            models: Vec::new(),
            model_specs: Vec::new(),
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
            rate_limits: None,
        }
    }
}

impl ProviderConfig {
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
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            let in_p = spec.input_price.unwrap_or(default_pricing.input_price);
            let out_p = spec.output_price.unwrap_or(default_pricing.output_price);
            let ca_p = if let Some(custom_cached) = spec.cached_price {
                custom_cached
            } else if in_p < 1e-6 {
                0.0
            } else if spec.input_price.is_some() {
                // 模型单独指定了 input_price，按 Provider 缓存折扣比缩放，严格保证 cached_price <= input_price
                let ratio = if default_pricing.input_price > 1e-6 {
                    (default_pricing.cached_price / default_pricing.input_price).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                (in_p * ratio).min(in_p)
            } else {
                default_pricing.cached_price.min(in_p)
            };

            let mode = spec.pricing_mode.unwrap_or(PricingMode::Uniform);
            let periods = spec.pricing_periods.clone();

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
        self.model_specs
            .iter()
            .find(|m| m.name == model_name)
            .and_then(|m| m.billing_mode)
            .unwrap_or(self.billing_mode)
    }

    pub fn get_model_spec(&self, model_name: &str) -> ModelSpec {
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            return spec.clone();
        }
        ModelSpec {
            name: model_name.to_string(),
            tier: ModelTier::Standard,
            priority: None,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
            billing_mode: None,
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

    /// Effective short-window rate limits for a model: model-level override
    /// merged field-by-field over the provider-level default (runtime mirror
    /// of `ponyllm-config::ProviderSection::effective_rate_limits`). `None`
    /// when neither is configured (unlimited).
    pub fn effective_rate_limits(&self, model_name: &str) -> Option<RateLimits> {
        let model_limits = self
            .model_specs
            .iter()
            .find(|m| m.name == model_name)
            .and_then(|m| m.rate_limits);
        RateLimits::resolve(self.rate_limits.as_ref(), model_limits.as_ref())
    }

    /// Resolves the effective proxy configuration for a model under this provider.
    ///
    /// - If the model specifies `proxy`:
    ///   - `"direct"`, `"none"`, or empty => `EffectiveProxy::Direct` (forces direct connection).
    ///   - custom URL => `EffectiveProxy::Custom(url)`.
    /// - If the model specifies `None` (inherit):
    ///   - If provider specifies `proxy`:
    ///     - `"direct"`, `"none"`, or empty => `EffectiveProxy::Direct`.
    ///     - custom URL => `EffectiveProxy::Custom(url)`.
    ///   - Otherwise => `EffectiveProxy::InheritGateway`.
    pub fn effective_proxy_for_model(&self, model_name: &str) -> EffectiveProxy<'_> {
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            if let Some(ref p) = spec.proxy {
                let trimmed = p.trim();
                if trimmed.eq_ignore_ascii_case("direct")
                    || trimmed.eq_ignore_ascii_case("none")
                    || trimmed.is_empty()
                {
                    return EffectiveProxy::Direct;
                }
                return EffectiveProxy::Custom(trimmed);
            }
        }
        if let Some(ref p) = self.proxy {
            let trimmed = p.trim();
            if trimmed.eq_ignore_ascii_case("direct")
                || trimmed.eq_ignore_ascii_case("none")
                || trimmed.is_empty()
            {
                return EffectiveProxy::Direct;
            }
            return EffectiveProxy::Custom(trimmed);
        }
        EffectiveProxy::InheritGateway
    }

    /// Effective total upstream timeout for a model: model override >
    /// provider override > `None` (caller falls back to the gateway default).
    pub fn effective_timeout_secs_for_model(&self, model_name: &str) -> Option<u64> {
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            if let Some(t) = spec.timeout_secs {
                return Some(t);
            }
        }
        self.timeout_secs
    }

    /// Effective native protocol for a model: model override > provider default.
    /// Returns `None` when neither is configured so callers fall back to the
    /// legacy URL heuristic (zero-migration for old configs).
    pub fn native_protocol(&self, model_name: &str) -> Option<UpstreamProtocol> {
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            if let Some(p) = spec.protocol {
                return Some(p);
            }
        }
        self.default_protocol
    }

    /// Configured endpoint base for a protocol, if overridden.
    pub fn endpoint_base_for(&self, protocol: UpstreamProtocol) -> Option<&str> {
        match protocol {
            UpstreamProtocol::Chat => self.chat_url.as_deref(),
            UpstreamProtocol::Responses => self.responses_url.as_deref(),
            UpstreamProtocol::Anthropic => self.messages_url.as_deref(),
            UpstreamProtocol::Antigravity => None,
            // Systemone 透传不走 per-protocol endpoint 覆盖,恒用 base_url +
            // `/systemone`(见 RoutedTarget::systemone_url);这里返回 None 让
            // with_endpoint 落到 base_url 兜底,仅保留编译穷举完整.
            UpstreamProtocol::Systemone => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveProxy<'a> {
    InheritGateway,
    Direct,
    Custom(&'a str),
}

pub fn default_request_body_limit() -> usize {
    128 * 1024 * 1024 // 128MB default for 1M context / multimodal payloads
}

/// Default total upstream budget: 20 minutes (see [`GatewayConfig::upstream_timeout_secs`]).
pub fn default_upstream_timeout_secs() -> u64 {
    1200
}

fn default_event_log_retention_days() -> u64 {
    7
}

fn default_event_log_max_bytes() -> u64 {
    512 * 1024 * 1024 // 512MB ring of hourly JSONL segments
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_web_dist_dir() -> String {
    "web/dist".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub bind_addr: String,
    pub api_key: String,
    #[serde(default)]
    pub default_strategy: GatewayRoutingStrategy,
    pub providers: HashMap<String, ProviderConfig>,
    pub max_retries: usize,
    pub flight_recorder_capacity: usize,
    /// Total wall-clock budget for one upstream call, in seconds (default 1200 =
    /// 20 minutes). Long-thinking streams routinely exceed the legacy 120s
    /// budget; the gateway replaces it with TTFB + tail-stall detection.
    #[serde(default = "default_upstream_timeout_secs")]
    pub upstream_timeout_secs: u64,
    /// Optional TTFB budget in seconds for upstream calls. None resolves to 90s. Some(0) disables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_ttfb_timeout_secs: Option<u64>,
    #[serde(default = "default_request_body_limit")]
    pub request_body_limit: usize,
    /// Hourly JSONL event-log directory. `None` (default) keeps events in the
    /// in-memory ring only; set to persist the single-append truth with rotation.
    #[serde(default)]
    pub event_log_dir: Option<String>,
    #[serde(default = "default_event_log_retention_days")]
    pub event_log_retention_days: u64,
    #[serde(default = "default_event_log_max_bytes")]
    pub event_log_max_bytes: u64,
    /// Optional default outbound HTTP proxy for upstream providers (e.g. "http://127.0.0.1:8899").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Whether to inherit system environment proxies (`http_proxy`/`https_proxy`).
    /// Defaults to `false` to isolate gateway from host terminal proxy pollution.
    #[serde(default)]
    pub use_system_proxy: bool,
    /// Whether `serve` mounts the web console (`web/dist`) under `/app/*`.
    /// Defaults to `true`; `--no-web` (or `web_enabled = false`) disables hosting
    /// while the gateway forwarding chain keeps running (WEB-01).
    #[serde(default = "default_true")]
    pub web_enabled: bool,
    /// Directory served as the web console SPA. Defaults to `web/dist`
    /// (relative to the serve working directory). Missing dir only warns (WEB-01).
    #[serde(default = "default_web_dist_dir")]
    pub web_dist_dir: String,
    /// Whether admin write operations (CUD and dial-test) are enabled (WEB-06).
    /// Defaults to `false` for security (fail-closed); set explicitly to
    /// `true` to allow web console management.
    #[serde(default = "default_false")]
    pub admin_write_enabled: bool,
    /// Telemetry snapshot file for dashboard persistence across restarts.
    /// `None` (default) derives `<config-dir>/telemetry-snapshot.json` in serve;
    /// empty string disables snapshot persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_snapshot_path: Option<String>,
    /// Auth compatibility mode (P0, task-20; contract `.agents/notes/auth-eval.md` §3.1).
    /// Canonical type lives in `ponyllm-config`; re-exported here for the
    /// runtime config. Old configs without the field deserialize to `dual`.
    #[serde(default = "default_auth_compat")]
    pub auth_compat: ponyllm_config::AuthCompat,
    /// Scoped gateway keys (P1, task-21): verified by `auth::authenticate`
    /// (salted SHA-256, never plaintext). Empty = legacy single `api_key`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gateway_keys: Vec<ponyllm_config::GatewayKeyEntry>,
    /// Whether background auto-refresh & keepalive for Antigravity accounts is enabled.
    #[serde(default = "default_true")]
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
    /// Explicit authentication mode (Phase-2 F1): `secured` default (fail-closed);
    /// `open` only via explicit opt-in. Canonical type in `ponyllm-config`.
    #[serde(default = "default_auth_mode")]
    pub auth_mode: ponyllm_config::AuthMode,
    /// F2 (VULN-01): failed-auth sliding window length in seconds.
    #[serde(default = "default_auth_fail_window_secs")]
    pub auth_fail_window_secs: u64,
    /// F2 (VULN-01): failed-auth budget per (client IP, key prefix) per window.
    #[serde(default = "default_auth_fail_limit")]
    pub auth_fail_limit: u32,
    /// F2 (VULN-01): base lockout seconds after the budget is exceeded.
    #[serde(default = "default_auth_lockout_secs")]
    pub auth_lockout_secs: u64,
    /// F4 (VULN-02): admin IP fence CIDRs (empty = fence disabled).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_ip_allowlist: Vec<String>,
    /// F3 (VULN-12): exact trusted proxy IPs for XFF hop skipping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
}

fn default_auth_mode() -> ponyllm_config::AuthMode {
    ponyllm_config::AuthMode::Secured
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

fn default_antigravity_refresh_interval_secs() -> u64 {
    // 15 minutes: the family-quota pre-exclusion ledger (ADR
    // 2026-10-04-antigravity-group-quota-aware-scheduling) is only as fresh as
    // the keepalive refresh rhythm. The old 24h default let a weekly bucket
    // exhaust and stay invisible to scheduling for a full day — every request
    // in that window burned one upstream 429 before the pool cooled. ~40
    // probes/hour across a 10-key pool is negligible egress cost.
    900
}

fn default_auth_compat() -> ponyllm_config::AuthCompat {
    ponyllm_config::AuthCompat::Dual
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".to_string(),
            api_key: String::new(),
            default_strategy: GatewayRoutingStrategy::Economy,
            providers: HashMap::new(),
            max_retries: 3,
            flight_recorder_capacity: 100,
            upstream_timeout_secs: default_upstream_timeout_secs(),
            upstream_ttfb_timeout_secs: None,
            request_body_limit: default_request_body_limit(),
            event_log_dir: None,
            event_log_retention_days: default_event_log_retention_days(),
            event_log_max_bytes: default_event_log_max_bytes(),
            proxy: None,
            use_system_proxy: false,
            web_enabled: true,
            web_dist_dir: default_web_dist_dir(),
            admin_write_enabled: false,
            telemetry_snapshot_path: None,
            auth_compat: default_auth_compat(),
            gateway_keys: Vec::new(),
            antigravity_auto_refresh: true,
            antigravity_refresh_interval_secs: default_antigravity_refresh_interval_secs(),
            cross_provider_quota_failover: false,
            auth_mode: default_auth_mode(),
            auth_fail_window_secs: default_auth_fail_window_secs(),
            auth_fail_limit: default_auth_fail_limit(),
            auth_lockout_secs: default_auth_lockout_secs(),
            admin_ip_allowlist: Vec::new(),
            trusted_proxies: Vec::new(),
        }
    }
}

impl GatewayConfig {
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
        if let Some(secs) = self.upstream_ttfb_timeout_secs {
            if secs == 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(secs))
            }
        } else {
            Some(ponyllm_core::DEFAULT_UPSTREAM_TTFB_TIMEOUT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_config_effective_ttfb_timeout() {
        let mut cfg = GatewayConfig::default();
        let mut prov_default = ProviderConfig::default();
        prov_default.base_url = "https://api.example.com".to_string();
        cfg.providers.insert("default_prov".to_string(), prov_default);

        let mut prov_custom = ProviderConfig::default();
        prov_custom.ttfb_timeout_secs = Some(120);
        cfg.providers.insert("custom_prov".to_string(), prov_custom);

        let mut prov_disabled = ProviderConfig::default();
        prov_disabled.ttfb_timeout_secs = Some(0);
        cfg.providers.insert("disabled_prov".to_string(), prov_disabled);

        let mut prov_tight = ProviderConfig::default();
        prov_tight.ttfb_timeout_secs = Some(10);
        cfg.providers.insert("tight_prov".to_string(), prov_tight);

        // 1. Default fallback is 90s
        assert_eq!(
            cfg.effective_ttfb_timeout("default_prov"),
            Some(ponyllm_core::DEFAULT_UPSTREAM_TTFB_TIMEOUT)
        );
        // Provider not explicitly in config also falls back to gateway default (90s)
        assert_eq!(
            cfg.effective_ttfb_timeout("unknown_prov"),
            Some(ponyllm_core::DEFAULT_UPSTREAM_TTFB_TIMEOUT)
        );

        // 2. Provider override (120s)
        assert_eq!(
            cfg.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );

        // 3. Provider override to 0 (disabled)
        assert_eq!(cfg.effective_ttfb_timeout("disabled_prov"), None);

        // 4. Gateway override (45s)
        cfg.upstream_ttfb_timeout_secs = Some(45);
        assert_eq!(
            cfg.effective_ttfb_timeout("default_prov"),
            Some(std::time::Duration::from_secs(45))
        );
        // Custom provider (120s) still overrides gateway (45s)
        assert_eq!(
            cfg.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );
        // Provider=0 still overrides gateway
        assert_eq!(cfg.effective_ttfb_timeout("disabled_prov"), None);

        // 5. Tight provider (10s) overrides gateway (60s)
        cfg.upstream_ttfb_timeout_secs = Some(60);
        assert_eq!(
            cfg.effective_ttfb_timeout("tight_prov"),
            Some(std::time::Duration::from_secs(10))
        );

        // 6. Gateway disabled (0)
        cfg.upstream_ttfb_timeout_secs = Some(0);
        assert_eq!(cfg.effective_ttfb_timeout("default_prov"), None);
        assert_eq!(
            cfg.effective_ttfb_timeout("custom_prov"),
            Some(std::time::Duration::from_secs(120))
        );
        assert_eq!(cfg.effective_ttfb_timeout("disabled_prov"), None);
    }
}



