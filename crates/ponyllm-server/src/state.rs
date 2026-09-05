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
    EventBus, EventCtx, MetricsCollector, MetricsProjection, StreamProjection,
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
}

impl RoutedTarget {
    pub fn resolve_thinking(&self, requested: Option<ponyllm_protocol::common::ReasoningEffort>) -> ponyllm_protocol::common::ReasoningEffort {
        self.thinking_spec.resolve(requested)
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
        return with_endpoint(p_cfg, o);
    }
    if let Some(m) = p_cfg.native_protocol(model_name) {
        // Native passthrough preferred: an inbound protocol the provider
        // natively serves (explicit endpoint override) beats the default.
        if let Some(i) = inbound {
            if i != m && p_cfg.endpoint_base_for(i).is_some() {
                return with_endpoint(p_cfg, i);
            }
        }
        return with_endpoint(p_cfg, m);
    }
    // No declarations: an inbound protocol with an explicit endpoint still
    // signals native support and wins over the URL heuristic.
    if let Some(i) = inbound {
        if p_cfg.endpoint_base_for(i).is_some() {
            return with_endpoint(p_cfg, i);
        }
    }
    with_endpoint(
        p_cfg,
        infer_legacy_protocol(p_name, &p_cfg.base_url),
    )
}

fn with_endpoint(p_cfg: &ProviderConfig, protocol: UpstreamProtocol) -> (UpstreamProtocol, Option<String>) {
    let endpoint_base = p_cfg
        .endpoint_base_for(protocol)
        .map(|s| s.to_string());
    (protocol, endpoint_base)
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
    /// Global reusable HTTP client with connection pool and TCP nodelay.
    pub http_client: reqwest::Client,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> Self {
        let capacity = config.flight_recorder_capacity;
        let flight_recorder = Arc::new(FlightRecorder::new(capacity));
        let metrics = Arc::new(MetricsCollector::new());
        let bus = Arc::new(EventBus::new(capacity));
        let metrics_proj = Arc::new(MetricsProjection::new(metrics.clone()));
        let stream_proj = Arc::new(StreamProjection::default());
        let http_client = ponyllm_core::executor::create_upstream_http_client();
        bus.add_projection(metrics_proj.clone());
        bus.add_projection(stream_proj.clone());
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
            http_client,
        }
    }

    /// Override the HTTP client (useful for mock transports in tests).
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
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

        tracing::info!(
            "Gateway configuration reloaded. Active providers: {:?}",
            pools_guard.keys().collect::<Vec<_>>()
        );

        *config_guard = new_config;
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
        let config = self.config.read();
        let strategy = parsed
            .strategy_override
            .or(header_strategy)
            .unwrap_or(config.default_strategy);

        let cached_provider = prompt.and_then(|p| self.hot_cache.probe_cached_provider(p));

        if parsed.is_auto {
            self.resolve_auto_targets(parsed, strategy, &config, cached_provider.as_deref(), proto_override, inbound)
        } else {
            self.resolve_pinned_targets(parsed, strategy, &config, cached_provider.as_deref(), proto_override, inbound)
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
    ) -> Result<Vec<RoutedTarget>> {
        let filter_1m = |c: &RoutedTarget| {
            if parsed.is_1m_context {
                is_context_capacity_compatible("1M", &c.context_window)
            } else {
                true
            }
        };

        if let Some(explicit_tier) = parsed.explicit_tier {
            let candidates: Vec<RoutedTarget> = self
                .collect_tier_candidates(explicit_tier, strategy, config, proto_override, inbound)
                .into_iter()
                .filter(filter_1m)
                .collect();

            if candidates.is_empty() {
                if parsed.is_1m_context {
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
            .filter(filter_1m)
            .collect();

        if !standard_candidates.is_empty() {
            return Ok(self.sort_candidates(standard_candidates, strategy, config, cached_provider, inbound));
        }

        // Adaptive Tier Elevation: Elevate to Flagship if Standard has no matching (or 1M) nodes
        let flagship_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Flagship, strategy, config, proto_override, inbound)
            .into_iter()
            .filter(filter_1m)
            .collect();

        if !flagship_candidates.is_empty() {
            return Ok(self.sort_candidates(flagship_candidates, strategy, config, cached_provider, inbound));
        }

        // Fallback to Light tier
        let light_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Light, strategy, config, proto_override, inbound)
            .into_iter()
            .filter(filter_1m)
            .collect();

        if !light_candidates.is_empty() {
            return Ok(self.sort_candidates(light_candidates, strategy, config, cached_provider, inbound));
        }

        if parsed.is_1m_context {
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
                    base_url: p_cfg.base_url.clone(),
                    physical_model: clean.clone(),
                    tier: spec.tier,
                    strategy,
                    upstream_protocol: protocol,
                    endpoint_base,
                    context_window: spec.context_window,
                    billing_mode,
                    pricing,
                    thinking_spec,
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
                        base_url: p_cfg.base_url.clone(),
                        physical_model: sub_model.to_string(),
                        tier: spec.tier,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window,
                        billing_mode,
                        pricing,
                        thinking_spec,
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
                        base_url: p_cfg.base_url.clone(),
                        physical_model: clean.clone(),
                        tier: spec.tier,
                        strategy,
                        upstream_protocol: protocol,
                        endpoint_base,
                        context_window: spec.context_window,
                        billing_mode,
                        pricing,
                        thinking_spec,
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
                    base_url: p_cfg.base_url.clone(),
                    physical_model: p_cfg.default_model.clone(),
                    tier,
                    strategy,
                    upstream_protocol: protocol,
                    endpoint_base,
                    context_window: default_spec.context_window,
                    billing_mode: default_billing,
                    pricing: default_pricing,
                    thinking_spec,
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
                            base_url: p_cfg.base_url.clone(),
                            physical_model: m.clone(),
                            tier,
                            strategy,
                            upstream_protocol: protocol,
                            endpoint_base,
                            context_window: spec.context_window,
                            billing_mode: m_billing,
                            pricing: m_pricing,
                            thinking_spec,
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
