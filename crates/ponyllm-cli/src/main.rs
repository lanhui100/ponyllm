use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ponyllm_core::pool::{ApiKeyEntry, GatewayRoutingStrategy, KeyPool, RoutingStrategy};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use ponyllm_cli::cli::{Cli, Commands, KeyCommands, ModelCommands, ProviderCommands, StrategyCommands};
use ponyllm_cli::config::{
    generate_sample_config, generate_secure_api_key, parse_gateway_auth_action, ConfigFile,
    GatewayAuthAction,
};
use ponyllm_cli::wizard::run_interactive_init;
use ponyllm_cli::tui::run_tui;

fn resolve_path(custom: Option<&str>) -> std::path::PathBuf {
    ConfigFile::resolve_path(custom)
}

fn build_gateway_config_and_pools(
    config_file: &ConfigFile,
    bind_override: Option<String>,
    retries_override: Option<usize>,
    api_key_override: Option<String>,
) -> (GatewayConfig, HashMap<String, Arc<KeyPool>>) {
    let mut gw_config = GatewayConfig::default();
    gw_config.default_strategy = config_file.gateway.default_strategy;
    gw_config.bind_addr = bind_override.unwrap_or_else(|| config_file.gateway.bind.clone());
    gw_config.max_retries = retries_override.unwrap_or(config_file.gateway.max_retries);
    gw_config.flight_recorder_capacity = config_file.gateway.flight_recorder_capacity;
    gw_config.request_body_limit = config_file.gateway.request_body_limit;
    gw_config.api_key = api_key_override.unwrap_or_else(|| config_file.gateway.api_key.clone());

    let mut pools = HashMap::new();

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
                billing_mode: m.billing_mode,
                input_price: m.input_price,
                cached_price: m.cached_price,
                output_price: m.output_price,
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

        let strat = match p_sec.strategy.as_str() {
            "priority" => RoutingStrategy::Priority,
            "weighted" => RoutingStrategy::WeightedRoundRobin,
            _ => RoutingStrategy::RoundRobin,
        };
        let pool = Arc::new(KeyPool::new(p_name, strat));
        for k in &p_sec.keys {
            pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
        }
        pools.insert(p_name.clone(), pool);
    }

    (gw_config, pools)
}

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
                let resolved = resolve_path(config.as_deref());
                let cfg = ConfigFile::load_or_default(resolved.to_str())?;
                println!("=== 已配置的模型提供商 (共 {} 个) ===", cfg.providers.len());
                println!("{:<14} {:<28} {:<24} {:<8} {:<10} {:<22} {:<6}", "提供商", "Base URL", "默认模型", "模式", "策略", "基准资费($/1M:入/缓/出)", "Keys");
                println!("{}", "-".repeat(115));
                for (name, p) in &cfg.providers {
                    let mode_str = match p.billing_mode {
                        ponyllm_core::pool::BillingMode::Plan => "plan(套餐)",
                        ponyllm_core::pool::BillingMode::Metered => "metered",
                        ponyllm_core::pool::BillingMode::Free => "free(免费)",
                    };
                    let pricing_str = format!("{:.2}/{:.3}/{:.2}", p.input_price, p.cached_price, p.output_price);
                    println!(
                        "{:<14} {:<28} {:<24} {:<8} {:<10} {:<22} {:<6}",
                        name, p.base_url, p.default_model, mode_str, p.strategy, pricing_str, p.keys.len()
                    );
                }
            }
            ProviderCommands::Add {
                name,
                base_url,
                model,
                strategy,
                billing_mode,
                input_price,
                cached_price,
                output_price,
                config,
            } => {
                if input_price < 0.0 || input_price.is_nan() || input_price.is_infinite() {
                    return Err(format!("常规输入单价 --input-price 必须为大于等于 0 的合法数值，输入: {}", input_price).into());
                }
                if cached_price < 0.0 || cached_price.is_nan() || cached_price.is_infinite() {
                    return Err(format!("缓存命中单价 --cached-price 必须为大于等于 0 的合法数值，输入: {}", cached_price).into());
                }
                if output_price < 0.0 || output_price.is_nan() || output_price.is_infinite() {
                    return Err(format!("输出生成单价 --output-price 必须为大于等于 0 的合法数值，输入: {}", output_price).into());
                }

                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
                    .unwrap_or_default();
                let mode = match billing_mode.trim().to_ascii_lowercase().as_str() {
                    "plan" => ponyllm_core::pool::BillingMode::Plan,
                    _ => ponyllm_core::pool::BillingMode::Metered,
                };
                cfg.add_provider_full(
                    &name,
                    &base_url,
                    &model,
                    &strategy,
                    mode,
                    input_price,
                    cached_price,
                    output_price,
                );
                cfg.save_to_path(path)?;
                println!("✅ 成功添加/更新提供商 '{}' (Base URL: {}, Model: {}, 资费: {}/{}/{})", name, base_url, model, input_price, cached_price, output_price);
                println!("   • 配置文件: {}", resolved.display());
            }
            ProviderCommands::Remove { name, config } => {
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if cfg.remove_provider(&name) {
                    cfg.save_to_path(path)?;
                    println!("✅ 成功删除提供商 '{}'", name);
                    println!("   • 配置文件: {}", resolved.display());
                } else {
                    println!("⚠️ 未找到提供商 '{}'", name);
                }
            }
        },
        Commands::Key(cmd) => match cmd {
            KeyCommands::List { provider, config } => {
                let resolved = resolve_path(config.as_deref());
                let cfg = ConfigFile::load_or_default(resolved.to_str())?;
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
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
                    .unwrap_or_default();
                cfg.add_key(&provider, &id, &key, priority, weight)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                cfg.save_to_path(path)?;
                println!("✅ 成功向提供商 '{}' 账户池添加/更新 Key '{}' (优先级: {}, 权重: {})", provider, id, priority, weight);
                println!("   • 配置文件: {}", resolved.display());
            }
            KeyCommands::Remove { provider, id, config } => {
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                match cfg.remove_key(&provider, &id) {
                    Ok(true) => {
                        cfg.save_to_path(path)?;
                        println!("✅ 成功从提供商 '{}' 中删除 Key '{}'", provider, id);
                        println!("   • 配置文件: {}", resolved.display());
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
            KeyCommands::Gateway { config, key, rotate } => {
                handle_manage_gateway_auth(config.as_deref(), key, rotate)?;
            }
        },
        Commands::Model(cmd) => match cmd {
            ModelCommands::List { config } => {
                let resolved = resolve_path(config.as_deref());
                let cfg = ConfigFile::load_or_default(resolved.to_str())?;
                println!("=== 已配置的模型目录 ===");
                println!("{:<12} {:<24} {:<6} {:<10} {:<8} {:<12} {:<24}", "提供商", "模型标识", "梯队", "模式", "上下文", "最大输出", "资费($/1M:入/缓/出)");
                println!("{}", "-".repeat(102));
                for (p_name, p) in &cfg.providers {
                    for m in p.list_all_models() {
                        let is_def = if m.name == p.default_model { " (★默认)" } else { "" };
                        let mode_desc = match p.get_model_billing_mode(&m.name) {
                            ponyllm_core::pool::BillingMode::Plan => "Plan(套餐)",
                            ponyllm_core::pool::BillingMode::Free => "0元免费",
                            ponyllm_core::pool::BillingMode::Metered => "按量计费",
                        };
                        let pricing_info = if m.input_price.is_some() || m.cached_price.is_some() || m.output_price.is_some() {
                            let pr = p.get_model_pricing(&m.name);
                            format!("★ {:.2}/{:.3}/{:.2}", pr.input_price, pr.cached_price, pr.output_price)
                        } else {
                            let pr = p.pricing();
                            format!("{:.2}/{:.3}/{:.2}(继承)", pr.input_price, pr.cached_price, pr.output_price)
                        };
                        println!(
                            "{:<12} {:<24} {:<6} {:<10} {:<8} {:<12} {:<24}",
                            p_name,
                            format!("{}{}", m.name, is_def),
                            m.tier.shorthand(),
                            mode_desc,
                            m.context_window,
                            m.max_output,
                            pricing_info,
                        );
                    }
                }
            }
            ModelCommands::Add {
                provider,
                model,
                context,
                max_output,
                inputs,
                outputs,
                tier,
                input_price,
                cached_price,
                output_price,
                billing_mode,
                config,
            } => {
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                let input_types: Vec<String> = inputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let output_types: Vec<String> = outputs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let tier_val = <ponyllm_core::pool::ModelTier as std::str::FromStr>::from_str(&tier)
                    .map_err(|e| format!("无效的能力梯队 --tier '{}': {}。仅支持 Flagship (F), Standard (S), Light (L)", tier, e))?;

                let mode_val = match billing_mode.as_deref() {
                    Some("plan") | Some("coding_plan") | Some("coding-plan") => Some(ponyllm_core::pool::BillingMode::Plan),
                    Some("metered") | Some("payg") => Some(ponyllm_core::pool::BillingMode::Metered),
                    Some("free") => Some(ponyllm_core::pool::BillingMode::Free),
                    Some(other) => {
                        return Err(format!("无效的计费模式 --billing-mode '{}'。仅支持 metered, plan, free", other).into());
                    }
                    None => None,
                };

                if let Some(p) = input_price {
                    if p < 0.0 || p.is_nan() || p.is_infinite() {
                        return Err(format!("常规输入单价 --input-price 必须为大于等于 0 的合法数值，输入: {}", p).into());
                    }
                }
                if let Some(p) = cached_price {
                    if p < 0.0 || p.is_nan() || p.is_infinite() {
                        return Err(format!("缓存命中单价 --cached-price 必须为大于等于 0 的合法数值，输入: {}", p).into());
                    }
                }
                if let Some(p) = output_price {
                    if p < 0.0 || p.is_nan() || p.is_infinite() {
                        return Err(format!("输出生成单价 --output-price 必须为大于等于 0 的合法数值，输入: {}", p).into());
                    }
                }

                let model_cfg = ponyllm_cli::config::ModelConfig {
                    name: model.clone(),
                    tier: tier_val,
                    billing_mode: mode_val,
                    context_window: context.clone(),
                    max_output: max_output.clone(),
                    input_types,
                    output_types,
                    input_price,
                    cached_price,
                    output_price,
                };
                cfg.upsert_model_config(&provider, model_cfg)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                cfg.save_to_path(path)?;
                let mode_label = mode_val.map(|m| format!("模式: {:?}", m)).unwrap_or_else(|| "模式: 继承提供商".to_string());
                println!(
                    "✅ 成功向提供商 '{}' 添加模型 '{}' [梯队: {}, {}] (上下文: {}, 输出: {})",
                    provider, model, tier_val.shorthand(), mode_label, context, max_output
                );
                println!("   • 配置文件: {}", resolved.display());
            }
            ModelCommands::Remove { provider, model, config } => {
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if cfg.remove_model(&provider, &model).unwrap_or(false) {
                    cfg.save_to_path(path)?;
                    println!("✅ 成功从提供商 '{}' 删除模型 '{}'", provider, model);
                    println!("   • 配置文件: {}", resolved.display());
                } else {
                    println!("⚠️ 未找到该模型配置");
                }
            }
            ModelCommands::Set { provider, model, config } => {
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                if let Some(p) = cfg.providers.get_mut(&provider) {
                    p.default_model = model.clone();
                    cfg.save_to_path(path)?;
                    println!("✅ 成功将提供商 '{}' 默认模型设为 '{}'", provider, model);
                    println!("   • 配置文件: {}", resolved.display());
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
                let resolved = resolve_path(config.as_deref());
                let cfg = ConfigFile::load_or_default(resolved.to_str())?;
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
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path))?;
                cfg.gateway.default_strategy = strat;
                cfg.save_to_path(path)?;
                let desc = match strat {
                    GatewayRoutingStrategy::Economy => "省钱优先",
                    GatewayRoutingStrategy::Speed => "极速优先",
                    GatewayRoutingStrategy::Reliable => "稳定优先",
                    GatewayRoutingStrategy::Balanced => "综合平衡",
                };
                println!("✅ 成功将全局默认调度策略切换为 '{}' ({}) 并已保存至 '{}'", strat, desc, resolved.display());
            }
        },
        Commands::Auth { config, key, rotate } => {
            handle_manage_gateway_auth(config.as_deref(), key, rotate)?;
        }
        Commands::Tui { config, gateway_url } => {
            let resolved = resolve_path(config.as_deref());
            let path = resolved.to_str().unwrap_or("ponyllm.toml");
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

            let resolved_config = resolve_path(config.as_deref());
            let mut config_file = ConfigFile::load_or_default(resolved_config.to_str())?;

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

            let mut newly_generated_key = None;
            let final_api_key = if let Some(ak) = api_key {
                ak
            } else if !config_file.gateway.api_key.is_empty() {
                config_file.gateway.api_key.clone()
            } else {
                let secure_key = generate_secure_api_key();
                config_file.gateway.api_key = secure_key.clone();
                let save_dest = resolved_config.to_str().unwrap_or("ponyllm.toml");
                let _ = config_file.save_to_path(save_dest);
                newly_generated_key = Some(secure_key.clone());
                secure_key
            };

            let (gw_config, pools) = build_gateway_config_and_pools(
                &config_file,
                Some(final_bind.clone()),
                retries,
                Some(final_api_key),
            );

            let state = Arc::new(AppState::new(gw_config.clone()));
            for (p_name, pool) in pools {
                state.register_pool(&p_name, pool);
            }

            // Spawn background config watcher for zero-downtime hot reload
            let watcher_path = resolved_config.clone();
            let watcher_state = state.clone();
            let watcher_bind = final_bind.clone();
            let watcher_retries = retries;
            tokio::spawn(async move {
                let mut last_modified = std::fs::metadata(&watcher_path)
                    .and_then(|m| m.modified())
                    .ok();

                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let current_modified = std::fs::metadata(&watcher_path)
                        .and_then(|m| m.modified())
                        .ok();

                    if current_modified.is_some() && current_modified != last_modified {
                        last_modified = current_modified;
                        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

                        if let Ok(content) = std::fs::read_to_string(&watcher_path) {
                            if let Ok(new_cfg_file) = toml::from_str::<ConfigFile>(&content) {
                                let (new_gw_cfg, new_pools) = build_gateway_config_and_pools(
                                    &new_cfg_file,
                                    Some(watcher_bind.clone()),
                                    watcher_retries,
                                    None,
                                );
                                watcher_state.reload_config_with_pools(new_gw_cfg, new_pools);
                                println!(
                                    "\n🔄 [配置热更新] 检测到 '{}' 发生物理变更，网关已完成零停机平滑热重载！",
                                    watcher_path.display()
                                );
                            } else {
                                eprintln!(
                                    "⚠️ [配置热更新] '{}' 语法解析失败，跳过本次重载以保持服务稳定",
                                    watcher_path.display()
                                );
                            }
                        }
                    }
                }
            });

            let app = create_app(state);
            let listener = tokio::net::TcpListener::bind(&gw_config.bind_addr).await?;

            let (host, p_str) = gw_config
                .bind_addr
                .split_once(':')
                .unwrap_or(("127.0.0.1", "8080"));

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
            println!("║  • 配置文件路径:      {:<48} ║", resolved_config.display());
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
            println!("║  • 请求体缓冲上限:    {:<48} ║", format!("{} MB (支持1M长上下文/多模态)", gw_config.request_body_limit / (1024 * 1024)));
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
        Commands::Status {
            config,
            gateway_url,
            api_key,
        } => {
            handle_gateway_status(config.as_deref(), gateway_url, api_key).await?;
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
    rotate: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
        .unwrap_or_default();

    let action = parse_gateway_auth_action(custom_key.as_deref(), rotate);

    match action {
        GatewayAuthAction::MisdirectedList => {
            println!("\n╔════════════════════════════════════════════════════════════════════════╗");
            println!("║              💡 PonyLLM 访问凭证与 Key 管理指引                         ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 'ponyllm auth' 用于查看或管理【网关自身的对外访问凭证 (Token)】     ║");
            println!("║  • 若要查看网关访问 Token:     ponyllm auth  或  ponyllm status         ║");
            println!("║  • 若要查看【上游模型厂商】Key: ponyllm key list                         ║");
            println!("╚════════════════════════════════════════════════════════════════════════╝\n");
            return Ok(());
        }
        GatewayAuthAction::Show => {
            let current_key = if cfg.gateway.api_key.is_empty()
                || cfg.gateway.api_key.eq_ignore_ascii_case("none")
            {
                "免鉴权 (开放模式)".to_string()
            } else {
                cfg.gateway.api_key.clone()
            };

            let host_port = if cfg.gateway.bind.starts_with("0.0.0.0") {
                let port = cfg
                    .gateway
                    .bind
                    .split_once(':')
                    .map(|(_, p)| p)
                    .unwrap_or("8080");
                format!("127.0.0.1:{}", port)
            } else {
                cfg.gateway.bind.clone()
            };

            let openai_base = format!("http://{}/v1", host_port);
            let anthropic_base = format!("http://{}", host_port);
            let token_val = if current_key.starts_with("免鉴权") {
                "none"
            } else {
                &current_key
            };
            println!("\n╔════════════════════════════════════════════════════════════════════════╗");
            println!("║              🔑 ponyllm 网关访问 API Key (Token) 状态                  ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║                                                                        ║");
            println!("║   网关 Token:   {}", format!("{:<55}", current_key));
            println!("║                                                                        ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 配置文件路径:  {:<53} ║", resolved.display());
            println!("║  • 客户端接入环境变量示例:                                             ║");
            println!("║    - export OPENAI_API_BASE={:<42} ║", openai_base);
            println!("║    - export OPENAI_API_KEY={:<43} ║", token_val);
            println!("║    - export ANTHROPIC_BASE_URL={:<39} ║", anthropic_base);
            println!("║    - export ANTHROPIC_API_KEY={:<41} ║", token_val);
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  👉 操作指引:                                                          ║");
            println!("║  • 轮转重置为新随机 Key:  ponyllm auth --rotate                        ║");
            println!("║  • 手动指定并保存自定义 Key: ponyllm auth <YOUR_SECRET_KEY>            ║");
            println!("║  • 查看完整服务与密钥池状态: ponyllm status                            ║");
            println!("╚════════════════════════════════════════════════════════════════════════╝\n");
            return Ok(());
        }
        GatewayAuthAction::Rotate => {
            let final_key = generate_secure_api_key();
            cfg.gateway.api_key = final_key.clone();
            cfg.save_to_path(path)?;

            println!("\n╔════════════════════════════════════════════════════════════════════════╗");
            println!("║              🔑 网关访问 API Key (Token) 已轮转就绪                   ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║                                                                        ║");
            println!("║   新 API Key:  {}", format!("{:<56}", final_key));
            println!("║                                                                        ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已同步持久化保存至: {:<46} ║", resolved.display());
            println!("║  • 请复制上方 API Key，用于 Cursor / Claude Code / SDK 鉴权连接。      ║");
            println!("╚════════════════════════════════════════════════════════════════════════╝\n");
        }
        GatewayAuthAction::Set(new_key) => {
            cfg.gateway.api_key = new_key.clone();
            cfg.save_to_path(path)?;

            println!("\n╔════════════════════════════════════════════════════════════════════════╗");
            println!("║              🔑 网关访问 API Key (Token) 已更新就绪                   ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║                                                                        ║");
            println!("║   新 API Key:  {}", format!("{:<56}", new_key));
            println!("║                                                                        ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已同步持久化保存至: {:<46} ║", resolved.display());
            println!("║  • 请复制上方 API Key，用于 Cursor / Claude Code / SDK 鉴权连接。      ║");
            println!("╚════════════════════════════════════════════════════════════════════════╝\n");
        }
    }

    Ok(())
}

async fn handle_gateway_status(
    config_path: Option<&str>,
    cli_gateway_url: Option<String>,
    cli_api_key: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
        .unwrap_or_default();

    let base_url = if let Some(u) = cli_gateway_url {
        u.trim_end_matches('/').to_string()
    } else {
        let (host, port) = cfg.gateway.bind.split_once(':').unwrap_or(("127.0.0.1", "8080"));
        let probe_host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
        format!("http://{}:{}", probe_host, port)
    };

    let active_key = if let Some(k) = cli_api_key {
        k
    } else {
        cfg.gateway.api_key.clone()
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let health_url = format!("{}/health", base_url);
    let metrics_url = format!("{}/v1/telemetry/metrics", base_url);

    let health_res = client.get(&health_url).send().await;

    let mut metrics_req = client.get(&metrics_url);
    if !active_key.is_empty() && !active_key.eq_ignore_ascii_case("none") {
        metrics_req = metrics_req.header("Authorization", format!("Bearer {}", active_key));
    }
    let metrics_res = metrics_req.send().await;

    let is_online = match &health_res {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    };

    let health_json: Option<serde_json::Value> = match health_res {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        _ => None,
    };

    let metrics_json: Option<serde_json::Value> = match metrics_res {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        _ => None,
    };

    let token_display = if active_key.is_empty() || active_key.eq_ignore_ascii_case("none") {
        "免鉴权 (开放模式)".to_string()
    } else {
        active_key.clone()
    };

    let strat_name = match cfg.gateway.default_strategy {
        GatewayRoutingStrategy::Economy => "省钱优先 (0元免费 > Plan套餐 > 缓存命中 > 按量低价)",
        GatewayRoutingStrategy::Speed => "极速优先 (实测 TTFT + t/s 最优)",
        GatewayRoutingStrategy::Reliable => "稳定优先 (高可用保障与429避让)",
        GatewayRoutingStrategy::Balanced => "综合平衡 (成本与响应速度均衡)",
    };

    println!("\n╔════════════════════════════════════════════════════════════════════════╗");
    println!("║              📊 ponyllm AI Gateway 服务运行全景看板                    ║");
    println!("╠════════════════════════════════════════════════════════════════════════╣");

    if is_online {
        let version = health_json
            .as_ref()
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        println!("║  • 服务健康状态:      🟢 在线运行中 (v{}){:>28} ║", version, "");
    } else {
        println!("║  • 服务健康状态:      🔴 离线 (未连接到网关服务){:>25} ║", "");
    }

    println!("║  • 探测服务基址:      {:<48} ║", base_url);
    println!("║  • 配置文件路径:      {:<48} ║", resolved.display());
    println!("║  • 全局调度策略:      {:<48} ║", strat_name);
    let openai_base = format!("{}/v1", base_url);
    let anthropic_base = base_url.clone();
    let token_val = if token_display.starts_with("免鉴权") { "none" } else { &token_display };
    println!("║  • 网关访问凭证 (Gateway Token):                                       ║");
    println!("║    - 当前生效 Token:  {:<48} ║", token_display);
    println!("║    - OpenAI SDK:      export OPENAI_API_KEY={:<26} ║", token_val);
    println!("║                       export OPENAI_API_BASE={:<25} ║", openai_base);
    println!("║    - Anthropic SDK:   export ANTHROPIC_API_KEY={:<23} ║", token_val);
    println!("║                       export ANTHROPIC_BASE_URL={:<22} ║", anthropic_base);
    println!("╠════════════════════════════════════════════════════════════════════════╣");
    println!("║  • 挂载模型提供商与密钥池 (Upstream Providers & Keys):                 ║");

    if cfg.providers.is_empty() {
        println!("║    (未配置任何上游提供商，请运行 'ponyllm provider add' 添加)          ║");
    } else {
        for (p_name, p_sec) in &cfg.providers {
            let key_count = p_sec.keys.len();
            let key_status = if key_count == 0 {
                "⚠️ 无可用 Key (池空)".to_string()
            } else {
                format!("{} 个上游 Key 就绪", key_count)
            };
            let def_model = &p_sec.default_model;
            let line_desc = format!("{:<10} | {:<16} | 默认模型: {}", p_name, key_status, def_model);
            println!("║    • {:<66} ║", line_desc);
        }
    }

    if let Some(m) = metrics_json {
        let total_req = m.get("total_requests").and_then(|v| v.as_u64()).unwrap_or(0);
        let succ_req = m.get("successful_requests").and_then(|v| v.as_u64()).unwrap_or(0);
        let fail_req = m.get("failed_requests").and_then(|v| v.as_u64()).unwrap_or(0);
        let total_tokens = m.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let succ_rate = if total_req > 0 {
            format!("{:.1}%", (succ_req as f64 / total_req as f64) * 100.0)
        } else {
            "100.0%".to_string()
        };

        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!("║  • 网关实时遥测指标 (Live Telemetry Metrics):                          ║");
        println!("║    - 总处理请求数:    {:<12} 成功率:       {:<15} ║", total_req, succ_rate);
        println!("║    - 成功 / 失败:     {:<12} 消耗 Tokens:  {:<15} ║", format!("{}/{}", succ_req, fail_req), total_tokens);
    } else if is_online {
        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!("║  • 网关实时遥测指标:  ⚠️ 无法获取指标 (可能鉴权 Token 不匹配)         ║");
    }

    println!("╚════════════════════════════════════════════════════════════════════════╝\n");

    if !is_online {
        println!("👉 排错提示: 网关服务当前未启动。请运行 'ponyllm serve' 启动服务，或使用 '-g <URL>' 探测指定网关。\n");
    }

    Ok(())
}
