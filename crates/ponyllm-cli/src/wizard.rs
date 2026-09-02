use std::collections::HashMap;
use inquire::{Confirm, Password, PasswordDisplayMode, Select, Text};
use crate::config::{ConfigFile, GatewaySection, KeySection, ProviderSection};



pub fn run_interactive_init(output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n========================================================");
    println!("  🚀 欢迎使用 ponyllm 统一网关配置初始化向导");
    println!("  请选择模型提供商接口并录入 API Key 以完成初始化");
    println!("========================================================\n");

    println!("\n--- 配置网关服务基础网络与鉴权 ---");
    let bind_options = vec![
        "0.0.0.0:8080 (全网卡监听 / 允许局域网与外部访问，推荐)",
        "127.0.0.1:8080 (仅限本机访问)",
        "自定义监听地址与端口 (例如 0.0.0.0:9000)",
    ];
    let bind_sel = Select::new("选择网关服务监听模式:", bind_options).prompt()?;
    let bind_addr = if bind_sel.starts_with("0.0.0.0") {
        "0.0.0.0:8080".to_string()
    } else if bind_sel.starts_with("127.0.0.1") {
        "127.0.0.1:8080".to_string()
    } else {
        Text::new("  请输入自定义监听地址 (Host:Port):")
            .with_default("0.0.0.0:8080")
            .prompt()?
    };

    let api_token = Text::new("  设置网关对外访问鉴权 API Token (Key):")
        .with_default("sk-ponyllm-local")
        .with_help_message("第三方客户端（如 Cursor、Claude Code、SDK）需使用该 Token 进行认证，留空或 none 为免鉴权")
        .prompt()?;

    let mut providers: HashMap<String, ProviderSection> = HashMap::new();

    loop {
        println!("\n--- 配置模型提供商 (Provider) ---");
        let provider_options = vec![
            "DeepSeek - OpenAI 协议 (https://api.deepseek.com, 默认模型: deepseek-v4-flash)",
            "DeepSeek - Anthropic 协议 (https://api.deepseek.com/anthropic, 默认模型: deepseek-v4-flash)",
            "OpenAI (官方接口及各大兼容中转: https://api.openai.com, 默认模型: gpt-4o)",
            "Anthropic (官方 Messages 接口: https://api.anthropic.com, 默认模型: claude-3-7-sonnet-20250219)",
            "OpenRouter (聚合网关)",
            "Custom (自定义兼容 API 提供商)",
        ];

        let selection = Select::new("选择要配置的提供商接口:", provider_options).prompt()?;
        let is_custom = selection.starts_with("Custom");

        let (p_name, default_url, default_model) = if selection.contains("DeepSeek - Anthropic") {
            ("deepseek-anthropic".to_string(), "https://api.deepseek.com/anthropic", "deepseek-v4-flash")
        } else if selection.contains("DeepSeek") {
            ("deepseek".to_string(), "https://api.deepseek.com", "deepseek-v4-flash")
        } else if selection.starts_with("OpenAI") {
            ("openai".to_string(), "https://api.openai.com", "gpt-4o")
        } else if selection.starts_with("Anthropic") {
            ("anthropic".to_string(), "https://api.anthropic.com", "claude-3-7-sonnet-20250219")
        } else if selection.starts_with("OpenRouter") {
            ("openrouter".to_string(), "https://openrouter.ai/api", "anthropic/claude-3.7-sonnet")
        } else {
            let custom_name = Text::new("  请输入自定义提供商标识名称 (英文小写):")
                .with_default("my-provider")
                .prompt()?;
            (custom_name, "https://api.example.com", "default-model")
        };

        let base_url = if is_custom {
            Text::new("  提供商 Base URL:")
                .with_default(default_url)
                .prompt()?
        } else {
            default_url.to_string()
        };

        let model = Text::new("  默认映射模型名称 (Model):")
            .with_default(default_model)
            .prompt()?;

        let strategy_options = vec!["priority (优先级主备)", "round_robin (多 Key 轮询)", "weighted (加权调度)"];
        let strat_sel = Select::new("  多 Key 调度策略:", strategy_options).prompt()?;
        let strat = if strat_sel.starts_with("priority") {
            "priority"
        } else if strat_sel.starts_with("weighted") {
            "weighted"
        } else {
            "round_robin"
        };

        let mut keys: Vec<KeySection> = Vec::new();

        loop {
            let key_idx = keys.len() + 1;
            println!("\n  [录入 Key #{}]", key_idx);
            let key_id = Text::new("    Key 唯一标识 (ID):")
                .with_default(&format!("{}-key-{}", p_name, key_idx))
                .prompt()?;

            let api_key = Password::new("    API Key 秘钥:")
                .with_display_mode(PasswordDisplayMode::Masked)
                .with_help_message("输入将被遮蔽，回车确认")
                .prompt()?;

            let priority: u32 = if strat == "priority" {
                let p_str = Text::new("    优先级 (数字越小优先级越高，1 为最高主 Key):")
                    .with_default(&key_idx.to_string())
                    .prompt()?;
                p_str.parse().unwrap_or(key_idx as u32)
            } else {
                1
            };

            let weight: u32 = if strat == "weighted" {
                let w_str = Text::new("    权重 (数字越大流量分配越多):")
                    .with_default("10")
                    .prompt()?;
                w_str.parse().unwrap_or(10)
            } else {
                10
            };

            keys.push(KeySection {
                id: key_id,
                api_key,
                priority,
                weight,
            });

            let add_another_key = Confirm::new("  是否为此提供商添加备用 Key (以启用故障无感倒换/池化)?")
                .with_default(false)
                .prompt()?;

            if !add_another_key {
                break;
            }
        }

        providers.insert(p_name.to_string(), ProviderSection {
            base_url,
            default_model: model,
            models: Vec::new(),
            model_configs: Vec::new(),
            strategy: strat.to_string(),
            keys,
        });

        let add_another_provider = Confirm::new("是否继续配置其他大模型提供商?")
            .with_default(false)
            .prompt()?;

        if !add_another_provider {
            break;
        }
    }

    let config = ConfigFile {
        gateway: GatewaySection {
            bind: bind_addr,
            max_retries: 3,
            flight_recorder_capacity: 200,
            api_key: api_token,
        },
        providers,
    };

    config.save_to_path(output_path)?;

    println!("\n========================================================");
    println!("  🎉 配置文件已成功生成并写入: {}", output_path);
    println!("  已配置提供商数: {}", config.providers.len());
    println!("\n  快速测试与启动:");
    println!("    ponyllm serve                    # 启动网关服务");
    println!("    ponyllm provider list            # 查看已配置提供商");
    println!("    ponyllm key list                 # 查看 Key 账户池");
    println!("    ponyllm tui                      # 打开全屏交互监控看板");
    println!("========================================================\n");

    Ok(())
}
