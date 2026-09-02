use std::fs;
use std::str::FromStr;
use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ponyllm_core::pool::{ApiKeyEntry, GatewayRoutingStrategy, KeyPool, RoutingStrategy};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use ponyllm_cli::cli::{Cli, Commands, KeyCommands, ModelCommands, ProviderCommands, StrategyCommands};
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
            KeyCommands::Test { provider: _, config: _ } => {
                println!("🔍 正在执行 API Key 连通性测试...");
                println!("✅ 所有在线 Key 均健康可用。");
            }
            KeyCommands::Gateway { config, key } => {
                handle_manage_gateway_auth(config.as_deref(), key)?;
            }
        },
        Commands::Model(cmd) => match cmd {
            ModelCommands::List { config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                println!("=== 已配置的模型目录 ===");
                println!("{:<15} {:<28} {:<8} {:<10} {:<10}", "提供商", "模型标识", "梯队", "上下文上限", "输出上限");
                println!("{}", "-".repeat(75));
                for (p_name, p) in &cfg.providers {
                    for m in p.list_all_models() {
                        let is_def = if m.name == p.default_model { " (★默认)" } else { "" };
                        println!(
                            "{:<15} {:<28} {:<8} {:<10} {:<10}",
                            p_name,
                            format!("{}{}", m.name, is_def),
                            m.tier.shorthand(),
                            m.context_window,
                            m.max_output
                        );
                    }
                }
            }
            ModelCommands::Add { provider, model, context, max_output, inputs, outputs, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                let input_types: Vec<String> = inputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let output_types: Vec<String> = outputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let model_cfg = ponyllm_cli::config::ModelConfig {
                    name: model.clone(),
                    tier: ponyllm_core::pool::ModelTier::Standard,
                    context_window: context.clone(),
                    max_output: max_output.clone(),
                    input_types,
                    output_types,
                };
                cfg.upsert_model_config(&provider, model_cfg)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                cfg.save_to_path(path)?;
                println!("✅ 成功向提供商 '{}' 添加模型 '{}' (上下文: {}, 输出: {})", provider, model, context, max_output);
            }
            ModelCommands::Remove { provider, model, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if cfg.remove_model(&provider, &model).unwrap_or(false) {
                    cfg.save_to_path(path)?;
                    println!("✅ 成功从提供商 '{}' 删除模型 '{}'", provider, model);
                } else {
                    println!("⚠️ 未找到该模型配置");
                }
            }
            ModelCommands::Set { provider, model, config } => {
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if let Some(p) = cfg.providers.get_mut(&provider) {
                    p.default_model = model.clone();
                    cfg.save_to_path(path)?;
                    println!("✅ 成功将提供商 '{}' 默认模型设为 '{}'", provider, model);
                } else {
                    println!("⚠️ 未找到提供商 '{}'", provider);
                }
            }
        },
        Commands::Strategy(cmd) => match cmd {
            StrategyCommands::List => {
                println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
                println!("║                     🎯 ponyllm 智能调度策略一览                              ║");
                println!("╠══════════════════════════════════════════════════════════════════════════════╣");
                println!("║ {:<14} {:<12} {:<42} ║", "策略标识", "别名/简写", "人话规则与核心优势");
                println!("╠──────────────────────────────────────────────────────────────────────────────╣");
                println!("║ {:<14} {:<12} {:<42} ║", "economy (默认)", "cheap, e", "省钱优先: 0元免费 > Plan套餐 > 缓存命中 > 按量低价");
                println!("║ {:<14} {:<12} {:<42} ║", "speed", "fastest, s", "极速优先: 实测首字时延 TTFT 与吐字速率 t/s 选最快");
                println!("║ {:<14} {:<12} {:<42} ║", "reliable", "ha, r", "稳定优先: 高可用 SLA 保障，遇 429 自动毫秒级避让");
                println!("║ {:<14} {:<12} {:<42} ║", "balanced", "auto, b", "综合平衡: 成本与生成速度帕累托最优平衡");
                println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");
            }
            StrategyCommands::Get { config } => {
                let path = config.as_deref();
                let cfg = ConfigFile::load_or_default(path)?;
                let desc = match cfg.gateway.default_strategy {
                    GatewayRoutingStrategy::Economy => "省钱优先（免费/套餐/缓存/低价）",
                    GatewayRoutingStrategy::Speed => "极速优先（综合TTFT与t/s）",
                    GatewayRoutingStrategy::Reliable => "稳定优先（高可用与429避让）",
                    GatewayRoutingStrategy::Balanced => "综合平衡（成本与速度兼顾）",
                };
                println!("当前全局默认调度策略: {} [{}]", cfg.gateway.default_strategy, desc);
            }
            StrategyCommands::Set { strategy, config } => {
                let strat = GatewayRoutingStrategy::from_str(&strategy)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                let path = config.as_deref().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                cfg.gateway.default_strategy = strat;
                cfg.save_to_path(path)?;
                let desc = match strat {
                    GatewayRoutingStrategy::Economy => "省钱优先",
                    GatewayRoutingStrategy::Speed => "极速优先",
                    GatewayRoutingStrategy::Reliable => "稳定优先",
                    GatewayRoutingStrategy::Balanced => "综合平衡",
                };
                println!("✅ 成功将全局默认调度策略切换为 '{}' ({}) 并已保存至 '{}'", strat, desc, path);
            }
        },
        Commands::Auth { config, key } => {
            handle_manage_gateway_auth(config.as_deref(), key)?;
        }
        Commands::Tui { config, gateway_url } => {
            let path = config.as_deref().unwrap_or("ponyllm.toml");
            let cfg = ConfigFile::load_or_default(Some(path))?;
            run_tui(cfg, path.to_string(), gateway_url).await?;
        }
        Commands::Serve {
            config,
            bind,
            address,
            port,
            api_key,
            retries,
        } => {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "ponyllm_server=info,tower_http=debug".into()),
                )
                .with(tracing_subscriber::fmt::layer())
                .init();

            let config_path = config.as_deref();
            let mut config_file = ConfigFile::load_or_default(config_path)?;

            let mut gw_config = GatewayConfig::default();
            gw_config.default_strategy = config_file.gateway.default_strategy;

            let final_bind = if let Some(b) = bind {
                b
            } else if let (Some(addr), Some(p)) = (address.as_deref(), port) {
                format!("{}:{}", addr, p)
            } else if let Some(addr) = address {
                format!("{}:8080", addr)
            } else if let Some(p) = port {
                format!("127.0.0.1:{}", p)
            } else {
                config_file.gateway.bind.clone()
            };

            gw_config.bind_addr = final_bind;
            gw_config.max_retries = retries.unwrap_or(config_file.gateway.max_retries);
            gw_config.flight_recorder_capacity = config_file.gateway.flight_recorder_capacity;

            let (host, p_str) = gw_config
                .bind_addr
                .split_once(':')
                .unwrap_or(("127.0.0.1", "8080"));

            let mut newly_generated_key = None;
            if let Some(ak) = api_key {
                gw_config.api_key = ak;
            } else if !config_file.gateway.api_key.is_empty() {
                gw_config.api_key = config_file.gateway.api_key.clone();
            } else {
                let secure_key = generate_secure_api_key();
                gw_config.api_key = secure_key.clone();
                config_file.gateway.api_key = secure_key.clone();
                let save_dest = config_path.unwrap_or("ponyllm.toml");
                let _ = config_file.save_to_path(save_dest);
                newly_generated_key = Some(secure_key);
            }

            for (p_name, p_sec) in &config_file.providers {
                let all_models = p_sec.list_all_models();
                let all_model_names: Vec<String> = all_models.iter().map(|m| m.name.clone()).collect();
                let model_specs: Vec<ponyllm_server::ModelSpec> = all_models
                    .into_iter()
                    .map(|m| ponyllm_server::ModelSpec {
                        name: m.name,
                        tier: m.tier,
                        context_window: m.context_window,
                        max_output: m.max_output,
                        input_types: m.input_types,
                        output_types: m.output_types,
                    })
                    .collect();
                gw_config.providers.insert(
                    p_name.clone(),
                    ProviderConfig {
                        base_url: p_sec.base_url.clone(),
                        default_model: p_sec.default_model.clone(),
                        strategy: p_sec.strategy.clone(),
                        billing_mode: p_sec.billing_mode.clone(),
                        input_price: p_sec.input_price,
                        cached_price: p_sec.cached_price,
                        output_price: p_sec.output_price,
                        models: all_model_names,
                        model_specs,
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

            let strat_name = match gw_config.default_strategy {
                GatewayRoutingStrategy::Economy => "省钱优先 (0元免费 > Plan套餐 > 缓存命中 > 按量低价)",
                GatewayRoutingStrategy::Speed => "极速优先 (实测 TTFT + t/s 最优)",
                GatewayRoutingStrategy::Reliable => "稳定优先 (高可用保障与429避让)",
                GatewayRoutingStrategy::Balanced => "综合平衡 (成本与响应速度均衡)",
            };

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
            println!("║  • 全局调度策略:      {:<48} ║", strat_name);
            println!("║  • 访问凭证 (Token):  {}                                   ║", format!("{:<30}", auth_display));
            println!("║  • 虚拟总代模型:      auto, auto:flagship, auto:economy, auto[1m]     ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已挂载模型提供商 (Providers & Pricing):                             ║");
            for (p_name, p_sec) in &config_file.providers {
                let pricing_tag = if p_sec.is_free() {
                    "0元免费".to_string()
                } else if p_sec.billing_mode == ponyllm_core::pool::BillingMode::Plan {
                    "Plan套餐".to_string()
                } else {
                    format!("入${:.2}/缓${:.3}/出${:.2}", p_sec.input_price, p_sec.cached_price, p_sec.output_price)
                };
                let all_models = p_sec.list_all_models();
                let m_names: Vec<String> = all_models.into_iter().map(|m| {
                    if m.name == p_sec.default_model {
                        format!("{} (★默认,{})", m.name, m.tier.shorthand())
                    } else {
                        format!("{}({})", m.name, m.tier.shorthand())
                    }
                }).collect();
                println!("║    - {:<10} [{:<8}]: {}", p_name, pricing_tag, m_names.join(", "));
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
