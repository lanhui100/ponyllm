use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use parking_lot::RwLock;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::{EventSink, EventSinkCtx};
use ponyllm_core::pool::refresh_gate::RefreshGate;
use ponyllm_core::pool::{
    is_context_capacity_compatible, parse_context_capacity_tokens, AntigravityTokenManager,
    BillingMode, EconomyScorer, GatewayRoutingStrategy, HotCacheTracker, KeyPool, ModelTier,
    ModelThinkingSpec, NodeLatencyMetrics, PricingConfig, RefreshPersistHook, SpeedScorer,
    UpstreamProtocol,
};
use ponyllm_core::{canonicalize_model_name, model_aliases};

use ponyllm_core::telemetry::{
    ConnectivitySampler, EventBus, EventCtx, MetricsCollector, MetricsProjection,
    StreamProjection, TimeseriesProjection,
};
use ponyllm_core::telemetry::{FlightRecorder, GatewayEvent};
use ponyllm_config::ConfigFile;
use crate::admin_store::{ConfigStore, ConfigStoreError};
use crate::config::{GatewayConfig, ProviderConfig};
use crate::frames::FrameConverter;
use crate::routes::models::ParsedRequestModel;

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
            if let Err(e) = crate::telemetry_snapshot::save_snapshot_with_live_cycles(&path, &snap, live_cycles) {
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
    pub fn resolve_thinking(&self, requested: Option<ponyllm_protocol::common::ReasoningEffort>) -> ponyllm_protocol::common::ReasoningEffort {
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
            if (t_lower == "file" && mod_lower == "document") || (t_lower == "document" && mod_lower == "file") {
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
        let base = self.endpoint_base.as_deref().unwrap_or(&self.base_url).trim_end_matches('/');
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
    pub pending_restored_usages: Arc<RwLock<HashMap<String, ponyllm_core::pool::usage::KeyUsageStateSnapshot>>>,
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
            None,
            false,
            gw_timeout,
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
                    proxy_clients.entry(proxy_client_key(trimmed, gw_timeout)).or_insert_with(|| {
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
                        proxy_clients.entry(proxy_client_key(trimmed, gw_timeout)).or_insert_with(|| {
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
        let pools: Arc<RwLock<HashMap<String, Arc<KeyPool>>>> = Arc::new(RwLock::new(HashMap::new()));
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
                        tracing::warn!("telemetry snapshot at {:?} unreadable, starting fresh", path);
                    }
                }
            }
        }
        let pending_restored_usages = Arc::new(RwLock::new(restored_usages));
        let cluster_telemetry_store = crate::cluster_telemetry::ClusterTelemetryStore::from_env().map(Arc::new);
        let cluster_telemetry_tracker = Arc::new(crate::cluster_telemetry::ClusterTelemetryTracker::new());

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
        Self {
            config: RwLock::new(config),
            pools,
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
        }
    }

    /// Best-effort immediate snapshot save (shutdown/test hooks).
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
    pub fn http_client_for_target(&self, provider_name: &str, model_name: &str) -> reqwest::Client {
        let cfg = self.config.read();
        let effective = cfg
            .providers
            .get(provider_name)
            .map(|p| p.effective_proxy_for_model(model_name))
            .unwrap_or(crate::config::EffectiveProxy::InheritGateway);
        let timeout_secs = cfg
            .providers
            .get(provider_name)
            .and_then(|p| p.effective_timeout_secs_for_model(model_name))
            .unwrap_or(cfg.upstream_timeout_secs);
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let gw_timeout = std::time::Duration::from_secs(cfg.upstream_timeout_secs);
        let use_sys = cfg.use_system_proxy;
        let proxy_url: Option<String> = match effective {
            crate::config::EffectiveProxy::InheritGateway => cfg.proxy.clone(),
            crate::config::EffectiveProxy::Direct => None,
            crate::config::EffectiveProxy::Custom(url) => Some(url.to_string()),
        };
        drop(cfg);

        // Fast paths for the gateway defaults (no per-target override):
        // reuse the prebuilt gateway/direct clients.
        if timeout == gw_timeout {
            match proxy_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
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
        let proxy_opt = url.trim().is_empty().then(|| url);
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
                Some((state == ponyllm_core::pool::KeyState::Active, mgr.project_id(), k.id.clone()))
            })
            .max_by_key(|(active, _, _)| *active)
            .map(|(_, project, key_id)| (project, key_id))
    }

    pub fn reload_config_with_pools(
        &self,
        new_config: GatewayConfig,
        new_pools: HashMap<String, Arc<KeyPool>>,
    ) {
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
                    proxy_clients_guard.entry(proxy_client_key(trimmed, gw_timeout)).or_insert_with(|| {
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
                        proxy_clients_guard.entry(proxy_client_key(trimmed, gw_timeout)).or_insert_with(|| {
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
                                        Self::advance_rotated_at(store.clone(), &key_id, throttled_patch.clone()).await;
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
            throttled.lock().await.insert(key_id.to_string(), std::time::Instant::now());
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
                    tracing::info!("Starting initial Antigravity quota & token refresh keepalive cycle");
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
                    (cfg.antigravity_auto_refresh, cfg.antigravity_refresh_interval_secs.max(60))
                };

                if enabled && last_run.elapsed() >= std::time::Duration::from_secs(interval_secs) {
                    tracing::info!("Starting scheduled Antigravity quota & token refresh keepalive cycle");
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
        let key_entries: Vec<(String, String, Arc<ponyllm_core::pool::AntigravityTokenManager>, Option<String>)> = {
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
                        pool.record_error(&key_id, ponyllm_core::pool::PoolErrorType::AuthInvalid { reason: Some(reason.clone()) });
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
            match mgr.with_client(&probe_client).fetch_quota(base_url.as_deref()).await {
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

                    let current_fraction = snapshot.quota_groups.as_ref().and_then(|groups| {
                        for g in groups {
                            for b in &g.buckets {
                                if b.window.eq_ignore_ascii_case("5h") || b.bucket_id.contains("5h") {
                                    return Some(b.remaining_fraction);
                                }
                            }
                        }
                        None
                    }).or_else(|| {
                        snapshot.models.values().next().map(|m| m.remaining_fraction)
                    });

                    let weekly_fraction = snapshot.quota_groups.as_ref().and_then(|groups| {
                        for g in groups {
                            for b in &g.buckets {
                                let win = b.window.to_lowercase();
                                let b_id = b.bucket_id.to_lowercase();
                                let b_desc = b.description.as_deref().unwrap_or("").to_lowercase();
                                let b_disp = b.display_name.as_deref().unwrap_or("").to_lowercase();
                                if win == "weekly" || b_id.contains("week") || b_desc.contains("week") || b_disp.contains("周") || b_id.contains("7d") {
                                    return Some(b.remaining_fraction);
                                }
                            }
                        }
                        None
                    });

                    if let Some(frac) = current_fraction {
                        if let Some(pool) = self.pools.read().get(&provider) {
                            if let Some(entry) = pool.snapshot_keys().into_iter().find(|k| k.id == key_id) {
                                entry
                                    .usage_tracker
                                    .observe_upstream_probe_dual(now_ms, Some(frac), weekly_fraction);
                            }
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
                    if let ponyllm_core::error::CoreError::UpstreamStatusError { status, body } = &e {
                        if let Some(pool_err) = ponyllm_core::executor::classify_probe_failure(status.as_u16(), body) {
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
    pub fn emit(
        &self,
        ctx: &EventCtx,
        provider: Option<String>,
        event: GatewayEvent,
    ) -> u64 {
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
        self.resolve_routed_targets_with_prompt_and_protocol(parsed, header_strategy, prompt, None, None)
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
        self.resolve_routed_targets_full(parsed, header_strategy, prompt, proto_override, inbound, &[])
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
            return Err(CoreError::Internal("No routing candidates available".to_string()));
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

        if let Some(explicit_tier) = parsed.explicit_tier {
            let candidates: Vec<RoutedTarget> = self
                .collect_tier_candidates(explicit_tier, strategy, config, proto_override, inbound)
                .into_iter()
                .filter(filter_compat)
                .collect();

            if candidates.is_empty() {
                if !required_modalities.is_empty() {
                    return Err(CoreError::UnsupportedModality {
                        required_modality: required_modalities.join(", "),
                        message: format!(
                            "No model candidate in tier '{:?}' supports required modalities {:?}",
                            explicit_tier, required_modalities
                        ),
                    });
                } else if parsed.is_1m_context {
                    return Err(CoreError::CapacityExhausted {
                        required_context: "1M".to_string(),
                        message: format!(
                            "No model candidate in tier '{:?}' meets 1M context requirement",
                            explicit_tier
                        ),
                    });
                } else {
                    return Err(CoreError::Internal(format!(
                        "No candidate models configured in gateway for tier '{:?}'",
                        explicit_tier
                    )));
                }
            }
            return Ok(self.sort_candidates(candidates, strategy, config, cached_provider, inbound));
        }

        // Default auto (no explicit tier): Try Standard -> Elevate to Flagship -> Fallback to Light
        let standard_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Standard, strategy, config, proto_override, inbound)
            .into_iter()
            .filter(filter_compat)
            .collect();

        if !standard_candidates.is_empty() {
            return Ok(self.sort_candidates(standard_candidates, strategy, config, cached_provider, inbound));
        }

        // Adaptive Tier Elevation: Elevate to Flagship if Standard has no matching (or 1M or modality) nodes
        let flagship_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Flagship, strategy, config, proto_override, inbound)
            .into_iter()
            .filter(filter_compat)
            .collect();

        if !flagship_candidates.is_empty() {
            return Ok(self.sort_candidates(flagship_candidates, strategy, config, cached_provider, inbound));
        }

        // Fallback to Light tier
        let light_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Light, strategy, config, proto_override, inbound)
            .into_iter()
            .filter(filter_compat)
            .collect();

        if !light_candidates.is_empty() {
            return Ok(self.sort_candidates(light_candidates, strategy, config, cached_provider, inbound));
        }

        if !required_modalities.is_empty() {
            Err(CoreError::UnsupportedModality {
                required_modality: required_modalities.join(", "),
                message: format!(
                    "No model candidate across any tier supports required modalities {:?}",
                    required_modalities
                ),
            })
        } else if parsed.is_1m_context {
            Err(CoreError::CapacityExhausted {
                required_context: "1M".to_string(),
                message: "No model candidate across any tier meets 1M context requirement"
                    .to_string(),
            })
        } else {
            Err(CoreError::Internal(
                "No candidate models available in gateway for auto routing".to_string(),
            ))
        }
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
                    base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
                    let (protocol, endpoint_base) =
                        resolve_effective_protocol(p_name, p_cfg, effective, proto_override, inbound);
                    candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
                    let (protocol, endpoint_base) =
                        resolve_effective_protocol(prefix, p_cfg, sub_effective, proto_override, inbound);
                    candidates.push(RoutedTarget {
                        provider_name: prefix.to_string(),
                        base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
                    || (p_name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3")))
                    || (p_name == "anthropic" && lower.starts_with("claude"))
                    || (p_name == "deepseek" && lower.starts_with("deepseek"))
                {
                    let spec = p_cfg.get_model_spec(effective);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(effective);
                    let billing_mode = p_cfg.get_model_billing_mode(effective);
                    let (protocol, endpoint_base) =
                        resolve_effective_protocol(p_name, p_cfg, effective, proto_override, inbound);
                    candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
        let mut visited_models: std::collections::HashSet<String> = std::collections::HashSet::new();
        visited_models.insert(clean.clone());
        visited_models.insert(effective.to_string());

        let mut queue: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::new();
        for target in &candidates {
            if let Some(p_cfg) = config.providers.get(&target.provider_name) {
                let spec = p_cfg.get_model_spec(&target.physical_model);
                for fb in &spec.fallbacks {
                    if visited_models.insert(fb.clone()) {
                        queue.push_back((fb.clone(), 1));
                    }
                }
            }
        }

        let mut sorted_primary = self.sort_candidates(candidates, strategy, config, cached_provider, inbound);

        let mut secondary_candidates = Vec::new();
        while let Some((fb_model, depth)) = queue.pop_front() {
            for (p_name, p_cfg) in &config.providers {
                if p_cfg.default_model == fb_model || p_cfg.models.iter().any(|m| m == &fb_model) {
                    let spec = p_cfg.get_model_spec(&fb_model);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(&fb_model);
                    let billing_mode = p_cfg.get_model_billing_mode(&fb_model);
                    let (protocol, endpoint_base) =
                        resolve_effective_protocol(p_name, p_cfg, &fb_model, proto_override, inbound);
                    secondary_candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
            let sorted_secondary = self.sort_candidates(secondary_candidates, strategy, config, cached_provider, inbound);
            sorted_primary.extend(sorted_secondary);
        }

        // 5. Context Capacity Monotonicity check
        if parsed.is_1m_context {
            let before_len = sorted_primary.len();
            sorted_primary.retain(|c| is_context_capacity_compatible("1M", &c.context_window));
            if sorted_primary.is_empty() && before_len > 0 {
                return Err(CoreError::CapacityExhausted {
                    required_context: "1M".to_string(),
                    message: format!(
                        "Model '{}' does not support 1M context requirement",
                        clean
                    ),
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
                let (protocol, endpoint_base) = resolve_effective_protocol(p_name, p_cfg, &p_cfg.default_model, proto_override, inbound);
                candidates.push(RoutedTarget {
                    provider_name: p_name.clone(),
                    base_url: default_spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
                            base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
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
        mut candidates: Vec<RoutedTarget>,
        strategy: GatewayRoutingStrategy,
        _config: &GatewayConfig,
        cached_provider: Option<&str>,
        inbound: Option<UpstreamProtocol>,
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
                        let cached = cached_provider.map(|p| p == c.provider_name).unwrap_or(false);
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
                        let cached = cached_provider.map(|p| p == c.provider_name).unwrap_or(false);
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
        sorted.sort_by_key(|c| std::cmp::Reverse(c.priority.unwrap_or(0)));
        sorted
    }

    /// List all exposed models: virtual auto models and physical configured models.
    /// The fourth tuple element is the effective native protocol (`chat` by
    /// default; `auto` for virtual models whose protocol resolves per request).
    pub fn list_all_models(&self) -> Vec<(String, String, Option<String>, String)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. auto virtual models
        result.push(("auto".to_string(), "ponyllm".to_string(), Some("Auto(智能·主力默认)".to_string()), "auto".to_string()));
        result.push(("auto:standard".to_string(), "ponyllm".to_string(), Some("Auto(智能·主力)".to_string()), "auto".to_string()));
        result.push(("auto:flagship".to_string(), "ponyllm".to_string(), Some("Auto(智能·旗舰)".to_string()), "auto".to_string()));
        result.push(("auto:economy".to_string(), "ponyllm".to_string(), Some("Auto(智能·省钱)".to_string()), "auto".to_string()));
        result.push(("auto:fastest".to_string(), "ponyllm".to_string(), Some("Auto(智能·极速)".to_string()), "auto".to_string()));
        result.push(("auto[1m]".to_string(), "ponyllm".to_string(), Some("Auto(智能·1M长上下文)".to_string()), "auto".to_string()));

        seen.insert("auto".to_string());
        seen.insert("auto:standard".to_string());
        seen.insert("auto:flagship".to_string());
        seen.insert("auto:economy".to_string());
        seen.insert("auto:fastest".to_string());
        seen.insert("auto[1m]".to_string());

        // 2. Physical configured models and their [1m] aliases.
        // Iteration is provider-name sorted so the list content and the bare
        // name's `owned_by` are deterministic across restarts (the config map
        // is a HashMap; unsorted iteration would randomize which provider's
        // alias survives name collisions).
        let config = self.config.read();
        let mut provider_names: Vec<&String> = config.providers.keys().collect();
        provider_names.sort();
        // Literal model names configured anywhere: `provider/model` aliases
        // must never shadow a literal name (literal names win routing via the
        // exact-match step in `resolve_pinned_targets`).
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
        // Provider count per model name: aliases are only emitted for models
        // shared by ≥2 providers — that is the only case where pinning
        // (`provider/model`) actually disambiguates, and it keeps the list
        // from ballooning for single-provider models.
        let mut model_provider_count: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for cfg in config.providers.values() {
            for m in cfg
                .models
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(cfg.default_model.as_str()).filter(|d| !d.is_empty()))
            {
                *model_provider_count.entry(m).or_insert(0) += 1;
            }
        }
        for provider_name in &provider_names {
            let cfg = &config.providers[*provider_name];
            let mut add_model_and_alias = |m: &str| {
                let proto = cfg
                    .native_protocol(m)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| {
                        infer_legacy_protocol(provider_name, &cfg.base_url).to_string()
                    });
                if !seen.contains(m) {
                    result.push((m.to_string(), provider_name.to_string(), None, proto.clone()));
                    seen.insert(m.to_string());
                }
                let spec = cfg.get_model_spec(m);
                // Per-provider explicit alias (`provider/model`, e.g.
                // `sense/deepseek-v4-flash`): the bare name is deduped to one
                // list entry although every provider carrying it remains a
                // failover candidate at runtime. Emitted only when the model
                // is shared by ≥2 providers and the alias string does not
                // collide with a configured literal model name.
                let shared = model_provider_count.get(m).copied().unwrap_or(0) >= 2;
                let prefixed = format!("{}/{}", provider_name, m);
                if shared && !seen.contains(&prefixed) {
                    if literal_names.contains(prefixed.as_str()) {
                        tracing::warn!(
                            provider = %provider_name,
                            model = %m,
                            alias = %prefixed,
                            "skipping provider/model alias: collides with a configured literal model name (literal names take routing precedence)"
                        );
                    } else {
                        let alias_display = spec
                            .display_name
                            .clone()
                            .unwrap_or_else(|| format!("{} ({})", m, provider_name));
                        result.push((
                            prefixed.clone(),
                            provider_name.to_string(),
                            Some(alias_display),
                            proto.clone(),
                        ));
                        seen.insert(prefixed.clone());
                    }
                }
                if parse_context_capacity_tokens(&spec.context_window) >= 1048576 {
                    let alias_1m = format!("{}[1m]", m);
                    if !seen.contains(&alias_1m) {
                        result.push((alias_1m.clone(), provider_name.to_string(), Some(format!("{} (1M 长上下文)", m)), proto.clone()));
                        seen.insert(alias_1m);
                    }
                    // Provider-scoped [1m] alias for shared 1M models.
                    let prefixed_1m = format!("{}[1m]", prefixed);
                    if shared && !seen.contains(&prefixed_1m) && !literal_names.contains(prefixed.as_str()) {
                        result.push((
                            prefixed_1m.clone(),
                            provider_name.to_string(),
                            Some(format!("{}[1m] ({})", m, provider_name)),
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
