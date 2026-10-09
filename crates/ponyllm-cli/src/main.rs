#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::format_in_format_args)]

use clap::Parser;
use ponyllm_cli::cli::{
    format_web_status_url, Cli, Commands, KeyCommands, KeysCommands, ModelCommands,
    ProviderCommands, UserCommands,
};
use ponyllm_cli::config::{
    generate_sample_config, generate_secure_api_key, parse_gateway_auth_action, ConfigFile,
    GatewayAuthAction,
};
use ponyllm_cli::tui::run_tui;
use ponyllm_cli::wizard::run_interactive_init;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use ponyllm_server::config_poller::ConfigSource;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn resolve_path(custom: Option<&str>) -> std::path::PathBuf {
    ConfigFile::resolve_path(custom)
}

/// Map a provider `strategy` string to the pool routing enum.
///
/// Normalizes case/whitespace (mirrors `parse_pool_strategy` in
/// ponyllm-server; keep the two in sync). Unknown values fall back to
/// sticky [`RoutingStrategy::Priority`] with a warn so a typo pins to the
/// primary key loudly instead of silently rotating.
fn parse_pool_strategy(raw: &str, provider_name: &str) -> RoutingStrategy {
    match raw.trim().to_ascii_lowercase().as_str() {
        "priority" => RoutingStrategy::Priority,
        "weighted" | "weighted_round_robin" => RoutingStrategy::WeightedRoundRobin,
        "round_robin" => RoutingStrategy::RoundRobin,
        unknown => {
            tracing::warn!(
                provider = %provider_name,
                strategy = %raw,
                "unknown pool strategy '{unknown}', falling back to sticky priority; \
                 use priority|round_robin|weighted explicitly"
            );
            RoutingStrategy::Priority
        }
    }
}

fn build_gateway_config_and_pools(
    config_file: &ConfigFile,
    bind_override: Option<String>,
    retries_override: Option<usize>,
    api_key_override: Option<String>,
    web_enabled_override: Option<bool>,
    web_dist_dir_override: Option<String>,
) -> (GatewayConfig, HashMap<String, Arc<KeyPool>>) {
    let mut gw_config = GatewayConfig::default();
    gw_config.bind_addr = bind_override.unwrap_or_else(|| config_file.gateway.bind.clone());
    gw_config.max_retries = retries_override.unwrap_or(config_file.gateway.max_retries);
    gw_config.flight_recorder_capacity = config_file.gateway.flight_recorder_capacity;
    gw_config.request_body_limit = config_file.gateway.request_body_limit;
    gw_config.api_key = api_key_override.unwrap_or_else(|| config_file.gateway.api_key.clone());
    gw_config.web_enabled = web_enabled_override.unwrap_or(config_file.gateway.web_enabled);
    gw_config.web_dist_dir =
        web_dist_dir_override.unwrap_or_else(|| config_file.gateway.web_dist_dir.clone());
    gw_config.proxy = config_file.gateway.proxy.clone();
    gw_config.use_system_proxy = config_file.gateway.use_system_proxy;
    gw_config.upstream_timeout_secs = config_file.gateway.upstream_timeout_secs;
    gw_config.upstream_ttfb_timeout_secs = config_file.gateway.upstream_ttfb_timeout_secs;
    gw_config.empty_stop_total_timeout_secs = config_file.gateway.empty_stop_total_timeout_secs;
    gw_config.admin_write_enabled = config_file.gateway.admin_write_enabled;
    gw_config.telemetry_snapshot_path = config_file.gateway.telemetry_snapshot_path.clone();
    // P0 auth_compat passthrough (disk format -> runtime config).
    gw_config.auth_compat = config_file.gateway.auth_compat;
    // P1 scoped gateway keys passthrough (disk format -> runtime config).
    gw_config.gateway_keys = config_file.gateway.gateway_keys.clone();
    gw_config.antigravity_auto_refresh = config_file.gateway.antigravity_auto_refresh;
    gw_config.antigravity_refresh_interval_secs =
        config_file.gateway.antigravity_refresh_interval_secs;
    gw_config.cross_provider_quota_failover = config_file.gateway.cross_provider_quota_failover;
    // Phase-2 auth hardening passthrough (F1/F2/F4/F3): auth mode, failure
    // budget, admin IP fence and trusted proxies (disk format -> runtime).
    gw_config.auth_mode = config_file.gateway.auth_mode;
    gw_config.auth_fail_window_secs = config_file.gateway.auth_fail_window_secs;
    gw_config.auth_fail_limit = config_file.gateway.auth_fail_limit;
    gw_config.auth_lockout_secs = config_file.gateway.auth_lockout_secs;
    gw_config.admin_ip_allowlist = config_file.gateway.admin_ip_allowlist.clone();
    gw_config.trusted_proxies = config_file.gateway.trusted_proxies.clone();
    // Phase-3 (VULN-05): HttpOnly cookie admin sessions (disk format -> runtime).
    gw_config.admin_session_enabled = config_file.gateway.admin_session_enabled;
    gw_config.admin_session_ttl_secs = config_file.gateway.admin_session_ttl_secs;

    // Startup observability for the quota-boundary default (bugfix 2026-10-02):
    // when cross-provider quota failover is disabled but ≥2 providers share a
    // model name, make the behavior change explicit so an upgrade is not silent.
    if !gw_config.cross_provider_quota_failover {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for p in gw_config.providers.values() {
            for m in p
                .models
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(p.default_model.as_str()).filter(|d| !d.is_empty()))
            {
                *counts.entry(m).or_insert(0) += 1;
            }
        }
        let shared: Vec<&&str> = counts
            .iter()
            .filter(|(_, c)| **c >= 2)
            .map(|(k, _)| k)
            .collect();
        if !shared.is_empty() {
            tracing::info!(
                models = ?shared,
                "cross_provider_quota_failover=false: quota exhaustion stops at the first provider for shared models; use `provider/model` (e.g. sense/deepseek-v4-flash) to pin a specific provider"
            );
        }
    }

    let mut pools = HashMap::new();

    for (p_name, p_sec) in &config_file.providers {
        let all_models = p_sec.list_all_models();
        let all_model_names: Vec<String> = all_models.iter().map(|m| m.name.clone()).collect();
        let model_specs: Vec<ponyllm_server::ModelSpec> = all_models
            .into_iter()
            .map(|m| ponyllm_server::ModelSpec {
                name: m.name,
                tier: m.tier,
                priority: m.priority,
                context_window: m.context_window,
                max_output: m.max_output,
                input_types: m.input_types,
                output_types: m.output_types,
                billing_mode: m.billing_mode,
                input_price: m.input_price,
                cached_price: m.cached_price,
                output_price: m.output_price,
                pricing_mode: m.pricing_mode,
                pricing_periods: m.pricing_periods,
                display_name: m.display_name,
                temperature: m.temperature,
                top_p: m.top_p,
                protocol: m.protocol,
                base_url: m.base_url,
                thinking_default: m.thinking_default,
                thinking_max: m.thinking_max,
                proxy: m.proxy,
                timeout_secs: m.timeout_secs,
                rate_limits: m.rate_limits,
                fallbacks: m.fallbacks,
            })
            .collect();

        gw_config.providers.insert(
            p_name.clone(),
            ProviderConfig {
                base_url: p_sec.base_url.clone(),
                default_model: p_sec.default_model.clone(),
                strategy: p_sec.strategy.clone(),
                billing_mode: p_sec.billing_mode,
                input_price: p_sec.input_price,
                cached_price: p_sec.cached_price,
                output_price: p_sec.output_price,
                models: all_model_names,
                model_specs,
                default_protocol: p_sec.default_protocol,
                chat_url: p_sec.chat_url.clone(),
                responses_url: p_sec.responses_url.clone(),
                messages_url: p_sec.messages_url.clone(),
                proxy: p_sec.proxy.clone(),
                timeout_secs: p_sec.timeout_secs,
                ttfb_timeout_secs: p_sec.ttfb_timeout_secs,
                rate_limits: p_sec.rate_limits,
                egress_pool: p_sec.egress_pool.clone(),
                egress_strategy: p_sec.egress_strategy.clone(),
            },
        );

        let strat = parse_pool_strategy(&p_sec.strategy, p_name);
        let pool = Arc::new(KeyPool::new(p_name, strat));
        for k in &p_sec.keys {
            if k.is_antigravity(p_sec.default_protocol, p_name) {
                if let Ok(cred) = k.to_antigravity_credential() {
                    let effective_proxy = p_sec
                        .proxy
                        .as_deref()
                        .or(config_file.gateway.proxy.as_deref());
                    let http_client =
                        ponyllm_core::executor::create_upstream_http_client_with_options(
                            effective_proxy,
                            config_file.gateway.use_system_proxy,
                        );
                    let mgr = Arc::new(ponyllm_core::pool::AntigravityTokenManager::new(
                        &k.id,
                        cred,
                        http_client,
                    ));
                    pool.add_key(ApiKeyEntry::new_antigravity(
                        &k.id, mgr, k.priority, k.weight,
                    ));
                    continue;
                }
            }
            pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
        }
        pools.insert(p_name.clone(), pool);
    }

    (gw_config, pools)
}

