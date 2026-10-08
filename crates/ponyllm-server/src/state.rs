use parking_lot::RwLock;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::{EventSink, EventSinkCtx};
use ponyllm_core::pool::refresh_gate::RefreshGate;
use ponyllm_core::pool::{
    is_context_capacity_compatible, parse_context_capacity_tokens, AntigravityTokenManager,
    BillingMode, EconomyScorer, EgressEntry, EgressPool, EgressStrategy, GatewayRoutingStrategy,
    HotCacheTracker, KeyPool, ModelThinkingSpec, ModelTier, NodeLatencyMetrics, PricingConfig,
    RefreshPersistHook, SpeedScorer, UpstreamProtocol,
};
use ponyllm_core::{canonicalize_model_name, model_aliases};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::admin_store::{ConfigStore, ConfigStoreError};
use crate::config::{GatewayConfig, ProviderConfig};
use crate::frames::FrameConverter;
use crate::routes::models::ParsedRequestModel;
use ponyllm_config::ConfigFile;
use ponyllm_core::telemetry::{
    ConnectivitySampler, EventBus, EventCtx, MetricsCollector, MetricsProjection, StreamProjection,
    TimeseriesProjection,
};
use ponyllm_core::telemetry::{FlightRecorder, GatewayEvent};

/// Config backend polling interval surfaced as overview `hot_reload_ms`
/// (file backend legacy mtime watcher).
pub const FILE_CONFIG_POLL_MS: u64 = 500;

/// How long a recently refreshed in-memory token is considered "newer" than
/// the Secret truth source during rebuild freshness checks.
const TOKEN_FRESHNESS_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Consecutive `invalid_grant` hits (with no intervening successful refresh)
/// before a key is quarantined — the propagation-window buffer.
const INVALID_GRANT_QUARANTINE_N: u32 = 3;

/// 空字符串视为禁用；显式路径优先，随 `event_log_dir` 次之。
fn resolve_snapshot_path(config: &GatewayConfig) -> Option<std::path::PathBuf> {
    if let Some(p) = config.telemetry_snapshot_path.as_deref() {
        if p.trim().is_empty() {
            return None;
        }
        return Some(std::path::PathBuf::from(p));
    }
    if let Some(dir) = config.event_log_dir.as_deref() {
        if !dir.trim().is_empty() {
            return Some(std::path::PathBuf::from(dir).join("telemetry-snapshot.json"));
        }
    }
    None
}

/// 收集 live per-key 周期用量归档（5h/周/月 四要素，来源为既有
/// `KeyUsageTracker::query_windows` → `CycleStats`，见 2026-09-25 四要素 ADR）。
/// 归档写入方：周期保存点（`spawn_snapshot_saver`）与 `save_telemetry_snapshot`。
fn collect_live_key_cycles(
    pools: &RwLock<HashMap<String, Arc<ponyllm_core::pool::KeyPool>>>,
    now_ms: u64,
) -> HashMap<String, crate::telemetry_snapshot::KeyUsageCycleArchive> {
    let mut cycles = HashMap::new();
    let pool_map = pools.read();
    for pool in pool_map.values() {
        for key in pool.snapshot_keys() {
            let (window_5h, weekly, monthly) = key.usage_tracker.query_windows(now_ms);
            cycles.insert(
                key.id.clone(),
                crate::telemetry_snapshot::KeyUsageCycleArchive {
                    window_5h: crate::telemetry_snapshot::window_usage_to_cycle_stats(&window_5h),
                    weekly: crate::telemetry_snapshot::window_usage_to_cycle_stats(&weekly),
                    monthly: crate::telemetry_snapshot::window_usage_to_cycle_stats(&monthly),
                },
            );
        }
    }
    cycles
}

fn spawn_snapshot_saver(
    path: std::path::PathBuf,
    timeseries: Arc<TimeseriesProjection>,
    metrics: Arc<MetricsCollector>,
    connectivity: Arc<ConnectivitySampler>,
    streams: Arc<StreamProjection>,
    pools: Arc<RwLock<HashMap<String, Arc<ponyllm_core::pool::KeyPool>>>>,
) {
    std::thread::Builder::new()
        .name("ponyllm-telemetry-snapshot".to_string())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let (key_usages, live_cycles) = {
                let mut usages = HashMap::new();
                let pool_map = pools.read();
                for pool in pool_map.values() {
                    for key in pool.snapshot_keys() {
                        usages.insert(key.id.clone(), key.usage_tracker.export_snapshot());
                    }
                }
                drop(pool_map);
                // Live per-key CycleStats (5h/周/月) feed the persisted
                // `key_usage_cycles` archive (read-modify-write inside the
                // save keeps prior archives and merges live).
                (usages, collect_live_key_cycles(&pools, now_ms))
            };
            let snap = crate::telemetry_snapshot::TelemetrySnapshot {
                version: crate::telemetry_snapshot::SNAPSHOT_VERSION,
                saved_at_ms: 0,
                timeseries: timeseries.snapshot_buckets(),
                metrics: metrics.snapshot_counters(),
                connectivity: connectivity.snapshot_state(),
                streams: streams.snapshot_nodes(),
                key_usages,
                pool_cycle_benchmark: Default::default(),
            };
            if let Err(e) =
                crate::telemetry_snapshot::save_snapshot_with_live_cycles(&path, &snap, live_cycles)
            {
                tracing::warn!("telemetry snapshot save failed: {}", e);
            }
        })
        .ok();
}

#[derive(Debug, Clone)]
pub struct RoutedTarget {
    pub provider_name: String,
    pub base_url: String,
    pub physical_model: String,
    pub tier: ModelTier,
    /// Explicit per-(provider, model) routing preference: a larger value ranks
    /// this candidate ahead of same-named candidates of other providers and
    /// ahead of hot-cache / strategy scores. `None` = no preference (0), so
    /// legacy configs keep their exact ordering behaviour.
    pub priority: Option<u32>,
    pub strategy: GatewayRoutingStrategy,
    /// Effective native upstream protocol: request header > model override >
    /// provider default > legacy URL heuristic.
    pub upstream_protocol: UpstreamProtocol,
    /// Configured per-protocol endpoint base, if the provider overrides it.
    /// Routes fall back to `base_url` when `None`.
    pub endpoint_base: Option<String>,
    pub context_window: String,
    pub billing_mode: BillingMode,
    pub pricing: PricingConfig,
    pub thinking_spec: ModelThinkingSpec,
    /// Model-level default sampling temperature (request value wins when present).
    pub temperature: Option<f32>,
    /// Model-level default nucleus sampling cutoff (request value wins when present).
    pub top_p: Option<f32>,
    pub input_types: Vec<String>,
    pub max_output: String,
    /// Declared output modalities for this model (e.g. `["text"]` or
    /// `["image"]`). The Images API endpoints require `image` here; chat
    /// translation ignores it (text models always emit text).
    pub output_types: Vec<String>,
}

impl RoutedTarget {
    pub fn resolve_thinking(
        &self,
        requested: Option<ponyllm_protocol::common::ReasoningEffort>,
    ) -> ponyllm_protocol::common::ReasoningEffort {
        self.thinking_spec.resolve(requested)
    }

    pub fn supports_modality(&self, modality: &str) -> bool {
        if modality.eq_ignore_ascii_case("text") {
            return true;
        }
        let mod_lower = modality.to_ascii_lowercase();
        self.input_types.iter().any(|t| {
            let t_lower = t.to_ascii_lowercase();
            if t_lower == mod_lower {
                return true;
            }
            // Alias tolerance: "file" and "document" are aliases
            if (t_lower == "file" && mod_lower == "document")
                || (t_lower == "document" && mod_lower == "file")
            {
                return true;
            }
            false
        })
    }

    pub fn supports_modalities(&self, modalities: &[&str]) -> bool {
        modalities.iter().all(|m| self.supports_modality(m))
    }
}

impl RoutedTarget {
    /// Upstream endpoint path for the resolved protocol: explicit per-protocol
    /// base wins, otherwise the provider base with the legacy normalizers.
    pub fn chat_completions_url(&self) -> String {
        normalize_chat_completions_url(self.endpoint_base.as_deref().unwrap_or(&self.base_url))
    }

    pub fn responses_url(&self) -> String {
        ponyllm_core::normalize_responses_url(
            self.endpoint_base.as_deref().unwrap_or(&self.base_url),
        )
    }

    pub fn messages_url(&self) -> String {
        normalize_messages_url(self.endpoint_base.as_deref().unwrap_or(&self.base_url))
    }

    pub fn antigravity_url(&self, _stream: bool) -> String {
        let base = self
            .endpoint_base
            .as_deref()
            .unwrap_or(&self.base_url)
            .trim_end_matches('/');
        format!("{}/v1internal:streamGenerateContent?alt=sse", base)
    }

    /// Systemone透传上游地址:显式 endpoint_base 优先,否则 base_url + `/systemone`.
    pub fn systemone_url(&self) -> String {
        ponyllm_core::normalize_systemone_url(
            self.endpoint_base.as_deref().unwrap_or(&self.base_url),
        )
    }
}

/// Legacy protocol guess preserved for zero-migration old configs that set
/// neither provider `default_protocol` nor model `protocol`. Single unified
/// heuristic for every routing branch: an `anthropic` path segment wins, else
/// an `anthropic` provider name (outside `/v1/chat` bases) wins.
fn infer_legacy_protocol(provider_name: &str, base_url: &str) -> UpstreamProtocol {
    let is_ant = base_url.contains("anthropic")
        || (provider_name.contains("anthropic") && !base_url.contains("v1/chat"));
    if is_ant {
        UpstreamProtocol::Anthropic
    } else {
        UpstreamProtocol::Chat
    }
}

fn resolve_effective_protocol(
    p_name: &str,
    p_cfg: &ProviderConfig,
    model_name: &str,
    proto_override: Option<UpstreamProtocol>,
    inbound: Option<UpstreamProtocol>,
) -> (UpstreamProtocol, Option<String>) {
    // Explicit request/model declarations always win outright.
    if let Some(o) = proto_override {
        return with_endpoint(p_cfg, model_name, o);
    }
    if let Some(m) = p_cfg.native_protocol(model_name) {
        // Native passthrough preferred: an inbound protocol the provider
        // natively serves (explicit endpoint override) beats the default.
        if let Some(i) = inbound {
            if i != m && p_cfg.endpoint_base_for(i).is_some() {
                return with_endpoint(p_cfg, model_name, i);
            }
        }
        return with_endpoint(p_cfg, model_name, m);
    }
    // No declarations: an inbound protocol with an explicit endpoint still
    // signals native support and wins over the URL heuristic.
    if let Some(i) = inbound {
        if p_cfg.endpoint_base_for(i).is_some() {
            return with_endpoint(p_cfg, model_name, i);
        }
    }
    with_endpoint(
        p_cfg,
        model_name,
        infer_legacy_protocol(p_name, &p_cfg.base_url),
    )
}

fn with_endpoint(
    p_cfg: &ProviderConfig,
    model_name: &str,
    protocol: UpstreamProtocol,
) -> (UpstreamProtocol, Option<String>) {
    let spec = p_cfg.get_model_spec(model_name);
    let endpoint_base = spec
        .base_url
        .or_else(|| p_cfg.endpoint_base_for(protocol).map(|s| s.to_string()));
    (protocol, endpoint_base)
}

#[derive(Debug, Clone)]
pub struct PendingAntigravityOAuth {
    pub created_at: std::time::Instant,
    pub code: Option<String>,
    pub error: Option<String>,
    pub redirect_uri: Option<String>,
    /// F5 (VULN-08): one-shot semantics — set once the flow is consumed by
    /// `authorize` (entry then removed); a second callback must never
    /// overwrite an already-received code.
    pub consumed: bool,
}

/// Cache key for a pooled proxy client: `(proxy_url, total_timeout_secs)`.
/// Distinct timeouts get distinct pools so a per-target override never
/// silently inherits a mismatched total budget.
fn proxy_client_key(url: &str, timeout: std::time::Duration) -> String {
    format!("{}|{}", url, timeout.as_secs())
}

