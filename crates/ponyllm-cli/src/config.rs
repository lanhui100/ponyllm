use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use ponyllm_core::pool::{
    default_cached_price, default_input_price, default_output_price, BillingMode,
    GatewayRoutingStrategy, ModelTier, PricingConfig,
};
use ponyllm_core::telemetry::FlightRecorder;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default = "default_api_key")]
    pub api_key: String,
    #[serde(default)]
    pub default_strategy: GatewayRoutingStrategy,
    #[serde(default = "default_request_body_limit")]
    pub request_body_limit: usize,
}

pub fn default_request_body_limit() -> usize {
    128 * 1024 * 1024 // 128MB
}

fn default_bind() -> String {
    "127.0.0.1:8080".to_string()
}
fn default_retries() -> usize {
    3
}
fn default_capacity() -> usize {
    200
}
pub fn generate_secure_api_key() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("sk-pony-{}", raw)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayAuthAction {
    Show,
    Rotate,
    Set(String),
    MisdirectedList,
}

pub fn parse_gateway_auth_action(custom_key: Option<&str>, rotate: bool) -> GatewayAuthAction {
    if let Some(k) = custom_key {
        let trimmed = k.trim();
        if trimmed.eq_ignore_ascii_case("list") {
            return GatewayAuthAction::MisdirectedList;
        }
        if trimmed.eq_ignore_ascii_case("show") || trimmed.eq_ignore_ascii_case("get") {
            return GatewayAuthAction::Show;
        }
        if trimmed.eq_ignore_ascii_case("rotate")
            || trimmed.eq_ignore_ascii_case("gen")
            || trimmed.eq_ignore_ascii_case("generate")
        {
            return GatewayAuthAction::Rotate;
        }
        if !trimmed.is_empty() {
            return GatewayAuthAction::Set(trimmed.to_string());
        }
    }
    if rotate {
        GatewayAuthAction::Rotate
    } else {
        GatewayAuthAction::Show
    }
}

fn default_api_key() -> String {
    generate_secure_api_key()
}

impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            max_retries: default_retries(),
            flight_recorder_capacity: default_capacity(),
            api_key: default_api_key(),
            default_strategy: GatewayRoutingStrategy::Economy,
            request_body_limit: default_request_body_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelConfig {
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
    "1M".to_string()
}
pub fn default_max_output() -> String {
    "32K".to_string()
}
pub fn default_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            tier: ModelTier::Flagship,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
        }
    }
}

impl ModelConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tier: ModelTier::Flagship,
            context_window: default_context_window(),
            max_output: default_max_output(),
            input_types: default_modalities(),
            output_types: default_modalities(),
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
    pub billing_mode: BillingMode,
    #[serde(default = "default_input_price")]
    pub input_price: f64,
    #[serde(default = "default_cached_price")]
    pub cached_price: f64,
    #[serde(default = "default_output_price")]
    pub output_price: f64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub keys: Vec<KeySection>,
}

impl ProviderSection {
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
    pub fn get_model_config(&self, model_name: &str) -> ModelConfig {
        if let Some(cfg) = self.model_configs.iter().find(|m| m.name == model_name) {
            return cfg.clone();
        }
        ModelConfig::new(model_name)
    }

    pub fn upsert_model_config(&mut self, cfg: ModelConfig) {
        if let Some(existing) = self.model_configs.iter_mut().find(|m| m.name == cfg.name) {
            *existing = cfg.clone();
        } else {
            self.model_configs.push(cfg.clone());
        }

        if cfg.name != self.default_model && !self.models.contains(&cfg.name) {
            self.models.push(cfg.name);
        }
    }

    pub fn list_all_models(&self) -> Vec<ModelConfig> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if !self.default_model.is_empty() {
            result.push(self.get_model_config(&self.default_model));
            seen.insert(self.default_model.clone());
        }

        for m in &self.models {
            if !seen.contains(m) {
                result.push(self.get_model_config(m));
                seen.insert(m.clone());
            }
        }

        for mc in &self.model_configs {
            if !seen.contains(&mc.name) {
                result.push(mc.clone());
                seen.insert(mc.name.clone());
            }
        }

        result
    }
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

impl ConfigFile {
    pub fn resolve_path(path: Option<&str>) -> std::path::PathBuf {
        ponyllm_core::resolve_config_path(path.map(Path::new))
    }

    pub fn load_or_default(path: Option<&str>) -> Result<Self, Box<dyn std::error::Error>> {
        let resolved = Self::resolve_path(path);
        if resolved.exists() {
            let content = fs::read_to_string(&resolved)?;
            let cfg: ConfigFile = toml::from_str(&content)?;
            Ok(cfg)
        } else if let Some(p) = path {
            Err(format!("指定的配置文件 '{}' 不存在，请检查路径或执行 'ponyllm init' 生成配置", p).into())
        } else {
            let content = generate_sample_config();
            let cfg: ConfigFile = toml::from_str(content)?;
            Ok(cfg)
        }
    }

