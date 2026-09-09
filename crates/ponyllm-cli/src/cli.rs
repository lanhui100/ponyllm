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

    /// Manage LLM providers (add, list, remove; use 'add agy' for Antigravity Google OAuth)
    #[command(subcommand)]
    Provider(ProviderCommands),

    /// Manage upstream provider API Key pools (DeepSeek, OpenAI, Antigravity, etc.)
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

        /// Disable web console hosting (`/app/*`); gateway forwarding unaffected
        #[arg(long)]
        no_web: bool,

        /// Override web console dist directory (default `web/dist`, relative to
        /// the serve working directory; absolute paths preferred for services)
        #[arg(long)]
        web_dist_dir: Option<String>,

        /// Enable verbose debug logging across server, core, and protocol translators
        #[arg(short = 'd', long)]
        debug: bool,
    },

    /// Start gateway service with Web Console focused (default port: 18080)
    #[command(alias = "ui", alias = "console")]
    Web {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Override listening port (default: 18080, non-standard high port to avoid conflicts)
        #[arg(short = 'p', long, default_value_t = 18080)]
        port: u16,

        /// Override listening address / host (default: 127.0.0.1)
        #[arg(short = 'a', long, default_value = "127.0.0.1")]
        address: String,

        /// Override bind address and port directly (e.g. 127.0.0.1:18080)
        #[arg(short = 'b', long)]
        bind: Option<String>,

        /// Override gateway access authorization API key / token
        #[arg(long)]
        api_key: Option<String>,

        /// Override web console dist directory (default `web/dist`)
        #[arg(long)]
        web_dist_dir: Option<String>,

        /// Do not automatically open web console in default browser
        #[arg(long)]
        no_open: bool,

        /// Automatically open web console in default browser (kept for backwards compatibility)
        #[arg(long, conflicts_with = "no_open", hide = true)]
        open: bool,

        /// Enable verbose debug logging across server, core, and protocol translators
        #[arg(short = 'd', long)]
        debug: bool,
    },

    /// Stop the gateway process associated with the configuration file (pidfile)
    Stop {
        /// Path to configuration file (must match the one `serve` was started with)
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Restart the gateway: stop the old process and launch a new one in background
    Restart {
        /// Path to configuration file (must match the one `serve` was started with)
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

        /// Disable web console hosting (`/app/*`); gateway forwarding unaffected
        #[arg(long)]
        no_web: bool,

        /// Override web console dist directory (default `web/dist`)
        #[arg(long)]
        web_dist_dir: Option<String>,
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
    /// Add a new provider interactively or via flags (use 'agy' or 'antigravity' for interactive OAuth2 authorization)
    Add {
        /// Provider name (e.g. openai, deepseek, anthropic, or 'agy'/'antigravity' for Google OAuth)
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

        /// Billing mode: 'metered' (default) or 'plan' (periodic quota)
        #[arg(long, default_value = "metered")]
        billing_mode: String,
        /// Regular input price in USD per 1M tokens
        #[arg(long, default_value_t = 0.50)]
        input_price: f64,

        /// Cached input price in USD per 1M tokens
        #[arg(long, default_value_t = 0.25)]
        cached_price: f64,

        /// Output generation price in USD per 1M tokens
        #[arg(long, default_value_t = 1.00)]
        output_price: f64,

        /// Default native wire protocol: chat, responses, anthropic, antigravity (unset = legacy URL heuristic)
        #[arg(long)]
        default_protocol: Option<ponyllm_core::pool::UpstreamProtocol>,

        /// Per-protocol endpoint base override for OpenAI Chat (unset = derive from base URL)
        #[arg(long)]
        chat_url: Option<String>,

        /// Per-protocol endpoint base override for OpenAI Responses
        #[arg(long)]
        responses_url: Option<String>,

        /// Per-protocol endpoint base override for Anthropic Messages
        #[arg(long)]
        messages_url: Option<String>,

        /// Outbound HTTP proxy (URL, 'auto' to detect system proxy, 'none' to force direct)
        #[arg(long)]
        proxy: Option<String>,

        /// Optional key identifier / label for OAuth providers (defaults to Google account email)
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// Key priority for provider connection pool (1 = highest, fallback to 2, 3...)
        #[arg(short = 'P', long, default_value_t = 1)]
        priority: u32,

        /// Key weight for weighted round-robin
        #[arg(short = 'W', long, default_value_t = 10)]
        weight: u32,

        /// Local callback redirect port for OAuth authorization (default: 51121)
        #[arg(long, default_value_t = ponyllm_core::pool::DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT)]
        port: u16,

        /// Do not attempt to open browser automatically during OAuth
        #[arg(long)]
        no_browser: bool,

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
    /// Interactively authorize and add an upstream account (e.g. Antigravity 'agy') via OAuth2
    #[command(alias = "auth-agy", alias = "agy-auth")]
    Auth {
        /// Target provider identifier ('agy' or 'antigravity')
        #[arg(default_value = "agy")]
        provider: String,

        /// Optional key identifier / label (defaults to Google account email)
        #[arg(value_name = "ID")]
        id: Option<String>,

        /// Key priority (1 = highest, fallback to 2, 3...)
        #[arg(short = 'P', long, default_value_t = 1)]
        priority: u32,

        /// Weight for weighted round-robin
        #[arg(short = 'W', long, default_value_t = 10)]
        weight: u32,

        /// Local callback redirect port (default: 51121)
        #[arg(long, default_value_t = ponyllm_core::pool::DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT)]
        port: u16,

        /// Do not attempt to open browser automatically
        #[arg(long)]
        no_browser: bool,

        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,
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

        /// Maximum output token limit (e.g. 32K, 64K, 4K)
        #[arg(short = 'o', long, default_value = "32K")]
        max_output: String,

        /// Supported input modalities (comma-separated: text,image,video,audio)
        #[arg(short = 'i', long, default_value = "text")]
        inputs: String,

        /// Supported output modalities (comma-separated: text,image,video,audio)
        #[arg(short = 'u', long, default_value = "text")]
        outputs: String,

        /// Capability tier: Flagship (F), Standard (S), Light (L)
        #[arg(short = 't', long, default_value = "Standard")]
        tier: String,

        /// Optional model-specific input price in USD per 1M tokens (overrides provider price)
        #[arg(long)]
        input_price: Option<f64>,

        /// Optional model-specific cached input price in USD per 1M tokens (overrides provider price)
        #[arg(long)]
        cached_price: Option<f64>,

        /// Optional model-specific output generation price in USD per 1M tokens (overrides provider price)
        #[arg(long)]
        output_price: Option<f64>,

        /// Billing mode override: metered, plan (coding plan subscription), free (defaults to inherit provider mode)
        #[arg(short = 'b', long)]
        billing_mode: Option<String>,

        /// Native wire protocol override: chat, responses, anthropic (defaults to inherit provider default)
        #[arg(long)]
        protocol: Option<ponyllm_core::pool::UpstreamProtocol>,

        /// Optional proxy override: URL, 'auto' to detect system proxy, 'direct'/'none' to force direct, or omit to inherit provider
        #[arg(long)]
        proxy: Option<String>,

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

/// Formats the Web Console URL or disabled indicator for status inspection display.
///
/// If `web_enabled` is false, returns a disabled indicator string.
/// If `api_key` is non-empty and not "none", returns `{base_url}/?token={api_key}`.
/// Otherwise returns `{base_url}/`.
pub fn format_web_status_url(base_url: &str, web_enabled: bool, api_key: &str) -> String {
    if !web_enabled {
        return "已关闭 (web_enabled = false)".to_string();
    }
    let trimmed = base_url.trim_end_matches('/');
    if !api_key.is_empty() && !api_key.eq_ignore_ascii_case("none") {
        format!("{}/?token={}", trimmed, api_key)
    } else {
        format!("{}/", trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_web_status_url() {
        // 1. With valid token
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080", true, "sk-pony-test-123"),
            "http://127.0.0.1:8080/?token=sk-pony-test-123"
        );
        // Trims trailing slash from base url
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080/", true, "sk-pony-test-123"),
            "http://127.0.0.1:8080/?token=sk-pony-test-123"
        );

        // 2. Without token (empty or case-insensitive "none")
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080", true, ""),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080", true, "none"),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080", true, "NONE"),
            "http://127.0.0.1:8080/"
        );

        // 3. Web disabled
        assert_eq!(
            format_web_status_url("http://127.0.0.1:8080", false, "sk-pony-test-123"),
            "已关闭 (web_enabled = false)"
        );
    }

    #[test]
    fn test_web_subcommand_defaults() {
        let cli = Cli::parse_from(["ponyllm", "web"]);
        match cli.command {
            Commands::Web { port, address, no_open, .. } => {
                assert_eq!(port, 18080);
                assert_eq!(address, "127.0.0.1");
                assert!(!no_open);
            }
            _ => panic!("expected Commands::Web"),
        }
    }

    #[test]
    fn test_web_subcommand_custom_port() {
        let cli = Cli::parse_from(["ponyllm", "web", "--port", "19090", "-a", "0.0.0.0"]);
        match cli.command {
            Commands::Web { port, address, .. } => {
                assert_eq!(port, 19090);
                assert_eq!(address, "0.0.0.0");
            }
            _ => panic!("expected Commands::Web"),
        }
    }

    #[test]
    fn test_web_subcommand_aliases_and_open() {
        let cli = Cli::parse_from(["ponyllm", "ui", "-p", "9999", "--open"]);
        match cli.command {
            Commands::Web { port, open, .. } => {
                assert_eq!(port, 9999);
                assert!(open);
            }
            _ => panic!("expected Commands::Web"),
        }
    }
}
