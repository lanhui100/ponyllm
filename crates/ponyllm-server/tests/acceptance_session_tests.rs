//! Phase-3 安全修复验收（task-9，隔离测试，Test Agent 于修复实施前编写）。
//!
//! 契约矩阵：VULN-05（HttpOnly Cookie 会话，FIX-CONTRACT.md C 类预留
//! `admin_session_enabled` 契约位，Phase-3 落地）。
//!
//! ## 后端契约
//! - `POST /api/admin/session`：Bearer/x-api-key 凭据换发会话；成功
//!   `Set-Cookie: ponyllm_session=<sid>; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=28800`，
//!   失败 401；
//! - `GET /api/admin/session` → `{"authenticated": bool}`（免凭据可探活）；
//! - `POST /api/admin/session/revoke`：吊销会话 → 204；
//! - cookie 鉴权分支：无 Authorization 头且 `admin_session_enabled` 时读
//!   `ponyllm_session` cookie；
//! - CSRF：cookie 鉴权下，GET/HEAD 以外方法须带 `X-Pony-Session: <sid>`，否则 403；
//! - 会话过期 → 401 信封 `code=session_expired`；TTL 8h（Max-Age=28800）；
//! - 默认 `admin_session_enabled=false` → 行为与现状一致（回归锚点）。
//!
//! ## 配置注入契约（Executor 按此实现，与 PONYLLM_ADMIN_IP_ALLOWLIST 同模式）
//! - `PONYLLM_ADMIN_SESSION_ENABLED=1`：`create_app` 挂载会话路由并启用 cookie 鉴权；
//! - `PONYLLM_ADMIN_SESSION_TTL_SECS=<n>`：测试钩子，覆盖 TTL（默认 28800），供过期用例。
//!
//! ## 红相说明
//! HEAD 上会话路由不存在：成功换发/探活/吊销/cookie 鉴权/CSRF/过期信封全部缺失 →
//! 对应断言失败（红相）；默认关闭路径行为与 HEAD 一致（绿，回归锚点）。

use std::sync::{Arc, Mutex, OnceLock};

use ponyllm_server::app::create_app;
use ponyllm_server::state::AppState;
use ponyllm_server::GatewayConfig;
use reqwest::StatusCode;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 启用会话功能的小型测试 app（admin_session_enabled via env 契约）。
async fn spawn_session_app(ttl_secs: Option<&str>) -> std::net::SocketAddr {
    let app = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_SESSION_ENABLED");
        std::env::remove_var("PONYLLM_ADMIN_SESSION_TTL_SECS");
        std::env::set_var("PONYLLM_ADMIN_SESSION_ENABLED", "1");
        if let Some(ttl) = ttl_secs {
            std::env::set_var("PONYLLM_ADMIN_SESSION_TTL_SECS", ttl);
        }
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_ADMIN_SESSION_ENABLED");
        std::env::remove_var("PONYLLM_ADMIN_SESSION_TTL_SECS");
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn post_session(addr: &std::net::SocketAddr, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{}/api/admin/session", addr))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
}

/// 从 Set-Cookie 提取 `ponyllm_session=<sid>` 的 sid 与完整 cookie 串。
fn extract_session(resp: &reqwest::Response) -> Option<(String, String)> {
    for v in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let raw = v.to_str().ok()?;
        if let Some(rest) = raw.strip_prefix("ponyllm_session=") {
            let sid = rest.split(';').next().unwrap_or("").trim().to_string();
            if !sid.is_empty() {
                let cookie = format!("ponyllm_session={}", sid);
                return Some((sid, cookie));
            }
        }
    }
    None
}

async fn create_session(addr: &std::net::SocketAddr) -> (String, String) {
    let resp = post_session(addr, "test-token").await;
    assert_eq!(resp.status(), StatusCode::OK, "换发会话应 200");
    let (sid, cookie) = extract_session(&resp).expect("应下发 ponyllm_session cookie");
    (sid, cookie)
}

