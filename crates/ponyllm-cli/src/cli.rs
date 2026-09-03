use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ponyllm",
    author,
    version,
    about = "High-performance LLM Unified Gateway, Management Service & TUI Dashboard",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize ponyllm.toml configuration (interactive wizard by default)
    Init {
        /// Output file destination
        #[arg(short, long, default_value = "ponyllm.toml")]
        output: String,

        /// Non-interactive mode (dump sample template directly)
        #[arg(long)]
        non_interactive: bool,
    },

    /// Manage LLM providers (add, list, remove)
    #[command(subcommand)]
    Provider(ProviderCommands),

    /// Manage upstream provider API Key pools (DeepSeek, OpenAI, SenseNova, etc.)
    #[command(subcommand)]
    Key(KeyCommands),

    /// Manage default models
    #[command(subcommand)]
    Model(ModelCommands),

    /// Manage global gateway routing strategy (economy, speed, reliable, balanced)
    #[command(subcommand)]
    Strategy(StrategyCommands),

    /// View, set or rotate gateway access API Token (Gateway API Key)
    Auth {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Custom API key to set, or action ('show', 'rotate')
        #[arg(value_name = "KEY")]
        key: Option<String>,

        /// Rotate and regenerate a new secure random gateway API Key
        #[arg(short, long)]
        rotate: bool,
    },

    /// Launch interactive full-screen TUI terminal dashboard
    #[command(alias = "dashboard", alias = "top")]
    Tui {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Live gateway URL to monitor
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },

    /// Start the ponyllm HTTP/SSE gateway server
    Serve {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Override bind address and port (e.g. 0.0.0.0:8080 or 127.0.0.1:8080)
        #[arg(short = 'b', long)]
        bind: Option<String>,

        /// Override listening address / host (e.g. 0.0.0.0 or 127.0.0.1)
        #[arg(short = 'a', long)]
        address: Option<String>,

        /// Override listening port (e.g. 8080, 9000)
        #[arg(short = 'p', long)]
        port: Option<u16>,

        /// Override gateway access authorization API key / token
        #[arg(long)]
        api_key: Option<String>,

        /// Override maximum retry attempts on transient failures
        #[arg(short, long)]
        retries: Option<usize>,
    },

    /// Inspect health, gateway token, provider pools and live metrics from a running gateway
    Status {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Override target gateway URL (defaults to bind from config, or http://127.0.0.1:8080)
        #[arg(short, long)]
        gateway_url: Option<String>,

        /// Override gateway access authorization API key / token (defaults to api_key from config)
        #[arg(long)]
        api_key: Option<String>,
    },

    /// View black-box flight recorder forensic frames
    Telemetry {
        /// Target gateway URL
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },

    /// Upgrade ponyllm to latest or specified release version
    #[command(alias = "update")]
    Upgrade {
        /// Check for available updates without installing
        #[arg(short, long)]
        check: bool,

        /// Force re-installation even if already up to date
        #[arg(short, long)]
        force: bool,

        /// Dry-run mode: show what would be downloaded without applying changes
        #[arg(long)]
        dry_run: bool,

        /// Target version tag (e.g. v0.2.8, latest)
        #[arg(short, long, value_name = "VERSION")]
        version: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum StrategyCommands {
    /// List all available routing strategies with human-friendly descriptions
    List,
    /// Get current default gateway strategy
    Get {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Set default gateway strategy (economy, speed, reliable, balanced)
    Set {
        /// Strategy name (economy, speed, reliable, balanced, or shorthand e/s/r/b)
        strategy: String,
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// List all configured upstream providers
    List {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add a new provider interactively or via flags
    Add {
        /// Provider name (e.g. openai, deepseek, deepseek-anthropic, anthropic)
        name: String,

        /// Base URL (e.g. https://api.deepseek.com or https://api.deepseek.com/anthropic)
        #[arg(short, long, default_value = "https://api.openai.com")]
        base_url: String,

        /// Default model name (e.g. deepseek-v4-flash, gpt-4o)
        #[arg(short, long, default_value = "gpt-4o")]
        model: String,

        /// Key balancing strategy (priority, round_robin, weighted)
        #[arg(short, long, default_value = "round_robin")]
        strategy: String,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove an existing provider
    Remove {
        /// Provider name to delete
        name: String,

        #[arg(short, long)]
        config: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeyCommands {
    /// List all configured API keys for providers
    List {
        /// Filter keys by provider name
        #[arg(short, long)]
        provider: Option<String>,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add a new API Key to a provider
    Add {
        /// Target provider name
        #[arg(short, long)]
        provider: String,

        /// Unique key identifier / label (e.g. deepseek-primary, key-backup-1)
        #[arg(short, long)]
        id: String,

        /// Raw API Key / secret token (e.g. sk-xxxx)
        #[arg(short, long)]
        key: String,

        /// Key priority (1 = highest, fallback to 2, 3...)
        #[arg(short = 'P', long, default_value_t = 1)]
        priority: u32,

        /// Weight for weighted round-robin
        #[arg(short = 'W', long, default_value_t = 10)]
        weight: u32,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove an API Key from a provider
    Remove {
        /// Target provider name
        #[arg(short, long)]
        provider: String,

        /// Key ID to remove
        #[arg(short, long)]
        id: String,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Test live network connectivity for keys
    Test {
        /// Specific provider to test
        #[arg(short, long)]
        provider: Option<String>,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// View, set or rotate gateway access API Token (Gateway API Key)
    Gateway {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Custom API key to set, or action ('show', 'rotate')
        #[arg(value_name = "KEY")]
        key: Option<String>,

        /// Rotate and regenerate a new secure random gateway API Key
        #[arg(short, long)]
        rotate: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModelCommands {
    /// List all models (default and additional) configured for each provider
    List {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add a new supported model to a provider with parameters
    Add {
        /// Target provider name
        provider: String,

        /// Model name to add (e.g. deepseek-chat, deepseek-reasoner, gpt-4o-mini)
        model: String,

        /// Context window size (e.g. 1M, 128K, 200K)
        #[arg(short = 'w', long, default_value = "1M")]
        context: String,

        /// Maximum output token limit (e.g. 32K, 64K, 8K)
        #[arg(short = 'o', long, default_value = "32K")]
        max_output: String,

        /// Supported input modalities (comma-separated: text,image,video,audio)
        #[arg(short = 'i', long, default_value = "text")]
        inputs: String,

        /// Supported output modalities (comma-separated: text,image,video,audio)
        #[arg(short = 'u', long, default_value = "text")]
        outputs: String,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove a model from a provider
    Remove {
        /// Target provider name
        provider: String,

        /// Model name to remove
        model: String,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Set default model for a provider
    Set {
        /// Target provider name
        provider: String,

        /// New default model name
        model: String,

        #[arg(short, long)]
        config: Option<String>,
    },
}
