use std::fs;
use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use ponyllm_cli::cli::{Cli, Commands};
use ponyllm_cli::config::{generate_sample_config, ConfigFile};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ponyllm=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { output } => {
            fs::write(&output, generate_sample_config())?;
            println!(" Successfully initialized sample configuration at '{}'", output);
        }
        Commands::Serve { config, bind, retries } => {
            let config_file = if let Some(path) = config {
                let content = fs::read_to_string(&path)?;
                toml::from_str::<ConfigFile>(&content)?
            } else if std::path::Path::new("ponyllm.toml").exists() {
                let content = fs::read_to_string("ponyllm.toml")?;
                toml::from_str::<ConfigFile>(&content)?
            } else {
                println!(" No configuration file specified or found. Using defaults.");
                toml::from_str::<ConfigFile>(generate_sample_config())?
            };

            let mut gw_config = GatewayConfig::default();
            if let Some(b) = bind {
                gw_config.bind_addr = b;
            } else {
                gw_config.bind_addr = config_file.gateway.bind;
            }

            if let Some(r) = retries {
                gw_config.max_retries = r;
            } else {
                gw_config.max_retries = config_file.gateway.max_retries;
            }

            gw_config.flight_recorder_capacity = config_file.gateway.flight_recorder_capacity;

            for (p_name, p_sec) in &config_file.providers {
                gw_config.providers.insert(
                    p_name.clone(),
                    ProviderConfig {
                        base_url: p_sec.base_url.clone(),
                        default_model: p_sec.default_model.clone(),
                    },
                );
            }

            let state = Arc::new(AppState::new(gw_config.clone()));

            for (p_name, p_sec) in &config_file.providers {
                let strat = match p_sec.strategy.as_str() {
                    "priority" => RoutingStrategy::Priority,
                    "weighted" => RoutingStrategy::WeightedRoundRobin,
                    _ => RoutingStrategy::RoundRobin,
                };
                let pool = Arc::new(KeyPool::new(p_name, strat));
                for k in &p_sec.keys {
                    pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
                }
                state.register_pool(p_name, pool);
            }

            let app = create_app(state);
            let listener = tokio::net::TcpListener::bind(&gw_config.bind_addr).await?;
            println!(" ponyllm gateway listening on http://{}", gw_config.bind_addr);
            println!(" Active endpoints: /v1/chat/completions, /v1/messages, /v1/responses");
            println!(" Telemetry: /health, /v1/telemetry/metrics, /v1/telemetry/recorder");

            axum::serve(listener, app).await?;
        }
        Commands::Status { gateway_url } => {
            let client = reqwest::Client::new();
            let health_url = format!("{}/health", gateway_url.trim_end_matches('/'));
            let metrics_url = format!("{}/v1/telemetry/metrics", gateway_url.trim_end_matches('/'));

            let health = client.get(&health_url).send().await?.json::<serde_json::Value>().await?;
            let metrics = client.get(&metrics_url).send().await?.json::<serde_json::Value>().await?;

            println!("=== ponyllm Gateway Status ===");
            println!("Health:  {}", health);
            println!("Metrics: {}", metrics);
        }
        Commands::Telemetry { gateway_url } => {
            let client = reqwest::Client::new();
            let rec_url = format!("{}/v1/telemetry/recorder", gateway_url.trim_end_matches('/'));
            let frames = client.get(&rec_url).send().await?.json::<serde_json::Value>().await?;

            println!("=== ponyllm Flight Recorder Frames ===");
            println!("{}", serde_json::to_string_pretty(&frames)?);
        }
    }

    Ok(())
}