// ---------------------------------------------------------------------------
// 红相用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_session_success_issues_http_only_strict_cookie() {
    let addr = spawn_session_app(None).await;
    let resp = post_session(&addr, "test-token").await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "VULN-05: 合法凭据换发会话应 200（HEAD 无该路由 → 404，红相成立）"
    );
    let raw = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ponyllm_session="))
        .expect("Set-Cookie 必须含 ponyllm_session");
    for part in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age=28800"] {
        assert!(
            raw.contains(part),
            "VULN-05: cookie 属性缺 {}，实际 {:?}",
            part,
            raw
        );
    }
}

#[tokio::test]
async fn get_session_probe_without_credentials() {
    let addr = spawn_session_app(None).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "VULN-05: GET /api/admin/session 免凭据探活应 200（HEAD 无路由 → 401/404，红相成立）"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], serde_json::json!(false));
}

#[tokio::test]
async fn get_session_authenticated_true_with_cookie() {
    let addr = spawn_session_app(None).await;
    let (_sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "VULN-05: cookie 有效时探活应 200 authenticated=true（HEAD 无 cookie 鉴权 → 401，红相成立）"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], serde_json::json!(true));
}

#[tokio::test]
async fn revoke_returns_204_and_requires_csrf_header() {
    let addr = spawn_session_app(None).await;
    let (sid, cookie) = create_session(&addr).await;

    // CSRF：cookie 鉴权下非 GET/HEAD 方法无 X-Pony-Session → 403
    let blocked = reqwest::Client::new()
        .post(format!("http://{}/api/admin/session/revoke", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        blocked.status(),
        StatusCode::FORBIDDEN,
        "VULN-05: 缺 X-Pony-Session 的跨站 POST 必须 403（HEAD 无 cookie 鉴权 → 401/404，红相成立）"
    );

    // 带 X-Pony-Session == sid → 204
    let revoked = reqwest::Client::new()
        .post(format!("http://{}/api/admin/session/revoke", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .header("X-Pony-Session", &sid)
        .send()
        .await
        .unwrap();
    assert_eq!(
        revoked.status(),
        StatusCode::NO_CONTENT,
        "VULN-05: 带 CSRF 头的吊销应 204（HEAD 无路由 → 404，红相成立）"
    );
}

#[tokio::test]
async fn csrf_allows_get_with_cookie_only() {
    let addr = spawn_session_app(None).await;
    let (_sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "VULN-05: cookie 鉴权下 GET 免 CSRF 头（HEAD 无 cookie 鉴权 → 401，红相成立）"
    );
}

#[tokio::test]
async fn expired_session_returns_401_session_expired() {
    let addr = spawn_session_app(Some("1")).await; // TTL=1s 测试钩子
    let (_sid, cookie) = create_session(&addr).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "VULN-05: 过期会话必须 401（HEAD 无会话机制，红相成立）"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "session_expired",
        "VULN-05: 过期信封 code=session_expired，实际 {:?}（HEAD 无该信封 → 红相成立）",
        body["error"]["code"]
    );
}

#[tokio::test]
async fn bad_credential_on_post_session_is_401() {
    // 伴生：换发时错误凭据 → 401（HEAD 无路由 → 404，红相；修复后路由存在 → 401）
    let addr = spawn_session_app(None).await;
    let resp = post_session(&addr, "sk-pony-wrong-token").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 回归锚点：默认 admin_session_enabled=false → 行为与现状一致（绿）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_disabled_session_routes_absent() {
    // 不设 PONYLLM_ADMIN_SESSION_ENABLED → 会话路由不挂载，行为与现状一致：
    // POST /api/admin/session（合法凭据）→ 404（无路由，绕过中间件直接全局 fallback）；
    // GET /api/admin/session 无凭据 → 同样 404（当前/关闭态行为锚点）。
    let app = {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_IP_ALLOWLIST");
        std::env::remove_var("PONYLLM_ADMIN_SESSION_ENABLED");
        std::env::remove_var("PONYLLM_ADMIN_SESSION_TTL_SECS");
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = post_session(&addr, "test-token").await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "回归锚点: 默认关闭时会话路由必须不存在（404）"
    );
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "回归锚点: 默认关闭时 GET /api/admin/session 仍 404（与现状一致，路由不存在）"
    );
}
// ---------------------------------------------------------------------------
// Phase-3b 会话审查修复（R-S1 … R-S6）
// ---------------------------------------------------------------------------

/// 带 FileConfigStore + admin_write_enabled 的会话写路径 app（R-S1）。
/// 返回 (addr, TempDir)：TempDir 必须由调用方持有至请求结束（store 每次读盘）。
async fn spawn_session_write_app() -> (std::net::SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("ponyllm.toml");
    std::fs::write(&config_path, "[gateway]\nbind = \"127.0.0.1:0\"\n").unwrap();
    let addr = {
        let app = {
            let _guard = env_lock().lock().unwrap();
            std::env::remove_var("PONYLLM_ADMIN_SESSION_ENABLED");
            std::env::set_var("PONYLLM_ADMIN_SESSION_ENABLED", "1");
            let mut config = GatewayConfig::default();
            config.api_key = "test-token".to_string();
            config.admin_write_enabled = true;
            let store = Arc::new(ponyllm_server::admin_store::FileConfigStore::new(
                config_path.to_str().unwrap(),
            ));
            let state = Arc::new(AppState::new(config).with_config_store(store));
            let app = create_app(state);
            std::env::remove_var("PONYLLM_ADMIN_SESSION_ENABLED");
            app
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    };
    (addr, tmp)
}

/// R-S1（回归锚点）：真实写路径缺 CSRF 头 → 403 code=csrf_failed。
/// HEAD 实测已成立（中间件 cookie 分支对所有非 GET/HEAD 实施双提交检查）。
#[tokio::test]
async fn rs1_cookie_write_path_blocked_without_csrf_header() {
    let (addr, _store_tmp) = spawn_session_write_app().await;
    let (_sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .put(format!("http://{}/api/admin/strategy", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({"strategy": "speed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "R-S1: 真实写路径缺 X-Pony-Session 必须 403"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "csrf_failed");
}

/// R-S1（回归锚点）：真实写路径携带 X-Pony-Session==sid → 200（写入成功）。
#[tokio::test]
async fn rs1_cookie_write_path_succeeds_with_csrf_header() {
    let (addr, _store_tmp) = spawn_session_write_app().await;
    let (sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .put(format!("http://{}/api/admin/strategy", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .header("X-Pony-Session", &sid)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({"strategy": "speed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "R-S1: 携带 CSRF 头的真实写路径必须 200"
    );
}

/// R-S2（红相）：会话换发端点纳入 auth_ratelimit —— 错误凭据连续达阈值后 429。
/// HEAD 上会话路由挂载于 auth 中间件之外，错误换发恒 401（无限流）→ 红相成立。
#[tokio::test]
async fn rs2_session_exchange_is_rate_limited() {
    let addr = spawn_session_app(None).await;
    let wrong = "sk-pony-admin-00000000000000000000000000000000";
    for i in 1..=35 {
        let resp = post_session(&addr, wrong).await;
        if i > 30 {
            assert_eq!(
                resp.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "R-S2: 第 {} 次错误换发必须 429（auth_ratelimit 纳入会话端点），实际 {}（HEAD 无限流 → 401，红相成立）",
                i,
                resp.status()
            );
        }
    }
}

/// R-S3（红相）：LRU 安全 —— 低权 key 会话洪泛不得把活跃管理员会话一次性挤出
/// （单 key 会话数上限 / 先清过期再淘汰）。HEAD 为全局 4096 LRU，Inference 洪泛
/// 会把最早的管理员会话当 LRU 挤掉 → 红相成立。
#[test]
fn rs3_low_priv_spray_cannot_evict_admin_session() {
    use ponyllm_config::KeyScope;
    use ponyllm_server::session::{SessionStore, MAX_SESSIONS};

    let store = SessionStore::new(std::time::Duration::from_secs(3600));
    let admin_sid = store.create(KeyScope::Admin);

    // 低权（Inference）会话洪泛填满全局表
    for _ in 0..MAX_SESSIONS {
        store.create(KeyScope::Inference);
    }

    assert_eq!(store.live_count(), MAX_SESSIONS);
    assert!(
        store.validate(&admin_sid).is_some(),
        "R-S3: 满表时低权洪泛不得挤掉活跃管理员会话（HEAD 全局 LRU → 管理员被挤出 → 红相成立）"
    );
}

/// R-S4 后端半（回归锚点）：服务端 revoke 后原 cookie 再请求 → 401 session_expired。
#[tokio::test]
async fn rs4_revoked_cookie_gets_401_session_expired() {
    let addr = spawn_session_app(None).await;
    let (sid, cookie) = create_session(&addr).await;
    let revoked = reqwest::Client::new()
        .post(format!("http://{}/api/admin/session/revoke", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .header("X-Pony-Session", &sid)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "R-S4: 已吊销 cookie 再请求必须 401"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "session_expired");
}

/// R-S5（红相）：校验/探活需滑动刷新 —— 响应带 Set-Cookie（Max-Age 续期）。
/// HEAD 上 handle_session_probe 仅返回 {authenticated:true}，无 Set-Cookie → 红相成立。
#[tokio::test]
async fn rs5_probe_response_renews_set_cookie() {
    let addr = spawn_session_app(None).await;
    let (_sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let setc = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ponyllm_session="))
        .map(|s| s.to_string());
    assert!(
        setc.is_some(),
        "R-S5: 校验/探活响应必须带 Set-Cookie 续期（HEAD 无 → 红相成立）"
    );
    assert!(
        setc.as_deref().unwrap().contains("Max-Age=28800"),
        "R-S5: 续期 cookie Max-Age 应为 TTL（28800），实际 {:?}",
        setc
    );
}

/// R-S6（红相）：cookie 鉴权分支仅在无 Authorization 且无 x-api-key 时进入。
/// 有效 x-api-key + 过期 cookie → 应按 x-api-key 认证成功（200）。
/// HEAD 上 cookie 分支只查 authorization 缺失 → 过期 cookie 抢先 401 session_expired
/// → 红相成立。
#[tokio::test]
async fn rs6_valid_x_api_key_wins_over_expired_cookie() {
    let addr = spawn_session_app(Some("1")).await; // TTL=1s
    let (_sid, cookie) = create_session(&addr).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await; // cookie 过期

    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/providers", addr))
        .header("x-api-key", "test-token")
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "R-S6: 有效 x-api-key 必须优先于过期 cookie 认证成功（HEAD 上过期 cookie 抢先 → 401 session_expired，红相成立）"
    );
}

// ---------------------------------------------------------------------------
// R-S8（对抗审查 finding#1 完整性）：换发/探活响应体须含 sid（JS 可读通道，
// 前端据此设 X-Pony-Session 头）
// ---------------------------------------------------------------------------

/// 换发成功响应体必须含 `sid` 字段且与 Set-Cookie 一致。
/// HEAD 响应体仅 {ok, scope} → 红相成立。
#[tokio::test]
async fn rs8_post_session_response_exposes_sid_for_js() {
    let addr = spawn_session_app(None).await;
    let resp = post_session(&addr, "test-token").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let cookie_sid = extract_session(&resp).map(|(s, _)| s);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sid_field = body["sid"].as_str().map(|s| s.to_string());
    assert!(
        sid_field.is_some(),
        "R-S8: 换发成功响应体必须含 sid 字段（JS 读取后设 X-Pony-Session 头），实际 {}（HEAD 仅 {{ok,scope}} → 红相成立）",
        body
    );
    assert_eq!(
        sid_field,
        cookie_sid,
        "R-S8: 响应体 sid 应与 Set-Cookie sid 一致，实际 {:?} vs {:?}",
        sid_field,
        cookie_sid
    );
}

/// 探活 200 响应体必须含 `sid`（前端刷新页面后重读 sid 设 CSRF 头）。
/// HEAD 探活仅 {authenticated:true} → 红相成立。
#[tokio::test]
async fn rs8_probe_response_exposes_sid_for_js() {
    let addr = spawn_session_app(None).await;
    let (sid, cookie) = create_session(&addr).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/session", addr))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["sid"].as_str(),
        Some(sid.as_str()),
        "R-S8: 探活 200 响应体必须含 sid（=cookie sid），实际 {}（HEAD 仅 {{authenticated:true}} → 红相成立）",
        body
    );
}
