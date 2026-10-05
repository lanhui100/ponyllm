//! Phase-2 安全修复验收（隔离测试，Test Agent 于业务实施前编写）。
//!
//! 契约矩阵：`.dev-team/report/FIX-CONTRACT.md` F1/F2/F4/F5。
//!
//! 红相要求：本文件在未修复 HEAD（a44b0ac）上必须失败，失败原因=对应缺失实现：
//! - F1  VULN-17：`auth_mode`（默认 "secured"）不存在 → 默认配置仍 open（请求 200 而非 401）；
//!       `reload_config_with_pools` 无空 key reload 拒绝逻辑 → 空 key 配置被直接应用。
//! - F2  VULN-01：`auth_ratelimit` 不存在 → 连续错误认证恒 401，绝不出现 429。
//! - F4  VULN-02：`admin_ip_allowlist` 不存在（亦无 `PONYLLM_ADMIN_IP_ALLOWLIST` 注入点）
//!       → 围栏失效，围栏外 IP 访问 /api/admin/* 仍 200（应为 404 fail-closed）。
//! - F5  VULN-08：`PendingAntigravityOAuth` 无 consumed 语义 / pending 视图向 readonly
//!       暴露 code → 同 state 二次 callback 覆盖首 code；readonly 可见 code。
//!
//! 配置注入契约（Executor 实现时按此接受）：与 `PONYLLM_CORS_ALLOWLIST` 同模式，由
//! `create_app` 读取环境变量：
//! - `PONYLLM_ADMIN_IP_ALLOWLIST`：CIDR 逗号列表，非空即对 /api/admin/* 启用 404 围栏；
//! - （推荐）`PONYLLM_TRUSTED_PROXIES`：trusted 代理 CIDR，供 `resolve_client_ip` 跳段。
//! 本文件测试用例对 XFF 采用/忽略两种解析语义均确定性成立（见 F4 用例注释）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

use ponyllm_config::{generate_scoped_gateway_key, KeyScope};
use ponyllm_server::app::create_app;
use ponyllm_server::state::AppState;
use ponyllm_server::GatewayConfig;
use reqwest::StatusCode;

/// Process env is global to the test binary: serialize every app build so an
/// env override set in one test can never interleave with another test's
/// build (cargo runs tests in one binary on multiple threads).
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Build an app under the process-wide env lock. `cfg_build` customizes the
/// default config; env vars are sanitized so no leaked allowlist/override
/// from another test weakens the assertions here.
async fn spawn_app(cfg_build: impl FnOnce(&mut GatewayConfig)) -> SocketAddr {
    let app = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let mut config = GatewayConfig::default();
        cfg_build(&mut config);
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // ConnectInfo 注入真实 TCP peer（127.0.0.1）：R1 peer-校验需要真实对端。
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .unwrap();
    });
    addr
}

/// App with a usable legacy admin token (`test-token`, dual compat → admin scope).
async fn spawn_admin_app() -> SocketAddr {
    spawn_app(|cfg| {
        cfg.api_key = "test-token".to_string();
    })
    .await
}

async fn get(addr: &SocketAddr, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{}{}", addr, path))
        .send()
        .await
        .unwrap()
}

async fn authed_get(addr: &SocketAddr, path: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{}{}", addr, path))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// F1  VULN-17: auth_mode 默认 secured（fail-closed）+ 空 key reload 拒绝
// ---------------------------------------------------------------------------

#[tokio::test]
async fn f1_default_config_is_secured_fail_closed() {
    // F1 契约：`auth_mode` 默认 "secured"。默认配置（api_key 空、无 scoped key）
    // 不再是 open 模式 —— 未认证请求必须 401。
    let addr = spawn_app(|_| {}).await;
    let resp = get(&addr, "/v1/models").await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "F1: 默认配置必须 secured（fail-closed）：未认证 /v1/models 应 401，实际 {}（HEAD 上 auth_mode 未实现 → open → 200，红相成立）",
        resp.status()
    );
}

