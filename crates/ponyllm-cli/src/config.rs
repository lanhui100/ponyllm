use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
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

impl ConfigFile {
    pub fn load_or_default(path: Option<&str>) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(p) = path {
            if !Path::new(p).exists() {
                return Err(format!("指定的配置文件 '{}' 不存在，请检查路径或执行 'ponyllm init' 生成配置", p).into());
            }
            let content = fs::read_to_string(p)?;
            let cfg: ConfigFile = toml::from_str(&content)?;
            Ok(cfg)
        } else if Path::new("ponyllm.toml").exists() {
            let content = fs::read_to_string("ponyllm.toml")?;
            let cfg: ConfigFile = toml::from_str(&content)?;
            Ok(cfg)
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
            keys: Vec::new(),
        });
        entry.base_url = base_url.to_string();
        entry.default_model = default_model.to_string();
        entry.strategy = strategy.to_string();
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
