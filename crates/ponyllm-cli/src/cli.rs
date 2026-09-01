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

    /// Manage API Key pools and perform live health checks
    #[command(subcommand)]
    Key(KeyCommands),

    /// Manage default models
    #[command(subcommand)]
    Model(ModelCommands),

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

        /// Override bind host and port (e.g. 127.0.0.1:8080)
        #[arg(short, long)]
        bind: Option<String>,

        /// Override maximum retry attempts on transient failures
        #[arg(short, long)]
        retries: Option<usize>,
    },

    /// Inspect health and live metrics from a running gateway
    Status {
        /// Target gateway URL
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },

    /// View black-box flight recorder forensic frames
    Telemetry {
        /// Target gateway URL
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// List all configured providers
    List {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add or update a model provider
    Add {
        /// Unique provider name (e.g., deepseek, openai, anthropic)
        name: String,

        /// Upstream Base URL (e.g., https://api.deepseek.com)
        #[arg(short, long)]
        base_url: String,

        /// Default model name
        #[arg(short, long)]
        model: String,

        /// Routing strategy (round_robin, priority, weighted)
        #[arg(short, long, default_value = "round_robin")]
        strategy: String,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove a model provider
    Remove {
        /// Provider name to delete
        name: String,

        #[arg(short, long)]
        config: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeyCommands {
    /// List API keys across providers (keys are automatically masked)
    List {
        /// Filter by specific provider
        #[arg(short, long)]
        provider: Option<String>,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add or update an API key in a provider's key pool
    Add {
        /// Target provider name
        #[arg(short = 'P', long)]
        provider: String,

        /// Unique Key identifier ID
        #[arg(short = 'i', long)]
        id: String,

        /// Secret API Key string
        #[arg(short = 'k', long)]
        key: String,

        /// Failover priority (1 = highest primary)
        #[arg(short = 'p', long, default_value_t = 1)]
        priority: u32,

        /// Weighted round-robin weight
        #[arg(short = 'w', long, default_value_t = 10)]
        weight: u32,

        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove an API key from a provider's key pool
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
}

#[derive(Debug, Subcommand)]
pub enum ModelCommands {
    /// List default models configured for each provider
    List {
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
