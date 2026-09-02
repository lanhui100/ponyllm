use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::pool::{
    is_context_capacity_compatible, parse_context_capacity_tokens, EconomyScorer,
    GatewayRoutingStrategy, HotCacheTracker, KeyPool, ModelTier, NodeLatencyMetrics, SpeedScorer,
};
use ponyllm_core::telemetry::{FlightRecorder, MetricsCollector};
use crate::config::{GatewayConfig, ProviderConfig};
use crate::routes::models::ParsedRequestModel;

#[derive(Debug, Clone)]
pub struct RoutedTarget {
    pub provider_name: String,
    pub base_url: String,
    pub physical_model: String,
    pub tier: ModelTier,
    pub strategy: GatewayRoutingStrategy,
    pub is_anthropic_upstream: bool,
    pub context_window: String,
}

#[derive(Debug)]
pub struct AppState {
    pub config: GatewayConfig,
    pub pools: RwLock<HashMap<String, Arc<KeyPool>>>,
    pub flight_recorder: Arc<FlightRecorder>,
    pub metrics: Arc<MetricsCollector>,
    pub hot_cache: Arc<HotCacheTracker>,
    pub node_metrics: RwLock<HashMap<String, Arc<NodeLatencyMetrics>>>,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> Self {
        let capacity = config.flight_recorder_capacity;
        Self {
            config,
            pools: RwLock::new(HashMap::new()),
            flight_recorder: Arc::new(FlightRecorder::new(capacity)),
            metrics: Arc::new(MetricsCollector::new()),
            hot_cache: Arc::new(HotCacheTracker::new()),
            node_metrics: RwLock::new(HashMap::new()),
        }
    }

    pub fn register_pool(&self, provider: &str, pool: Arc<KeyPool>) {
        self.pools.write().insert(provider.to_string(), pool);
    }

    pub fn get_pool(&self, provider: &str) -> Option<Arc<KeyPool>> {
        self.pools.read().get(provider).cloned()
    }

    pub fn get_or_create_node_metrics(&self, provider: &str) -> Arc<NodeLatencyMetrics> {
        let read = self.node_metrics.read();
        if let Some(m) = read.get(provider) {
            return m.clone();
        }
        drop(read);
        let mut write = self.node_metrics.write();
        write
            .entry(provider.to_string())
            .or_insert_with(|| Arc::new(NodeLatencyMetrics::default()))
            .clone()
    }

    /// Resolve ordered list of candidate targets for multi-provider transparent failover
    pub fn resolve_routed_targets(
        &self,
        parsed: &ParsedRequestModel,
        header_strategy: Option<GatewayRoutingStrategy>,
    ) -> Result<Vec<RoutedTarget>> {
        let strategy = parsed
            .strategy_override
            .or(header_strategy)
            .unwrap_or(self.config.default_strategy);

        if parsed.is_auto {
            self.resolve_auto_targets(parsed, strategy)
        } else {
            self.resolve_pinned_targets(parsed, strategy)
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
                .collect_tier_candidates(explicit_tier, strategy)
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
            return Ok(self.sort_candidates(candidates, strategy));
        }

        // Default auto (no explicit tier): Try Standard -> Elevate to Flagship -> Fallback to Light
        let standard_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Standard, strategy)
            .into_iter()
            .filter(filter_1m)
            .collect();

        if !standard_candidates.is_empty() {
            return Ok(self.sort_candidates(standard_candidates, strategy));
        }

