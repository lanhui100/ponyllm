use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub gateway: GatewaySection,
    #[serde(default)]
    pub providers: HashMap<String, ProviderSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySection {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_retries")]
    pub max_retries: usize,
    #[serde(default = "default_capacity")]
    pub flight_recorder_capacity: usize,
}

fn default_bind() -> String {
    "127.0.0.1:8080".to_string()
}
fn default_retries() -> usize {
    3
}
fn default_capacity() -> usize {
    100
}

impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            max_retries: default_retries(),
            flight_recorder_capacity: default_capacity(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSection {
    pub base_url: String,
    pub default_model: String,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub keys: Vec<KeySection>,
}

fn default_strategy() -> String {
    "round_robin".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySection {
    pub id: String,
    pub api_key: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_priority() -> u32 {
    1
}
fn default_weight() -> u32 {
    10
}

pub fn generate_sample_config() -> &'static str {
    r#"# ponyllm Unified Gateway Configuration

[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200

# OpenAI Provider
[providers.openai]
base_url = "https://api.openai.com"
default_model = "gpt-4o"
strategy = "round_robin"
keys = [
    { id = "openai-main", api_key = "sk-proj-xxxx", priority = 1, weight = 10 },
    { id = "openai-backup", api_key = "sk-proj-yyyy", priority = 2, weight = 5 },
]

# DeepSeek Provider (supports Chat, Anthropic Messages and Responses API)
[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-reasoner"
strategy = "priority"
keys = [
    { id = "deepseek-1", api_key = "sk-xxxx", priority = 1, weight = 10 },
]

# Anthropic Provider
[providers.anthropic]
base_url = "https://api.anthropic.com"
default_model = "claude-3-7-sonnet-20250219"
strategy = "round_robin"
keys = [
    { id = "anthropic-1", api_key = "sk-ant-xxxx", priority = 1, weight = 10 },
]
"#
}