#[derive(Debug)]
pub struct AppState {
    pub config: RwLock<GatewayConfig>,
    pub pools: Arc<RwLock<HashMap<String, Arc<KeyPool>>>>,
    /// Per-provider egress pools (contract `2026-10-07-egress-pool-contract`):
    /// exit-IP rotation with independent per-entry cooldowns. Populated from
    /// `config.providers[*].egress_pool` at build/reload time (and rebuilt on
    /// admin provider writes); absent for pool-less providers (legacy proxy
    /// semantics). Parallel to `pools` so the admin quota view can read it.
    pub egress_pools: Arc<RwLock<HashMap<String, Arc<EgressPool>>>>,
    pub flight_recorder: Arc<FlightRecorder>,
    pub metrics: Arc<MetricsCollector>,
    pub hot_cache: Arc<HotCacheTracker>,
    /// Single-append observability bus: the only write path for telemetry.
    pub event_bus: Arc<EventBus>,
    pub metrics_proj: Arc<MetricsProjection>,
    pub stream_proj: Arc<StreamProjection>,
    pub connectivity_sampler: Arc<ConnectivitySampler>,
    pub timeseries_proj: Arc<TimeseriesProjection>,
    /// Global reusable HTTP client with connection pool and TCP nodelay.
    pub http_client: reqwest::Client,
    /// Dedicated direct client for models/providers explicitly overriding to direct.
    direct_client: reqwest::Client,
    /// Shared HTTP clients per explicit proxy URL (connection pooling reuse across models & providers).
    proxy_clients: RwLock<HashMap<String, reqwest::Client>>,
    /// Pending Antigravity OAuth states with TTL for CSRF validation and callback capture.
    pub pending_antigravity_oauth: RwLock<HashMap<String, PendingAntigravityOAuth>>,
    /// Admin API persistence boundary (WEB-03): `None` for SDK/embedded builds
    /// (write endpoints answer 503 admin_store_unavailable). Hand-written Debug
    /// because the trait object is not Debug.
    pub config_store: Option<std::sync::Arc<dyn crate::admin_store::ConfigStore>>,
    /// Process start instant for admin service/status uptime (WEB-03).
    pub started_at: std::time::Instant,
    /// 全集群遥测持久化与聚合引擎
    pub cluster_telemetry_store: Option<Arc<crate::cluster_telemetry::ClusterTelemetryStore>>,
    pub cluster_telemetry_tracker: Arc<crate::cluster_telemetry::ClusterTelemetryTracker>,
    /// Write queue lock serializing admin config mutations (WEB-06).
    pub admin_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Dashboard telemetry snapshot path (`None` disables persistence).
    pub telemetry_snapshot_path: Option<std::path::PathBuf>,
    /// Pending usage state snapshots waiting for pools to be registered
    pub pending_restored_usages:
        Arc<RwLock<HashMap<String, ponyllm_core::pool::usage::KeyUsageStateSnapshot>>>,
    /// Config backend polling interval surfaced as overview `hot_reload_ms`.
    /// file=500 (legacy mtime watcher), kubernetes=2000 (Secret poll).
    pub config_poll_ms: u64,
    /// Draining flag for graceful shutdown: once `true`, the config poller
    /// stops, the antigravity keepalive worker skips rounds, and refresh
    /// write-backs are suppressed.
    pub shutdown_rx: Arc<tokio::sync::watch::Receiver<bool>>,
    /// Cross-replica antigravity refresh serialization gate (injected by the
    /// CLI when `PONYLLM_LOCK_DATABASE_URL` is set).
    pub refresh_gate: Arc<RwLock<Option<Arc<dyn RefreshGate>>>>,
    /// key_id → instant of the last successful refresh in THIS process.
    /// Drives the rebuild token-freshness guard and the invalid_grant
    /// reconciliation buffer.
    pub last_antigravity_refresh: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    /// key_id → consecutive `invalid_grant` count with no intervening
    /// successful refresh. Quarantine only after N consecutive hits (the
    /// propagation-window buffer from the HA review).
    pub antigravity_invalid_grant_count: Arc<tokio::sync::Mutex<HashMap<String, u32>>>,
    /// Data-plane egress guard verdict cache (VULN-07/F6): `(proxied, host)`
    /// → last verdict with expiry. Per-request re-validation of routed
    /// upstream URLs pays no DNS on the hot path. Keyed by BOTH the proxied
    /// mode and the host so a proxied Ok verdict can never satisfy a direct
    /// dial (mode isolation — a direct dial must always re-resolve, otherwise
    /// the DNS-rebinding defense is silently disabled). TTLs: positive 5s /
    /// deterministic refusal 10s / transient DNS failure 1s (fail-closed on
    /// miss). See [`AppState::data_plane_egress_guard`].
    pub egress_guard_cache: std::sync::Mutex<HashMap<(bool, String), EgressGuardVerdict>>,
    /// Per-`(proxied, host)` in-flight resolution dedup (thundering-herd
    /// guard): concurrent cache misses collapse onto one resolution; waiters
    /// are woken by the owner and re-read the cache.
    pub egress_guard_inflight: std::sync::Mutex<HashMap<(bool, String), Arc<tokio::sync::Notify>>>,
    /// F1 (VULN-17): authentication mode latched at startup. A Secured-start
    /// gateway refuses runtime reloads that would flip it open.
    pub startup_auth_state: StartupAuthState,
    /// F4 (VULN-02): parsed admin IP fence CIDRs. `None`/empty = fence off.
    /// Non-empty → `/api/admin/*` requires the resolved client IP inside.
    pub admin_ip_allowlist: Arc<parking_lot::RwLock<Option<Vec<ipnet::IpNet>>>>,
    /// F3 (VULN-12): exact proxy IPs trusted to append `X-Forwarded-For`.
    pub trusted_proxies: Arc<parking_lot::RwLock<Vec<std::net::IpAddr>>>,
    /// F2 (VULN-01): auth-failure rate limiter (sliding window per
    /// (client IP, key prefix), tiered lockout).
    pub auth_ratelimiter: Arc<crate::auth_ratelimit::AuthRateLimiter>,
    /// Phase-3 (VULN-05): per-pod HttpOnly-cookie admin session store.
    /// `None` = sessions disabled (routes absent, cookie auth off).
    pub admin_session_store: Arc<parking_lot::RwLock<Option<Arc<crate::session::SessionStore>>>>,
    /// PonySentry telemetry & error reporting client.
    pub sentry: Arc<ponyllm_core::sentry::SentryClient>,
    /// Model circuit breaker: `(provider_name, model_name)` -> cooled_until instant.
    /// Tracks sudden upstream model deactivations (404 / 400 / 403) to prevent downstream disruptions.
    pub model_breaker: Arc<parking_lot::RwLock<HashMap<(String, String), std::time::Instant>>>,
}

/// F1: authentication mode frozen at startup (see `AppState::startup_auth_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupAuthState {
    /// A credential is required; empty-key runtime reloads are rejected.
    Secured,
    /// Explicit `auth_mode=open` at startup; reloads stay open.
    Open,
}

/// Parse F4 admin fence CIDR strings; invalid entries are dropped (never a
/// startup failure — a typo must not brick the gateway, it just narrows the
/// fence to the valid entries). Bare IPs (no mask) are accepted as host nets —
/// `ipnet`'s `FromStr` only accepts masked forms, so `127.0.0.1` normalizes
/// to `127.0.0.1/32`.
///
/// R4 (Phase-2b) fail-closed semantics:
/// - Empty input list → `None` = fence not configured (off).
/// - Non-empty input where EVERY entry is unparseable → `Some(vec![])` =
///   fence ACTIVE matching nothing → every `/api/admin/*` call denied (404).
///   A misconfigured allowlist must fail closed, never silently disable the
///   fence (that would reopen the surface the operator meant to lock).
pub(crate) fn parse_admin_allowlist(raw: &[String]) -> Option<Vec<ipnet::IpNet>> {
    let mut out = Vec::new();
    let mut saw_any_entry = false;
    for s in raw {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        saw_any_entry = true;
        match s.parse::<ipnet::IpNet>() {
            Ok(net) => out.push(net),
            Err(_) => match s.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(v4)) => {
                    out.push(ipnet::IpNet::V4(
                        ipnet::Ipv4Net::new(v4, 32).expect("v4 /32"),
                    ));
                }
                Ok(std::net::IpAddr::V6(v6)) => {
                    out.push(ipnet::IpNet::V6(
                        ipnet::Ipv6Net::new(v6, 128).expect("v6 /128"),
                    ));
                }
                Err(_) => {
                    tracing::warn!(cidr = %s, "admin_ip_allowlist: dropping unparseable entry");
                }
            },
        }
    }
    if !saw_any_entry {
        // Nothing configured → fence off.
        None
    } else {
        Some(out)
    }
}

/// Parse F3 trusted proxy IP strings (exact IPs per the F3 contract);
/// invalid entries are dropped with a warning.
pub(crate) fn parse_trusted_proxies(raw: &[String]) -> Vec<std::net::IpAddr> {
    raw.iter()
        .filter_map(|s| {
            let s = s.trim();
            match s.parse::<std::net::IpAddr>() {
                Ok(ip) => Some(ip),
                Err(_) => {
                    tracing::warn!(proxy = %s, "trusted_proxies: dropping unparseable IP");
                    None
                }
            }
        })
        .collect()
}

/// Cached verdict for the data-plane egress guard (VULN-07/F6, R3).
///
/// Fields are `pub` so acceptance tests can observe the verdicts and cache
/// TTLs (see `tests/acceptance_sec_egress_cache_tests.rs` and
/// `tests/acceptance_egress_proxied_tests.rs`).
#[derive(Clone, Copy, Debug)]
pub struct EgressGuardVerdict {
    pub ok: bool,
    /// Stability classification: `true` = transient DNS failure (short TTL,
    /// re-checked almost immediately); `false` = deterministic refusal.
    pub transient: bool,
    pub expires_at: std::time::Instant,
}

/// RAII owner guard for the egress single-flight `(proxied, host)` entries.
///
/// A cancelled or panicking owner between registration and the explicit
/// removal would otherwise strand its `(mode, host)` key forever: every
/// waiter would see a stale `Occupied` entry, never become owner, and loop
/// through the bounded wait indefinitely (per-host hang / DoS). On `Drop`
/// (normal return, task cancellation, or panic unwinding) the entry is
/// removed so the next waiter can take over as owner and complete.
struct InflightEntry<'a> {
    map: &'a std::sync::Mutex<HashMap<(bool, String), Arc<tokio::sync::Notify>>>,
    key: (bool, String),
    removed: bool,
}

impl Drop for InflightEntry<'_> {
    fn drop(&mut self) {
        if !self.removed {
            // into_inner aligns with the codebase's cache-lock idiom: a
            // poisoned mutex must still remove the stale entry, otherwise a
            // cancelled owner could strand the key on the poison path too.
            let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
            map.remove(&self.key);
        }
    }
}

impl std::fmt::Debug for dyn crate::admin_store::ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigStore")
    }
}