    /// Save configuration atomically (write to temp file, sync, then rename)
    pub fn save_to_path(&self, path: &str) -> std::io::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let target_path = Path::new(path);
        let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
        let temp_file_name = format!(
            ".{}.tmp.{}",
            target_path.file_name().and_then(|f| f.to_str()).unwrap_or("ponyllm"),
            std::process::id()
        );
        let temp_path = parent.join(temp_file_name);

        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        if let Err(e) = fs::rename(&temp_path, target_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(e);
        }

        Ok(())
    }

    pub fn add_provider(&mut self, name: &str, base_url: &str, default_model: &str, strategy: &str) {
        let entry = self.providers.entry(name.to_string()).or_insert_with(|| ProviderSection {
            base_url: base_url.to_string(),
            default_model: default_model.to_string(),
            strategy: strategy.to_string(),
            billing_mode: BillingMode::Metered,
            input_price: default_input_price(),
            cached_price: default_cached_price(),
            output_price: default_output_price(),
            models: Vec::new(),
            model_configs: Vec::new(),
            keys: Vec::new(),
        });
        entry.base_url = base_url.to_string();
        entry.default_model = default_model.to_string();
        entry.strategy = strategy.to_string();
    }

    pub fn update_provider(&mut self, name: &str, base_url: &str, default_model: &str, strategy: &str) -> Result<(), String> {
        let p = self.providers.get_mut(name)
            .ok_or_else(|| format!("提供商 '{}' 不存在", name))?;
        p.base_url = base_url.to_string();
        p.default_model = default_model.to_string();
        p.strategy = strategy.to_string();
        Ok(())
    }

    pub fn add_model(&mut self, provider: &str, model: &str) -> Result<(), String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在，请先使用 'ponyllm provider add' 添加", provider))?;
        if !p.models.contains(&model.to_string()) && p.default_model != model {
            p.models.push(model.to_string());
        }
        Ok(())
    }

    pub fn upsert_model_config(&mut self, provider: &str, model_cfg: ModelConfig) -> Result<(), String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        p.upsert_model_config(model_cfg);
        Ok(())
    }

    pub fn remove_model(&mut self, provider: &str, model: &str) -> Result<bool, String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        if p.default_model == model {
            return Err(format!("无法直接删除默认主模型 '{}'。若要删除，请先指定其他模型为默认主模型", model));
        }
        let len_models_before = p.models.len();
        p.models.retain(|m| m != model);
        let len_cfgs_before = p.model_configs.len();
        p.model_configs.retain(|m| m.name != model);
        Ok(p.models.len() < len_models_before || p.model_configs.len() < len_cfgs_before)
    }

    pub fn set_default_model(&mut self, provider: &str, model: &str) -> Result<(), String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        let old_default = std::mem::replace(&mut p.default_model, model.to_string());
        if !old_default.is_empty() && old_default != model && !p.models.contains(&old_default) {
            p.models.push(old_default);
        }
        p.models.retain(|m| m != model);
        Ok(())
    }

    pub fn remove_provider(&mut self, name: &str) -> bool {
        self.providers.remove(name).is_some()
    }

    pub fn add_key(&mut self, provider: &str, id: &str, api_key: &str, priority: u32, weight: u32) -> Result<(), String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在，请先使用 'ponyllm provider add' 添加", provider))?;
        
        if let Some(existing) = p.keys.iter_mut().find(|k| k.id == id) {
            existing.api_key = api_key.to_string();
            existing.priority = priority;
            existing.weight = weight;
        } else {
            p.keys.push(KeySection {
                id: id.to_string(),
                api_key: api_key.to_string(),
                priority,
                weight,
            });
        }
        Ok(())
    }

    pub fn remove_key(&mut self, provider: &str, id: &str) -> Result<bool, String> {
        let p = self.providers.get_mut(provider)
            .ok_or_else(|| format!("提供商 '{}' 不存在", provider))?;
        let len_before = p.keys.len();
        p.keys.retain(|k| k.id != id);
        Ok(p.keys.len() < len_before)
    }

    pub fn mask_key(api_key: &str) -> String {
        FlightRecorder::sanitize_key(api_key)
    }
}

pub fn generate_sample_config() -> &'static str {
    r#"# ponyllm Unified Gateway Configuration

[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200

# DeepSeek Provider (OpenAI 协议: /v1/chat/completions, /v1/responses)
[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-v4-flash"
strategy = "priority"
keys = [
    { id = "deepseek-primary", api_key = "sk-xxxx", priority = 1, weight = 10 },
]

# DeepSeek Provider (Anthropic Messages 协议: /v1/messages)
[providers.deepseek-anthropic]
base_url = "https://api.deepseek.com/anthropic"
default_model = "deepseek-v4-flash"
strategy = "priority"
keys = [
    { id = "ds-ant-primary", api_key = "sk-xxxx", priority = 1, weight = 10 },
]

# OpenAI Provider
[providers.openai]
base_url = "https://api.openai.com"
default_model = "gpt-4o"
strategy = "round_robin"
keys = [
    { id = "openai-main", api_key = "sk-proj-xxxx", priority = 1, weight = 10 },
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
