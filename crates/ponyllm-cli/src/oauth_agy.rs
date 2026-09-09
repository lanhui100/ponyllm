use std::collections::HashMap;
use std::sync::Arc;
use ponyllm_core::pool::{
    build_authorization_url, exchange_code_for_credential, parse_code_from_input,
    AntigravityTokenManager, DEFAULT_ANTIGRAVITY_ENDPOINT, DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT,
    UpstreamProtocol,
};
use ponyllm_config::{ConfigFile, KeySection, ProviderSection};

/// Check if the CLI is running inside an SSH session or in a headless environment.
pub fn is_ssh_or_headless() -> bool {
    if std::env::var("SSH_CLIENT").is_ok()
        || std::env::var("SSH_CONNECTION").is_ok()
        || std::env::var("SSH_TTY").is_ok()
    {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
            return true;
        }
    }
    false
}

/// Attempt to open URL in system default browser
pub fn open_in_browser(url: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", url]).spawn();
    }
}

async fn bind_available_loopback(preferred_port: u16) -> std::io::Result<(tokio::net::TcpListener, u16)> {
    for p in preferred_port..preferred_port + 10 {
        if let Ok(listener) = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", p)).await {
            return Ok((listener, p));
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

async fn read_stdin_line() -> std::io::Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal()
        || std::env::var("PONYLLM_NON_INTERACTIVE").is_ok()
        || std::env::var("PONYLLM_DISABLE_STDIN").is_ok()
    {
        return futures_util::future::pending::<std::io::Result<String>>().await;
    }
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            futures_util::future::pending::<()>().await;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Ok(line);
        }
    }
}