impl AppState {
    pub fn new(config: GatewayConfig) -> Self {
        let capacity = config.flight_recorder_capacity;
        let flight_recorder = Arc::new(FlightRecorder::new(capacity));
        let metrics = Arc::new(MetricsCollector::new());
        let bus = Arc::new(EventBus::new(capacity));
        let metrics_proj = Arc::new(MetricsProjection::new(metrics.clone()));
        let stream_proj = Arc::new(StreamProjection::default());
        let gw_timeout = std::time::Duration::from_secs(config.upstream_timeout_secs);
        let direct_client = ponyllm_core::executor::create_upstream_http_client_with_timeout(
            None, false, gw_timeout,
        );
        let http_client = ponyllm_core::executor::create_upstream_http_client_with_timeout(
            config.proxy.as_deref(),
            config.use_system_proxy,
            gw_timeout,
        );
        let mut proxy_clients = HashMap::new();
        for (_, p_cfg) in &config.providers {
            if let Some(proxy) = &p_cfg.proxy {
                let trimmed = proxy.trim();
                if !trimmed.is_empty()
                    && !trimmed.eq_ignore_ascii_case("direct")
                    && !trimmed.eq_ignore_ascii_case("none")
                {
                    proxy_clients
                        .entry(proxy_client_key(trimmed, gw_timeout))
                        .or_insert_with(|| {
                            ponyllm_core::executor::create_upstream_http_client_with_timeout(
                                Some(trimmed),
                                config.use_system_proxy,
                                gw_timeout,
                            )
                        });
                }
            }
            for m_spec in &p_cfg.model_specs {
                if let Some(proxy) = &m_spec.proxy {
                    let trimmed = proxy.trim();
                    if !trimmed.is_empty()
                        && !trimmed.eq_ignore_ascii_case("direct")
                        && !trimmed.eq_ignore_ascii_case("none")
                    {
                        proxy_clients
                            .entry(proxy_client_key(trimmed, gw_timeout))
                            .or_insert_with(|| {
                                ponyllm_core::executor::create_upstream_http_client_with_timeout(
                                    Some(trimmed),
                                    config.use_system_proxy,
                                    gw_timeout,
                                )
                            });
                    }
                }
            }
        }

        let connectivity_sampler = Arc::new(ConnectivitySampler::default());
        let timeseries_proj = Arc::new(TimeseriesProjection::default());

        bus.add_projection(metrics_proj.clone());
        bus.add_projection(stream_proj.clone());
        bus.add_projection(timeseries_proj.clone());
        bus.add_projection(connectivity_sampler.clone());
        bus.add_projection(Arc::new(FrameConverter::new(flight_recorder.clone())));
        if let Some(dir) = config.event_log_dir.clone() {
            crate::segments::spawn_segment_drain(
                &bus,
                dir,
                config.event_log_retention_days,
                config.event_log_max_bytes,
            );
        }
        let telemetry_snapshot_path = resolve_snapshot_path(&config);
        let pools: Arc<RwLock<HashMap<String, Arc<KeyPool>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let egress_pools: Arc<RwLock<HashMap<String, Arc<EgressPool>>>> =
            Arc::new(RwLock::new(Self::build_egress_pools(&config)));
        let mut restored_usages = HashMap::new();
        if let Some(ref path) = telemetry_snapshot_path {
            if path.is_file() {
                match crate::telemetry_snapshot::load_snapshot(path) {
                    Some(snap) => {
                        timeseries_proj.restore_buckets(snap.timeseries);
                        metrics.restore_counters(&snap.metrics);
                        connectivity_sampler.restore_state(snap.connectivity);
                        stream_proj.restore_nodes(snap.streams);
                        restored_usages = snap.key_usages;
                        tracing::info!("telemetry snapshot restored from {:?}", path);
                    }
                    None => {
                        tracing::warn!(
                            "telemetry snapshot at {:?} unreadable, starting fresh",
                            path
                        );
                    }
                }
            }
        }
        let pending_restored_usages = Arc::new(RwLock::new(restored_usages));
        let cluster_telemetry_store =
            crate::cluster_telemetry::ClusterTelemetryStore::from_env().map(Arc::new);
        let cluster_telemetry_tracker =
            Arc::new(crate::cluster_telemetry::ClusterTelemetryTracker::new());

        if let Some(ref store) = cluster_telemetry_store {
            let store_clone = store.clone();
            let tracker_clone = cluster_telemetry_tracker.clone();
            let timeseries_clone = timeseries_proj.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
                loop {
                    interval.tick().await;
                    let buckets = timeseries_clone.snapshot_buckets();
                    tracker_clone.record_local_snapshot(&buckets);
                    let deltas = tracker_clone.drain_deltas();
                    if !deltas.is_empty() {
                        if let Err(e) = store_clone.flush_deltas(deltas).await {
                            tracing::warn!("failed to flush cluster telemetry deltas to PG: {}", e);
                        }
                    }
                }
            });
        }

        if let Some(ref path) = telemetry_snapshot_path {
            spawn_snapshot_saver(
                path.clone(),
                timeseries_proj.clone(),
                metrics.clone(),
                connectivity_sampler.clone(),
                stream_proj.clone(),
                pools.clone(),
            );
        }
        // Phase-2 auth hardening (F1/F2/F4/F3): compute before `config` is
        // moved into the RwLock below.
        let startup_auth_state = if config.auth_mode == ponyllm_config::AuthMode::Open {
            StartupAuthState::Open
        } else {
            StartupAuthState::Secured
        };
        let admin_allowlist = parse_admin_allowlist(&config.admin_ip_allowlist);
        let trusted = parse_trusted_proxies(&config.trusted_proxies);
        let ratelimiter = crate::auth_ratelimit::AuthRateLimiter::new(
            config.auth_fail_window_secs,
            config.auth_fail_limit,
            config.auth_lockout_secs,
        );
        let admin_sessions = config.admin_session_enabled.then(|| {
            Arc::new(crate::session::SessionStore::new(
                std::time::Duration::from_secs(config.admin_session_ttl_secs),
            ))
        });
        Self {
            config: RwLock::new(config),
            pools,
            egress_pools,
            flight_recorder,
            metrics,
            hot_cache: Arc::new(HotCacheTracker::new()),
            event_bus: bus,
            metrics_proj,
            stream_proj,
            connectivity_sampler,
            timeseries_proj,
            http_client,
            direct_client,
            proxy_clients: RwLock::new(proxy_clients),
            pending_antigravity_oauth: RwLock::new(HashMap::new()),
            config_store: None,
            started_at: std::time::Instant::now(),
            cluster_telemetry_store,
            cluster_telemetry_tracker,
            admin_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            telemetry_snapshot_path,
            pending_restored_usages,
            config_poll_ms: FILE_CONFIG_POLL_MS,
            shutdown_rx: Arc::new(tokio::sync::watch::channel(false).1),
            refresh_gate: Arc::new(RwLock::new(None)),
            last_antigravity_refresh: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            antigravity_invalid_grant_count: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            egress_guard_cache: std::sync::Mutex::new(HashMap::new()),
            egress_guard_inflight: std::sync::Mutex::new(HashMap::new()),
            startup_auth_state,
            admin_ip_allowlist: Arc::new(parking_lot::RwLock::new(admin_allowlist)),
            trusted_proxies: Arc::new(parking_lot::RwLock::new(trusted)),
            auth_ratelimiter: Arc::new(ratelimiter),
            admin_session_store: Arc::new(parking_lot::RwLock::new(admin_sessions)),
            sentry: Arc::new({
                let endpoint = std::env::var("PONY_SENTRY_URL")
                    .or_else(|_| std::env::var("PONY_SENTRY_DSN"))
                    .unwrap_or_default();
                if endpoint.is_empty() {
                    ponyllm_core::sentry::SentryClient::noop()
                } else {
                    let client_token = std::env::var("PONY_SENTRY_CLIENT_TOKEN").ok().filter(|s| !s.is_empty());
                    ponyllm_core::sentry::SentryClient::new(ponyllm_core::sentry::SentryConfig {
                        endpoint,
                        client_token,
                        environment: std::env::var("PONY_SENTRY_ENVIRONMENT").ok().or_else(|| Some("production".into())),
                        release: option_env!("CARGO_PKG_VERSION").map(|s| s.to_string()),
                        buffer_capacity: 1024,
                    })
                }
            }),
            model_breaker: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        }
    }

    /// Record a sudden model outage (404/400 model not found/unsupported) for circuit breaking
    pub fn record_model_outage(&self, provider: &str, model: &str, cooldown: std::time::Duration) {
        let until = std::time::Instant::now() + cooldown;
        let mut breaker = self.model_breaker.write();
        breaker.insert((provider.to_string(), model.to_string()), until);

        // Report to PonySentry if enabled
        let mut tags = HashMap::new();
        tags.insert("event_type".to_string(), "model_circuit_breaker_tripped".to_string());
        tags.insert("provider".to_string(), provider.to_string());
        tags.insert("model".to_string(), model.to_string());
        tags.insert("cooldown_secs".to_string(), cooldown.as_secs().to_string());

        let mut extra = serde_json::Map::new();
        extra.insert("provider".to_string(), serde_json::json!(provider));
        extra.insert("model".to_string(), serde_json::json!(model));
        extra.insert("cooldown_secs".to_string(), serde_json::json!(cooldown.as_secs()));

        self.sentry.capture_error(
            "ModelOutageCircuitBreaker",
            &format!("Model '{model}' on provider '{provider}' tripped circuit breaker for {}s due to sudden outage", cooldown.as_secs()),
            Some(tags),
            Some(serde_json::Value::Object(extra)),
        );
        tracing::warn!(
            provider = %provider,
            model = %model,
            cooldown_secs = cooldown.as_secs(),
            "Model circuit breaker tripped: model marked down"
        );
    }

    /// Check if a model is currently cooling down under the model circuit breaker
    pub fn is_model_cooling_down(&self, provider: &str, model: &str) -> bool {
        let now = std::time::Instant::now();
        let breaker = self.model_breaker.read();
        if let Some(until) = breaker.get(&(provider.to_string(), model.to_string())) {
            *until > now
        } else {
            false
        }
    }
    pub fn save_telemetry_snapshot(&self) -> std::io::Result<()> {
        let path = match &self.telemetry_snapshot_path {
            Some(p) => p.clone(),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "telemetry snapshot disabled",
                ))
            }
        };
        let snap = crate::telemetry_snapshot::TelemetrySnapshot {
            version: crate::telemetry_snapshot::SNAPSHOT_VERSION,
            saved_at_ms: 0,
            timeseries: self.timeseries_proj.snapshot_buckets(),
            metrics: self.metrics.snapshot_counters(),
            connectivity: self.connectivity_sampler.snapshot_state(),
            streams: self.stream_proj.snapshot_nodes(),
            key_usages: {
                let mut usages = HashMap::new();
                let pools = self.pools.read();
                for pool in pools.values() {
                    for key in pool.snapshot_keys() {
                        usages.insert(key.id.clone(), key.usage_tracker.export_snapshot());
                    }
                }
                usages
            },
            pool_cycle_benchmark: Default::default(),
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let live_cycles = collect_live_key_cycles(&self.pools, now_ms);
        crate::telemetry_snapshot::save_snapshot_with_live_cycles(&path, &snap, live_cycles)
    }

    /// Override the HTTP client (useful for mock transports in tests).
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.direct_client = client.clone();
        self.http_client = client;
        self
    }

    /// Attach the admin config persistence boundary (WEB-03). Builder style so
    /// the CLI serve path can enable it while SDK builds stay `None`.
    pub fn with_config_store(
        mut self,
        store: std::sync::Arc<dyn crate::admin_store::ConfigStore>,
    ) -> Self {
        self.config_store = Some(store);
        self
    }

    /// Override the config backend polling interval (overview `hot_reload_ms`).
    pub fn with_config_poll_ms(mut self, ms: u64) -> Self {
        self.config_poll_ms = ms;
        self
    }

    /// Share the graceful-shutdown watch: when the sender flips to `true`,
    /// this process is draining (stop polling / refresh / persist).
    pub fn with_shutdown_rx(mut self, rx: tokio::sync::watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Arc::new(rx);
        self
    }

    /// Install (or clear) the cross-replica refresh serialization gate.
    pub fn with_refresh_gate(self, gate: Option<Arc<dyn RefreshGate>>) -> Self {
        *self.refresh_gate.write() = gate;
        self
    }

    /// True once the graceful-shutdown signal fired: the process is draining.
    pub fn is_draining(&self) -> bool {
        *self.shutdown_rx.borrow()
    }

    /// Return the HTTP client for the given provider and model target.
    ///
    /// Respects model-level proxy override > provider-level proxy > gateway default.
    /// Connections are pooled and reused across targets pointing to the same proxy endpoint.
    /// Providers with an egress pool (contract `2026-10-07-egress-pool-contract`)
    /// get the DIRECT client: the pool REPLACES the single-proxy semantics and
    /// per-attempt proxy selection happens inside the executor.
    pub fn http_client_for_target(&self, provider_name: &str, model_name: &str) -> reqwest::Client {
        let cfg = self.config.read();
        let timeout_secs = cfg
            .providers
            .get(provider_name)
            .and_then(|p| p.effective_timeout_secs_for_model(model_name))
            .unwrap_or(cfg.upstream_timeout_secs);
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let gw_timeout = std::time::Duration::from_secs(cfg.upstream_timeout_secs);
        let use_sys = cfg.use_system_proxy;
        // Same-source effective proxy as the data-plane egress guard
        // (`data_plane_egress_guard_for_target`): a drift here would either
        // break availability (guard thinks proxied, client dials direct) or
        // break SSRF guarantees (guard skips DNS for a direct target).
        let has_egress_pool = cfg
            .providers
            .get(provider_name)
            .map(|p| !p.egress_pool.is_empty())
            .unwrap_or(false);
        let proxy_url: Option<String> = if has_egress_pool {
            None
        } else {
            cfg.effective_proxy_url_for(provider_name, model_name)
        };
        drop(cfg);

        // Fast paths for the gateway defaults (no per-target override):
        // reuse the prebuilt gateway/direct clients.
        if timeout == gw_timeout {
            match proxy_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                None => return self.direct_client.clone(),
                Some(url) => return self.get_or_create_proxy_client(url, use_sys, timeout),
            }
        }
        // Overridden timeout: build/refresh a client keyed by (url, timeout).
        let url = proxy_url.as_deref().unwrap_or("");
        self.get_or_create_proxy_client(url, use_sys, timeout)
    }

    /// Helper for retrieving or lazily building a connection pool client for
    /// a proxy endpoint, keyed by `(url, total_timeout)` so per-target
    /// timeout overrides get their own pool instead of inheriting a
    /// mismatched budget.
    fn get_or_create_proxy_client(
        &self,
        url: &str,
        use_system_proxy: bool,
        total_timeout: std::time::Duration,
    ) -> reqwest::Client {
        let key = proxy_client_key(url, total_timeout);
        if let Some(client) = self.proxy_clients.read().get(&key) {
            return client.clone();
        }
        let mut write = self.proxy_clients.write();
        if let Some(client) = write.get(&key) {
            return client.clone();
        }
        let proxy_opt = if url.trim().is_empty() {
            None
        } else {
            // REGRESSION FIX (egress proxied fast path, ADR
            // 2026-10-06-egress-guard-proxied-dns-skip): the previous
            // `url.trim().is_empty().then(|| url)` inverted this — a NON-empty
            // proxy URL produced `None`, so any cache miss (gateway-default
            // proxy, or provider/model proxy with a per-target timeout
            // override) built a DIRECT client while the egress guard assumed
            // the dial went through the proxy and skipped local DNS → SSRF
            // decoupling. Non-empty must build the proxied client.
            Some(url)
        };
        let client = ponyllm_core::executor::create_upstream_http_client_with_timeout(
            proxy_opt,
            use_system_proxy,
            total_timeout,
        );
        write.insert(key, client.clone());
        client
    }

    /// Return the HTTP client for the given provider (inherits provider default proxy).
    pub fn http_client_for_provider(&self, provider_name: &str) -> reqwest::Client {
        self.http_client_for_target(provider_name, "")
    }

    /// Build per-provider egress pools from a runtime config (contract
    /// `2026-10-07-egress-pool-contract`). Providers without a non-empty
    /// `egress_pool` are absent from the map (legacy proxy semantics).
    fn build_egress_pools(config: &GatewayConfig) -> HashMap<String, Arc<EgressPool>> {
        let mut out = HashMap::new();
        for (name, p_cfg) in &config.providers {
            if let Some(pool) = Self::build_egress_pool_for_cfg(name, p_cfg) {
                out.insert(name.clone(), pool);
            }
        }
        out
    }

    /// Build the live `EgressPool` for one provider entry, normalizing
    /// `direct`/`none`/empty entries and validating proxy URLs against the
    /// shared egress policy. `None` when the pool is empty (or every entry
    /// was invalid and dropped) — the legacy proxy path.
    fn build_egress_pool_for_cfg(name: &str, p_cfg: &ProviderConfig) -> Option<Arc<EgressPool>> {
        let raw = p_cfg.effective_egress_pool()?;
        // Strategy must parse like the admin PUT gate (which 400s bad values):
        // an unparseable strategy never builds a pool — the provider silently
        // falls back to the legacy proxy semantics with a loud warning, so the
        // config load and the admin write path agree (review STRATEGY-DRIFT).
        let strategy = match p_cfg.egress_strategy.parse::<EgressStrategy>() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    provider = %name,
                    strategy = %p_cfg.egress_strategy,
                    "invalid egress_strategy: {} — egress pool disabled, falling back to proxy semantics",
                    e
                );
                return None;
            }
        };
        let pool = EgressPool::new(name, strategy);
        for (i, entry) in raw.iter().enumerate() {
            let id = format!("egress-{}", i);
            let trimmed = entry.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("direct")
                || trimmed.eq_ignore_ascii_case("none")
            {
                pool.add_egress(EgressEntry::direct(id));
                continue;
            }
            // Defense in depth: a config that bypassed the admin write gate
            // must not let a pool entry dial a private/metadata target.
            if let Err(reason) = ponyllm_config::validate_egress_entry(trimmed) {
                tracing::warn!(
                    provider = %name,
                    entry = %trimmed,
                    "dropping invalid egress pool entry: {}",
                    reason
                );
                continue;
            }
            pool.add_egress(EgressEntry::proxy(id, trimmed));
        }
        if pool.is_empty() {
            None
        } else {
            Some(Arc::new(pool))
        }
    }

    /// Egress runtime for one provider/model target: the provider's live pool
    /// (if configured) plus per-proxy-URL upstream clients with the target's
    /// effective timeout (direct entries reuse the base client). Both `None`
    /// = legacy single-proxy path, byte-identical to before the contract.
    pub fn egress_runtime_for_target(
        &self,
        provider_name: &str,
        model_name: &str,
    ) -> (
        Option<Arc<EgressPool>>,
        Option<HashMap<String, reqwest::Client>>,
    ) {
        let Some(pool) = self.egress_pools.read().get(provider_name).cloned() else {
            return (None, None);
        };
        let cfg = self.config.read();
        let timeout_secs = cfg
            .providers
            .get(provider_name)
            .and_then(|p| p.effective_timeout_secs_for_model(model_name))
            .unwrap_or(cfg.upstream_timeout_secs);
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let use_sys = cfg.use_system_proxy;
        drop(cfg);
        let mut clients = HashMap::new();
        for url in pool.proxy_urls() {
            clients.insert(
                url.clone(),
                self.get_or_create_proxy_client(&url, use_sys, timeout),
            );
        }
        (Some(pool), Some(clients))
    }

    /// Rebuild one provider's egress pool from the current runtime config
    /// (admin create/update paths). A cleared/empty pool removes the entry,
    /// falling back to the legacy proxy semantics. When the pool shape
    /// (entries + strategy) is unchanged, the existing `Arc` is kept so live
    /// per-exit cooldowns survive no-op admin writes.
    pub fn rebuild_egress_pool_for(&self, provider_name: &str) {
        let fresh = {
            let cfg = self.config.read();
            cfg.providers
                .get(provider_name)
                .and_then(|p| Self::build_egress_pool_for_cfg(provider_name, p))
        };
        let mut map = self.egress_pools.write();
        match fresh {
            Some(p) => {
                let shape_changed = map
                    .get(provider_name)
                    .is_none_or(|existing| existing.shape() != p.shape());
                if shape_changed {
                    map.insert(provider_name.to_string(), p);
                }
            }
            None => {
                map.remove(provider_name);
            }
        }
    }

    /// Probe-only variant of [`Self::http_client_for_provider`] (H2 red-team
    /// B2): resolves the SAME effective proxy (so the probe shares the data
    /// plane's egress IP — P0-7 consistency is preserved; only the redirect
    /// policy and timeouts differ), but never follows redirects. Used by the
    /// admin quota/model-list probes whose target URL is operator-controlled.
    pub fn probe_http_client_for_provider(&self, provider_name: &str) -> reqwest::Client {
        let proxy_url = {
            let cfg = self.config.read();
            match cfg
                .providers
                .get(provider_name)
                .map(|p| p.effective_proxy_for_model(""))
                .unwrap_or(crate::config::EffectiveProxy::InheritGateway)
            {
                crate::config::EffectiveProxy::Custom(url) => Some(url.to_string()),
                crate::config::EffectiveProxy::Direct => Some(String::new()),
                crate::config::EffectiveProxy::InheritGateway => cfg.proxy.clone(),
            }
        };
        // Empty string = explicit direct (mirrors create_upstream semantics:
        // empty/None both mean "no proxy", and the probe builder treats them
        // identically). Mirror the data-plane rule: no ambient system proxy
        // unless the deployment opted in — the probe builder never inherits
        // system proxy, matching the fail-closed posture.
        let proxy_opt = proxy_url.as_deref().filter(|s| !s.is_empty());
        ponyllm_core::executor::create_probe_http_client_with_options(proxy_opt)
    }

    /// Data-plane egress guard for DIRECT dials (VULN-07/F6, R3): re-validate
    /// a routed upstream URL immediately before dialing so a provider
    /// hostname that rebinds to an internal address AFTER the write-time
    /// check is refused here, fail-closed (no dial on refusal). A short TTL
    /// cache keyed by `(proxied=false, host)` keeps the hot path DNS-free:
    /// positive verdicts cached 5s, deterministic refusals 10s, transient DNS
    /// failures 1s; a cache miss re-resolves with the 5s fail-closed bound
    /// inside `egress::check_data_plane_url`.
    ///
    /// Residual TOCTOU boundary (stated honestly): a hostname that rebinds
    /// WITHIN the positive-cache window (≤5s) is not re-resolved until the
    /// window expires; this is bounded to the TTL by design, and mitigated
    /// further by `redirect(Policy::none)` on the data-plane client (a 3xx
    /// cannot steer to an internal target after the gate) and by fail-closed
    /// refusals on every miss. The admin-probe path (`check_probe_url`)
    /// re-resolves per call with no positive cache.
    ///
    /// Loopback upstreams stay legitimate by design (documented data-plane
    /// shape — local Ollama); LAN model-server names AND literal IPs are
    /// lifted via `PONYLLM_PROBE_ALLOWLIST` (same operator hatch as admin
    /// probes).
    pub async fn data_plane_egress_guard(&self, url: &str) -> std::result::Result<(), String> {
        self.guard_url(false, url).await
    }

    /// Data-plane egress guard when the target's effective outbound proxy is
    /// known (model > provider > gateway, same source as the upstream HTTP
    /// client via [`GatewayConfig::effective_proxy_url_for`]): if the proxy is
    /// fast-path eligible (see [`crate::egress::proxy_fast_path_eligible`])
    /// the guard runs the NO-DNS proxied policy — the trusted proxy owns DNS +
    /// egress for the target, and the gateway's local resolution is irrelevant
    /// to where bytes go (it was the source of the observed 5s fail-closed +
    /// 10s negative-cache 503 storms for GFW-blocked Google domains).
    /// Otherwise (socks / unparseable proxy / no_proxy-exempt target /
    /// system-proxy-only) it falls back to the full direct check so the guard
    /// never skips DNS for a target that is actually dialed directly.
    pub async fn data_plane_egress_guard_for_target(
        &self,
        provider_name: &str,
        model_name: &str,
        url: &str,
    ) -> std::result::Result<(), String> {
        let proxied = {
            let cfg = self.config.read();
            // Conservative degradation (ADR Non-Goal): with `use_system_proxy`
            // the client also layers system/env proxies and honors NO_PROXY,
            // so an explicit proxy is no longer a guarantee of where bytes
            // go — always run the full direct check (宁多解析不少解析).
            let has_egress_pool = cfg
                .providers
                .get(provider_name)
                .map(|p| !p.egress_pool.is_empty())
                .unwrap_or(false);
            if has_egress_pool {
                // The egress pool REPLACES the single-proxy semantics and the
                // per-attempt exit is unknown at guard time: run the full
                // direct check so a pool `direct` entry can never skip local
                // DNS (SSRF decoupling must not regress).
                false
            } else if cfg.use_system_proxy {
                false
            } else {
                cfg.effective_proxy_url_for(provider_name, model_name)
                    .map(|p| crate::egress::proxy_fast_path_eligible(&p, url))
                    .unwrap_or(false)
            }
        };
        if proxied {
            tracing::trace!(
                provider = %provider_name,
                model = %model_name,
                url = %url,
                "data-plane egress guard: proxied fast path (no local DNS)"
            );
        }
        self.guard_url(proxied, url).await
    }

    /// Shared guard core: verdict cache keyed by `(proxied, host)` with
    /// tri-state TTLs (Ok 5s / deterministic refusal 10s / transient DNS
    /// failure 1s) and per-key in-flight single-flight dedup so a burst of
    /// concurrent cache misses collapses onto ONE resolution instead of N
    /// `spawn_blocking` DNS lookups.
    async fn guard_url(&self, proxied: bool, url: &str) -> std::result::Result<(), String> {
        let host = crate::egress::parse_host(url)?.to_ascii_lowercase();
        let key = (proxied, host.clone());
        loop {
            {
                let cache = self
                    .egress_guard_cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if let Some(v) = cache.get(&key) {
                    if v.expires_at > std::time::Instant::now() {
                        if v.ok {
                            return Ok(());
                        }
                        return Err(format!(
                            "egress guard refused data-plane upstream host '{}' (cached)",
                            host
                        ));
                    }
                }
            }
            // Register the in-flight entry; only the owner performs the
            // resolution, waiters get woken and re-check the cache. The owner
            // holds an `InflightEntry` RAII guard so cancellation/panic
            // between registration and removal cannot strand the key forever
            // (a stranded key would leave every waiter in the 6s re-check loop
            // without ever becoming owner → per-host hang / DoS).
            let (nf, owner_guard) = {
                let mut inflight = self
                    .egress_guard_inflight
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match inflight.entry(key.clone()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let nf = Arc::new(tokio::sync::Notify::new());
                        e.insert(nf.clone());
                        (
                            nf,
                            Some(InflightEntry {
                                map: &self.egress_guard_inflight,
                                key: key.clone(),
                                removed: false,
                            }),
                        )
                    }
                    std::collections::hash_map::Entry::Occupied(o) => (o.get().clone(), None),
                }
            };
            let Some(mut owner_guard) = owner_guard else {
                // Waiter: bounded wait (owner may have raced the removal);
                // re-check the cache on wake — the verdict is inserted before
                // notify. If the owner was cancelled, its Drop removed the
                // entry, so this waiter (or the next) becomes the new owner.
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(6), nf.notified()).await;
                continue;
            };
            // Owner: resolve outside any lock.
            let checked = if proxied {
                crate::egress::check_data_plane_url_proxied(url).await
            } else {
                crate::egress::check_data_plane_url(url).await
            };
            let now = std::time::Instant::now();
            let verdict = match &checked {
                Ok(()) => EgressGuardVerdict {
                    ok: true,
                    transient: false,
                    expires_at: now + std::time::Duration::from_secs(5),
                },
                Err(r) => EgressGuardVerdict {
                    ok: false,
                    transient: r.transient,
                    expires_at: now
                        + if r.transient {
                            std::time::Duration::from_secs(1)
                        } else {
                            std::time::Duration::from_secs(10)
                        },
                },
            };
            {
                let mut cache = self
                    .egress_guard_cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                cache.insert(key.clone(), verdict);
            }
            // Remove the in-flight entry BEFORE notifying: a waiter that
            // registers after the removal creates a fresh entry instead of
            // waiting on a stale notifier forever.
            let wake = {
                let mut inflight = self
                    .egress_guard_inflight
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                inflight.remove(&key)
            };
            if let Some(nf) = wake {
                nf.notify_waiters();
            }
            owner_guard.removed = true;
            return checked.map_err(String::from);
        }
    }

    /// Best-effort Antigravity identity for envelope translation (P0-6, B7):
    /// `(project_id, key_id)` from the provider's pool. Prefers an Active
    /// key's manager so cooling/disabled credentials don't donate a stale
    /// project; the key id salts the session hash so identical prompts
    /// under different credentials don't cluster. Falls back to any
    /// Antigravity manager, else `None` (callers use defaults).
    /// Read-only: never touches the round-robin counters. Cross-key
    /// failover inside one request can still mix projects —
    /// single-project-per-provider is the supported topology until
    /// translation moves per-attempt.
    pub fn peek_antigravity_identity(&self, provider_name: &str) -> Option<(String, String)> {
        let pools = self.pools.read();
        let pool = pools.get(provider_name)?;
        let keys = pool.snapshot_keys();
        keys.iter()
            .filter_map(|k| {
                let mgr = k.antigravity_manager()?;
                let state = k.current_state();
                Some((
                    state == ponyllm_core::pool::KeyState::Active,
                    mgr.project_id(),
                    k.id.clone(),
                ))
            })
            .max_by_key(|(active, _, _)| *active)
            .map(|(_, project, key_id)| (project, key_id))
    }

    pub fn reload_config_with_pools(
        &self,
        new_config: GatewayConfig,
        new_pools: HashMap<String, Arc<KeyPool>>,
    ) {
        // F1+B5 (VULN-17): a Secured-start gateway must never flip open at
        // runtime. `reload_config_with_pools` wholesale-replaces the in-memory
        // config; either an explicit `auth_mode="open"` in the new config or
        // an empty/`none` api_key with no scoped keys would turn every
        // endpoint unauthenticated the moment the truth source (k8s Secret)
        // is changed/emptied. Refuse and keep the previous secured config;
        // entering open mode requires a restart (startup guards re-check).
        let tries_to_open = new_config.auth_mode == ponyllm_config::AuthMode::Open
            || ((new_config.api_key.trim().is_empty()
                || new_config.api_key.trim().eq_ignore_ascii_case("none"))
                && new_config.gateway_keys.is_empty());
        if self.startup_auth_state == StartupAuthState::Secured && tries_to_open {
            tracing::error!(
                "F1/B5 fail-closed: refusing config reload that would open the gateway \
                 (auth_mode=open or empty credentials); keeping previous secured config. \
                 To run open mode set auth_mode='open' at startup and restart."
            );
            return;
        }
        // Hot-reload survival: capture the current per-key usage trackers so a
        // fresh pool rebuilt from config keeps its measurement history (slices,
        // completed cycles, capacity EWMA) by key id — accounts/config churn
        // must never zero the cycle history. Keys missing here fall back to the
        // persisted snapshot file below.
        let mut donor_pools: HashMap<String, Arc<KeyPool>> = {
            let pools_guard = self.pools.read();
            pools_guard.clone()
        };

        let mut config_guard = self.config.write();
        let mut pools_guard = self.pools.write();

        // Track which key ids got a live donor so the file fallback below
        // never overwrites a fresher in-memory tracker with disk state.
        let mut transplanted_ids: HashSet<String> = HashSet::new();

        for (name, pool) in &new_pools {
            // Donors are scoped per provider: only reuse measurement state from
            // the OLD pool with the same provider name (key ids may repeat
            // across providers).
            let donors: HashMap<String, Arc<ponyllm_core::pool::ApiKeyEntry>> = donor_pools
                .get(name)
                .map(|old_pool| {
                    old_pool
                        .snapshot_keys()
                        .into_iter()
                        .filter(|k| pool.snapshot_keys().iter().any(|n| n.id == k.id))
                        .map(|k| (k.id.clone(), k))
                        .collect()
                })
                .unwrap_or_default();
            for id in pool.snapshot_keys().iter().map(|k| k.id.clone()) {
                if donors.contains_key(&id) {
                    transplanted_ids.insert(id);
                }
            }
            let matched = pool.import_matched_usage_trackers(&donors);
            // 热重载还必须继承运行时状态（禁用原因 / 进行中的冷冻及其原因与
            // 硬错误消息）：否则一次配置重载就会把 3 天资格冻结清掉，账号被
            // 复活后下一请求再次撞 403，重演整池锤打（2026-10-04 事故路径）。
            if let Some(old_pool) = donor_pools.get(name) {
                pool.inherit_runtime_state(old_pool);
            }
            tracing::debug!(
                provider = %name,
                donated = matched,
                "hot reload: reused usage trackers by key id"
            );
        }
        donor_pools.clear();

        for (name, pool) in new_pools {
            pools_guard.insert(name, pool);
        }

        pools_guard.retain(|name, _| new_config.providers.contains_key(name));

        let mut proxy_clients_guard = self.proxy_clients.write();
        proxy_clients_guard.clear();
        let gw_timeout = std::time::Duration::from_secs(new_config.upstream_timeout_secs);
        for (_, p_cfg) in &new_config.providers {
            if let Some(proxy) = &p_cfg.proxy {
                let trimmed = proxy.trim();
                if !trimmed.is_empty()
                    && !trimmed.eq_ignore_ascii_case("direct")
                    && !trimmed.eq_ignore_ascii_case("none")
                {
                    proxy_clients_guard
                        .entry(proxy_client_key(trimmed, gw_timeout))
                        .or_insert_with(|| {
                            ponyllm_core::executor::create_upstream_http_client_with_timeout(
                                Some(trimmed),
                                new_config.use_system_proxy,
                                gw_timeout,
                            )
                        });
                }
            }
            for m_spec in &p_cfg.model_specs {
                if let Some(proxy) = &m_spec.proxy {
                    let trimmed = proxy.trim();
                    if !trimmed.is_empty()
                        && !trimmed.eq_ignore_ascii_case("direct")
                        && !trimmed.eq_ignore_ascii_case("none")
                    {
                        proxy_clients_guard
                            .entry(proxy_client_key(trimmed, gw_timeout))
                            .or_insert_with(|| {
                                ponyllm_core::executor::create_upstream_http_client_with_timeout(
                                    Some(trimmed),
                                    new_config.use_system_proxy,
                                    gw_timeout,
                                )
                            });
                    }
                }
            }
        }

        tracing::info!(
            "Gateway configuration reloaded. Active providers: {:?}",
            pools_guard.keys().collect::<Vec<_>>()
        );
        self.metrics.record_config_reload();

        // Rebuild egress pools from the new config (contract
        // `2026-10-07-egress-pool-contract`). A provider whose pool shape
        // (entries + strategy) is unchanged keeps its live per-exit cooldown
        // state across the reload — a hot reload must not resurrect an exit
        // mid-quota-window and hammer it again (same principle as the key
        // pool's `inherit_runtime_state`).
        {
            let fresh = Self::build_egress_pools(&new_config);
            let mut egress_guard = self.egress_pools.write();
            egress_guard.retain(|name, old| {
                fresh
                    .get(name)
                    .is_some_and(|new_p| old.shape() == new_p.shape())
            });
            for (name, new_p) in fresh {
                egress_guard.entry(name).or_insert_with(|| new_p);
            }
        }

        *config_guard = new_config;
        drop(config_guard);
        drop(pools_guard);

        // File fallback: keys that survived config changes in the persisted
        // snapshot (e.g. temporarily removed then re-added) still restore
        // their history, without clobbering just-transplanted trackers.
        // Runs after the pools write-lock is released (it re-reads pools).
        self.restore_pool_usages_from_snapshot(&transplanted_ids);

        self.attach_antigravity_rotation_hooks_all();
    }

    /// Restore per-key usage history from the persisted telemetry snapshot for
    /// keys that did NOT receive a live in-memory donor during a hot reload.
    /// Combined with the read-modify-write `key_usages` merging in
    /// `save_snapshot_with_live_cycles`, an account removed and later re-added
    /// with the same id keeps its slices, completed cycles and capacity.
    fn restore_pool_usages_from_snapshot(&self, transplanted_ids: &HashSet<String>) {
        let path = match &self.telemetry_snapshot_path {
            Some(p) => p.clone(),
            None => return,
        };
        let saved = match crate::telemetry_snapshot::load_snapshot_file(&path) {
            Some(f) => f.snapshot.key_usages,
            None => return,
        };
        let pools = self.pools.read();
        for pool in pools.values() {
            for key in pool.snapshot_keys() {
                if transplanted_ids.contains(&key.id) {
                    continue;
                }
                if let Some(s) = saved.get(&key.id) {
                    key.usage_tracker.import_snapshot(s.clone());
                    tracing::info!(
                        provider = %pool.provider,
                        key_id = %key.id,
                        "hot reload: restored usage history from persisted snapshot"
                    );
                }
            }
        }
    }

    /// Rebuild-time freshness guard (HA review S1-3): before the config
    /// watcher replaces the pools from a freshly loaded (possibly stale)
    /// Secret snapshot, protect Antigravity refresh tokens that THIS process
    /// rotated recently (in-memory token newer than the truth source) from
    /// being clobbered by the older Secret value.
    ///
    /// Mutates `config_file`'s provider key `api_key` fields in place when the
    /// manager for that key holds a token refreshed within
    /// [`TOKEN_FRESHNESS_WINDOW`] that differs from the loaded value.
    /// Read the cross-replica rotation clock (Best-effort: the file backend
    /// has none, and a read failure degrades to the in-memory window below).
    /// Untrusted FUTURE timestamps (P11-sec S2-2) are treated as absent and
    /// warned about — a marker ahead of this node's wall clock is either clock
    /// skew or tampering, and must not keep a dead key alive forever.
    pub async fn trusted_rotated_at(&self) -> Option<u64> {
        let Some(marker) = (match &self.config_store {
            Some(store) => store.load_rotated_at().await.ok().flatten(),
            None => None,
        }) else {
            return None;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if !rotated_at_is_trusted(marker, now) {
            tracing::warn!(
                marker,
                now,
                "rotated_at marker is in the future; treating as untrusted (clock skew or tampering)"
            );
            return None;
        }
        Some(marker)
    }

    pub async fn apply_token_freshness_guard(&self, config_file: &mut ConfigFile) {
        // Cross-replica rotation clock from the truth source.
        let secret_rotated_at: Option<u64> = self.trusted_rotated_at().await;
        // Snapshot the managers' current refresh tokens (read-only, short-lived).
        let current: HashMap<(String, String), String> = {
            let pools = self.pools.read();
            let mut map = HashMap::new();
            for (prov_name, pool) in pools.iter() {
                for entry in pool.snapshot_keys() {
                    if let Some(mgr) = entry.antigravity_manager() {
                        map.insert(
                            (prov_name.clone(), entry.id.clone()),
                            mgr.credential_snapshot().refresh_token,
                        );
                    }
                }
            }
            map
        };
        let recent: HashMap<String, std::time::Instant> = {
            let map = self.last_antigravity_refresh.lock().await;
            map.clone()
        };

        for (prov_name, p_sec) in config_file.providers.iter_mut() {
            for k in p_sec.keys.iter_mut() {
                let Some(in_memory_token) =
                    current.get(&(prov_name.clone(), k.id.clone())).cloned()
                else {
                    continue; // no live manager for this key
                };
                let Some(refresh_time) = recent.get(&k.id) else {
                    continue; // not refreshed in this process
                };
                // In-memory freshness window (fallback when the store has no
                // rotated_at clock).
                let within_window = refresh_time.elapsed() <= TOKEN_FRESHNESS_WINDOW;
                // Authoritative cross-replica check: this process refreshed
                // AFTER the Secret's last rotation marker.
                let refresh_epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(refresh_time.elapsed().as_secs());
                let newer_than_secret = secret_rotated_at
                    .map(|s| refresh_epoch > s)
                    .unwrap_or(false);
                if !within_window && !newer_than_secret {
                    continue; // refresh is old and predates the Secret clock
                }
                if in_memory_token.is_empty() || in_memory_token == k.api_key {
                    continue; // no divergence to protect
                }
                tracing::warn!(
                    provider = %prov_name,
                    key_id = %k.id,
                    "rebuild freshness guard: using in-memory refresh token (newer than Secret snapshot)"
                );
                k.api_key = in_memory_token;
            }
        }
    }

    /// Attach the multi-node HA wiring to one Antigravity manager:
    /// 1. the cross-replica refresh serialization gate (if configured);
    /// 2. the persist-within-lock hook — a *system* write, intentionally NOT
    ///    gated by `admin_write_enabled` (gating it would let credentials rot
    ///    in read-only deployments). It only rewrites the single rotated key
    ///    entry (never adds/removes providers or keys), serializes on the same
    ///    `admin_write_lock` as the admin CUD path, and retries bounded times
    ///    on optimistic-concurrency conflicts.
    ///
    /// The legacy detached rotation-hook persistence is suppressed (no-op) —
    /// the persist hook now owns write-back so it completes *inside* the
    /// refresh gate's critical section (see S1-3 of the HA review).
    pub fn attach_antigravity_rotation_hook(
        &self,
        provider_name: &str,
        mgr: &Arc<AntigravityTokenManager>,
    ) {
        // 1. Serialization gate (cross-replica).
        if let Some(gate) = self.refresh_gate.read().clone() {
            mgr.set_refresh_gate(Some(gate));
        }
        // 2. Persist-within-lock write-back.
        if let Some(ref store) = self.config_store {
            let store_clone = store.clone();
            let prov_name = provider_name.to_string();
            let write_lock = self.admin_write_lock.clone();
            let metrics = self.metrics.clone();
            let last_map = self.last_antigravity_refresh.clone();
            let shutdown = self.shutdown_rx.clone();
            let mgr_clone = mgr.clone();
            // Throttle rotated_at Secret patches (key_id → last patch time).
            let throttled_patch: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>> =
                Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            // Suppress the legacy detached rotation-hook persistence.
            mgr.set_rotation_hook(Arc::new(|_, _| {}));
            let persist: RefreshPersistHook = Arc::new(move |key_id: String| {
                let store = store_clone.clone();
                let prov = prov_name.clone();
                let write_lock = write_lock.clone();
                let metrics = metrics.clone();
                let last_map = last_map.clone();
                let shutdown = shutdown.clone();
                let mgr = mgr_clone.clone();
                let throttled_patch = throttled_patch.clone();
                Box::pin(async move {
                    if *shutdown.borrow() {
                        return Ok(()); // draining: no writes during drain
                    }
                    let token = mgr.credential_snapshot().refresh_token.clone();
                    if token.trim().is_empty() {
                        return Ok(());
                    }
                    // Freshness is recorded on ANY successful upstream refresh,
                    // regardless of whether the write-back persists — the
                    // in-memory token is newer than the Secret snapshot even
                    // when the write failed (sec P1 S2-2 scenario B), so the
                    // rebuild freshness guard must still protect it.
                    let mut freshness = last_map.lock().await;
                    freshness.insert(key_id.clone(), std::time::Instant::now());
                    drop(freshness);
                    let attempts: u32 = 3;
                    for attempt in 0..attempts {
                        let _guard = write_lock.lock().await;
                        match store.load().await {
                            Ok((mut cfg, version)) => {
                                let Some(k) = cfg
                                    .providers
                                    .get_mut(&prov)
                                    .and_then(|p| p.keys.iter_mut().find(|k| k.id == key_id))
                                else {
                                    return Ok(()); // key vanished: nothing to persist
                                };
                                if k.api_key == token {
                                    // Nothing changed (non-rotated refresh):
                                    // advance the cross-replica rotation clock
                                    // (throttled) so peers see "recently
                                    // refreshed" without config churn.
                                    Self::advance_rotated_at(
                                        store.clone(),
                                        &key_id,
                                        throttled_patch.clone(),
                                    )
                                    .await;
                                    return Ok(());
                                }
                                k.api_key = token.clone();
                                cfg.config_version += 1;
                                match store.save(&cfg, &version).await {
                                    Ok(()) => {
                                        Self::advance_rotated_at(
                                            store.clone(),
                                            &key_id,
                                            throttled_patch.clone(),
                                        )
                                        .await;
                                        tracing::info!(
                                            provider = %prov,
                                            key_id = %key_id,
                                            "persisted rotated Antigravity refresh token (within lock)"
                                        );
                                        return Ok(());
                                    }
                                    Err(ConfigStoreError::Conflict { .. })
                                        if attempt + 1 < attempts =>
                                    {
                                        // Re-load next attempt for a fresh version.
                                        tokio::time::sleep(std::time::Duration::from_millis(
                                            100 * (1 << attempt),
                                        ))
                                        .await;
                                        continue;
                                    }
                                    Err(e) => {
                                        metrics.record_refresh_persist_failure();
                                        return Err(format!("token write-back failed: {}", e));
                                    }
                                }
                            }
                            Err(e) => {
                                metrics.record_refresh_persist_failure();
                                return Err(format!("config load failed for write-back: {}", e));
                            }
                        }
                    }
                    metrics.record_refresh_persist_failure();
                    Err(format!(
                        "token write-back failed after {} attempts",
                        attempts
                    ))
                })
            });
            mgr.set_persist_hook(Some(persist));
        }
    }

    /// Advance the cross-replica Antigravity rotation clock (`rotated_at` Secret
    /// data key), throttled to at most one patch per key per 10 minutes so a
    /// routine refresh does not churn the Secret's resourceVersion every round.
    /// Best-effort: a failed patch only warns — the in-memory freshness map and
    /// the local `rotated_at` read still cover the common paths.
    async fn advance_rotated_at(
        store: std::sync::Arc<dyn ConfigStore>,
        key_id: &str,
        throttled: std::sync::Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    ) {
        const ROTATED_AT_THROTTLE: std::time::Duration = std::time::Duration::from_secs(600);
        {
            let map = throttled.lock().await;
            if let Some(last) = map.get(key_id) {
                if last.elapsed() < ROTATED_AT_THROTTLE {
                    return; // recently patched
                }
            }
        }
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        match store.patch_rotated_at(now_epoch).await {
            Ok(()) => {
                throttled
                    .lock()
                    .await
                    .insert(key_id.to_string(), std::time::Instant::now());
            }
            Err(e) => {
                // Conflict is expected when another writer bumped the Secret; the
                // next refresh round retries the patch. Log, never fail the
                // refresh (the token itself was already persisted).
                tracing::warn!(key_id, error = %e, "rotated_at patch failed (non-fatal)");
            }
        }
    }
    /// Automatically scan all registered pools and attach rotation hooks for any
    /// AntigravityTokenManagers.
    pub fn attach_antigravity_rotation_hooks_all(&self) {
        let pools = self.pools.read();
        for (prov_name, pool) in pools.iter() {
            for key_entry in pool.snapshot_keys() {
                if let Some(mgr) = key_entry.antigravity_manager() {
                    self.attach_antigravity_rotation_hook(prov_name, &mgr);
                }
            }
        }
    }

    /// Background worker to periodically refresh Antigravity tokens and quota snapshots.
    ///
    /// Prevents standby / low-priority keys from expiring under Google's 180-day
    /// inactivity rule, while keeping quota buckets warm without requiring manual
    /// console button clicks.
    pub fn spawn_antigravity_auto_refresh_worker(self: &Arc<Self>) {
        let state = self.clone();
        tokio::spawn(async move {
            // Initial delay after startup (30 seconds) to let bootstrap & routes settle.
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            let mut last_run = std::time::Instant::now();
            // Immediate initial pass on startup cycle
            {
                let enabled = state.config.read().antigravity_auto_refresh;
                if enabled {
                    tracing::info!(
                        "Starting initial Antigravity quota & token refresh keepalive cycle"
                    );
                    state.perform_antigravity_keepalive_cycle().await;
                    last_run = std::time::Instant::now();
                }
            }

            loop {
                // Short sleep tick (5 seconds) so runtime interval updates in hot-reload
                // are detected promptly without waiting for the full 24h sleep.
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

                let (enabled, interval_secs) = {
                    let cfg = state.config.read();
                    (
                        cfg.antigravity_auto_refresh,
                        cfg.antigravity_refresh_interval_secs.max(60),
                    )
                };

                if enabled && last_run.elapsed() >= std::time::Duration::from_secs(interval_secs) {
                    tracing::info!(
                        "Starting scheduled Antigravity quota & token refresh keepalive cycle"
                    );
                    state.perform_antigravity_keepalive_cycle().await;
                    last_run = std::time::Instant::now();
                }
            }
        });
    }

    /// Execute a single pass of token refresh + quota query for all Antigravity keys across pools.
    pub async fn perform_antigravity_keepalive_cycle(&self) {
        if self.is_draining() {
            return; // graceful shutdown: no refresh work during drain
        }
        // Collect Antigravity key managers without holding locks across async operations.
        let key_entries: Vec<(
            String,
            String,
            Arc<ponyllm_core::pool::AntigravityTokenManager>,
            Option<String>,
        )> = {
            let pools = self.pools.read();
            let cfg = self.config.read();
            let mut list = Vec::new();
            for (provider, pool) in pools.iter() {
                let base_url = cfg.providers.get(provider).map(|p| p.base_url.clone());
                for entry in pool.snapshot_keys() {
                    if let Some(mgr) = entry.antigravity_manager() {
                        list.push((provider.clone(), entry.id.clone(), mgr, base_url.clone()));
                    }
                }
            }
            list
        };

        if key_entries.is_empty() {
            return;
        }

        tracing::info!(
            count = key_entries.len(),
            "Running Antigravity keepalive cycle for accounts"
        );

        for (provider, key_id, mgr, base_url) in key_entries {
            if self.is_draining() {
                return;
            }
            // Stagger calls by 1.5s to avoid burst hammering upstream OAuth/API endpoints.
            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

            // 1. Force refresh token: resets upstream Google 180-day inactivity
            // window. The refresh serialization gate inside the manager decides
            // whether THIS replica runs the OAuth call this round.
            match mgr.force_refresh_token().await {
                Ok(_) => {
                    {
                        let mut map = self.antigravity_invalid_grant_count.lock().await;
                        map.remove(&key_id);
                    }
                    tracing::debug!(
                        provider = %provider,
                        key_id = %key_id,
                        "Antigravity OAuth token refreshed successfully in keepalive cycle"
                    );
                }
                Err(ponyllm_core::error::CoreError::RefreshSkipped { .. }) => {
                    // Another replica holds the serialization lock: skip this
                    // round (quota included) and let its write-back propagate.
                    tracing::info!(
                        provider = %provider,
                        key_id = %key_id,
                        "Antigravity refresh skipped in keepalive cycle (lock held by another replica); skipping quota this round"
                    );
                    continue;
                }
                Err(ponyllm_core::error::CoreError::AuthInvalid { ref reason, .. }) => {
                    // invalid_grant reconciliation buffer (HA review S2-2/S3-3):
                    // if this process refreshed the key recently, OR the Secret
                    // rotated_at clock shows another replica refreshed it
                    // recently, the rejection may be a stale-propagation
                    // artifact from the lock holder's rotation. Quarantine only
                    // after INVALID_GRANT_QUARANTINE_N consecutive hits with no
                    // intervening successful refresh and no recent rotation.
                    let recently_persisted = {
                        let map = self.last_antigravity_refresh.lock().await;
                        map.get(&key_id)
                            .map(|t| t.elapsed() < TOKEN_FRESHNESS_WINDOW)
                            .unwrap_or(false)
                    };
                    let secret_recently_rotated = {
                        let now_epoch = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        self.trusted_rotated_at()
                            .await
                            .map(|r| now_epoch.saturating_sub(r) < 300)
                            .unwrap_or(false)
                    };
                    if recently_persisted || secret_recently_rotated {
                        tracing::warn!(
                            provider = %provider,
                            key_id = %key_id,
                            reason = %reason,
                            "invalid_grant while a recent refresh/rotation exists — deferring quarantine (propagation window)"
                        );
                        continue;
                    }
                    let hits = {
                        let mut map = self.antigravity_invalid_grant_count.lock().await;
                        let n = map.entry(key_id.clone()).or_insert(0);
                        *n += 1;
                        *n
                    };
                    if hits < INVALID_GRANT_QUARANTINE_N {
                        tracing::warn!(
                            provider = %provider,
                            key_id = %key_id,
                            hits,
                            reason = %reason,
                            "invalid_grant seen but below quarantine threshold; keeping key alive"
                        );
                        continue;
                    }
                    tracing::warn!(
                        provider = %provider,
                        key_id = %key_id,
                        reason = %reason,
                        "Antigravity key permanently rejected (invalid_grant, {} consecutive) during keepalive cycle",
                        hits
                    );
                    if let Some(pool) = self.pools.read().get(&provider) {
                        pool.record_error(
                            &key_id,
                            ponyllm_core::pool::PoolErrorType::AuthInvalid {
                                reason: Some(reason.clone()),
                            },
                        );
                    }
                    continue;
                }
                Err(e) => {
                    // Transient (network/5xx) OR gate-unavailable: skip the
                    // whole round (quota included) — fail soft, never burn.
                    tracing::warn!(
                        provider = %provider,
                        key_id = %key_id,
                        error = %e,
                        "Transient error refreshing Antigravity token during keepalive cycle; skipping this round"
                    );
                    continue;
                }
            }

            // 2. Fetch latest quota snapshot to warm quota buckets
            let probe_client = self.probe_http_client_for_provider(&provider);
            match mgr
                .with_client(&probe_client)
                .fetch_quota(base_url.as_deref())
                .await
            {
                Ok(snapshot) => {
                    tracing::debug!(
                        provider = %provider,
                        key_id = %key_id,
                        models_count = snapshot.models.len(),
                        "Antigravity quota snapshot refreshed successfully in keepalive cycle"
                    );

                    // Autonomous capacity calibration in background keepalive
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let current_fraction = snapshot
                        .quota_groups
                        .as_ref()
                        .and_then(|groups| {
                            for g in groups {
                                for b in &g.buckets {
                                    if b.window.eq_ignore_ascii_case("5h")
                                        || b.bucket_id.contains("5h")
                                    {
                                        return Some(b.remaining_fraction);
                                    }
                                }
                            }
                            None
                        })
                        .or_else(|| {
                            snapshot
                                .models
                                .values()
                                .next()
                                .map(|m| m.remaining_fraction)
                        });

                    let weekly_fraction = snapshot.quota_groups.as_ref().and_then(|groups| {
                        for g in groups {
                            for b in &g.buckets {
                                let win = b.window.to_lowercase();
                                let b_id = b.bucket_id.to_lowercase();
                                let b_desc = b.description.as_deref().unwrap_or("").to_lowercase();
                                let b_disp = b.display_name.as_deref().unwrap_or("").to_lowercase();
                                if win == "weekly"
                                    || b_id.contains("week")
                                    || b_desc.contains("week")
                                    || b_disp.contains("周")
                                    || b_id.contains("7d")
                                {
                                    return Some(b.remaining_fraction);
                                }
                            }
                        }
                        None
                    });

                    if let Some(frac) = current_fraction {
                        if let Some(pool) = self.pools.read().get(&provider) {
                            if let Some(entry) =
                                pool.snapshot_keys().into_iter().find(|k| k.id == key_id)
                            {
                                entry.usage_tracker.observe_upstream_probe_dual(
                                    now_ms,
                                    Some(frac),
                                    weekly_fraction,
                                );
                            }
                        }
                    }

                    // Probe-sourced quota groups are read-only metadata
                    // (ADR `2026-10-04-antigravity-group-quota-aware-scheduling`):
                    // they refresh the entry's decayed verdicts for the
                    // admin view/unlock hints, but they NO LONGER pre-write
                    // a family-exhausted verdict that would block selection.
                    // Real-429 writeback is the only ledger producer now.
                    if let Some(pool) = self.pools.read().get(&provider) {
                        if let Some(entry) =
                            pool.snapshot_keys().into_iter().find(|k| k.id == key_id)
                        {
                            entry.apply_quota_groups(
                                snapshot.quota_groups.as_deref(),
                                chrono::Utc::now(),
                            );
                        }
                    }
                }
                Err(e) => {
                    // Probe-facing 403s get the SAME *deterministic hard*
                    // pool action as the request path: `AccountEligibility`
                    // (long freeze for "not eligible"), `AccountValidationRequired`
                    // / `PolicyViolation` (isolate) — see `classify_probe_failure`.
                    // Soft signals (quota wording / unknown-403 / 429 / 5xx /
                    // network timeout) deliberately return None and stay
                    // fail-soft: a keepalive probe must never cool a healthy
                    // key over a WAF/HTML/scope 403 or transport jitter
                    // (the observed 2026-10-04 `quota_probe_failed` was
                    // exactly such a misread).
                    if let ponyllm_core::error::CoreError::UpstreamStatusError { status, body } = &e
                    {
                        if let Some(pool_err) =
                            ponyllm_core::executor::classify_probe_failure(status.as_u16(), body)
                        {
                            if let Some(pool) = self.pools.read().get(&provider) {
                                let preview: String =
                                    format!("{pool_err:?}").chars().take(300).collect();
                                tracing::warn!(
                                    provider = %provider,
                                    key_id = %key_id,
                                    status = %status,
                                    error = %preview,
                                    "keepalive quota probe hit an upstream 403; applying the same pool action as the request path"
                                );
                                pool.record_error(&key_id, pool_err);
                            }
                        }
                    }
                    tracing::debug!(
                        provider = %provider,
                        key_id = %key_id,
                        error = %e,
                        "Antigravity quota fetch encountered non-fatal error during keepalive cycle"
                    );
                }
            }
        }
    }

    pub fn register_pool(&self, provider: &str, pool: Arc<KeyPool>) {
        {
            let mut pending = self.pending_restored_usages.write();
            if !pending.is_empty() {
                for key in pool.snapshot_keys() {
                    if let Some(saved) = pending.remove(&key.id) {
                        key.usage_tracker.import_snapshot(saved);
                        tracing::info!(provider = %provider, key_id = %key.id, "Restored usage tracker state from snapshot");
                    }
                }
            }
        }
        self.pools.write().insert(provider.to_string(), pool);
    }

    pub fn get_pool(&self, provider: &str) -> Option<Arc<KeyPool>> {
        self.pools.read().get(provider).cloned()
    }

    pub fn get_or_create_node_metrics(&self, provider: &str) -> Arc<NodeLatencyMetrics> {
        self.stream_proj.node_for(provider)
    }

    /// Build the per-request event sink wired to the bus. Replaces the legacy
    /// attempt observer: every per-key retry AND every cross-provider fallback
    /// attempt lands in the log with its own status code, key id and upstream
    /// error body; metrics and frames derive from the same events.
    pub fn event_sink(&self, sink_ctx: EventSinkCtx) -> EventSink {
        let bus = self.event_bus.clone();
        let ctx = EventCtx {
            request_id: sink_ctx.request_id.clone(),
            session_id: None,
            model: sink_ctx.model.clone(),
            endpoint: sink_ctx.endpoint.clone(),
            start: sink_ctx.start,
        };
        let provider = sink_ctx.provider.clone();
        Arc::new(move |event: GatewayEvent| {
            bus.append(&ctx, Some(provider.clone()), event);
        })
    }

    /// Emit one event on the bus with an explicit provider.
    pub fn emit(&self, ctx: &EventCtx, provider: Option<String>, event: GatewayEvent) -> u64 {
        self.event_bus.append(ctx, provider, event)
    }

    /// Resolve ordered list of candidate targets for multi-provider transparent failover
    pub fn resolve_routed_targets(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
    ) -> Result<Vec<RoutedTarget>> {
        self.resolve_routed_targets_with_prompt(parsed, header_strategy, None)
    }

    /// Resolve ordered list of candidate targets for a model request, with optional prompt for hot cache probing
    pub fn resolve_routed_targets_with_prompt(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
        prompt: Option<&str>,
    ) -> Result<Vec<RoutedTarget>> {
        self.resolve_routed_targets_with_prompt_and_protocol(
            parsed,
            header_strategy,
            prompt,
            None,
            None,
        )
    }

    /// Same as above with an explicit per-request protocol override
    /// (`x-pony-protocol` header; invalid values are ignored by the caller).
    /// `inbound` is the entry protocol; same-native candidates win ties so
    /// passthrough is preferred over translation without overriding strategy.
    pub fn resolve_routed_targets_with_prompt_and_protocol(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
        prompt: Option<&str>,
        proto_override: Option<UpstreamProtocol>,
        inbound: Option<UpstreamProtocol>,
    ) -> Result<Vec<RoutedTarget>> {
        self.resolve_routed_targets_full(
            parsed,
            header_strategy,
            prompt,
            proto_override,
            inbound,
            &[],
        )
    }

    /// Full routed targets resolution with modality requirements filtering
    pub fn resolve_routed_targets_full(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
        prompt: Option<&str>,
        proto_override: Option<UpstreamProtocol>,
        inbound: Option<UpstreamProtocol>,
        required_modalities: &[&str],
    ) -> Result<Vec<RoutedTarget>> {
        let config = self.config.read();
        let strategy = parsed
            .strategy_override
            .or(header_strategy)
            .unwrap_or(config.default_strategy);

        let cached_provider = prompt.and_then(|p| self.hot_cache.probe_cached_provider(p));

        if parsed.is_auto {
            self.resolve_auto_targets(
                parsed,
                strategy,
                &config,
                cached_provider.as_deref(),
                proto_override,
                inbound,
                required_modalities,
            )
        } else {
            self.resolve_pinned_targets(
                parsed,
                strategy,
                &config,
                cached_provider.as_deref(),
                proto_override,
                inbound,
            )
        }
    }

    /// Resolve single best target
    pub fn resolve_routed_target(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
    ) -> Result<RoutedTarget> {
        let mut targets = self.resolve_routed_targets(parsed, header_strategy)?;
        if targets.is_empty() {
            return Err(CoreError::Internal(
                "No routing candidates available".to_string(),
            ));
        }
        Ok(targets.remove(0))
    }

    fn resolve_auto_targets(
        &self,
        parsed: &ParsedRequestModel,
        strategy: GatewayRoutingStrategy,
        config: &GatewayConfig,
        cached_provider: Option<&str>,
        proto_override: Option<UpstreamProtocol>,
        inbound: Option<UpstreamProtocol>,
        required_modalities: &[&str],
    ) -> Result<Vec<RoutedTarget>> {
        let filter_compat = |c: &RoutedTarget| {
            if parsed.is_1m_context && !is_context_capacity_compatible("1M", &c.context_window) {
                return false;
            }
            if !required_modalities.is_empty() && !c.supports_modalities(required_modalities) {
                return false;
            }
            true
        };

        // Collect all primary (Flagship & Standard) candidates first to maximize agent quality
        let mut candidates: Vec<RoutedTarget> = Vec::new();
        candidates.extend(
            self.collect_tier_candidates(
                ModelTier::Flagship,
                strategy,
                config,
                proto_override,
                inbound,
            )
            .into_iter()
            .filter(filter_compat),
        );
        candidates.extend(
            self.collect_tier_candidates(
                ModelTier::Standard,
                strategy,
                config,
                proto_override,
                inbound,
            )
            .into_iter()
            .filter(filter_compat),
        );

        // If no primary candidates, fall back to Light
        if candidates.is_empty() {
            candidates.extend(
                self.collect_tier_candidates(
                    ModelTier::Light,
                    strategy,
                    config,
                    proto_override,
                    inbound,
                )
                .into_iter()
                .filter(filter_compat),
            );
        }

        if candidates.is_empty() {
            // Report to PonySentry if auto candidate pool is completely exhausted
            let mut tags = HashMap::new();
            tags.insert("event_type".to_string(), "auto_pool_exhausted".to_string());
            self.sentry.capture_error(
                "AutoPoolExhausted",
                "No candidate models available in gateway for pure auto routing",
                Some(tags),
                None,
            );

            if !required_modalities.is_empty() {
                return Err(CoreError::UnsupportedModality {
                    required_modality: required_modalities.join(", "),
                    message: format!(
                        "No model candidate supports required modalities {:?}",
                        required_modalities
                    ),
                });
            } else if parsed.is_1m_context {
                return Err(CoreError::CapacityExhausted {
                    required_context: "1M".to_string(),
                    message: "No model candidate meets 1M context requirement".to_string(),
                });
            } else {
                return Err(CoreError::Internal(
                    "No candidate models available in gateway for auto routing".to_string(),
                ));
            }
        }

        Ok(self.sort_auto_candidates(
            candidates,
            strategy,
            config,
            cached_provider,
            inbound,
        ))
    }

    fn resolve_pinned_targets(
        &self,
        parsed: &ParsedRequestModel,
        strategy: GatewayRoutingStrategy,
        config: &GatewayConfig,
        cached_provider: Option<&str>,
        proto_override: Option<UpstreamProtocol>,
        inbound: Option<UpstreamProtocol>,
    ) -> Result<Vec<RoutedTarget>> {
        let clean = &parsed.clean_model_name;
        let canonical = canonicalize_model_name(clean);
        let effective = canonical.as_str();
        let mut candidates = Vec::new();

        // 1. Match exact model name across all providers (alias never shadows config)
        for (p_name, p_cfg) in &config.providers {
            if p_cfg.default_model == *clean || p_cfg.models.iter().any(|m| m == clean) {
                let spec = p_cfg.get_model_spec(clean);
                let thinking_spec = spec.thinking_spec();
                let pricing = p_cfg.get_model_pricing(clean);
                let billing_mode = p_cfg.get_model_billing_mode(clean);
                let (protocol, endpoint_base) =
                    resolve_effective_protocol(p_name, p_cfg, clean, proto_override, inbound);
                candidates.push(RoutedTarget {
                    provider_name: p_name.clone(),
                    base_url: spec
                        .base_url
                        .clone()
                        .unwrap_or_else(|| p_cfg.base_url.clone()),
                    physical_model: clean.clone(),
                    tier: spec.tier,
                    priority: spec.priority,
                    strategy,
                    upstream_protocol: protocol,
                    endpoint_base,
                    context_window: spec.context_window,
                    billing_mode,
                    pricing,
                    thinking_spec,
                    temperature: spec.temperature,
                    top_p: spec.top_p,
                    input_types: spec.input_types,
                    max_output: spec.max_output,
                    output_types: spec.output_types.clone(),
                });
            }
        }

        // 1b. Canonical alias fallback: retired/renamed request names map to the
        // live upstream name (e.g. deepseek-v4.1-flash -> deepseek-flash).
        if candidates.is_empty() && effective != clean.as_str() {
            for (p_name, p_cfg) in &config.providers {
                if p_cfg.default_model == effective || p_cfg.models.iter().any(|m| m == effective) {
                    let spec = p_cfg.get_model_spec(effective);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(effective);
                    let billing_mode = p_cfg.get_model_billing_mode(effective);
                    let (protocol, endpoint_base) = resolve_effective_protocol(
                        p_name,
                        p_cfg,
                        effective,
                        proto_override,
                        inbound,
                    );
                    candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec
                            .base_url
                            .clone()
                            .unwrap_or_else(|| p_cfg.base_url.clone()),
                        physical_model: effective.to_string(),
                        tier: spec.tier,
                        priority: spec.priority,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window,
                        billing_mode,
                        pricing,
                        thinking_spec,
                        temperature: spec.temperature,
                        top_p: spec.top_p,
                        input_types: spec.input_types,
                        max_output: spec.max_output,
                        output_types: spec.output_types.clone(),
                    });
                }
            }
        }

        // 2. Prefix matching (e.g. "deepseek/deepseek-chat")
        if candidates.is_empty() {
            if let Some((prefix, sub_model)) = clean.split_once('/') {
                let sub_canonical = canonicalize_model_name(sub_model);
                let sub_effective = sub_canonical.as_str();
                if let Some(p_cfg) = config.providers.get(prefix) {
                    let spec = p_cfg.get_model_spec(sub_effective);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(sub_effective);
                    let billing_mode = p_cfg.get_model_billing_mode(sub_effective);
                    let (protocol, endpoint_base) = resolve_effective_protocol(
                        prefix,
                        p_cfg,
                        sub_effective,
                        proto_override,
                        inbound,
                    );
                    candidates.push(RoutedTarget {
                        provider_name: prefix.to_string(),
                        base_url: spec
                            .base_url
                            .clone()
                            .unwrap_or_else(|| p_cfg.base_url.clone()),
                        physical_model: sub_effective.to_string(),
                        tier: spec.tier,
                        priority: spec.priority,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window,
                        billing_mode,
                        pricing,
                        thinking_spec,
                        temperature: spec.temperature,
                        top_p: spec.top_p,
                        input_types: spec.input_types,
                        max_output: spec.max_output,
                        output_types: spec.output_types.clone(),
                    });
                }
            }
        }

        // 3. Keyword heuristic matching
        if candidates.is_empty() {
            let lower = effective.to_lowercase();
            for (p_name, p_cfg) in &config.providers {
                if lower.contains(p_name)
                    || (p_name == "openai"
                        && (lower.starts_with("gpt")
                            || lower.starts_with("o1")
                            || lower.starts_with("o3")))
                    || (p_name == "anthropic" && lower.starts_with("claude"))
                    || (p_name == "deepseek" && lower.starts_with("deepseek"))
                {
                    let spec = p_cfg.get_model_spec(effective);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(effective);
                    let billing_mode = p_cfg.get_model_billing_mode(effective);
                    let (protocol, endpoint_base) = resolve_effective_protocol(
                        p_name,
                        p_cfg,
                        effective,
                        proto_override,
                        inbound,
                    );
                    candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec
                            .base_url
                            .clone()
                            .unwrap_or_else(|| p_cfg.base_url.clone()),
                        physical_model: effective.to_string(),
                        tier: spec.tier,
                        priority: spec.priority,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window,
                        billing_mode,
                        pricing,
                        thinking_spec,
                        temperature: spec.temperature,
                        top_p: spec.top_p,
                        input_types: spec.input_types,
                        max_output: spec.max_output,
                        output_types: spec.output_types.clone(),
                    });
                }
            }
        }

        if candidates.is_empty() {
            return Err(CoreError::Internal(format!(
                "No provider configured to handle model '{}'",
                clean
            )));
        }

        // 4b. Configured Model Fallbacks (Secondary Candidates for DR)
        // If the primary model defines explicit fallbacks, resolve them in order and
        // append as secondary candidates so if all primary provider targets fail or converge early
        // (e.g. deterministic empty STOP), the router can fail over to the fallback model.
        // We use a visited set and a depth limit of 3 to prevent cyclic/duplicate references.
        let mut visited_models: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        visited_models.insert(clean.clone());
        visited_models.insert(effective.to_string());

        let mut queue: std::collections::VecDeque<(String, usize)> =
            std::collections::VecDeque::new();
        for target in &candidates {
            if let Some(p_cfg) = config.providers.get(&target.provider_name) {
                let spec = p_cfg.get_model_spec(&target.physical_model);
                for fb in &spec.fallbacks {
                    if visited_models.insert(fb.clone()) {
                        queue.push_back((fb.clone(), 1));
                    }
                }
                // Automatic intra-family fallback for Gemini 3:
                // If gemini-3.*-flash-high or gemini-3.*-flash-tiered experiences empty STOP or upstream choke,
                // automatically fallback to gemini-3.*-flash-medium within the same provider if supported.
                if target.physical_model.contains("gemini-3")
                    && (target.physical_model.ends_with("-high")
                        || target.physical_model.ends_with("-tiered"))
                {
                    let base = target
                        .physical_model
                        .strip_suffix("-high")
                        .or_else(|| target.physical_model.strip_suffix("-tiered"))
                        .unwrap_or(&target.physical_model);
                    let med_model = format!("{}-medium", base);
                    if p_cfg.models.iter().any(|m| m == &med_model)
                        && visited_models.insert(med_model.clone())
                    {
                        queue.push_back((med_model, 1));
                    }
                }
            }
        }

        let mut sorted_primary =
            self.sort_candidates(candidates, strategy, config, cached_provider, inbound);

        let mut secondary_candidates = Vec::new();
        while let Some((fb_model, depth)) = queue.pop_front() {
            for (p_name, p_cfg) in &config.providers {
                if p_cfg.default_model == fb_model || p_cfg.models.iter().any(|m| m == &fb_model) {
                    let spec = p_cfg.get_model_spec(&fb_model);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(&fb_model);
                    let billing_mode = p_cfg.get_model_billing_mode(&fb_model);
                    let (protocol, endpoint_base) = resolve_effective_protocol(
                        p_name,
                        p_cfg,
                        &fb_model,
                        proto_override,
                        inbound,
                    );
                    secondary_candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec
                            .base_url
                            .clone()
                            .unwrap_or_else(|| p_cfg.base_url.clone()),
                        physical_model: fb_model.clone(),
                        tier: spec.tier,
                        priority: spec.priority,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window.clone(),
                        billing_mode,
                        pricing,
                        thinking_spec,
                        temperature: spec.temperature,
                        top_p: spec.top_p,
                        input_types: spec.input_types.clone(),
                        max_output: spec.max_output.clone(),
                        output_types: spec.output_types.clone(),
                    });

                    if depth < 3 {
                        for next_fb in &spec.fallbacks {
                            if visited_models.insert(next_fb.clone()) {
                                queue.push_back((next_fb.clone(), depth + 1));
                            }
                        }
                    }
                }
            }
        }

        if !secondary_candidates.is_empty() {
            let sorted_secondary = self.sort_candidates(
                secondary_candidates,
                strategy,
                config,
                cached_provider,
                inbound,
            );
            sorted_primary.extend(sorted_secondary);
        }

        // 5. Context Capacity Monotonicity check
        if parsed.is_1m_context {
            let before_len = sorted_primary.len();
            sorted_primary.retain(|c| is_context_capacity_compatible("1M", &c.context_window));
            if sorted_primary.is_empty() && before_len > 0 {
                return Err(CoreError::CapacityExhausted {
                    required_context: "1M".to_string(),
                    message: format!("Model '{}' does not support 1M context requirement", clean),
                });
            }
        }

        Ok(sorted_primary)
    }

    fn collect_tier_candidates(
        &self,
        tier: ModelTier,
        strategy: GatewayRoutingStrategy,
        config: &GatewayConfig,
        proto_override: Option<UpstreamProtocol>,
        inbound: Option<UpstreamProtocol>,
    ) -> Vec<RoutedTarget> {
        let mut candidates = Vec::new();
        for (p_name, p_cfg) in &config.providers {
            let default_spec = p_cfg.get_model_spec(&p_cfg.default_model);
            let default_pricing = p_cfg.get_model_pricing(&p_cfg.default_model);
            let default_billing = p_cfg.get_model_billing_mode(&p_cfg.default_model);
            if default_spec.tier == tier {
                let thinking_spec = default_spec.thinking_spec();
                let (protocol, endpoint_base) = resolve_effective_protocol(
                    p_name,
                    p_cfg,
                    &p_cfg.default_model,
                    proto_override,
                    inbound,
                );
                candidates.push(RoutedTarget {
                    provider_name: p_name.clone(),
                    base_url: default_spec
                        .base_url
                        .clone()
                        .unwrap_or_else(|| p_cfg.base_url.clone()),
                    physical_model: p_cfg.default_model.clone(),
                    tier,
                    priority: default_spec.priority,
                    strategy,
                    upstream_protocol: protocol,
                    endpoint_base,
                    context_window: default_spec.context_window,
                    billing_mode: default_billing,
                    pricing: default_pricing,
                    thinking_spec,
                    temperature: default_spec.temperature,
                    top_p: default_spec.top_p,
                    input_types: default_spec.input_types,
                    max_output: default_spec.max_output,
                    output_types: default_spec.output_types.clone(),
                });
            }
            for m in &p_cfg.models {
                if m != &p_cfg.default_model {
                    let spec = p_cfg.get_model_spec(m);
                    let thinking_spec = spec.thinking_spec();
                    let m_pricing = p_cfg.get_model_pricing(m);
                    let m_billing = p_cfg.get_model_billing_mode(m);
                    if spec.tier == tier {
                        let (protocol, endpoint_base) =
                            resolve_effective_protocol(p_name, p_cfg, m, proto_override, inbound);
                        candidates.push(RoutedTarget {
                            provider_name: p_name.clone(),
                            base_url: spec
                                .base_url
                                .clone()
                                .unwrap_or_else(|| p_cfg.base_url.clone()),
                            physical_model: m.clone(),
                            tier,
                            priority: spec.priority,
                            strategy,
                            upstream_protocol: protocol,
                            endpoint_base,
                            context_window: spec.context_window,
                            billing_mode: m_billing,
                            pricing: m_pricing,
                            thinking_spec,
                            temperature: spec.temperature,
                            top_p: spec.top_p,
                            input_types: spec.input_types,
                            max_output: spec.max_output,
                            output_types: spec.output_types.clone(),
                        });
                    }
                }
            }
        }
        candidates
    }

    fn sort_candidates(
        &self,
        candidates: Vec<RoutedTarget>,
        strategy: GatewayRoutingStrategy,
        config: &GatewayConfig,
        cached_provider: Option<&str>,
        inbound: Option<UpstreamProtocol>,
    ) -> Vec<RoutedTarget> {
        self.sort_candidates_internal(
            candidates,
            strategy,
            config,
            cached_provider,
            inbound,
            false,
        )
    }

    fn sort_auto_candidates(
        &self,
        candidates: Vec<RoutedTarget>,
        strategy: GatewayRoutingStrategy,
        config: &GatewayConfig,
        cached_provider: Option<&str>,
        inbound: Option<UpstreamProtocol>,
    ) -> Vec<RoutedTarget> {
        self.sort_candidates_internal(candidates, strategy, config, cached_provider, inbound, true)
    }

    fn sort_candidates_internal(
        &self,
        mut candidates: Vec<RoutedTarget>,
        strategy: GatewayRoutingStrategy,
        _config: &GatewayConfig,
        cached_provider: Option<&str>,
        inbound: Option<UpstreamProtocol>,
        is_auto: bool,
    ) -> Vec<RoutedTarget> {
        // Passthrough-first tiebreak: stable native-first order BEFORE the
        // strategy sort, so strategy stays primary and same-native wins ties.
        if let Some(inbound) = inbound {
            candidates.sort_by_key(|c| c.upstream_protocol != inbound);
        }
        // Decorate-Sort-Undecorate: snapshot every dynamic signal once per
        // candidate so comparators stay pure functions (strict weak ordering
        // holds even while pools and latency metrics mutate concurrently).
        let mut sorted: Vec<RoutedTarget> = match strategy {
            GatewayRoutingStrategy::Economy => {
                let mut keyed: Vec<(f64, RoutedTarget)> = candidates
                    .into_iter()
                    .map(|c| {
                        let cached = cached_provider
                            .map(|p| p == c.provider_name)
                            .unwrap_or(false);
                        let score = EconomyScorer::score_candidate(
                            &c.pricing,
                            c.billing_mode,
                            cached,
                            10_000,
                            1000,
                        );
                        (score, c)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                keyed.into_iter().map(|(_, c)| c).collect()
            }
            GatewayRoutingStrategy::Speed => {
                let mut keyed: Vec<(f64, RoutedTarget)> = candidates
                    .into_iter()
                    .map(|c| {
                        let metrics = self.get_or_create_node_metrics(&c.provider_name);
                        (SpeedScorer::estimate_total_latency_ms(&metrics, 512), c)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                keyed.into_iter().map(|(_, c)| c).collect()
            }
            GatewayRoutingStrategy::Balanced => {
                let mut keyed: Vec<(f64, RoutedTarget)> = candidates
                    .into_iter()
                    .map(|c| {
                        let cached = cached_provider
                            .map(|p| p == c.provider_name)
                            .unwrap_or(false);
                        let score = EconomyScorer::score_candidate(
                            &c.pricing,
                            c.billing_mode,
                            cached,
                            10_000,
                            1000,
                        );
                        let metrics = self.get_or_create_node_metrics(&c.provider_name);
                        let lat = SpeedScorer::estimate_total_latency_ms(&metrics, 512);
                        (score + (lat / 1000.0) * 0.1, c)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                keyed.into_iter().map(|(_, c)| c).collect()
            }
            GatewayRoutingStrategy::Reliable => {
                let mut keyed: Vec<(usize, RoutedTarget)> = candidates
                    .into_iter()
                    .map(|c| {
                        let active = self
                            .get_pool(&c.provider_name)
                            .map(|p| p.active_key_count())
                            .unwrap_or(0);
                        (active, c)
                    })
                    .collect();
                keyed.sort_by_key(|b| std::cmp::Reverse(b.0));
                keyed.into_iter().map(|(_, c)| c).collect()
            }
        };
        // Explicit priority is the PRIMARY routing key: a stable final sort
        // preserves everyone's strategy-scored order within equal priorities,
        // while a higher `priority` (larger number) always ranks first. This
        // deliberate precedence means an operator's explicit preference wins
        // over hot-cache stickiness and price/latency/reliability scores
        // (documented in the model-priority ADR); `None` = 0 keeps legacy
        // configurations byte-for-byte identical to the pre-priority ordering.
        if is_auto {
            let auto_models = _config.auto_models.clone();
            sorted.sort_by_key(|c| {
                // 1. Health & circuit breaker check:
                // If model is under model-breaker cooldown or all keys are dead, deprioritize.
                let is_circuit_broken = self.is_model_cooling_down(&c.provider_name, &c.physical_model);
                let has_active_keys = self
                    .get_pool(&c.provider_name)
                    .map(|p| p.active_key_count() > 0)
                    .unwrap_or(true);
                let is_unhealthy = is_circuit_broken || !has_active_keys;

                // 2. Billing mode: Free tier first (0 cost), Paid tier second
                let is_free = c.billing_mode == BillingMode::Free || c.pricing.is_free();
                let fee_rank = if is_free { 0 } else { 1 };

                // 3. Auto model preference list match
                // If model matches user/system auto_models list (exact or prefix), lower rank index wins.
                let model_rank = auto_models
                    .iter()
                    .position(|m| {
                        m.eq_ignore_ascii_case(&c.physical_model)
                            || c.physical_model.to_ascii_lowercase().starts_with(&m.to_ascii_lowercase())
                    })
                    .unwrap_or(auto_models.len() + 10);

                // 4. Model tier: Flagship (0) -> Standard (1) -> Light (2)
                let tier_rank = match c.tier {
                    ModelTier::Flagship => 0,
                    ModelTier::Standard => 1,
                    ModelTier::Light => 2,
                };

                (
                    is_unhealthy, // false (healthy) comes before true (unhealthy/cooling)
                    fee_rank,     // 0 (Free) comes before 1 (Paid)
                    model_rank,   // preferred configured auto_models rank
                    tier_rank,    // Flagship -> Standard -> Light
                    std::cmp::Reverse(c.priority.unwrap_or(0)),
                )
            });
        } else {
            sorted.sort_by_key(|c| std::cmp::Reverse(c.priority.unwrap_or(0)));
        }
        sorted
    }

    /// List all exposed models: virtual auto models and physical configured models.
    /// The fourth tuple element is the effective native protocol (`chat` by
    /// default; `auto` for virtual models whose protocol resolves per request).
    pub fn list_all_models(&self) -> Vec<(String, String, Option<String>, String)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. auto virtual model (Pure auto only)
        result.push((
            "auto".to_string(),
            "ponyllm".to_string(),
            Some("Auto(智能·高可用主力)".to_string()),
            "auto".to_string(),
        ));
        seen.insert("auto".to_string());

        // 2. Physical configured models and their [1m] aliases.
        // Iteration is provider-name sorted so the list content and the bare
        // name's `owned_by` are deterministic across restarts (the config map
        // is a HashMap; unsorted iteration would randomize which provider's
        // alias survives name collisions).
        let config = self.config.read();
        let mut provider_names: Vec<&String> = config.providers.keys().collect();
        provider_names.sort();
        let literal_names: std::collections::HashSet<&str> = config
            .providers
            .values()
            .flat_map(|cfg| {
                cfg.models
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(cfg.default_model.as_str()).filter(|d| !d.is_empty()))
            })
            .collect();
        let mut model_provider_count: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for cfg in config.providers.values() {
            for m in cfg
                .models
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(cfg.default_model.as_str()).filter(|d| !d.is_empty()))
            {
                let canonical_m = if m.starts_with("gemini-3.8-flash-") {
                    "gemini-3.8-flash"
                } else {
                    m
                };
                *model_provider_count.entry(canonical_m).or_insert(0) += 1;
            }
        }
        for provider_name in &provider_names {
            let cfg = &config.providers[*provider_name];
            let mut add_model_and_alias = |m: &str| {
                // If model is an internal gemini-3.8-flash variant suffix, normalize to base gemini-3.8-flash for client listing
                let canonical_m = if m.starts_with("gemini-3.8-flash-") {
                    "gemini-3.8-flash"
                } else {
                    m
                };

                let proto = cfg
                    .native_protocol(m)
                    .or_else(|| cfg.native_protocol(canonical_m))
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| {
                        infer_legacy_protocol(provider_name, &cfg.base_url).to_string()
                    });
                let spec = cfg.get_model_spec(m);

                // 1. Bare model entry (deduped across providers)
                if !seen.contains(canonical_m) {
                    result.push((
                        canonical_m.to_string(),
                        provider_name.to_string(),
                        None,
                        proto.clone(),
                    ));
                    seen.insert(canonical_m.to_string());
                }

                // 2. Format provider-qualified alias provider/model
                let shared = model_provider_count.get(canonical_m).copied().unwrap_or(0) >= 2;
                let prefixed = format!("{}/{}", provider_name, canonical_m);
                if shared && !seen.contains(&prefixed) && !literal_names.contains(prefixed.as_str())
                {
                    let alias_display = spec
                        .display_name
                        .clone()
                        .unwrap_or_else(|| format!("{} ({})", canonical_m, provider_name));
                    result.push((
                        prefixed.clone(),
                        provider_name.to_string(),
                        Some(alias_display),
                        proto.clone(),
                    ));
                    seen.insert(prefixed.clone());
                }

                if parse_context_capacity_tokens(&spec.context_window) >= 1048576 {
                    let alias_1m = format!("{}[1m]", canonical_m);
                    if !seen.contains(&alias_1m) {
                        result.push((
                            alias_1m.clone(),
                            provider_name.to_string(),
                            Some(format!("{} (1M 长上下文)", canonical_m)),
                            proto.clone(),
                        ));
                        seen.insert(alias_1m);
                    }
                    let prefixed_1m = format!("{}[1m]", prefixed);
                    if shared
                        && !seen.contains(&prefixed_1m)
                        && !literal_names.contains(prefixed.as_str())
                    {
                        result.push((
                            prefixed_1m.clone(),
                            provider_name.to_string(),
                            Some(format!("{}[1m] ({})", canonical_m, provider_name)),
                            proto.clone(),
                        ));
                        seen.insert(prefixed_1m);
                    }
                }
            };

            if !cfg.default_model.is_empty() {
                add_model_and_alias(&cfg.default_model);
                let canonical = cfg.default_model.clone();
                for alias in model_aliases(&canonical) {
                    add_model_and_alias(alias);
                }
            }
            for m in &cfg.models {
                if m != &cfg.default_model {
                    add_model_and_alias(m);
                    for alias in model_aliases(m) {
                        add_model_and_alias(alias);
                    }
                }
            }
        }
        result
    }

    /// Legacy compatibility helper
    pub fn resolve_provider(&self, model: &str) -> Option<(String, ProviderConfig)> {
        let parsed = ParsedRequestModel::parse(model);
        if let Ok(target) = self.resolve_routed_target(&parsed, None) {
            let config = self.config.read();
            if let Some(cfg) = config.providers.get(&target.provider_name) {
                return Some((target.provider_name, cfg.clone()));
            }
        }
        None
    }
}

pub use ponyllm_core::{
    normalize_chat_completions_url, normalize_messages_url, normalize_responses_url,
    normalize_systemone_url,
};

/// Pure predicate for the cross-replica rotation clock: a marker more than
/// 60s ahead of the local wall clock is untrusted (clock skew/tampering).
const ROTATED_AT_FUTURE_TOLERANCE_SECS: u64 = 60;
fn rotated_at_is_trusted(marker: u64, now: u64) -> bool {
    marker <= now.saturating_add(ROTATED_AT_FUTURE_TOLERANCE_SECS)
}

#[cfg(test)]
mod tests {
    use super::rotated_at_is_trusted;

    #[test]
    fn rotated_at_future_marker_beyond_tolerance_is_untrusted() {
        let now = 1_700_000_000u64;
        // Normal (recent or slightly ahead within tolerance) is trusted.
        assert!(rotated_at_is_trusted(now, now));
        assert!(rotated_at_is_trusted(now - 300, now));
        assert!(rotated_at_is_trusted(now + 60, now));
        // Beyond the 60s tolerance: tampering / clock skew -> untrusted.
        assert!(!rotated_at_is_trusted(now + 61, now));
        assert!(!rotated_at_is_trusted(now + 3600, now));
    }
}
