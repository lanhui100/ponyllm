use std::fs;
use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use ponyllm_cli::cli::{Cli, Commands, KeyCommands, ModelCommands, ProviderCommands};
use ponyllm_cli::config::{generate_sample_config, generate_secure_api_key, ConfigFile};
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
            KeyCommands::Gateway { config, key } => {
                handle_manage_gateway_auth(config.as_deref(), key)?;
            }
        },
        Commands::Model(cmd) => match cmd {
            ModelCommands::List { config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                println!("=== 已配置模型清单与参数规格 (Configured Models & Specs) ===");
                println!("{:<16} {:<24} {:<8} {:<10} {:<10} {:<18} {:<18}", "提供商", "模型名称", "默认", "上下文", "最大输出", "输入模态", "输出模态");
                println!("{}", "-".repeat(108));
                for (name, p) in &cfg.providers {
                    let all_models = p.list_all_models();
                    for m in all_models {
                        let is_def = if m.name == p.default_model { "★ 是" } else { "否" };
                        let in_str = m.input_types.join(",");
                        let out_str = m.output_types.join(",");
                        println!(
                            "{:<16} {:<24} {:<8} {:<10} {:<10} {:<18} {:<18}",
                            name, m.name, is_def, m.context_window, m.max_output, in_str, out_str
                        );
                    }
                }
            }
            ModelCommands::Add { provider, model, context, max_output, inputs, outputs, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                let in_list: Vec<String> = inputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let out_list: Vec<String> = outputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

                const VALID_MODALITIES: [&str; 4] = ["text", "image", "video", "audio"];
                for inp in &in_list {
                    if !VALID_MODALITIES.contains(&inp.as_str()) {
                        eprintln!("❌ 非法输入模态 '{}'，有效模态包括: text, image, video, audio", inp);
                        return Ok(());
                    }
                }
                for out in &out_list {
                    if !VALID_MODALITIES.contains(&out.as_str()) {
                        eprintln!("❌ 非法输出模态 '{}'，有效模态包括: text, image, video, audio", out);
                        return Ok(());
                    }
                }

                let model_cfg = ponyllm_cli::config::ModelConfig {
                    name: model.clone(),
                    context_window: context.clone(),
                    max_output: max_output.clone(),
                    input_types: if in_list.is_empty() { vec!["text".to_string()] } else { in_list },
                    output_types: if out_list.is_empty() { vec!["text".to_string()] } else { out_list },
                };

                match cfg.upsert_model_config(&provider, model_cfg) {
                    Ok(()) => {
                        cfg.save_to_path(path)?;
                        println!("✅ 成功为提供商 '{}' 添加/更新模型 '{}' (上下文: {}, 最大输出: {})", provider, model, context, max_output);
                    }
                    Err(e) => {
                        println!("❌ 添加失败: {}", e);
                    }
                }
            }
            ModelCommands::Remove { provider, model, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                match cfg.remove_model(&provider, &model) {
                    Ok(true) => {
                        cfg.save_to_path(path)?;
                        println!("✅ 成功从提供商 '{}' 中移除模型 '{}'", provider, model);
                    }
                    Ok(false) => {
                        println!("⚠️ 提供商 '{}' 中未找到附加模型 '{}' (注意: 默认模型请使用 'ponyllm model set' 修改)", provider, model);
                    }
                    Err(e) => {
                        println!("❌ 移除失败: {}", e);
                    }
                }
            }
            ModelCommands::Set { provider, model, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                match cfg.set_default_model(&provider, &model) {
                    Ok(()) => {
                        cfg.save_to_path(path)?;
                        println!("✅ 成功将提供商 '{}' 的默认主模型更新为 '{}'", provider, model);
                    }
                    Err(e) => {
                        println!("❌ 设置失败: {}", e);
                    }
                }
            }
        },
        Commands::Auth { config, key } => {
            handle_manage_gateway_auth(config.as_deref(), key)?;
        }
        Commands::Tui { config, gateway_url } => {
            let config_path = config.unwrap_or_else(|| "ponyllm.toml".to_string());
            let cfg = ConfigFile::load_or_default(Some(&config_path))?;
            run_tui(cfg, config_path, gateway_url).await?;
        }
        Commands::Serve { config, bind, address, port, api_key, retries } => {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "ponyllm=info,tower_http=info".into()),
                )
                .with(tracing_subscriber::fmt::layer())
                .init();

            let config_path = config.as_deref();
            let mut config_file = ConfigFile::load_or_default(config_path)?;

            let mut gw_config = GatewayConfig::default();

            // 1. Resolve host and port hierarchy
            let (mut host, mut p_str) = match config_file.gateway.bind.split_once(':') {
                Some((h, port_val)) => (h.to_string(), port_val.to_string()),
                None => ("127.0.0.1".to_string(), "8080".to_string()),
            };

            if let Some(b) = bind {
                if let Some((h, port_val)) = b.split_once(':') {
                    host = h.to_string();
                    p_str = port_val.to_string();
                } else {
                    host = b;
                }
            }
            if let Some(a) = address {
                host = a;
            }
            if let Some(port_num) = port {
                p_str = port_num.to_string();
            }

            gw_config.bind_addr = format!("{}:{}", host, p_str);

            if let Some(r) = retries {
                gw_config.max_retries = r;
            } else {
                gw_config.max_retries = config_file.gateway.max_retries;
            }

            gw_config.flight_recorder_capacity = config_file.gateway.flight_recorder_capacity;
            
            // 2. Resolve or auto-generate secure API key
            let mut newly_generated_key: Option<String> = None;
            if let Some(k) = api_key {
                gw_config.api_key = k;
            } else if !config_file.gateway.api_key.trim().is_empty() {
                gw_config.api_key = config_file.gateway.api_key.clone();
            } else {
                // Auto-generate high-entropy secure API key if not specified
                let secure_key = generate_secure_api_key();
                gw_config.api_key = secure_key.clone();
                config_file.gateway.api_key = secure_key.clone();
                let save_dest = config_path.unwrap_or("ponyllm.toml");
                let _ = config_file.save_to_path(save_dest);
                newly_generated_key = Some(secure_key);
            }

            for (p_name, p_sec) in &config_file.providers {
                let all_model_names: Vec<String> = p_sec.list_all_models().into_iter().map(|m| m.name).collect();
                gw_config.providers.insert(
                    p_name.clone(),
                    ProviderConfig {
                        base_url: p_sec.base_url.clone(),
                        default_model: p_sec.default_model.clone(),
                        models: all_model_names,
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

            let is_all_interfaces = host == "0.0.0.0";
            let auth_display = if gw_config.api_key.is_empty() || gw_config.api_key.eq_ignore_ascii_case("none") {
                "免鉴权 (开放模式)".to_string()
            } else {
                gw_config.api_key.clone()
            };

            if let Some(gen_k) = &newly_generated_key {
                println!("\n💡 [自动生成访问凭证] 检测到未配置 API Key，已自动生成并保存高熵秘钥: {}", gen_k);
            }

            println!("\n╔════════════════════════════════════════════════════════════════════════╗");
            println!("║              🚀 ponyllm AI Gateway 服务已就绪                          ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 本地接入 Base URL:                                                  ║");
            if is_all_interfaces {
                println!("║    - OpenAI 客户端:   http://127.0.0.1:{}/v1 (局域网: http://0.0.0.0:{}/v1)║", p_str, p_str);
                println!("║    - Anthropic 客户端: http://127.0.0.1:{}    (局域网: http://0.0.0.0:{})   ║", p_str, p_str);
            } else {
                println!("║    - OpenAI 客户端:   http://{}:{}/v1                             ║", host, p_str);
                println!("║    - Anthropic 客户端: http://{}:{}                                ║", host, p_str);
            }
            println!("║    - 监听全地址:      http://{}                                     ║", gw_config.bind_addr);
            println!("║  • 访问凭证 (API Token):{}                                   ║", format!("{:<30}", auth_display));
            println!("║  • 标准模型路由 (Models):                                              ║");
            println!("║    - http://127.0.0.1:{}/v1/models                                      ║", p_str);
            println!("║    - http://127.0.0.1:{}/models                                         ║", p_str);
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已挂载模型提供商 (Providers & Models):                              ║");
            for (p_name, p_sec) in &config_file.providers {
                let all_models = p_sec.list_all_models();
                let m_names: Vec<String> = all_models.into_iter().map(|m| {
                    if m.name == p_sec.default_model {
                        format!("{} (★默认)", m.name)
                    } else {
                        m.name
                    }
                }).collect();
                println!("║    - {:<12} [{}]: {}", p_name, p_sec.strategy, m_names.join(", "));
            }
            println!("╚════════════════════════════════════════════════════════════════════════╝\n");

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
        Commands::Upgrade { check, force, dry_run, version } => {
            ponyllm_cli::upgrade::run_upgrade(check, force, dry_run, version).await?;
        }
    }

    Ok(())
}

fn handle_manage_gateway_auth(
    config_path: Option<&str>,
    custom_key: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path.unwrap_or("ponyllm.toml");
    let mut cfg = ConfigFile::load_or_default(Some(path))?;

    let final_key = match custom_key {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => generate_secure_api_key(),
    };

    cfg.gateway.api_key = final_key.clone();
    cfg.save_to_path(path)?;

    println!("\n╔════════════════════════════════════════════════════════════════════════╗");
    println!("║              🔑 网关访问 API Key (Token) 已更新就绪                   ║");
    println!("╠════════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                        ║");
    println!("║   API Key:  {}", format!("{:<58}", final_key));
    println!("║                                                                        ║");
    println!("╠════════════════════════════════════════════════════════════════════════╣");
    println!("║  • 已同步持久化保存至: {:<46} ║", path);
    println!("║  • 请复制上方 API Key，用于 Cursor / Claude Code / SDK 鉴权连接。      ║");
    println!("╚════════════════════════════════════════════════════════════════════════╝\n");

    Ok(())
}
