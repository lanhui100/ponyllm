use std::collections::HashMap;
use ponyllm_core::pool::{
    default_cached_price, default_input_price, default_output_price, BillingMode,
    GatewayRoutingStrategy, ModelTier, PricingConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_mode: Option<BillingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price: Option<f64>,
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
            billing_mode: None,
            input_price: None,
            cached_price: None,
            output_price: None,
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

            PricingConfig {
                input_price: in_p,
                cached_price: ca_p,
                output_price: out_p,
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
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
            billing_mode: None,
            input_price: None,
            cached_price: None,
            output_price: None,
        }
    }
}

pub fn default_request_body_limit() -> usize {
    128 * 1024 * 1024 // 128MB default for 1M context / multimodal payloads
}

fn default_event_log_retention_days() -> u64 {
    7
}

fn default_event_log_max_bytes() -> u64 {
    512 * 1024 * 1024 // 512MB ring of hourly JSONL segments
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
            request_body_limit: default_request_body_limit(),
            event_log_dir: None,
            event_log_retention_days: default_event_log_retention_days(),
            event_log_max_bytes: default_event_log_max_bytes(),
        }
    }
}

