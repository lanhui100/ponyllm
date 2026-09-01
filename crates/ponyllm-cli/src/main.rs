//! ponyllm CLI binary entry point.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "ponyllm", version, about = "High-performance LLM unified gateway & management service")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the ponyllm gateway server
    Serve {
        #[arg(short, long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
    /// Inspect or manage API key pools
    Keys,
    /// View gateway status and metrics
    Status,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Serve { bind }) => {
            println!("Starting ponyllm gateway on {}", bind);
        }
        Some(Commands::Keys) => {
            println!("Listing configured key pools...");
        }
        Some(Commands::Status) => {
            println!("ponyllm status: OK");
        }
        None => {
            println!("ponyllm gateway CLI. Use --help for commands.");
        }
    }
}
