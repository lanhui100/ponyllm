use std::fs;
use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use ponyllm_cli::cli::{Cli, Commands, KeyCommands, ModelCommands, ProviderCommands};
use ponyllm_cli::config::{generate_sample_config, ConfigFile};
use ponyllm_cli::wizard::run_interactive_init;
use ponyllm_cli::tui::run_tui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { output, non_interactive } => {
            if non_interactive {
                fs::write(&output, generate_sample_config())?;
                println!("✅ 已成功以静默模式写入默认配置至 '{}'", output);
            } else {
                run_interactive_init(&output)?;
            }
        }
        Commands::Provider(cmd) => match cmd {
            ProviderCommands::List { config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                println!("=== 已配置的模型提供商 (共 {} 个) ===", cfg.providers.len());
                println!("{:<15} {:<32} {:<28} {:<12} {:<8}", "提供商名称", "Base URL", "默认模型", "调度策略", "Key 数量");
                println!("{}", "-".repeat(95));
                for (name, p) in &cfg.providers {
                    println!(
                        "{:<15} {:<32} {:<28} {:<12} {:<8}",
                        name, p.base_url, p.default_model, p.strategy, p.keys.len()
                    );
                }
            }
            ProviderCommands::Add { name, base_url, model, strategy, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| std::path::Path::new(path).exists()))
                    .unwrap_or_default();
                cfg.add_provider(&name, &base_url, &model, &strategy);
                cfg.save_to_path(path)?;
                println!("✅ 成功添加/更新提供商 '{}' (Base URL: {}, Model: {})", name, base_url, model);
            }
            ProviderCommands::Remove { name, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if cfg.remove_provider(&name) {
                    cfg.save_to_path(path)?;
                    println!("✅ 成功删除提供商 '{}'", name);
                } else {
                    println!("⚠️ 未找到提供商 '{}'", name);
                }
            }
        },
        Commands::Key(cmd) => match cmd {
            KeyCommands::List { provider, config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                println!("=== API Key 账户池 ===");
                println!("{:<15} {:<20} {:<25} {:<8} {:<8}", "所属提供商", "Key ID", "API Key (已脱敏)", "优先级", "权重");
                println!("{}", "-".repeat(80));
                for (p_name, p) in &cfg.providers {
                    if let Some(target_p) = &provider {
                        if p_name != target_p {
                            continue;
                        }
                    }
                    for k in &p.keys {
                        println!(
                            "{:<15} {:<20} {:<25} {:<8} {:<8}",
                            p_name, k.id, ConfigFile::mask_key(&k.api_key), k.priority, k.weight
                        );
                    }
                }
            }
            KeyCommands::Add { provider, id, key, priority, weight, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| std::path::Path::new(path).exists()))
                    .unwrap_or_default();
                cfg.add_key(&provider, &id, &key, priority, weight)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                cfg.save_to_path(path)?;
                println!("✅ 成功向提供商 '{}' 账户池添加/更新 Key '{}' (优先级: {}, 权重: {})", provider, id, priority, weight);
            }
            KeyCommands::Remove { provider, id, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                match cfg.remove_key(&provider, &id) {
                    Ok(true) => {
                        cfg.save_to_path(path)?;
                        println!("✅ 成功从提供商 '{}' 中删除 Key '{}'", provider, id);
                    }
                    Ok(false) => {
                        println!("⚠️ 在提供商 '{}' 中未找到 Key '{}'", provider, id);
                    }
                    Err(e) => {
                        println!("❌ 错误: {}", e);
                    }
                }
            }
            KeyCommands::Test { provider, config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()?;

                println!("=== 开始拨测 Key 连通性 ===");
                for (p_name, p) in &cfg.providers {
                    if let Some(target_p) = &provider {
                        if p_name != target_p {
                            continue;
                        }
                    }
                    println!("\n▶ 正在测试提供商 '{}' (Base URL: {})...", p_name, p.base_url);
                    for k in &p.keys {
                        print!("  • 测试 Key '{}' ({})... ", k.id, ConfigFile::mask_key(&k.api_key));
                        let start = std::time::Instant::now();
                        let test_url = format!("{}/v1/models", p.base_url.trim_end_matches('/'));
                        
                        let req = client.get(&test_url)
                            .header("Authorization", format!("Bearer {}", k.api_key.trim()))
                            .header("x-api-key", k.api_key.trim())
                            .header("anthropic-version", "2023-06-01");

                        let res = req.send().await;

                        let elapsed = start.elapsed().as_millis();
                        match res {
                            Ok(resp) if resp.status().is_success() => {
                                println!("🟢 连通正常 (HTTP {}, {} ms)", resp.status(), elapsed);
                            }
                            Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
                                println!("🔴 认证失败 (HTTP {}, 无效的 API Key, {} ms)", resp.status(), elapsed);
                            }
                            Ok(resp) => {
                                println!("🟡 响应异常 (HTTP {}, {} ms)", resp.status(), elapsed);
                            }
                            Err(e) => {
                                println!("🔴 网络连接失败 ({})", e);
                            }
                        }
                    }
                }
            }
        },
        Commands::Model(cmd) => match cmd {
            ModelCommands::List { config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                println!("=== 各提供商默认模型 ===");
                println!("{:<15} {:<35}", "提供商", "默认模型 (Default Model)");
                println!("{}", "-".repeat(50));
                for (name, p) in &cfg.providers {
                    println!("{:<15} {:<35}", name, p.default_model);
                }
            }
            ModelCommands::Set { provider, model, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if let Some(p) = cfg.providers.get_mut(&provider) {
                    p.default_model = model.clone();
                    cfg.save_to_path(path)?;
                    println!("✅ 成功将提供商 '{}' 的默认模型更新为 '{}'", provider, model);
                } else {
                    println!("⚠️ 未找到提供商 '{}'", provider);
                }
            }
        },
        Commands::Tui { config, gateway_url } => {
            let config_path = config.unwrap_or_else(|| "ponyllm.toml".to_string());
            let cfg = ConfigFile::load_or_default(Some(&config_path))?;
            run_tui(cfg, config_path, gateway_url).await?;
        }
        Commands::Serve { config, bind, retries } => {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "ponyllm=info,tower_http=info".into()),
                )
                .with(tracing_subscriber::fmt::layer())
                .init();

            let config_path = config.as_deref();
            let config_file = ConfigFile::load_or_default(config_path)?;

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
            println!("🚀 ponyllm gateway listening on http://{}", gw_config.bind_addr);
            println!("📡 Active endpoints: /v1/chat/completions, /v1/messages, /v1/responses");
            println!("📈 Telemetry: /health, /v1/telemetry/metrics, /v1/telemetry/recorder");

            axum::serve(listener, app).await?;
        }
        Commands::Status { gateway_url } => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()?;
            let health_url = format!("{}/health", gateway_url.trim_end_matches('/'));
            let metrics_url = format!("{}/v1/telemetry/metrics", gateway_url.trim_end_matches('/'));

            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let health = resp.json::<serde_json::Value>().await?;
                    let metrics = client.get(&metrics_url).send().await?.json::<serde_json::Value>().await?;
                    println!("=== ponyllm Gateway Status ===");
                    println!("Health:  {}", health);
                    println!("Metrics: {}", metrics);
                }
                Ok(resp) => {
                    eprintln!("⚠️ 网关响应非 200 状态码: HTTP {}", resp.status());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("❌ 无法连接到 ponyllm 网关 ({})。\n👉 排错提示: 请先运行 'ponyllm serve' 启动服务，或检查 '--gateway-url' 是否正确。\n(底层错误: {})", gateway_url, e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Telemetry { gateway_url } => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()?;
            let rec_url = format!("{}/v1/telemetry/recorder", gateway_url.trim_end_matches('/'));

            match client.get(&rec_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let frames = resp.json::<serde_json::Value>().await?;
                    println!("=== ponyllm Flight Recorder Frames ===");
                    println!("{}", serde_json::to_string_pretty(&frames)?);
                }
                Ok(resp) => {
                    eprintln!("⚠️ 网关响应非 200 状态码: HTTP {}", resp.status());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("❌ 无法连接到 ponyllm 网关 ({})。\n👉 排错提示: 请先运行 'ponyllm serve' 启动服务。\n(底层错误: {})", gateway_url, e);
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}