struct ServerOptions {
    config: Option<String>,
    config_backend: String,
    bind: Option<String>,
    address: Option<String>,
    port: Option<u16>,
    api_key: Option<String>,
    retries: Option<usize>,
    no_web: bool,
    web_dist_dir: Option<String>,
    is_web_focused: bool,
    open_browser: bool,
    debug: bool,
}

/// [`ponyllm_server::config_poller::ConfigSource`] over the Kubernetes
/// config store: poll the Secret and derive the identity from the RAW config
/// bytes (SHA-256 over `data['ponyllm.toml']` BEFORE parsing — P1-arch S1-1).
/// Hashing the parsed config would be nondeterministic (HashMap iteration
/// order), so this is the only stable change signal.
struct KubeStoreSource {
    store: std::sync::Arc<ponyllm_server::admin_store::KubernetesConfigStore>,
}

impl KubeStoreSource {
    fn new(store: std::sync::Arc<ponyllm_server::admin_store::KubernetesConfigStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl ponyllm_server::config_poller::ConfigSource for KubeStoreSource {
    async fn snapshot(&self) -> Result<(String, ConfigFile), String> {
        self.store.load_raw_hash().await.map_err(|e| e.to_string())
    }
}

fn open_in_browser(url: &str) {
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
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = url;
    }
}

async fn run_server(opts: ServerOptions) -> Result<(), Box<dyn std::error::Error>> {
    let default_filter = if opts.debug {
        "ponyllm_server=debug,ponyllm_protocol=debug,ponyllm_core=debug,tower_http=debug"
    } else {
        "ponyllm_server=info,ponyllm_protocol=info,ponyllm_core=info,tower_http=debug"
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .ok();

    // H2: the loopback egress hatch is test/dev-only. A production process
    // with it set silently disables SSRF protection for 127/8 on admin
    // probes — fail loud here so misconfiguration is impossible to miss.
    if std::env::var("PONYLLM_ALLOW_LOOPBACK_PROBE").as_deref() == Ok("1") {
        tracing::warn!(
            "PONYLLM_ALLOW_LOOPBACK_PROBE=1 is set: admin-probe egress guard accepts loopback targets. Never enable in production."
        );
        eprintln!(
            "⚠️ [安全警告] PONYLLM_ALLOW_LOOPBACK_PROBE=1 已设置：管理探针出站守卫放行回环地址，仅允许测试/本地开发使用，生产环境禁止设置！"
        );
    }

    let resolved_config = resolve_path(opts.config.as_deref());

    // Config truth-source backend (multi-node HA): `file` (default, local
    // TOML) or `kubernetes` (Secret `ponyllm-live-config`). The backend's
    // initial load replaces the local file load for kubernetes.
    let config_backend = opts.config_backend.clone();
    // The kubernetes backend also yields the startup raw-bytes content hash,
    // which seeds the poller's change-detection baseline (P1-arch S3-1: a
    // Secret change between startup and the first poll must still fire).
    let mut poller_initial_identity: Option<String> = None;
    let mut kube_source: Option<
        std::sync::Arc<ponyllm_server::admin_store::KubernetesConfigStore>,
    > = None;
    let store: std::sync::Arc<dyn ponyllm_server::admin_store::ConfigStore> = match config_backend
        .as_str()
    {
        "file" => std::sync::Arc::new(ponyllm_server::admin_store::FileConfigStore::new(
            resolved_config.to_str().unwrap_or("ponyllm.toml"),
        )),
        "kubernetes" => {
            let k =
                ponyllm_server::admin_store::KubernetesConfigStore::from_env("ponyllm-live-config")
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> {
                        format!("kubernetes config backend init failed: {}", e).into()
                    })?;
            let kube_arc = std::sync::Arc::new(k);
            kube_source = Some(kube_arc.clone());
            // The store handed to AppState IS the Kubernetes store; the
            // poller additionally wraps it in KubeStoreSource for the
            // raw-bytes-hash identity.
            kube_arc.clone() as std::sync::Arc<dyn ponyllm_server::admin_store::ConfigStore>
        }
        other => {
            return Err(format!("unknown --config-backend '{}' (file|kubernetes)", other).into())
        }
    };

    let mut config_file = if config_backend == "kubernetes" {
        let src = KubeStoreSource::new(
            kube_source
                .clone()
                .expect("kube source set for kubernetes backend"),
        );
        let (hash, cfg) = src
            .snapshot()
            .await
            .map_err(|e| -> Box<dyn std::error::Error> {
                format!("kubernetes config backend load failed: {}", e).into()
            })?;
        poller_initial_identity = Some(hash);
        cfg
    } else {
        ConfigFile::load_or_default(resolved_config.to_str())?
    };

    let final_bind = if let Some(b) = opts.bind {
        b
    } else if let (Some(addr), Some(p)) = (opts.address.as_deref(), opts.port) {
        format!("{}:{}", addr, p)
    } else if let Some(addr) = opts.address {
        format!("{}:8080", addr)
    } else if let Some(p) = opts.port {
        format!("127.0.0.1:{}", p)
    } else {
        config_file.gateway.bind.clone()
    };

    let mut newly_generated_key = None;
    let final_api_key = if let Some(ak) = opts.api_key {
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

    let (mut gw_config, pools) = build_gateway_config_and_pools(
        &config_file,
        Some(final_bind.clone()),
        opts.retries,
        Some(final_api_key.clone()),
        // `--no-web` is a process-level switch: hot reload must not flip it
        // back on when the config file still says `web_enabled = true`.
        opts.no_web.then_some(false),
        opts.web_dist_dir.clone(),
    );
    // P0 open-mode guard (contract §4): open (empty/`none` key) on a
    // non-loopback bind refuses to start (fail-fast, non-zero exit).
    if let Err(reason) = ponyllm_config::validate_bind_auth_combo(
        &final_bind,
        &final_api_key,
        config_file.gateway.auth_compat,
    ) {
        eprintln!("❌ {}", reason);
        return Err(reason.into());
    }
    // Phase-2 F1: explicit `auth_mode="open"` on a non-loopback bind is
    // refused too — an open gateway must never face the public network,
    // even when a (now-ignored) key is present.
    if config_file.gateway.auth_mode == ponyllm_config::AuthMode::Open {
        let host = final_bind
            .split_once(':')
            .map(|(h, _)| h.trim())
            .unwrap_or(final_bind.trim());
        let loopback = host.eq_ignore_ascii_case("127.0.0.1")
            || host.eq_ignore_ascii_case("localhost")
            || host == "::1"
            || host == "[::1]";
        if !loopback {
            return Err(format!(
                "拒绝启动：auth_mode='open'（免鉴权）禁止绑定非环回地址 '{}'。请改用 secured 模式并配置网关口令。",
                final_bind
            )
            .into());
        }
    }
    if gw_config.telemetry_snapshot_path.is_none() {
        let snap = ponyllm_server::telemetry_snapshot::snapshot_path_for_config(
            None,
            gw_config.event_log_dir.as_deref(),
            Some(&resolved_config),
        );
        gw_config.telemetry_snapshot_path = snap.map(|p| p.to_string_lossy().into_owned());
    }

    // Graceful-shutdown watch: `true` = draining. Shared with the config
    // poller (stops), the antigravity worker (skips), the refresh persist
    // hook (no writes) and the axum graceful shutdown future.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let mut state = AppState::new(gw_config.clone())
        .with_config_store(store.clone())
        .with_config_poll_ms(if config_backend == "kubernetes" {
            ponyllm_server::config_poller::KUBERNETES_POLL_INTERVAL_MS
        } else {
            ponyllm_server::state::FILE_CONFIG_POLL_MS
        })
        .with_shutdown_rx(shutdown_rx.clone());
    // Graceful-drain flag for the refresh gate: once true, the gate refuses
    // acquisitions so no OAuth refresh starts during drain (P1-arch S3-4).
    let draining = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Cross-replica antigravity refresh serialization: enabled only when the
    // operator provides the lock DB (multi-node deployments; Phase 2+).
    if std::env::var("PONYLLM_LOCK_DATABASE_URL")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        let lock =
            ponyllm_server::refresh_lock::PostgresRefreshLock::new(Some(state.metrics.clone()))
                .with_draining(draining.clone());
        let gate: std::sync::Arc<dyn ponyllm_core::pool::refresh_gate::RefreshGate> =
            std::sync::Arc::new(lock);
        state = state.with_refresh_gate(Some(gate));
        tracing::info!("antigravity refresh serialization enabled (PONYLLM_LOCK_DATABASE_URL set)");
    } else {
        // Fail loud: a multi-replica deployment missing the lock DB silently
        // regresses to concurrent refreshes on one egress IP.
        tracing::warn!(
            "antigravity refresh serialization DISABLED: PONYLLM_LOCK_DATABASE_URL is not set.              Multi-replica deployments MUST set it (same-egress-IP concurrent refresh risk)."
        );
    }
    let state = Arc::new(state);
    for (p_name, pool) in pools {
        state.register_pool(&p_name, pool);
    }
    state.attach_antigravity_rotation_hooks_all();
    state.spawn_antigravity_auto_refresh_worker();

    // Spawn background config watcher for zero-downtime hot reload
    let watcher_path = resolved_config.clone();
    let watcher_state = state.clone();
    let watcher_bind = final_bind.clone();
    let watcher_retries = opts.retries;
    // Web hosting is restart-only (the axum router is built once in
    // create_app): pin the process-level CLI switches so a config-file
    // edit can never silently flip them mid-flight; a file-side
    // `web_enabled`/`web_dist_dir` change takes effect on restart.
    // Only pin when the CLI flag was explicitly given (P2-1).
    let watcher_web_enabled = opts.no_web.then_some(false);
    let watcher_web_dist_dir = opts.web_dist_dir.clone();
    if config_backend == "kubernetes" {
        // Kubernetes backend: 2s raw-bytes-hash poll of the Secret truth source.
        let source = KubeStoreSource::new(
            kube_source
                .clone()
                .expect("kube source for kubernetes backend"),
        );
        let st_poll = state.clone();
        let st_change = state.clone();
        let initial_identity = poller_initial_identity.take();
        tokio::spawn(async move {
            let source = source;
            let bind = watcher_bind.clone();
            let retries = watcher_retries;
            let web_enabled = watcher_web_enabled;
            let web_dist_dir = watcher_web_dist_dir.clone();
            let on_change = move |mut new_cfg_file: ConfigFile| {
                let st = st_change.clone();
                let bind = bind.clone();
                let web_dist_dir = web_dist_dir.clone();
                tokio::spawn(async move {
                    // Rebuild-time freshness guard (HA S1-3): never let a
                    // stale Secret snapshot clobber a token we just rotated.
                    st.apply_token_freshness_guard(&mut new_cfg_file).await;
                    let (new_gw_cfg, new_pools) = build_gateway_config_and_pools(
                        &new_cfg_file,
                        Some(bind),
                        retries,
                        None,
                        web_enabled,
                        web_dist_dir,
                    );
                    st.reload_config_with_pools(new_gw_cfg, new_pools);
                    tracing::info!(
                        "kubernetes config change applied (hot reload) — config_reload_total incremented"
                    );
                    println!("\n🔄 [配置热更新] Secret 内容变更，网关已完成零停机平滑热重载！");
                });
            };
            let stop_flag = st_poll.clone();
            ponyllm_server::config_poller::run_config_poller(
                &source,
                std::time::Duration::from_millis(
                    ponyllm_server::config_poller::KUBERNETES_POLL_INTERVAL_MS,
                ),
                initial_identity,
                on_change,
                move || *stop_flag.shutdown_rx.borrow(),
            )
            .await;
        });
    } else {
        // File backend: legacy mtime watcher (500ms), stops on drain.
        tokio::spawn(async move {
            let mut last_modified = std::fs::metadata(&watcher_path)
                .and_then(|m| m.modified())
                .ok();

            loop {
                if *watcher_state.shutdown_rx.borrow() {
                    tracing::info!("config file watcher stopped (draining)");
                    return;
                }
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
                                // Pin the process-level switch across hot reloads.
                                watcher_web_enabled,
                                watcher_web_dist_dir.clone(),
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
    }

    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind(&gw_config.bind_addr).await?;

    let (host, p_str) = gw_config
        .bind_addr
        .split_once(':')
        .unwrap_or(("127.0.0.1", "8080"));

    let is_all_interfaces = host == "0.0.0.0";
    let probe_host = if is_all_interfaces { "127.0.0.1" } else { host };
    let has_token =
        !gw_config.api_key.is_empty() && !gw_config.api_key.eq_ignore_ascii_case("none");
    let web_base_url = format!("http://{}:{}/", probe_host, p_str);
    let web_direct_url = if has_token {
        // R9: 复用 format_web_status_url —— encodeURIComponent 等价编码 + fragment
        // 传递，避免特殊字符截断/误解码（与 cli.rs 同一来源，防漂移）。
        format_web_status_url(&web_base_url, true, &gw_config.api_key)
    } else {
        web_base_url.clone()
    };

    // Phase-2 F1: emptiness no longer implies open — display the real mode.
    let auth_display = if gw_config.auth_mode == ponyllm_config::AuthMode::Open {
        "免鉴权 (显式 auth_mode=open，仅限环回绑定)".to_string()
    } else if gw_config.api_key.is_empty() || gw_config.api_key.eq_ignore_ascii_case("none") {
        "未配置凭证 (secured fail-closed：所有 API 需认证)".to_string()
    } else {
        gw_config.api_key.clone()
    };

    if let Some(gen_k) = &newly_generated_key {
        println!(
            "\n💡 [自动生成访问凭证] 检测到未配置 API Key，已自动生成并保存高熵秘钥: {}",
            gen_k
        );
    }

    // WEB-01 P1-3: web mount state is ops-visible (absolute dist path +
    // enabled/dist-hit status) so a CWD-dependent miss is diagnosable.
    let dist_abs = std::path::Path::new(&gw_config.web_dist_dir)
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| format!("{} (缺失)", gw_config.web_dist_dir));
    let hit = std::path::Path::new(&gw_config.web_dist_dir)
        .join("index.html")
        .is_file();
    let web_state = if !gw_config.web_enabled {
        format!("已关闭 (--no-web) | {}", dist_abs)
    } else if hit {
        format!("已托管 /* + /app/* | {}", dist_abs)
    } else {
        format!("目录缺失仅告警 | {}", dist_abs)
    };
    tracing::info!(web_enabled = gw_config.web_enabled, web_dist_dir = %dist_abs, dist_hit = hit, "web console mount state");

    if opts.is_web_focused {
        println!("\n╔════════════════════════════════════════════════════════════════════════╗");
        println!("║              🌐 ponyllm Web 控制台服务已就绪                           ║");
        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!("║  • 控制台根路径:      {:<48} ║", web_base_url);
        println!("║  • 访问凭证 (Token):  {:<48} ║", auth_display);
        println!("║  • 监听地址:          {:<48} ║", gw_config.bind_addr);
        println!(
            "║  • API 接入点:        {:<48} ║",
            format!("http://{}:{}/v1", probe_host, p_str)
        );
        // 全局调度策略配置已移除（wave-2）；排序由 Auto 智能路由接管，此行不再由配置驱动。
        println!("║  • 路由模式:          {:<48} ║", "Auto 智能路由");
        println!(
            "║  • 配置文件路径:      {:<48} ║",
            resolved_config.display()
        );
        println!(
            "║  • Web 托管状态:      {:<48} ║",
            web_state.chars().take(44).collect::<String>()
        );
        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!("║  • 已挂载模型提供商 (Providers & Pricing):                             ║");
        for (p_name, p_sec) in &config_file.providers {
            let pricing_tag = if p_sec.is_free() {
                "0元免费".to_string()
            } else if p_sec.billing_mode == ponyllm_core::pool::BillingMode::Plan {
                "Plan套餐".to_string()
            } else {
                format!(
                    "入${:.2}/缓${:.3}/出${:.2}",
                    p_sec.input_price, p_sec.cached_price, p_sec.output_price
                )
            };
            let all_models = p_sec.list_all_models();
            let m_names: Vec<String> = all_models
                .into_iter()
                .map(|m| {
                    if m.name == p_sec.default_model {
                        format!("{} (★默认,{})", m.name, m.tier.shorthand())
                    } else {
                        format!("{}({})", m.name, m.tier.shorthand())
                    }
                })
                .collect();
            println!(
                "║    - {:<10} [{:<8}]: {}",
                p_name,
                pricing_tag,
                m_names.join(", ")
            );
        }
        println!("╚════════════════════════════════════════════════════════════════════════╝");
        if has_token {
            println!("\n👉 控制台免密授权直达链接 (Ctrl+点击直接在浏览器打开):");
            println!("   \x1b[4;36m{}\x1b[0m\n", web_direct_url);
        } else {
            println!("\n👉 控制台直达链接 (Ctrl+点击直接在浏览器打开):");
            println!("   \x1b[4;36m{}\x1b[0m\n", web_base_url);
        }
    } else {
        println!("\n╔════════════════════════════════════════════════════════════════════════╗");
        println!("║              🚀 ponyllm AI Gateway 服务已就绪                          ║");
        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!(
            "║  • 配置文件路径:      {:<48} ║",
            resolved_config.display()
        );
        println!("║  • 本地接入 Base URL:                                                  ║");
        if is_all_interfaces {
            println!(
                "║    - OpenAI 客户端:   http://127.0.0.1:{}/v1 (局域网: http://0.0.0.0:{}/v1)║",
                p_str, p_str
            );
            println!(
                "║    - Anthropic 客户端: http://127.0.0.1:{}    (局域网: http://0.0.0.0:{})   ║",
                p_str, p_str
            );
        } else {
            println!(
                "║    - OpenAI 客户端:   http://{}:{}/v1                             ║",
                host, p_str
            );
            println!(
                "║    - Anthropic 客户端: http://{}:{}                                ║",
                host, p_str
            );
        }
        println!(
            "║    - 监听全地址:      http://{}                                     ║",
            gw_config.bind_addr
        );
        // 全局调度策略配置已移除（wave-2）；排序由 Auto 智能路由接管，此行不再由配置驱动。
        println!("║  • 路由模式:          {:<48} ║", "Auto 智能路由");
        println!(
            "║  • 请求体缓冲上限:    {:<48} ║",
            format!(
                "{} MB (支持1M长上下文/多模态)",
                gw_config.request_body_limit / (1024 * 1024)
            )
        );
        println!(
            "║  • 访问凭证 (Token):  {}                                   ║",
            format!("{:<30}", auth_display)
        );
        println!("║  • 虚拟总代模型:      auto, auto:flagship, auto:economy, auto[1m]     ║");
        println!(
            "║  • Web 控制台:        {:<48} ║",
            web_state.chars().take(44).collect::<String>()
        );
        println!("╠════════════════════════════════════════════════════════════════════════╣");
        println!("║  • 已挂载模型提供商 (Providers & Pricing):                             ║");
        for (p_name, p_sec) in &config_file.providers {
            let pricing_tag = if p_sec.is_free() {
                "0元免费".to_string()
            } else if p_sec.billing_mode == ponyllm_core::pool::BillingMode::Plan {
                "Plan套餐".to_string()
            } else {
                format!(
                    "入${:.2}/缓${:.3}/出${:.2}",
                    p_sec.input_price, p_sec.cached_price, p_sec.output_price
                )
            };
            let all_models = p_sec.list_all_models();
            let m_names: Vec<String> = all_models
                .into_iter()
                .map(|m| {
                    if m.name == p_sec.default_model {
                        format!("{} (★默认,{})", m.name, m.tier.shorthand())
                    } else {
                        format!("{}({})", m.name, m.tier.shorthand())
                    }
                })
                .collect();
            println!(
                "║    - {:<10} [{:<8}]: {}",
                p_name,
                pricing_tag,
                m_names.join(", ")
            );
        }
        println!("╚════════════════════════════════════════════════════════════════════════╝\n");
    }

    if opts.open_browser {
        println!(
            "🚀 正在自动在默认浏览器中打开 Web 控制台: {}",
            web_direct_url
        );
        open_in_browser(&web_direct_url);
    }

    // 声明 pidfile 归属，供 `ponyllm stop/restart` 认领本实例。
    if let Some(warn) = ponyllm_cli::lifecycle::claim_pidfile(&resolved_config) {
        eprintln!("{}", warn);
    }

    // Graceful shutdown (P1-OPS-001): SIGTERM/SIGINT → stop accepting new
    // connections → drain in-flight requests (SSE included) up to the drain
    // deadline → force exit. The drain deadline must stay below the
    // Deployment `terminationGracePeriodSeconds` minus `preStop` sleep.
    let mut serve_task = tokio::spawn(ponyllm_server::serve::serve_with_shutdown(
        listener,
        app,
        shutdown_rx,
        ponyllm_server::serve::DEFAULT_DRAIN_TIMEOUT,
    ));

    // Wait for a termination signal or a natural server exit.
    let signaled = tokio::select! {
        _ = shutdown_signal() => {
            tracing::info!("termination signal received; starting graceful drain");
            draining.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = shutdown_tx.send(true);
            true
        }
        res = &mut serve_task => {
            // The server finished on its own (listener error etc.).
            ponyllm_cli::lifecycle::release_pidfile(&resolved_config);
            return match res {
                Ok(result) => result.map_err(|e| -> Box<dyn std::error::Error> { e.into() }),
                Err(e) => Err(e.into()),
            };
        }
    };
    debug_assert!(signaled);

    // Draining: wait for the serve task to finish (drain deadline is inside
    // serve_with_shutdown). On timeout, the task is aborted (connections
    // force-closed) — the client-retry contract from the HA ADR applies.
    match tokio::time::timeout(ponyllm_server::serve::DEFAULT_DRAIN_TIMEOUT, serve_task).await {
        Ok(res) => {
            ponyllm_cli::lifecycle::release_pidfile(&resolved_config);
            // `res: Result<io::Result<()>, JoinError>`; serve already applied
            // its internal drain deadline, so unwind join first.
            res.map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        }
        Err(_) => {
            tracing::warn!("graceful drain did not finish in time; aborting serve task");
            ponyllm_cli::lifecycle::release_pidfile(&resolved_config);
        }
    }

    Ok(())
}

/// Cross-platform SIGTERM/SIGINT wait. Unix installs real handlers; other
/// platforms fall back to Ctrl-C.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut interrupt = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            output,
            non_interactive,
        } => {
            if non_interactive {
                if std::path::Path::new(&output).exists() {
                    return Err(format!(
                        "目标配置文件 '{}' 已存在，非交互模式禁止静默覆写",
                        output
                    )
                    .into());
                }
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
                println!(
                    "{:<14} {:<28} {:<22} {:<8} {:<10} {:<10} {:<22} {:<18} {:<6}",
                    "提供商",
                    "Base URL",
                    "默认模型",
                    "模式",
                    "策略",
                    "原生协议",
                    "基准资费($/1M:入/缓/出)",
                    "代理(Proxy)",
                    "Keys"
                );
                println!("{}", "-".repeat(145));
                for (name, p) in &cfg.providers {
                    let mode_str = match p.billing_mode {
                        ponyllm_core::pool::BillingMode::Plan => "plan(套餐)",
                        ponyllm_core::pool::BillingMode::Metered => "metered",
                        ponyllm_core::pool::BillingMode::Free => "free(免费)",
                    };
                    let proto_str = p
                        .default_protocol
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "auto(启发式)".to_string());
                    let pricing_str = format!(
                        "{:.2}/{:.3}/{:.2}",
                        p.input_price, p.cached_price, p.output_price
                    );
                    let proxy_str = p.proxy.as_deref().unwrap_or("-");
                    println!(
                        "{:<14} {:<28} {:<22} {:<8} {:<10} {:<10} {:<22} {:<18} {:<6}",
                        name,
                        p.base_url,
                        p.default_model,
                        mode_str,
                        p.strategy,
                        proto_str,
                        pricing_str,
                        proxy_str,
                        p.keys.len()
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
                default_protocol,
                chat_url,
                responses_url,
                messages_url,
                proxy,
                id,
                priority,
                weight,
                port,
                no_browser,
                config,
            } => {
                if name.eq_ignore_ascii_case("agy") || name.eq_ignore_ascii_case("antigravity") {
                    let resolved_proxy = match proxy.as_deref() {
                        Some("auto") => ponyllm_core::detect_system_proxy(),
                        Some("none") | Some("direct") => None,
                        Some(u) => {
                            let trimmed = u.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        }
                        None => None,
                    };
                    ponyllm_cli::oauth_agy::handle_key_auth_agy(
                        &name,
                        id.as_deref(),
                        priority,
                        weight,
                        port,
                        no_browser,
                        config.as_deref(),
                        resolved_proxy.as_deref(),
                    )
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
                    return Ok(());
                }

                if input_price < 0.0 || input_price.is_nan() || input_price.is_infinite() {
                    return Err(format!(
                        "常规输入单价 --input-price 必须为大于等于 0 的合法数值，输入: {}",
                        input_price
                    )
                    .into());
                }
                if cached_price < 0.0 || cached_price.is_nan() || cached_price.is_infinite() {
                    return Err(format!(
                        "缓存命中单价 --cached-price 必须为大于等于 0 的合法数值，输入: {}",
                        cached_price
                    )
                    .into());
                }
                if output_price < 0.0 || output_price.is_nan() || output_price.is_infinite() {
                    return Err(format!(
                        "输出生成单价 --output-price 必须为大于等于 0 的合法数值，输入: {}",
                        output_price
                    )
                    .into());
                }
                ponyllm_cli::config::validate_provider_fields(
                    &base_url,
                    &model,
                    &strategy,
                    &billing_mode,
                    chat_url.as_deref(),
                    responses_url.as_deref(),
                    messages_url.as_deref(),
                )
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

                let resolved_proxy = match proxy.as_deref() {
                    Some("auto") => {
                        let detected = ponyllm_core::detect_system_proxy();
                        if let Some(ref d) = detected {
                            println!("🔍 已自动探测到系统代理: {}", d);
                        } else {
                            println!("⚠️ 未探测到系统活动代理，保持直连");
                        }
                        detected
                    }
                    Some("none") | Some("direct") => None,
                    Some(u) => {
                        let trimmed = u.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        }
                    }
                    None => None,
                };

                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
                    .unwrap_or_default();
                let mode = match billing_mode.trim().to_ascii_lowercase().as_str() {
                    "plan" => ponyllm_core::pool::BillingMode::Plan,
                    "free" => ponyllm_core::pool::BillingMode::Free,
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
                if default_protocol.is_some()
                    || chat_url.is_some()
                    || responses_url.is_some()
                    || messages_url.is_some()
                    || resolved_proxy.is_some()
                {
                    if let Some(p) = cfg.providers.get_mut(&name) {
                        p.default_protocol = default_protocol;
                        if chat_url.is_some() {
                            p.chat_url = chat_url.clone();
                        }
                        if responses_url.is_some() {
                            p.responses_url = responses_url.clone();
                        }
                        if messages_url.is_some() {
                            p.messages_url = messages_url.clone();
                        }
                        if resolved_proxy.is_some() {
                            p.proxy = resolved_proxy.clone();
                        }
                    }
                }
                cfg.save_to_path(path)?;
                let proxy_info = resolved_proxy
                    .map(|p| format!(", 代理: {}", p))
                    .unwrap_or_default();
                println!(
                    "✅ 成功添加/更新提供商 '{}' (Base URL: {}, Model: {}, 资费: {}/{}/{}{})",
                    name, base_url, model, input_price, cached_price, output_price, proxy_info
                );
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
                println!(
                    "{:<15} {:<20} {:<25} {:<8} {:<8}",
                    "所属提供商", "Key ID", "API Key (已脱敏)", "优先级", "权重"
                );
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
                            p_name,
                            k.id,
                            k.masked_display_key(),
                            k.priority,
                            k.weight
                        );
                    }
                }
            }
            KeyCommands::Add {
                provider,
                id,
                key,
                priority,
                weight,
                config,
            } => {
                if (provider.eq_ignore_ascii_case("agy")
                    || provider.eq_ignore_ascii_case("antigravity"))
                    && !key.starts_with("1//")
                    && !key.trim().starts_with('{')
                {
                    println!("💡 提示: 如需添加 Antigravity，请运行: ponyllm provider add agy");
                }
                let resolved = resolve_path(config.as_deref());
                let path = resolved.to_str().unwrap_or("ponyllm.toml");
                let mut cfg = ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists()))
                    .unwrap_or_default();
                cfg.add_key(&provider, &id, &key, priority, weight)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                cfg.save_to_path(path)?;
                println!(
                    "✅ 成功向提供商 '{}' 账户池添加/更新 Key '{}' (优先级: {}, 权重: {})",
                    provider, id, priority, weight
                );
                println!("   • 配置文件: {}", resolved.display());
            }
            KeyCommands::Remove {
                provider,
                id,
                config,
            } => {
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
            KeyCommands::Test { provider, config } => {
                handle_test_keys(provider, config).await?;
            }
            KeyCommands::Gateway {
                config,
                key,
                rotate,
                show,
            } => {
                handle_manage_gateway_auth(config.as_deref(), key, rotate, show)?;
            }
            KeyCommands::Auth {
                provider,
                id,
                priority,
                weight,
                port,
                no_browser,
                config,
            } => {
                ponyllm_cli::oauth_agy::handle_key_auth_agy(
                    &provider,
                    id.as_deref(),
                    priority,
                    weight,
                    port,
                    no_browser,
                    config.as_deref(),
                    None,
                )
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e })?;
            }
        },
        Commands::Model(cmd) => {
            match cmd {
                ModelCommands::List { config } => {
                    let resolved = resolve_path(config.as_deref());
                    let cfg = ConfigFile::load_or_default(resolved.to_str())?;
                    println!("=== 已配置的模型目录 ===");
                    println!(
                        "{:<12} {:<24} {:<6} {:<10} {:<8} {:<10} {:<10} {:<24} {:<20}",
                        "提供商",
                        "模型标识",
                        "梯队",
                        "模式",
                        "上下文",
                        "最大输出",
                        "原生协议",
                        "资费($/1M:入/缓/出)",
                        "网络代理(Proxy)"
                    );
                    println!("{}", "-".repeat(138));
                    for (p_name, p) in &cfg.providers {
                        for m in p.list_all_models() {
                            let is_def = if m.name == p.default_model {
                                " (★默认)"
                            } else {
                                ""
                            };
                            let proto_str = m
                                .protocol
                                .or(p.default_protocol)
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "auto".to_string());
                            let mode_desc = match p.get_model_billing_mode(&m.name) {
                                ponyllm_core::pool::BillingMode::Plan => "Plan(套餐)",
                                ponyllm_core::pool::BillingMode::Free => "0元免费",
                                ponyllm_core::pool::BillingMode::Metered => "按量计费",
                            };
                            let pricing_info = if m.input_price.is_some()
                                || m.cached_price.is_some()
                                || m.output_price.is_some()
                            {
                                let pr = p.get_model_pricing(&m.name);
                                format!(
                                    "★ {:.2}/{:.3}/{:.2}",
                                    pr.input_price, pr.cached_price, pr.output_price
                                )
                            } else {
                                let pr = p.pricing();
                                format!(
                                    "{:.2}/{:.3}/{:.2}(继承)",
                                    pr.input_price, pr.cached_price, pr.output_price
                                )
                            };
                            let proxy_desc = if let Some(ref pxy) = m.proxy {
                                if pxy.eq_ignore_ascii_case("direct")
                                    || pxy.eq_ignore_ascii_case("none")
                                {
                                    "★ 强制直连".to_string()
                                } else {
                                    format!("★ {}", pxy)
                                }
                            } else if let Some(ref pxy) = p.proxy {
                                format!("{}(继承)", pxy)
                            } else {
                                "- (直连)".to_string()
                            };
                            println!(
                                "{:<12} {:<24} {:<6} {:<10} {:<8} {:<10} {:<10} {:<24} {:<20}",
                                p_name,
                                format!("{}{}", m.name, is_def),
                                m.tier.shorthand(),
                                mode_desc,
                                m.context_window,
                                m.max_output,
                                proto_str,
                                pricing_info,
                                proxy_desc,
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
                    protocol,
                    proxy,
                    thinking_default,
                    thinking_max,
                    config,
                } => {
                    let resolved = resolve_path(config.as_deref());
                    let path = resolved.to_str().unwrap_or("ponyllm.toml");
                    let mut cfg = ConfigFile::load_or_default(Some(path))?;
                    let input_types: Vec<String> = inputs
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let output_types: Vec<String> = outputs
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let tier_val = <ponyllm_core::pool::ModelTier as std::str::FromStr>::from_str(&tier)
                    .map_err(|e| format!("无效的能力梯队 --tier '{}': {}。仅支持 Flagship (F), Standard (S), Light (L)", tier, e))?;

                    let mode_val = match billing_mode.as_deref() {
                        Some("plan") | Some("coding_plan") | Some("coding-plan") => {
                            Some(ponyllm_core::pool::BillingMode::Plan)
                        }
                        Some("metered") | Some("payg") => {
                            Some(ponyllm_core::pool::BillingMode::Metered)
                        }
                        Some("free") => Some(ponyllm_core::pool::BillingMode::Free),
                        Some(other) => {
                            return Err(format!(
                                "无效的计费模式 --billing-mode '{}'。仅支持 metered, plan, free",
                                other
                            )
                            .into());
                        }
                        None => None,
                    };

                    let model_proxy = match proxy.as_deref() {
                        Some("auto") => {
                            let detected = ponyllm_core::detect_system_proxy();
                            if let Some(ref d) = detected {
                                println!("🔍 已自动探测到系统代理: {}", d);
                            } else {
                                println!("⚠️ 未探测到系统活动代理，保持继承/直连");
                            }
                            detected
                        }
                        Some("direct") | Some("none") => Some("direct".to_string()),
                        Some(u) => {
                            let trimmed = u.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        }
                        None => None,
                    };

                    if let Some(p) = input_price {
                        if p < 0.0 || p.is_nan() || p.is_infinite() {
                            return Err(format!(
                                "常规输入单价 --input-price 必须为大于等于 0 的合法数值，输入: {}",
                                p
                            )
                            .into());
                        }
                    }
                    if let Some(p) = cached_price {
                        if p < 0.0 || p.is_nan() || p.is_infinite() {
                            return Err(format!(
                                "缓存命中单价 --cached-price 必须为大于等于 0 的合法数值，输入: {}",
                                p
                            )
                            .into());
                        }
                    }
                    if let Some(p) = output_price {
                        if p < 0.0 || p.is_nan() || p.is_infinite() {
                            return Err(format!(
                                "输出生成单价 --output-price 必须为大于等于 0 的合法数值，输入: {}",
                                p
                            )
                            .into());
                        }
                    }

                    let thinking_default_effort = thinking_default.as_deref().map(|s| {
                        ponyllm_core::pool::ModelThinkingSpec::match_4tier_effort(Some(s))
                    });
                    let thinking_max_effort = thinking_max.as_deref().map(|s| {
                        ponyllm_core::pool::ModelThinkingSpec::match_4tier_effort(Some(s))
                    });

                    let model_cfg = ponyllm_cli::config::ModelConfig {
                        name: model.clone(),
                        tier: tier_val,
                        priority: None,
                        billing_mode: mode_val,
                        context_window: context.clone(),
                        max_output: max_output.clone(),
                        input_types,
                        output_types,
                        input_price,
                        cached_price,
                        output_price,
                        pricing_mode: None,
                        pricing_periods: Vec::new(),
                        display_name: None,
                        temperature: None,
                        top_p: None,
                        protocol,
                        base_url: None,
                        thinking_default: thinking_default_effort,
                        thinking_max: thinking_max_effort,
                        proxy: model_proxy.clone(),
                        timeout_secs: None,
                        rate_limits: None,
                        fallbacks: Vec::new(),
                    };

                    cfg.upsert_model_config(&provider, model_cfg)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
                    cfg.save_to_path(path)?;
                    let mode_label = mode_val
                        .map(|m| format!("模式: {:?}", m))
                        .unwrap_or_else(|| "模式: 继承提供商".to_string());
                    let proxy_label = match model_proxy.as_deref() {
                        Some("direct") | Some("none") => ", 代理: 强制直连".to_string(),
                        Some(p) => format!(", 代理: {}", p),
                        None => "".to_string(),
                    };
                    println!(
                    "✅ 成功向提供商 '{}' 添加模型 '{}' [梯队: {}, {}] (上下文: {}, 输出: {}{})",
                    provider, model, tier_val.shorthand(), mode_label, context, max_output, proxy_label
                );
                    println!("   • 配置文件: {}", resolved.display());
                }
                ModelCommands::Remove {
                    provider,
                    model,
                    config,
                } => {
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
                ModelCommands::Set {
                    provider,
                    model,
                    config,
                } => {
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
            }
        }
        Commands::Auth {
            config,
            key,
            rotate,
            show,
        } => {
            handle_manage_gateway_auth(config.as_deref(), key, rotate, show)?;
        }
        Commands::Keys(cmd) => match cmd {
            KeysCommands::List { config } => {
                handle_gateway_keys_list(config.as_deref())?;
            }
            KeysCommands::Issue {
                scope,
                id,
                user,
                config,
            } => {
                handle_gateway_keys_issue(
                    config.as_deref(),
                    &scope,
                    id.as_deref(),
                    user.as_deref(),
                )?;
            }
            KeysCommands::Revoke { id, config } => {
                handle_gateway_keys_revoke(config.as_deref(), &id)?;
            }
        },
        Commands::User(cmd) => match cmd {
            UserCommands::List { config } => {
                handle_users_list(config.as_deref())?;
            }
            UserCommands::Add {
                id,
                name,
                models,
                max_tokens,
                enabled,
                config,
            } => {
                handle_users_add(
                    config.as_deref(),
                    &id,
                    name.as_deref(),
                    models.as_deref(),
                    max_tokens,
                    enabled,
                )?;
            }
            UserCommands::Remove { id, config } => {
                handle_users_remove(config.as_deref(), &id)?;
            }
            UserCommands::ResetUsage {
                id,
                gateway_url,
                api_key,
                config,
            } => {
                handle_users_reset_usage(config.as_deref(), &id, &gateway_url, api_key.as_deref())
                    .await?;
            }
        },
        Commands::Tui {
            config,
            gateway_url,
        } => {
            let resolved = resolve_path(config.as_deref());
            let path = resolved.to_str().unwrap_or("ponyllm.toml");
            let cfg = ConfigFile::load_or_default(Some(path))?;
            run_tui(cfg, path.to_string(), gateway_url).await?;
        }
        Commands::Serve {
            config,
            config_backend,
            bind,
            address,
            port,
            api_key,
            retries,
            no_web,
            web_dist_dir,
            debug,
        } => {
            run_server(ServerOptions {
                config,
                config_backend,
                bind,
                address,
                port,
                api_key,
                retries,
                no_web,
                web_dist_dir,
                is_web_focused: false,
                open_browser: false,
                debug,
            })
            .await?;
        }
        Commands::Web {
            config,
            config_backend,
            port,
            address,
            bind,
            api_key,
            web_dist_dir,
            no_open,
            open: _,
            debug,
        } => {
            run_server(ServerOptions {
                config,
                config_backend,
                bind,
                address: Some(address),
                port: Some(port),
                api_key,
                retries: None,
                no_web: false,
                web_dist_dir,
                is_web_focused: true,
                open_browser: !no_open,
                debug,
            })
            .await?;
        }
        Commands::Stop { config } => {
            match ponyllm_cli::lifecycle::stop_serve(config.as_deref()).await {
                Ok(msg) => println!("✅{}", msg),
                Err(e) => {
                    eprintln!("❌ {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Restart {
            config,
            config_backend,
            bind,
            address,
            port,
            api_key,
            retries,
            no_web,
            web_dist_dir,
        } => {
            match ponyllm_cli::lifecycle::restart_serve(
                config.as_deref(),
                config_backend,
                bind,
                address,
                port,
                api_key,
                retries,
                no_web,
                web_dist_dir,
            )
            .await
            {
                Ok(msg) => println!("✅{}", msg),
                Err(e) => {
                    eprintln!("❌ {}", e);
                    std::process::exit(1);
                }
            }
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
            let rec_url = format!(
                "{}/v1/telemetry/recorder",
                gateway_url.trim_end_matches('/')
            );

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
        Commands::Upgrade {
            check,
            force,
            dry_run,
            version,
        } => {
            ponyllm_cli::upgrade::run_upgrade(check, force, dry_run, version).await?;
            if !check && !dry_run {
                println!("💡 二进制已更新，正在运行的服务仍是旧代码：配置热更新管不到二进制，请执行 `ponyllm restart` 重启服务生效。");
            }
        }
    }

    Ok(())
}

fn handle_manage_gateway_auth(
    config_path: Option<&str>,
    custom_key: Option<String>,
    rotate: bool,
    show_plaintext: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();

    let action = parse_gateway_auth_action(custom_key.as_deref(), rotate);

    match action {
        GatewayAuthAction::MisdirectedList => {
            println!(
                "\n╔════════════════════════════════════════════════════════════════════════╗"
            );
            println!("║              💡 PonyLLM 访问凭证与 Key 管理指引                         ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 'ponyllm auth' 用于查看或管理【网关自身的对外访问凭证 (Token)】     ║");
            println!("║  • 若要查看网关访问 Token:     ponyllm auth  或  ponyllm status         ║");
            println!(
                "║  • 若要查看【上游模型厂商】Key: ponyllm key list                         ║"
            );
            println!(
                "╚════════════════════════════════════════════════════════════════════════╝\n"
            );
            return Ok(());
        }
        GatewayAuthAction::MisdirectedAgy => {
            println!("\n💡 如需添加 Antigravity，请运行：\n   👉 ponyllm provider add agy\n");
            return Ok(());
        }
        GatewayAuthAction::Show => {
            // P0: default masked display; `--show` reveals plaintext (contract §4).
            // C2 (Phase-2b): display follows the EXPLICIT `auth_mode`, not key
            // emptiness — secured+empty-key is NOT open (fail-closed default).
            let key_empty =
                cfg.gateway.api_key.is_empty() || cfg.gateway.api_key.eq_ignore_ascii_case("none");
            let current_key = if cfg.gateway.auth_mode == ponyllm_config::AuthMode::Open {
                "免鉴权 (显式开放模式)".to_string()
            } else if key_empty {
                "未配置凭证 (secured：所有 API 需认证)".to_string()
            } else if show_plaintext {
                cfg.gateway.api_key.clone()
            } else {
                ConfigFile::mask_key(&cfg.gateway.api_key)
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
            } else if current_key.starts_with("未配置凭证") {
                "" // secured + 未配置：无凭证可导出
            } else {
                &current_key
            };
            println!(
                "\n╔════════════════════════════════════════════════════════════════════════╗"
            );
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
            println!(
                "╚════════════════════════════════════════════════════════════════════════╝\n"
            );
            return Ok(());
        }
        GatewayAuthAction::Rotate => {
            let final_key = generate_secure_api_key();
            cfg.gateway.api_key = final_key.clone();
            cfg.save_to_path(path)?;

            println!(
                "\n╔════════════════════════════════════════════════════════════════════════╗"
            );
            println!("║              🔑 网关访问 API Key (Token) 已轮转就绪                   ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║                                                                        ║");
            println!("║   新 API Key:  {}", format!("{:<56}", final_key));
            println!("║                                                                        ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已同步持久化保存至: {:<46} ║", resolved.display());
            println!("║  • 请复制上方 API Key，用于 Cursor / Claude Code / SDK 鉴权连接。      ║");
            println!(
                "╚════════════════════════════════════════════════════════════════════════╝\n"
            );
        }
        GatewayAuthAction::Set(new_key) => {
            // P0 weak-key guard: refuse to persist `123456`-class secrets.
            if let Err(reason) = ponyllm_config::validate_gateway_key_strength(&new_key) {
                eprintln!("❌ {}", reason);
                return Err(reason.into());
            }
            cfg.gateway.api_key = new_key.clone();
            cfg.save_to_path(path)?;

            println!(
                "\n╔════════════════════════════════════════════════════════════════════════╗"
            );
            println!("║              🔑 网关访问 API Key (Token) 已更新就绪                   ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║                                                                        ║");
            println!("║   新 API Key:  {}", format!("{:<56}", new_key));
            println!("║                                                                        ║");
            println!("╠════════════════════════════════════════════════════════════════════════╣");
            println!("║  • 已同步持久化保存至: {:<46} ║", resolved.display());
            println!("║  • 请复制上方 API Key，用于 Cursor / Claude Code / SDK 鉴权连接。      ║");
            println!(
                "╚════════════════════════════════════════════════════════════════════════╝\n"
            );
        }
    }

    Ok(())
}

/// List scoped gateway keys (P1): id/scope/prefix only, never plaintext.
/// The legacy single `api_key` is shown as an implicit `admin` entry.
fn handle_gateway_keys_list(config_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use ponyllm_cli::config::KeyScope;
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    println!("=== 网关分级 Key（仅哈希存储，明文只在签发时显示一次） ===");
    println!(
        "{:<24} {:<10} {:<20} {:<10}",
        "ID", "SCOPE", "PREFIX", "STATUS"
    );
    println!("{}", "-".repeat(70));
    let open = cfg.gateway.api_key.is_empty() || cfg.gateway.api_key.eq_ignore_ascii_case("none");
    if !open {
        println!(
            "{:<24} {:<10} {:<20} {:<10}",
            "(legacy api_key)",
            KeyScope::Admin.as_str(),
            "sk-pony-*",
            "active"
        );
    }
    for k in &cfg.gateway.gateway_keys {
        // Deleted entries are gone entirely (hard delete since 2026-09-21);
        // only expiry can still mark a row non-active.
        let status = if let Some(exp) = k.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if now > exp {
                "expired"
            } else {
                "active"
            }
        } else {
            "active"
        };
        println!(
            "{:<24} {:<10} {:<20} {:<10}",
            k.id,
            k.scope.as_str(),
            format!("{}***", k.prefix),
            status
        );
    }
    if cfg.gateway.gateway_keys.is_empty() && open {
        // C2 (Phase-2b): the "empty" hint follows the explicit auth mode —
        // secured+empty is fail-closed, NOT an open gateway.
        if cfg.gateway.auth_mode == ponyllm_config::AuthMode::Open {
            println!("(空：显式开放模式，无凭证)");
        } else {
            println!("(未配置凭证：secured 模式，所有 API 需认证)");
        }
    }
    Ok(())
}

/// Issue a scoped gateway key (P1): plaintext shown ONCE, only the salted
/// SHA-256 hash is persisted. Fails when the id already exists.
fn handle_gateway_keys_issue(
    config_path: Option<&str>,
    scope_raw: &str,
    id_opt: Option<&str>,
    user_opt: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use ponyllm_cli::config::{generate_scoped_gateway_key, KeyScope};
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    let scope = match scope_raw.trim().to_ascii_lowercase().as_str() {
        "admin" => KeyScope::Admin,
        "inference" | "infer" => KeyScope::Inference,
        "readonly" | "read" => KeyScope::Readonly,
        other => {
            let msg = format!(
                "未知 scope '{}'（仅 admin | inference | readonly）；机器 key 永不签发 operator",
                other
            );
            eprintln!("❌ {}", msg);
            return Err(msg.into());
        }
    };
    let id = id_opt
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{}-{}",
                scope.as_str(),
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            )
        });
    if cfg.gateway.gateway_keys.iter().any(|k| k.id == id) {
        let msg = format!("Key id '{}' 已存在（先删除再重发，不做原地加权）", id);
        eprintln!("❌ {}", msg);
        return Err(msg.into());
    }
    let (plaintext, mut entry) = generate_scoped_gateway_key(&id, scope);
    entry.id = id.clone();
    entry.user_id = user_opt.map(|s| s.trim().to_string());
    cfg.gateway.gateway_keys.push(entry);
    cfg.save_to_path(path)?;
    println!("\n⚠️  明文仅显示一次，请立即复制保存；服务端只存哈希，丢失不可找回。");
    println!("scope={} id={}", scope.as_str(), id);
    if let Some(uid) = user_opt {
        println!("bound_user: {}", uid);
    }
    println!("key: {}", plaintext);
    Ok(())
}

/// Delete a scoped gateway key by id (hard delete since 2026-09-21: removes
/// the entry from disk + memory and keeps no record of it). Fail-closed
/// immediately.
fn handle_gateway_keys_revoke(
    config_path: Option<&str>,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    let before = cfg.gateway.gateway_keys.len();
    cfg.gateway.gateway_keys.retain(|k| k.id != id);
    if cfg.gateway.gateway_keys.len() == before {
        let msg = format!("Key id '{}' 不存在", id);
        eprintln!("❌ {}", msg);
        return Err(msg.into());
    }
    cfg.save_to_path(path)?;
    println!(
        "✅ Key id '{}' 已删除（无残留记录，热加载约 500ms 内全网生效）。",
        id
    );
    Ok(())
}

fn handle_users_list(config_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    println!("=== PonyLLM 用户列表 ===");
    println!(
        "{:<20} {:<15} {:<8} {:<15} {:<25}",
        "ID", "NAME", "ENABLED", "MAX_TOKENS", "ALLOWED_MODELS"
    );
    println!("{}", "-".repeat(85));
    if cfg.gateway.users.is_empty() {
        println!("(暂无配置用户)");
    } else {
        for u in &cfg.gateway.users {
            let max_t = u
                .max_tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unlimited".to_string());
            let models = u
                .allowed_models
                .as_ref()
                .map(|m| m.join(","))
                .unwrap_or_else(|| "* (all)".to_string());
            println!(
                "{:<20} {:<15} {:<8} {:<15} {:<25}",
                u.id,
                if u.name.is_empty() { "-" } else { &u.name },
                u.enabled,
                max_t,
                models
            );
        }
    }
    Ok(())
}

fn handle_users_add(
    config_path: Option<&str>,
    id: &str,
    name: Option<&str>,
    models: Option<&str>,
    max_tokens: Option<u64>,
    enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    let allowed_models = models.map(|m| {
        m.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    });
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let user_entry = ponyllm_config::UserEntry {
        id: id.trim().to_string(),
        name: name.unwrap_or_default().trim().to_string(),
        enabled,
        allowed_models,
        max_tokens,
        created_at: now,
        username: None,
        password_hash: None,
        role: ponyllm_config::UserRole::User,
        token_version: 0,
    };

    if let Some(existing) = cfg.gateway.users.iter_mut().find(|u| u.id == id) {
        *existing = user_entry;
        println!("✅ 用户 '{}' 配置已更新。", id);
    } else {
        cfg.gateway.users.push(user_entry);
        println!("✅ 用户 '{}' 已创建。", id);
    }
    cfg.save_to_path(path)?;
    Ok(())
}

fn handle_users_remove(
    config_path: Option<&str>,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let mut cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    let before = cfg.gateway.users.len();
    cfg.gateway.users.retain(|u| u.id != id);
    if cfg.gateway.users.len() == before {
        let msg = format!("用户 '{}' 不存在", id);
        eprintln!("❌ {}", msg);
        return Err(msg.into());
    }
    cfg.save_to_path(path)?;
    println!("✅ 用户 '{}' 已移除。", id);
    Ok(())
}

async fn handle_users_reset_usage(
    config_path: Option<&str>,
    id: &str,
    gateway_url: &str,
    api_key_override: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config_path);
    let path = resolved.to_str().unwrap_or("ponyllm.toml");
    let cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();
    let token = api_key_override.map(|s| s.to_string()).or_else(|| {
        if !cfg.gateway.api_key.is_empty() {
            Some(cfg.gateway.api_key.clone())
        } else {
            None
        }
    });
    let client = reqwest::Client::new();
    let url = format!(
        "{}/api/admin/users/{}/reset-usage",
        gateway_url.trim_end_matches('/'),
        id
    );
    let mut req = client.post(&url);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {}", t));
    }
    let resp = req.send().await?;
    if resp.status().is_success() {
        println!("✅ 用户 '{}' 的 Token 消耗用量已成功重置为 0。", id);
    } else {
        let err_text = resp.text().await?;
        eprintln!("❌ 重置失败: {}", err_text);
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
    let cfg =
        ConfigFile::load_or_default(Some(path).filter(|_| resolved.exists())).unwrap_or_default();

    let base_url = if let Some(u) = cli_gateway_url {
        u.trim_end_matches('/').to_string()
    } else {
        let (host, port) = cfg
            .gateway
            .bind
            .split_once(':')
            .unwrap_or(("127.0.0.1", "8080"));
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

    let has_key = !active_key.is_empty() && !active_key.eq_ignore_ascii_case("none");

    let use_color = std::env::var_os("NO_COLOR").is_none()
        && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let paint = |code: &str, text: &str| {
        if use_color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };

    let divider = paint("90", "──────────────────────────────────────────────────");

    println!("{}", divider);
    println!("【服务状态】");
    println!("{}", divider);
    if is_online {
        let version = health_json
            .as_ref()
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        println!("状态：{}", paint("32", &format!("运行中 {version}")));
    } else {
        println!("状态：{}", paint("31", "未运行"));
    }

    println!("地址：{}", base_url);
    println!("配置：{}", resolved.display());
    // 全局调度策略配置已移除（wave-2）；排序由 Auto 智能路由接管，此行不再由配置驱动。
    println!("策略：Auto 智能路由");

    println!();
    println!("{}", divider);
    println!("【连接】");
    println!("{}", divider);
    if has_key {
        println!("  密钥：{}", active_key);
    } else {
        println!("  密钥：未设置，不用填也能连");
    }

    let web_display = if cfg.gateway.web_enabled {
        let raw_url = format_web_status_url(&base_url, true, &active_key);
        paint("4;36", &raw_url)
    } else {
        format_web_status_url(&base_url, false, &active_key)
    };
    println!("  Web 控制台：{}", web_display);

    println!("  OpenAI 地址：{}/v1", base_url);
    println!("  Anthropic 地址：{}", base_url);

    println!();
    println!("{}", divider);
    if cfg.providers.is_empty() {
        println!("【提供商】");
        println!("{}", divider);
        println!("  还没有添加提供商，执行 ponyllm provider add 添加");
    } else {
        println!("【提供商 {} 个】", cfg.providers.len());
        println!("{}", divider);
        let mut ordered: Vec<(&String, _)> = cfg.providers.iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(b.0));
        for (p_name, p_sec) in ordered {
            if p_sec.keys.is_empty() {
                println!(
                    "  {}：默认模型 {}，还没有密钥，暂时不能用",
                    p_name, p_sec.default_model
                );
            } else if p_sec.keys.len() == 1 {
                println!(
                    "  {}：默认模型 {}，有 1 个密钥",
                    p_name, p_sec.default_model
                );
            } else {
                println!(
                    "  {}：默认模型 {}，有 {} 个密钥",
                    p_name,
                    p_sec.default_model,
                    p_sec.keys.len()
                );
            }
        }
    }

    if let Some(m) = metrics_json {
        let total_req = m
            .get("total_requests")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let succ_req = m
            .get("successful_requests")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let fail_req = m
            .get("failed_requests")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total_tokens = m.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

        println!();
        println!("{}", divider);
        println!("【用量】");
        println!("{}", divider);
        if total_req == 0 {
            println!("  还没有请求");
        } else if fail_req == 0 {
            println!(
                "  共 {} 次请求，都成功了，共用 {} Tokens",
                total_req, total_tokens
            );
        } else {
            println!(
                "  共 {} 次请求，成功 {} 次，失败 {} 次，共用 {} Tokens",
                total_req, succ_req, fail_req, total_tokens
            );
        }
    } else if is_online {
        println!();
        println!("{}", divider);
        println!("【用量】");
        println!("{}", divider);
        println!("  暂时看不到用量，检查密钥是否正确");
    }

    if !is_online {
        println!();
        println!("{}", divider);
        println!("【提示】");
        println!("{}", divider);
        println!("服务没有运行，先执行 ponyllm serve 启动。");
    }

    Ok(())
}

async fn handle_test_keys(
    provider: Option<String>,
    config: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_path(config.as_deref());
    let cfg = ConfigFile::load_or_default(resolved.to_str())?;

    println!("🔍 正在执行 API Key 连通性测试...");
    println!("   • 配置文件: {}", resolved.display());

    let mut tested_count = 0;
    let mut success_count = 0;

    for (p_name, p) in &cfg.providers {
        if let Some(ref target) = provider {
            if p_name != target {
                continue;
            }
        }

        println!("\n▶ 提供商: {} (端点: {})", p_name, p.base_url);
        let effective_proxy = p.proxy.as_deref().or(cfg.gateway.proxy.as_deref());
        if let Some(pxy) = effective_proxy {
            println!("  [网络代理] {}", pxy);
        }

        for k in &p.keys {
            tested_count += 1;
            print!("  • [{}] {} ... ", k.id, k.masked_display_key());
            std::io::Write::flush(&mut std::io::stdout())?;

            if k.is_antigravity(p.default_protocol, p_name) {
                match k.to_antigravity_credential() {
                    Ok(cred) => {
                        let client =
                            ponyllm_core::executor::create_upstream_http_client_with_options(
                                effective_proxy,
                                cfg.gateway.use_system_proxy,
                            );
                        let mgr =
                            ponyllm_core::pool::AntigravityTokenManager::new(&k.id, cred, client);
                        let start = std::time::Instant::now();
                        match mgr.get_valid_token().await {
                            Ok(_) => {
                                let latency = start.elapsed();
                                println!("✅ 有效 (OAuth续期成功, 耗时 {}ms)", latency.as_millis());
                                success_count += 1;

                                print!("    ↳ 正在抓取 Antigravity 模型配额与重置时间... ");
                                std::io::Write::flush(&mut std::io::stdout())?;
                                match mgr.fetch_quota(Some(&p.base_url)).await {
                                    Ok(snapshot) => {
                                        println!(
                                            "✅ 成功 (获取到 {} 个模型)",
                                            snapshot.models.len()
                                        );
                                        println!(
                                            "      {:<28} {:<12} {:<24} {:<16}",
                                            "模型", "剩余额度", "恢复时间(北京时间)", "距离恢复"
                                        );
                                        println!("      {}", "-".repeat(82));
                                        let mut models: Vec<_> = snapshot.models.values().collect();
                                        models.sort_by_key(|m| &m.model_id);
                                        for m in models {
                                            let pct =
                                                format!("{:.1}%", m.remaining_fraction * 100.0);
                                            let (beijing_time, remaining_desc) = match m.reset_time
                                            {
                                                Some(utc_dt) => {
                                                    let bj_dt = utc_dt + chrono::Duration::hours(8);
                                                    let now = chrono::Utc::now();
                                                    let diff = if utc_dt > now {
                                                        let dur = utc_dt - now;
                                                        let hours = dur.num_hours();
                                                        let mins = dur.num_minutes() % 60;
                                                        format!("{}小时{}分后", hours, mins)
                                                    } else {
                                                        "已就绪".to_string()
                                                    };
                                                    (
                                                        bj_dt
                                                            .format("%Y-%m-%d %H:%M:%S")
                                                            .to_string(),
                                                        diff,
                                                    )
                                                }
                                                None => ("N/A".to_string(), "N/A".to_string()),
                                            };
                                            println!(
                                                "      {:<28} {:<12} {:<24} {:<16}",
                                                m.model_id, pct, beijing_time, remaining_desc
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        println!("⚠️ 额度抓取异常: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                println!("❌ 鉴权失败: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        println!("❌ 凭证格式错误: {}", e);
                    }
                }
            } else {
                let client = ponyllm_core::executor::create_upstream_http_client_with_options(
                    effective_proxy,
                    cfg.gateway.use_system_proxy,
                );
                let probe_url = if let Some(ref chat) = p.chat_url {
                    chat.clone()
                } else {
                    format!("{}/models", p.base_url.trim_end_matches('/'))
                };

                let start = std::time::Instant::now();
                let mut req = client
                    .get(&probe_url)
                    .timeout(std::time::Duration::from_secs(5))
                    .header(reqwest::header::USER_AGENT, "ponyllm-cli/dialtest");

                let is_anthropic = p
                    .default_protocol
                    .map(|p| p.is_anthropic())
                    .unwrap_or(false)
                    || p.base_url.contains("anthropic");

                if is_anthropic {
                    req = req
                        .header("x-api-key", &k.api_key)
                        .header("anthropic-version", "2023-06-01");
                } else {
                    req = req.header(
                        reqwest::header::AUTHORIZATION,
                        format!("Bearer {}", k.api_key),
                    );
                }

                match req.send().await {
                    Ok(resp) => {
                        let latency = start.elapsed();
                        if resp.status().is_success() {
                            println!("✅ 健康 (HTTP 200, 耗时 {}ms)", latency.as_millis());
                            success_count += 1;
                        } else {
                            println!(
                                "⚠️ 异常 (HTTP {}, 耗时 {}ms)",
                                resp.status().as_u16(),
                                latency.as_millis()
                            );
                        }
                    }
                    Err(e) => {
                        println!("❌ 网络连接失败: {}", e);
                    }
                }
            }
        }
    }

    println!("\n=== 探测总结 ===");
    println!(
        "共测试 {} 个密钥凭证，成功: {}，失败/异常: {}",
        tested_count,
        success_count,
        tested_count - success_count
    );
    Ok(())
}

#[cfg(test)]
mod pool_strategy_tests {
    use super::parse_pool_strategy;
    use ponyllm_core::pool::RoutingStrategy;

    #[test]
    fn explicit_round_robin_maps_to_round_robin() {
        // Guard the opt-out: deleting the RR arm must turn this red.
        assert_eq!(
            parse_pool_strategy("round_robin", "p"),
            RoutingStrategy::RoundRobin
        );
        // Normalization matches parse_pool_strategy in ponyllm-server.
        assert_eq!(
            parse_pool_strategy(" Round_Robin ", "p"),
            RoutingStrategy::RoundRobin
        );
    }

    #[test]
    fn priority_and_weighted_aliases() {
        assert_eq!(
            parse_pool_strategy("priority", "p"),
            RoutingStrategy::Priority
        );
        assert_eq!(
            parse_pool_strategy("weighted", "p"),
            RoutingStrategy::WeightedRoundRobin
        );
        assert_eq!(
            parse_pool_strategy("weighted_round_robin", "p"),
            RoutingStrategy::WeightedRoundRobin
        );
    }

    #[test]
    fn unknown_and_empty_fall_back_to_sticky_priority() {
        // Sticky default: typos pin to the primary key (with a warn),
        // never silently rotate.
        assert_eq!(parse_pool_strategy("", "p"), RoutingStrategy::Priority);
        assert_eq!(
            parse_pool_strategy("round-robin", "p"),
            RoutingStrategy::Priority
        );
        assert_eq!(
            parse_pool_strategy("random", "p"),
            RoutingStrategy::Priority
        );
    }
}
