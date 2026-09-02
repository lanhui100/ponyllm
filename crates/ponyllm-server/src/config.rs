use std::collections::HashMap;
use ponyllm_core::pool::{
    default_cached_price, default_input_price, default_output_price, BillingMode,
    GatewayRoutingStrategy, ModelTier, PricingConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSpec {
    pub name: String,
    #[serde(default)]
    pub tier: ModelTier,
    #[serde(default = "default_context_window")]
    pub context_window: String,
    #[serde(default = "default_max_output")]
    pub max_output: String,
    #[serde(default = "default_modalities")]
    pub input_types: Vec<String>,
    #[serde(default = "default_modalities")]
    pub output_types: Vec<String>,
}

pub fn default_context_window() -> String {
    "128K".to_string()
}
pub fn default_max_output() -> String {
    "4K".to_string()
}
pub fn default_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            tier: ModelTier::Standard,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
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
}

fn default_strategy() -> String {
    "round_robin".to_string()
}

impl ProviderConfig {
    pub fn pricing(&self) -> PricingConfig {
        PricingConfig {
            input_price: self.input_price,
            cached_price: self.cached_price,
            output_price: self.output_price,
        }
    }

    pub fn is_free(&self) -> bool {
        self.pricing().is_free()
    }

    pub fn get_model_spec(&self, model_name: &str) -> ModelSpec {
        if let Some(spec) = self.model_specs.iter().find(|m| m.name == model_name) {
            return spec.clone();
        }
        ModelSpec {
            name: model_name.to_string(),
            tier: ModelTier::Standard,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
        }
    }
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
        }
    }
}
