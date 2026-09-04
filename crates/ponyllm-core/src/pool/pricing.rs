use serde::{Deserialize, Serialize};

/// Pricing specification for a model provider (Units: USD per 1 Million Tokens)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricingConfig {
    /// Regular input price per 1M tokens (cache miss)
    #[serde(default = "default_input_price")]
    pub input_price: f64,

    /// Cached input price per 1M tokens (cache hit read, usually 10%~50% of input_price)
    #[serde(default = "default_cached_price")]
    pub cached_price: f64,

    /// Output generation price per 1M tokens
    #[serde(default = "default_output_price")]
    pub output_price: f64,
}

pub fn default_input_price() -> f64 {
    0.50
}

pub fn default_cached_price() -> f64 {
    0.25
}

pub fn default_output_price() -> f64 {
    1.00
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            input_price: default_input_price(),
            cached_price: default_cached_price(),
            output_price: default_output_price(),
        }
    }
}

impl PricingConfig {
    /// Check if this provider is genuinely free (prices explicitly 0.0 with epsilon precision)
    pub fn is_free(&self) -> bool {
        self.input_price.abs() < 1e-6 && self.cached_price.abs() < 1e-6 && self.output_price.abs() < 1e-6
    }

    /// Estimate total cost for given input tokens, cached state, and expected output tokens
    pub fn estimate_cost(&self, input_tokens: usize, is_cached: bool, expected_output_tokens: usize) -> f64 {
        if self.is_free() {
            return 0.0;
        }
        let in_rate = if is_cached { self.cached_price.max(0.0) } else { self.input_price.max(0.0) };
        let out_rate = self.output_price.max(0.0);
        let in_cost = (input_tokens as f64 / 1_000_000.0) * in_rate;
        let out_cost = (expected_output_tokens as f64 / 1_000_000.0) * out_rate;
        in_cost + out_cost
    }
}

/// Provider billing model: Metered (Pay-as-you-go) or Plan (Periodic fixed quota)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    #[default]
    Metered,
    Plan,
    Free,
}