        // Adaptive Tier Elevation: Elevate to Flagship if Standard has no matching (or 1M) nodes
        let flagship_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Flagship, strategy)
            .into_iter()
            .filter(filter_1m)
            .collect();

        if !flagship_candidates.is_empty() {
            return Ok(self.sort_candidates(flagship_candidates, strategy));
        }

        // Fallback to Light tier
        let light_candidates: Vec<RoutedTarget> = self
            .collect_tier_candidates(ModelTier::Light, strategy)
            .into_iter()
            .filter(filter_1m)
            .collect();

        if !light_candidates.is_empty() {
            return Ok(self.sort_candidates(light_candidates, strategy));
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
    ) -> Result<Vec<RoutedTarget>> {
        let clean = &parsed.clean_model_name;
        let mut candidates = Vec::new();

        // 1. Match exact model name across all providers
        for (p_name, p_cfg) in &self.config.providers {
            if p_cfg.default_model == *clean || p_cfg.models.iter().any(|m| m == clean) {
                let spec = p_cfg.get_model_spec(clean);
                let is_ant = p_cfg.base_url.contains("anthropic")
                    || (p_name.contains("anthropic") && !p_cfg.base_url.contains("v1/chat"));
                candidates.push(RoutedTarget {
                    provider_name: p_name.clone(),
                    base_url: p_cfg.base_url.clone(),
                    physical_model: clean.clone(),
                    tier: spec.tier,
                    strategy,
                    is_anthropic_upstream: is_ant,
                    context_window: spec.context_window,
                });
            }
        }

        // 2. Prefix matching (e.g. "deepseek/deepseek-chat")
        if candidates.is_empty() {
            if let Some((prefix, sub_model)) = clean.split_once('/') {
                if let Some(p_cfg) = self.config.providers.get(prefix) {
                    let spec = p_cfg.get_model_spec(sub_model);
                    let is_ant = p_cfg.base_url.contains("anthropic");
                    candidates.push(RoutedTarget {
                        provider_name: prefix.to_string(),
                        base_url: p_cfg.base_url.clone(),
                        physical_model: sub_model.to_string(),
                        tier: spec.tier,
                        strategy,
                        is_anthropic_upstream: is_ant,
                        context_window: spec.context_window,
                    });
                }
            }
        }

        // 3. Keyword heuristic matching
        if candidates.is_empty() {
            let lower = clean.to_lowercase();
            for (p_name, p_cfg) in &self.config.providers {
                if lower.contains(p_name)
                    || (p_name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3")))
                    || (p_name == "anthropic" && lower.starts_with("claude"))
                    || (p_name == "deepseek" && lower.starts_with("deepseek"))
                {
                    let spec = p_cfg.get_model_spec(clean);
                    let is_ant = p_cfg.base_url.contains("anthropic");
                    candidates.push(RoutedTarget {
                        provider_name: p_name.clone(),
                        base_url: p_cfg.base_url.clone(),
                        physical_model: clean.clone(),
                        tier: spec.tier,
                        strategy,
                        is_anthropic_upstream: is_ant,
                        context_window: spec.context_window,
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

        Ok(self.sort_candidates(candidates, strategy))
    }

    fn collect_tier_candidates(&self, tier: ModelTier, strategy: GatewayRoutingStrategy) -> Vec<RoutedTarget> {
        let mut candidates = Vec::new();
        for (p_name, p_cfg) in &self.config.providers {
            let default_spec = p_cfg.get_model_spec(&p_cfg.default_model);
            if default_spec.tier == tier {
                let is_ant = p_cfg.base_url.contains("anthropic");
                candidates.push(RoutedTarget {
                    provider_name: p_name.clone(),
                    base_url: p_cfg.base_url.clone(),
                    physical_model: p_cfg.default_model.clone(),
                    tier,
                    strategy,
                    is_anthropic_upstream: is_ant,
                    context_window: default_spec.context_window,
                });
            }
            for m in &p_cfg.models {
                if m != &p_cfg.default_model {
                    let spec = p_cfg.get_model_spec(m);
                    if spec.tier == tier {
                        let is_ant = p_cfg.base_url.contains("anthropic");
                        candidates.push(RoutedTarget {
                            provider_name: p_name.clone(),
                            base_url: p_cfg.base_url.clone(),
                            physical_model: m.clone(),
                            tier,
                            strategy,
                            is_anthropic_upstream: is_ant,
                            context_window: spec.context_window,
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
    ) -> Vec<RoutedTarget> {
        match strategy {
            GatewayRoutingStrategy::Economy => {
                candidates.sort_by(|a, b| {
                    let p_a = self.config.providers.get(&a.provider_name);
                    let p_b = self.config.providers.get(&b.provider_name);
                    let pricing_a = p_a.map(|p| p.pricing()).unwrap_or_default();
                    let pricing_b = p_b.map(|p| p.pricing()).unwrap_or_default();
                    let billing_a = p_a.map(|p| p.billing_mode.clone()).unwrap_or_default();
                    let billing_b = p_b.map(|p| p.billing_mode.clone()).unwrap_or_default();
                    let score_a = EconomyScorer::score_candidate(&pricing_a, billing_a, false, 100_000, 1000);
                    let score_b = EconomyScorer::score_candidate(&pricing_b, billing_b, false, 100_000, 1000);
                    score_a.total_cmp(&score_b)
                });
            }
            GatewayRoutingStrategy::Speed => {
                candidates.sort_by(|a, b| {
                    let metrics_a = self.get_or_create_node_metrics(&a.provider_name);
                    let metrics_b = self.get_or_create_node_metrics(&b.provider_name);
                    let lat_a = SpeedScorer::estimate_total_latency_ms(&metrics_a, 512);
                    let lat_b = SpeedScorer::estimate_total_latency_ms(&metrics_b, 512);
                    lat_a.total_cmp(&lat_b)
                });
            }
            GatewayRoutingStrategy::Balanced => {
                candidates.sort_by(|a, b| {
                    let p_a = self.config.providers.get(&a.provider_name);
                    let p_b = self.config.providers.get(&b.provider_name);
                    let pricing_a = p_a.map(|p| p.pricing()).unwrap_or_default();
                    let pricing_b = p_b.map(|p| p.pricing()).unwrap_or_default();
                    let billing_a = p_a.map(|p| p.billing_mode.clone()).unwrap_or_default();
                    let billing_b = p_b.map(|p| p.billing_mode.clone()).unwrap_or_default();
                    let econ_a = EconomyScorer::score_candidate(&pricing_a, billing_a, false, 100_000, 1000);
                    let econ_b = EconomyScorer::score_candidate(&pricing_b, billing_b, false, 100_000, 1000);

                    let metrics_a = self.get_or_create_node_metrics(&a.provider_name);
                    let metrics_b = self.get_or_create_node_metrics(&b.provider_name);
                    let speed_a = SpeedScorer::estimate_total_latency_ms(&metrics_a, 512);
                    let speed_b = SpeedScorer::estimate_total_latency_ms(&metrics_b, 512);

                    let score_a = econ_a * 0.5 + (speed_a / 100.0) * 0.5;
                    let score_b = econ_b * 0.5 + (speed_b / 100.0) * 0.5;
                    score_a.total_cmp(&score_b)
                });
            }
            _ => {}
        }
        candidates
    }

    /// Return all configured and virtual models: (model_id, provider_name, display_name)
    pub fn list_all_models(&self) -> Vec<(String, String, Option<String>)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. Inject Auto virtual general models
        result.push(("auto".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·主力默认)".to_string())));
        result.push(("auto:standard".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·主力)".to_string())));
        result.push(("auto:flagship".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·旗舰)".to_string())));
        result.push(("auto:economy".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·省钱模式)".to_string())));
        result.push(("auto:fastest".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·极速模式)".to_string())));
        result.push(("auto[1m]".to_string(), "ponyllm".to_string(), Some("PonyLLM Auto (智能总代·1M长上下文)".to_string())));

        seen.insert("auto".to_string());
        seen.insert("auto:standard".to_string());
        seen.insert("auto:flagship".to_string());
        seen.insert("auto:economy".to_string());
        seen.insert("auto:fastest".to_string());
        seen.insert("auto[1m]".to_string());

        // 2. Physical configured models and their [1m] aliases
        for (provider_name, cfg) in &self.config.providers {
            let mut add_model_and_alias = |m: &str| {
                if !seen.contains(m) {
                    result.push((m.to_string(), provider_name.clone(), None));
                    seen.insert(m.to_string());
                }
                let spec = cfg.get_model_spec(m);
                if parse_context_capacity_tokens(&spec.context_window) >= 1048576 {
                    let alias_1m = format!("{}[1m]", m);
                    if !seen.contains(&alias_1m) {
                        result.push((alias_1m.clone(), provider_name.clone(), Some(format!("{} (1M 长上下文)", m))));
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
            if let Some(cfg) = self.config.providers.get(&target.provider_name) {
                return Some((target.provider_name, cfg.clone()));
            }
        }
        None
    }
}

pub fn normalize_chat_completions_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{}/chat/completions", trimmed)
    } else {
        format!("{}/v1/chat/completions", trimmed)
    }
}

pub fn normalize_messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/messages") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{}/messages", trimmed)
    } else {
        format!("{}/v1/messages", trimmed)
    }
}
