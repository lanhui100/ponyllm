use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;
use ponyllm_core::pool::KeyPool;
use ponyllm_core::telemetry::{FlightRecorder, MetricsCollector};
use crate::config::GatewayConfig;

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
}
