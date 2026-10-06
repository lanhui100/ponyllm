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