#[tokio::test]
async fn f1_reload_config_rejects_empty_key_config() {
    // F1 契约：`reload_config_with_pools` 对空 api_key + 无 scoped key 的 reload
    // 必须拒绝应用（保持原配置）并告警。
    let state = {
        let mut cfg = GatewayConfig::default();
        cfg.api_key = "keep-me-123".to_string();
        Arc::new(AppState::new(cfg))
    };
    let empty_key = GatewayConfig {
        api_key: String::new(),
        ..GatewayConfig::default()
    };
    state.reload_config_with_pools(empty_key, HashMap::new());

    let after = state.config.read().api_key.clone();
    assert_eq!(
        after, "keep-me-123",
        "F1: 空 key reload 必须被拒绝（配置保持 'keep-me-123'）；实际为 {:?}（HEAD 上无拒绝逻辑 → 空 key 被应用，红相成立）",
        after
    );
}

// ---------------------------------------------------------------------------
// F2  VULN-01: 认证限流（滑动窗口 30/60s → 锁 15min 退避；429 信封）
// ---------------------------------------------------------------------------

/// 30 次错误认证后（同一 IP + 同一 key 前缀）第 31 次起必须 429。
/// HEAD 上无 auth_ratelimit → 恒 401，断言 429 失败（红相）。
#[tokio::test]
async fn f2_auth_rate_limit_returns_429_after_threshold() {
    let addr = spawn_admin_app().await;
    let client = reqwest::Client::new();
    let wrong = "sk-pony-admin-00000000000000000000000000000000"; // 合法前缀、错误密钥
    let mut last_status = StatusCode::OK;
    for i in 1..=35 {
        let resp = client
            .get(format!("http://{}/v1/models", addr))
            .header("Authorization", format!("Bearer {}", wrong))
            .header("X-Forwarded-For", "203.0.113.50")
            .send()
            .await
            .unwrap();
        last_status = resp.status();
        if i > 30 {
            assert_eq!(
                resp.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "F2: 第 {} 次错误认证（30/60s 阈值后）必须 429（锁定退避），实际 {}（HEAD 上无限流 → 401，红相成立）",
                i,
                resp.status()
            );
        }
    }
    let _ = last_status;
}

/// 伴生回归：正确 key 在全新 (ip, 前缀) 键上必须 200（不得被限流误伤）。
/// 该用例在 HEAD 上即绿（HEAD 无限流）；修复后仍须绿。
#[tokio::test]
async fn f2_correct_key_still_200_on_fresh_budget() {
    let addr = spawn_admin_app().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/v1/models", addr))
        .header("Authorization", "Bearer test-token")
        .header("X-Forwarded-For", "198.51.100.77")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// 伴生回归：不同 key 前缀不得共享限流预算（(ip, key前缀) 键的第二维）。
/// HEAD 上恒 401（绿）；修复后仍须 401（不得被另一前缀的预算误伤）。
#[tokio::test]
async fn f2_different_key_prefix_not_cross_blocked() {
    let addr = spawn_admin_app().await;
    let client = reqwest::Client::new();
    let wrong_admin = "sk-pony-admin-00000000000000000000000000000000";
    let wrong_infer = "sk-pony-infer-11111111111111111111111111111111";
    for _ in 0..30 {
        let resp = client
            .get(format!("http://{}/v1/models", addr))
            .header("Authorization", format!("Bearer {}", wrong_admin))
            .header("X-Forwarded-For", "203.0.113.60")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    // 同一 IP、不同前缀：独立预算 → 401 而非 429
    let resp = client
        .get(format!("http://{}/v1/models", addr))
        .header("Authorization", format!("Bearer {}", wrong_infer))
        .header("X-Forwarded-For", "203.0.113.60")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "F2: 不同 key 前缀不得被误伤（应 401，实际 {}）",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// F4  VULN-02: admin IP 围栏（allowlist 非空即 404 fail-closed）
// ---------------------------------------------------------------------------

/// 围栏外 IP 访问 /api/admin/providers 必须 404。
/// XFF 三跳取法（leftmost/rightmost/peer）下 203.0.113.9、192.0.2.1、127.0.0.1
/// 均不在 allowlist {198.51.100.0/24} 内 → 任意解析语义都 404，确定性成立。
/// HEAD 上无围栏 → 200，断言 404 失败（红相）。
#[tokio::test]
async fn f4_admin_fence_404_for_outside_ip() {
    let addr = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::set_var("PONYLLM_ADMIN_IP_ALLOWLIST", "198.51.100.0/24");
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .unwrap();
        });
        addr
    };
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header("Authorization", "Bearer test-token")
        .header("X-Forwarded-For", "203.0.113.9, 192.0.2.1")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "F4: allowlist 非空时围栏外 IP 访问管理接口必须 404 fail-closed，实际 {}（HEAD 无围栏 → 200，红相成立）",
        resp.status()
    );
}

