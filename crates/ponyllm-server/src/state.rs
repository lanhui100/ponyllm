use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;
use ponyllm_core::pool::KeyPool;
use ponyllm_core::telemetry::{FlightRecorder, MetricsCollector};
use crate::config::{GatewayConfig, ProviderConfig};

#[derive(Debug)]
pub struct AppState {
    pub config: GatewayConfig,
    pub pools: RwLock<HashMap<String, Arc<KeyPool>>>,
    pub flight_recorder: Arc<FlightRecorder>,
    pub metrics: Arc<MetricsCollector>,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> Self {
        let capacity = config.flight_recorder_capacity;
        Self {
            config,
            pools: RwLock::new(HashMap::new()),
            flight_recorder: Arc::new(FlightRecorder::new(capacity)),
            metrics: Arc::new(MetricsCollector::new()),
        }
    }

    pub fn register_pool(&self, provider: &str, pool: Arc<KeyPool>) {
        self.pools.write().insert(provider.to_string(), pool);
    }

    pub fn get_pool(&self, provider: &str) -> Option<Arc<KeyPool>> {
        self.pools.read().get(provider).cloned()
    }

    /// Resolve target upstream provider dynamically based on requested model name
    pub fn resolve_provider(&self, model: &str) -> Option<(String, ProviderConfig)> {
        // 1. Exact match on default_model or configured models list
        for (name, cfg) in &self.config.providers {
            if cfg.default_model == model || cfg.models.iter().any(|m| m == model) {
                return Some((name.clone(), cfg.clone()));
            }
        }

        // 2. Explicit prefix matching (e.g. "deepseek/deepseek-reasoner" or "anthropic/claude-3-7-sonnet")
        if let Some((prefix, _)) = model.split_once('/') {
            if let Some(cfg) = self.config.providers.get(prefix) {
                return Some((prefix.to_string(), cfg.clone()));
            }
        }

        // 3. Keyword / model family heuristic matching
        let lower = model.to_lowercase();
        for (name, cfg) in &self.config.providers {
            if lower.contains(name)
                || (name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("text-embedding")))
                || (name == "anthropic" && lower.starts_with("claude"))
                || (name == "deepseek" && lower.starts_with("deepseek"))
            {
                return Some((name.clone(), cfg.clone()));
            }
        }

        // 4. Fallback to first available provider
        self.config.providers.iter().next().map(|(n, c)| (n.clone(), c.clone()))
    }

    /// Return all unique configured models across all providers: (model_id, provider_name)
    pub fn list_all_models(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for (provider_name, cfg) in &self.config.providers {
            if !cfg.default_model.is_empty() {
                result.push((cfg.default_model.clone(), provider_name.clone()));
            }
            for m in &cfg.models {
                if m != &cfg.default_model && !result.iter().any(|(existing_m, _)| existing_m == m) {
                    result.push((m.clone(), provider_name.clone()));
                }
            }
        }
        result
    }
}
