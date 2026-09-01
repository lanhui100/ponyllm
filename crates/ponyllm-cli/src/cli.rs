use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "ponyllm", author, version, about = "High-performance LLM Unified Gateway and Management Service", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a sample ponyllm.toml configuration file
    Init {
        #[arg(short, long, default_value = "ponyllm.toml")]
        output: String,
    },
    /// Start the ponyllm HTTP/SSE gateway server
    Serve {
        #[arg(short, long)]
        config: Option<String>,
        #[arg(short, long)]
        bind: Option<String>,
        #[arg(short, long)]
        retries: Option<usize>,
    },
    /// Inspect health and live metrics from a running gateway
    Status {
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },
    /// View black-box flight recorder forensic frames
    Telemetry {
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
    },
}