/// 伴生回归：围栏内 IP 放行。allowlist 同时含 198.51.100.0/24、127.0.0.1（直连 peer）
/// 与 192.0.2.1（最右跳）→ 任意 XFF 解析语义下均判定为内网 → 200。
/// HEAD 上 200（绿）；修复后须保持 200。
#[tokio::test]
async fn f4_admin_fence_allows_inside_ip() {
    let addr = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::set_var(
            "PONYLLM_ADMIN_IP_ALLOWLIST",
            "198.51.100.0/24,127.0.0.1,192.0.2.1",
        );
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .unwrap();
        });
        addr
    };
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header("Authorization", "Bearer test-token")
        .header("X-Forwarded-For", "198.51.100.9, 192.0.2.1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// 伴生回归：空 allowlist（未配置）不启用围栏 → 200。
/// HEAD 上 200（绿）；修复后须保持。
#[tokio::test]
async fn f4_empty_allowlist_keeps_open_admin_read() {
    let addr = spawn_admin_app().await;
    let resp = authed_get(&addr, "/api/admin/providers", "test-token").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// F5  VULN-08: OAuth state 一次性消费（同 state 二次 code 拒覆盖；code 仅 admin 可见）
// ---------------------------------------------------------------------------

async fn spawn_oauth_admin_app_async(
    readonly_key: Option<(String, ponyllm_config::GatewayKeyEntry)>,
) -> SocketAddr {
    spawn_app(|cfg| {
        cfg.api_key = "test-token".to_string();
        cfg.admin_write_enabled = true;
        if let Some((_, entry)) = &readonly_key {
            cfg.gateway_keys = vec![entry.clone()];
        }
    })
    .await
}

async fn seed_oauth_state(addr: &SocketAddr, state: &str) {
    let resp = reqwest::Client::new()
        .get(format!(
            "http://{}/api/admin/oauth/antigravity/auth-url?state={}&redirect_uri=http://localhost:51121/oauth2callback",
            addr, state
        ))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "auth-url 播种失败");
}

/// 同 state 二次 callback 必须拒覆盖：首个 code 保留。
/// HEAD 上 callback 处理无条件覆盖 → pending.code 变为 CODE_B（红相）。
#[tokio::test]
async fn f5_same_state_second_code_does_not_overwrite() {
    let addr = spawn_oauth_admin_app_async(None).await;
    seed_oauth_state(&addr, "st-f5a-1").await;

    for code in ["CODE_A", "CODE_B"] {
        let resp = get(&addr, &format!("/oauth2callback?code={}&state=st-f5a-1", code)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = authed_get(&addr, "/api/admin/oauth/antigravity/pending?state=st-f5a-1", "test-token").await;
    if resp.status() == StatusCode::NOT_FOUND {
        // state 已被消费（一次性语义的更强实现）→ 同样满足契约
        return;
    }
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["code"].as_str(),
        Some("CODE_A"),
        "F5: 同 state 二次 code 必须拒覆盖（pending.code 应保持 CODE_A），实际 {:?}（HEAD 上二次 callback 覆盖 → CODE_B，红相成立）",
        body["code"]
    );
}

/// readonly scope 不得见 pending code。
/// HEAD 上 pending 视图向 readonly（AdminRead 允许）原样暴露 code（红相）。
#[tokio::test]
async fn f5_code_hidden_from_readonly_scope() {
    let (readonly_plain, readonly_entry) = generate_scoped_gateway_key("ro-acc-1", KeyScope::Readonly);
    let addr = spawn_oauth_admin_app_async(Some((readonly_plain.clone(), readonly_entry))).await;
    seed_oauth_state(&addr, "st-f5b-1").await;

    let resp = get(&addr, "/oauth2callback?code=CODE_RO&state=st-f5b-1").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = authed_get(&addr, "/api/admin/oauth/antigravity/pending?state=st-f5b-1", &readonly_plain).await;
    if resp.status() == StatusCode::FORBIDDEN {
        // 更强的实现（pending 收归 admin-only）亦满足契约
        return;
    }
    assert_eq!(resp.status(), StatusCode::OK, "readonly 应仍可查询 pending（仅 code 隐藏）");
    let body: serde_json::Value = resp.json().await.unwrap();
    let code = body.get("code");
    assert!(
        code.is_none() || code.and_then(|c| c.as_str()).is_none(),
        "F5: readonly scope 不得见 OAuth code，实际 {:?}（HEAD 上 pending 视图向 readonly 暴露 code，红相成立）",
        code
    );
}

/// 伴生回归：admin scope 可见 pending code（换票流程依赖）。
/// HEAD 上 200+code（绿）；修复后须保持。
#[tokio::test]
async fn f5_admin_scope_sees_code() {
    let addr = spawn_oauth_admin_app_async(None).await;
    seed_oauth_state(&addr, "st-f5c-1").await;
    let resp = get(&addr, "/oauth2callback?code=CODE_ADMIN&state=st-f5c-1").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = authed_get(&addr, "/api/admin/oauth/antigravity/pending?state=st-f5c-1", "test-token").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"].as_str(), Some("CODE_ADMIN"));
}

// ---------------------------------------------------------------------------
// R1（Phase-2b）集成断言：伪造 XFF 无法绕过 admin 围栏
// ---------------------------------------------------------------------------

/// 直连 peer（127.0.0.1）不在 trusted_proxies 时，伪造 XFF 必须被忽略：
/// 围栏按 peer 判定 → 围栏外 → 404。HEAD 上 resolve_client_ip 无 peer 校验 →
/// 伪造 XFF 命中 allowlist → 200（红相成立）。
#[tokio::test]
async fn r1_forged_xff_cannot_bypass_admin_fence() {
    let addr = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::set_var("PONYLLM_ADMIN_IP_ALLOWLIST", "203.0.113.0/24"); // 攻击者伪造的目标段
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .unwrap();
        });
        addr
    };
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header("Authorization", "Bearer test-token")
        .header("X-Forwarded-For", "203.0.113.9") // 伪造：声称来自 allowlist 内
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "R1: 直连 peer 非 trusted 时伪造 XFF 不得绕过围栏（应 404），实际 {}（HEAD 无 peer 校验 → XFF=203.0.113.9 命中 allowlist → 200，红相成立）",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// R4（Phase-2b）：admin allowlist 全解析失败 → fail-closed（非静默关闭）
// ---------------------------------------------------------------------------

