use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::{EventSink, EventSinkCtx};
use ponyllm_core::pool::{
    is_context_capacity_compatible, parse_context_capacity_tokens, BillingMode, EconomyScorer,
    GatewayRoutingStrategy, HotCacheTracker, KeyPool, ModelTier, ModelThinkingSpec, NodeLatencyMetrics, PricingConfig,
    SpeedScorer, UpstreamProtocol,
};

use ponyllm_core::telemetry::{
    ConnectivitySampler, EventBus, EventCtx, MetricsCollector, MetricsProjection,
    StreamProjection, TimeseriesProjection,
};
use ponyllm_core::telemetry::{FlightRecorder, GatewayEvent};
use crate::config::{GatewayConfig, ProviderConfig};
use crate::frames::FrameConverter;
use crate::routes::models::ParsedRequestModel;

#[derive(Debug, Clone)]
pub struct RoutedTarget {
    pub provider_name: String,
    pub base_url: String,
    pub physical_model: String,
    pub tier: ModelTier,
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
}

impl RoutedTarget {
    pub fn resolve_thinking(&self, requested: Option<ponyllm_protocol::common::ReasoningEffort>) -> ponyllm_protocol::common::ReasoningEffort {
        self.thinking_spec.resolve(requested)
    }

    pub fn supports_modality(&self, modality: &str) -> bool {
        if modality.eq_ignore_ascii_case("text") {
            return true;
        }
        self.input_types.iter().any(|t| t.eq_ignore_ascii_case(modality))
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

#[derive(Debug)]
pub struct AppState {
    pub config: RwLock<GatewayConfig>,
    pub pools: RwLock<HashMap<String, Arc<KeyPool>>>,
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
    /// Write queue lock serializing admin config mutations (WEB-06).
    pub admin_write_lock: Arc<tokio::sync::Mutex<()>>,
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
        let direct_client = ponyllm_core::executor::create_upstream_http_client_with_options(
            None,
            false,
        );
        let http_client = ponyllm_core::executor::create_upstream_http_client_with_options(
            config.proxy.as_deref(),
            config.use_system_proxy,
        );
        let mut proxy_clients = HashMap::new();
        for (_, p_cfg) in &config.providers {
            if let Some(proxy) = &p_cfg.proxy {
                let trimmed = proxy.trim();
                if !trimmed.is_empty()
                    && !trimmed.eq_ignore_ascii_case("direct")
                    && !trimmed.eq_ignore_ascii_case("none")
                {
                    proxy_clients.entry(trimmed.to_string()).or_insert_with(|| {
                        ponyllm_core::executor::create_upstream_http_client_with_options(
                            Some(trimmed),
                            config.use_system_proxy,
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
                        proxy_clients.entry(trimmed.to_string()).or_insert_with(|| {
                            ponyllm_core::executor::create_upstream_http_client_with_options(
                                Some(trimmed),
                                config.use_system_proxy,
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
        Self {
            config: RwLock::new(config),
            pools: RwLock::new(HashMap::new()),
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
            admin_write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
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

        match effective {
            crate::config::EffectiveProxy::InheritGateway => self.http_client.clone(),
            crate::config::EffectiveProxy::Direct => self.direct_client.clone(),
            crate::config::EffectiveProxy::Custom(url) => {
                self.get_or_create_proxy_client(url, cfg.use_system_proxy)
            }
        }
    }

    /// Helper for retrieving or lazily building a connection pool client for a proxy endpoint.
    fn get_or_create_proxy_client(&self, url: &str, use_system_proxy: bool) -> reqwest::Client {
        if let Some(client) = self.proxy_clients.read().get(url) {
            return client.clone();
        }
        let mut write = self.proxy_clients.write();
        if let Some(client) = write.get(url) {
            return client.clone();
        }
        let client = ponyllm_core::executor::create_upstream_http_client_with_options(
            Some(url),
            use_system_proxy,
        );
        write.insert(url.to_string(), client.clone());
        client
    }

    /// Return the HTTP client for the given provider (inherits provider default proxy).
    pub fn http_client_for_provider(&self, provider_name: &str) -> reqwest::Client {
        self.http_client_for_target(provider_name, "")
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
        let mut config_guard = self.config.write();
        let mut pools_guard = self.pools.write();

        for (name, pool) in new_pools {
            pools_guard.insert(name, pool);
        }

        pools_guard.retain(|name, _| new_config.providers.contains_key(name));

        let mut proxy_clients_guard = self.proxy_clients.write();
        proxy_clients_guard.clear();
        for (_, p_cfg) in &new_config.providers {
            if let Some(proxy) = &p_cfg.proxy {
                let trimmed = proxy.trim();
                if !trimmed.is_empty()
                    && !trimmed.eq_ignore_ascii_case("direct")
                    && !trimmed.eq_ignore_ascii_case("none")
                {
                    proxy_clients_guard.entry(trimmed.to_string()).or_insert_with(|| {
                        ponyllm_core::executor::create_upstream_http_client_with_options(
                            Some(trimmed),
                            new_config.use_system_proxy,
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
                        proxy_clients_guard.entry(trimmed.to_string()).or_insert_with(|| {
                            ponyllm_core::executor::create_upstream_http_client_with_options(
                                Some(trimmed),
                                new_config.use_system_proxy,
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

        *config_guard = new_config;
        drop(config_guard);
        drop(pools_guard);

        self.attach_antigravity_rotation_hooks_all();
    }

    pub fn attach_antigravity_rotation_hook(
        &self,
        provider_name: &str,
        mgr: &Arc<ponyllm_core::pool::AntigravityTokenManager>,
    ) {
        if let Some(ref store) = self.config_store {
            let store_clone = store.clone();
            let prov_name = provider_name.to_string();
            let write_lock = self.admin_write_lock.clone();
            mgr.set_rotation_hook(Arc::new(move |key_id, new_rf| {
                let store = store_clone.clone();
                let prov = prov_name.clone();
                let k_id = key_id.to_string();
                let n_rf = new_rf.to_string();
                let write_lock = write_lock.clone();
                tokio::spawn(async move {
                    let _guard = write_lock.lock().await;
                    if let Ok(mut cfg) = store.load() {
                        if let Some(p) = cfg.providers.get_mut(&prov) {
                            if let Some(k) = p.keys.iter_mut().find(|k| k.id == k_id) {
                                k.api_key = n_rf;
                                cfg.config_version += 1;
                                if let Err(e) = store.save(&cfg) {
                                    tracing::error!(%e, provider = %prov, key_id = %k_id, "failed to persist rotated Antigravity token");
                                } else {
                                    tracing::info!(provider = %prov, key_id = %k_id, "successfully persisted rotated Antigravity token");
                                }
                            }
                        }
                    }
                });
            }));
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

    pub fn register_pool(&self, provider: &str, pool: Arc<KeyPool>) {
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
            model: None,
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
        let mut candidates = Vec::new();

        // 1. Match exact model name across all providers
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
                });
            }
        }

        // 2. Prefix matching (e.g. "deepseek/deepseek-chat")
        if candidates.is_empty() {
            if let Some((prefix, sub_model)) = clean.split_once('/') {
                if let Some(p_cfg) = config.providers.get(prefix) {
                    let spec = p_cfg.get_model_spec(sub_model);
                    let thinking_spec = spec.thinking_spec();
                    let pricing = p_cfg.get_model_pricing(sub_model);
                    let billing_mode = p_cfg.get_model_billing_mode(sub_model);
                    let (protocol, endpoint_base) =
                        resolve_effective_protocol(prefix, p_cfg, sub_model, proto_override, inbound);
                    candidates.push(RoutedTarget {
                        provider_name: prefix.to_string(),
                        base_url: spec.base_url.clone().unwrap_or_else(|| p_cfg.base_url.clone()),
                        physical_model: sub_model.to_string(),
                        tier: spec.tier,
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
                    });
                }
            }
        }

        // 3. Keyword heuristic matching
        if candidates.is_empty() {
            let lower = clean.to_lowercase();
            for (p_name, p_cfg) in &config.providers {
                if lower.contains(p_name)
                    || (p_name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3")))
                    || (p_name == "anthropic" && lower.starts_with("claude"))
                    || (p_name == "deepseek" && lower.starts_with("deepseek"))
                {
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

        // 5. Context Capacity Monotonicity check
        if parsed.is_1m_context {
            let before_len = candidates.len();
            candidates.retain(|c| is_context_capacity_compatible("1M", &c.context_window));
            if candidates.is_empty() && before_len > 0 {
                return Err(CoreError::CapacityExhausted {
                    required_context: "1M".to_string(),
                    message: format!(
                        "Model '{}' does not support 1M context requirement",
                        clean
                    ),
                });
            }
        }

        Ok(self.sort_candidates(candidates, strategy, config, cached_provider, inbound))
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
        match strategy {
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
        }
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

        // 2. Physical configured models and their [1m] aliases
        let config = self.config.read();
        for (provider_name, cfg) in &config.providers {
            let mut add_model_and_alias = |m: &str| {
                let proto = cfg
                    .native_protocol(m)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| {
                        infer_legacy_protocol(provider_name, &cfg.base_url).to_string()
                    });
                if !seen.contains(m) {
                    result.push((m.to_string(), provider_name.clone(), None, proto.clone()));
                    seen.insert(m.to_string());
                }
                let spec = cfg.get_model_spec(m);
                if parse_context_capacity_tokens(&spec.context_window) >= 1048576 {
                    let alias_1m = format!("{}[1m]", m);
                    if !seen.contains(&alias_1m) {
                        result.push((alias_1m.clone(), provider_name.clone(), Some(format!("{} (1M 长上下文)", m)), proto));
                        seen.insert(alias_1m);
                    }
                }
            };

            if !cfg.default_model.is_empty() {
                add_model_and_alias(&cfg.default_model);
            }
            for m in &cfg.models {
                if m != &cfg.default_model {
                    add_model_and_alias(m);
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
};