/// Main interactive handler for `ponyllm provider add agy` / `ponyllm key auth agy [ID]`
pub async fn handle_key_auth_agy(
    provider_arg: &str,
    id_arg: Option<&str>,
    priority: u32,
    weight: u32,
    preferred_port: u16,
    no_browser: bool,
    config_path: Option<&str>,
    cli_proxy: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (target_provider, custom_id) = match provider_arg.to_lowercase().as_str() {
        "agy" | "antigravity" => ("antigravity".to_string(), id_arg.map(str::to_string)),
        other => ("antigravity".to_string(), Some(other.to_string())),
    };

    let resolved = ConfigFile::resolve_path(config_path);
    let path_str = resolved.to_str().unwrap_or("ponyllm.toml");

    let mut cfg = ConfigFile::load_or_default(Some(path_str).filter(|_| resolved.exists()))
        .unwrap_or_default();

    let (listener, bound_port) = bind_available_loopback(
        if preferred_port > 0 { preferred_port } else { DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT },
    )
    .await?;

    let redirect_uri = format!("http://localhost:{}/oauth2callback", bound_port);
    let state = uuid::Uuid::new_v4().to_string();
    let auth_url = build_authorization_url(&redirect_uri, &state);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1);
    let tx_shared = Arc::new(tokio::sync::Mutex::new(Some(tx)));
    let tx_route = tx_shared.clone();

    let app = axum::Router::new().route(
        "/oauth2callback",
        axum::routing::get(move |axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>| {
            let tx_inner = tx_route.clone();
            async move {
                if let Some(code) = params.get("code") {
                    let mut guard = tx_inner.lock().await;
                    if let Some(sender) = guard.take() {
                        let _ = sender.send(code.clone()).await;
                    }
                    axum::response::Html(r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Antigravity 授权成功</title></head>
<body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; text-align: center; padding: 60px 20px; background-color: #0f172a; color: #f8fafc;">
  <div style="max-width: 480px; margin: 0 auto; background: #1e293b; border-radius: 12px; padding: 32px; box-shadow: 0 10px 25px rgba(0,0,0,0.5); border: 1px solid #334155;">
    <h1 style="color: #22c55e; font-size: 28px; margin-bottom: 12px;">✅ 授权成功</h1>
    <p style="font-size: 16px; color: #94a3b8; line-height: 1.6;">已成功捕获 Google Antigravity 访问凭证。<br>您可以关闭此网页，并返回终端继续查看配额与配置。</p>
  </div>
</body>
</html>"#.to_string())
                } else {
                    let err = params.get("error").map(|s| s.as_str()).unwrap_or("unknown_error");
                    axum::response::Html(format!(r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Antigravity 授权失败</title></head>
<body style="font-family: sans-serif; text-align: center; padding: 60px 20px; background: #0f172a; color: #f8fafc;">
  <div style="max-width: 480px; margin: 0 auto; background: #1e293b; border-radius: 12px; padding: 32px; border: 1px solid #ef4444;">
    <h1 style="color: #ef4444; font-size: 26px;">❌ 授权失败</h1>
    <p style="color: #94a3b8;">Google 授权异常: {}</p>
  </div>
</body>
</html>"#, err))
                }
            }
        }),
    );

    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let headless = no_browser || is_ssh_or_headless();

    println!();
    if headless {
        println!("🔗 请在浏览器打开以下链接完成授权 (支持按住 Ctrl 点击)：");
        println!("   \x1b]8;;{}\x1b\\👉 [点击此处打开 Google 授权页面]\x1b]8;;\x1b\\", auth_url);
        println!("   若终端不支持点击，请直接复制以下链接在浏览器中打开：");
        println!("{}", auth_url);
        println!();
        print!("请输入重定向 URL 或 Code（按 Ctrl+C 取消）: ");
    } else {
        println!("🚀 正在打开浏览器授权（若未弹出可按住 Ctrl 点击）：");
        println!("   \x1b]8;;{}\x1b\\👉 [点击此处打开 Google 授权页面]\x1b]8;;\x1b\\", auth_url);
        println!("{}", auth_url);
        println!();
        open_in_browser(&auth_url);
        print!("等待授权中（可直接粘贴重定向 URL 或 Code，按 Ctrl+C 取消）: ");
    }
    std::io::Write::flush(&mut std::io::stdout())?;

    let captured_code = tokio::select! {
        Some(code) = rx.recv() => {
            code
        }
        line_res = read_stdin_line() => {
            match line_res {
                Ok(text) => {
                    if let Some(code) = parse_code_from_input(&text) {
                        code
                    } else {
                        server_handle.abort();
                        return Err("未能从输入中提取出有效的 OAuth Code，请确认输入是否完整。".into());
                    }
                }
                Err(e) => {
                    server_handle.abort();
                    return Err(format!("读取终端输入失败: {}", e).into());
                }
            }
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
            server_handle.abort();
            return Err("授权等待超时 (5分钟)，流程已终止。".into());
        }
    };

    server_handle.abort();

    let detected_proxy = ponyllm_core::detect_system_proxy();
    let effective_proxy = cli_proxy
        .map(str::to_string)
        .or_else(|| cfg.providers.get(&target_provider).and_then(|p| p.proxy.clone()))
        .or_else(|| cfg.gateway.proxy.clone())
        .or(detected_proxy);

    let http_client = ponyllm_core::executor::create_upstream_http_client_with_options(
        effective_proxy.as_deref(),
        cfg.gateway.use_system_proxy || effective_proxy.is_some(),
    );

    let auth_res = exchange_code_for_credential(
        &http_client,
        &captured_code,
        &redirect_uri,
    )
    .await?;

    let final_id = if let Some(custom) = custom_id {
        custom
    } else if let Some(ref email) = auth_res.email {
        format!("ag-{}", email)
    } else {
        format!("ag-account-{}", &uuid::Uuid::new_v4().to_string()[..6])
    };

    let provider_base_url = {
        let p_sec = cfg.providers.entry(target_provider.clone()).or_insert_with(|| {
            ProviderSection {
                base_url: DEFAULT_ANTIGRAVITY_ENDPOINT.to_string(),
                default_model: "claude-sonnet-4-6".to_string(),
                strategy: "round_robin".to_string(),
                billing_mode: ponyllm_core::pool::BillingMode::Metered,
                input_price: 0.0,
                cached_price: 0.0,
                output_price: 0.0,
                models: vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-opus-4-6".to_string(),
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-pro".to_string(),
                ],
                default_protocol: Some(UpstreamProtocol::Antigravity),
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                keys: vec![],
                model_configs: vec![],
            }
        });

        if let Some(pxy) = cli_proxy {
            p_sec.proxy = Some(pxy.to_string());
        } else if p_sec.proxy.is_none() {
            if let Some(ref pxy) = effective_proxy {
                p_sec.proxy = Some(pxy.clone());
            }
        }

        if let Some(existing_key) = p_sec.keys.iter_mut().find(|k| k.id == final_id) {
            existing_key.api_key = auth_res.credential.refresh_token.clone();
            existing_key.priority = priority;
            existing_key.weight = weight;
        } else {
            p_sec.keys.push(KeySection {
                id: final_id.clone(),
                api_key: auth_res.credential.refresh_token.clone(),
                priority,
                weight,
            });
        }
        p_sec.base_url.clone()
    };

    cfg.save_to_path(path_str)?;

    let account_label = auth_res.email.as_deref().unwrap_or(&final_id);
    println!("\n✅ 授权成功！凭证已保存（账户: {}）", account_label);

    let mgr = AntigravityTokenManager::new(&final_id, auth_res.credential, http_client);
    let quota_res = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        mgr.fetch_quota(Some(&provider_base_url)),
    )
    .await;

    if let Ok(Ok(snapshot)) = quota_res {
        println!("可用模型与配额:");
        let mut models: Vec<_> = snapshot.models.values().collect();
        models.sort_by_key(|m| &m.model_id);
        for m in models {
            let pct = format!("{:.1}%", m.remaining_fraction * 100.0);
            let remaining_desc = match m.reset_time {
                Some(utc_dt) => {
                    let now = chrono::Utc::now();
                    if utc_dt > now {
                        let dur = utc_dt - now;
                        let hours = dur.num_hours();
                        let mins = dur.num_minutes() % 60;
                        format!("{}小时{}分后重置", hours, mins)
                    } else {
                        "已就绪".to_string()
                    }
                }
                None => "已就绪".to_string(),
            };
            println!("  • {:<20} {:>6} ({})", m.model_id, pct, remaining_desc);
        }
    }
    println!();
    Ok(())
}