/// `PONYLLM_ADMIN_IP_ALLOWLIST` 非空但全部非法 → 必须 fail-closed（404），
/// 不得静默退化为无围栏。HEAD 上 parse_admin_allowlist 全非法 → None → 无围栏 → 200
/// （红相成立）。
#[tokio::test]
async fn r4_allowlist_all_invalid_env_fail_closed() {
    let addr = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::set_var("PONYLLM_ADMIN_IP_ALLOWLIST", "not-a-cidr,!!!garbage,999.999.1.1"); // 非空、全非法
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .unwrap();
        });
        addr
    };
    let resp = authed_get(&addr, "/api/admin/providers", "test-token").await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "R4: allowlist 非空但全非法必须 fail-closed（404），实际 {}（HEAD 全非法 → None → 无围栏 → 200，红相成立）",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// R5（Phase-2b）：reload 守卫扩展 —— auth_mode secured→open 一律拒绝
// ---------------------------------------------------------------------------

/// Secured 启动的网关，reload 到 auth_mode=open（即便带着 key）必须被拒绝并保持
/// Secured。HEAD 上 reload 守卫只拦空 key → open reload 被应用（红相成立）。
#[tokio::test]
async fn r5_reload_secured_to_open_rejected() {
    let state = {
        let mut cfg = GatewayConfig::default(); // auth_mode = Secured（默认）
        cfg.api_key = "keep-me-123".to_string();
        Arc::new(AppState::new(cfg))
    };
    let flips_open = GatewayConfig {
        api_key: "keep-me-123".to_string(),
        auth_mode: ponyllm_config::AuthMode::Open,
        ..GatewayConfig::default()
    };
    state.reload_config_with_pools(flips_open, HashMap::new());

    let after = state.config.read().auth_mode;
    assert_eq!(
        after,
        ponyllm_config::AuthMode::Secured,
        "R5: secured→open 的 reload 必须被拒绝（配置保持 Secured），实际 {:?}（HEAD 无该守卫 → open 被应用，红相成立）",
        after
    );
}

// ---------------------------------------------------------------------------
// R6（Phase-2b）：overview auth_mode 回显 config.auth_mode（而非 api_key 推断）
// ---------------------------------------------------------------------------

/// 带 FileConfigStore 的 overview 测试 app（overview 依赖 config store）。
async fn spawn_overview_app(
    cfg_build: impl FnOnce(&mut GatewayConfig),
) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("ponyllm.toml");
    std::fs::write(&config_path, "[gateway]\nbind = \"127.0.0.1:0\"\n").unwrap();
    let app = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        let mut config = GatewayConfig::default();
        cfg_build(&mut config);
        let store = Arc::new(ponyllm_server::admin_store::FileConfigStore::new(
            config_path.to_str().unwrap(),
        ));
        let state = Arc::new(AppState::new(config).with_config_store(store));
        create_app(state)
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .unwrap();
    });
    (addr, tmp)
}

async fn overview_auth_mode(addr: &SocketAddr, token: Option<&str>) -> String {
    let mut req = reqwest::Client::new().get(format!("http://{}/api/admin/overview", addr));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "overview 应可访问");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["auth_mode"].as_str().unwrap_or("").to_string()
}

/// 显式 open + 有 key → overview 必须回显 "open"。
/// HEAD 上 auth_mode() 按 api_key 非空推断 "secured"（红相成立）。
#[tokio::test]
async fn r6_overview_echoes_open_when_explicit_open_with_key() {
    let (addr, _tmp) = spawn_overview_app(|cfg| {
        cfg.auth_mode = ponyllm_config::AuthMode::Open;
        cfg.api_key = "test-token".to_string(); // 显式 open 且带着 key
    })
    .await;
    assert_eq!(
        overview_auth_mode(&addr, None).await,
        "open",
        "R6: 显式 open+key 时 overview 必须回显 open（HEAD 按 api_key 推断 secured，红相成立）"
    );
}

/// secured + 空 api_key（但有 scoped admin key 可鉴权）→ overview 必须回显 "secured"。
/// HEAD 上 auth_mode() 按 api_key 为空推断 "open"（红相成立）。
#[tokio::test]
async fn r6_overview_echoes_secured_when_secured_with_empty_key() {
    let (admin_plain, admin_entry) = generate_scoped_gateway_key("r6-a1", KeyScope::Admin);
    let (addr, _tmp) = spawn_overview_app(|cfg| {
        cfg.auth_mode = ponyllm_config::AuthMode::Secured;
        cfg.api_key = String::new(); // 空 key
        cfg.gateway_keys = vec![admin_entry.clone()];
    })
    .await;
    assert_eq!(
        overview_auth_mode(&addr, Some(&admin_plain)).await,
        "secured",
        "R6: secured+空 key 时 overview 必须回显 secured（HEAD 按空 api_key 推断 open，红相成立）"
    );
}
