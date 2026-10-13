//! Wave 1 红相契约测试：方案 B (HttpOnly Cookie 认证与刷新恢复)
//!
//! 验证契约：
//! 1. POST /api/user/login 响应需包含 Set-Cookie 头：
//!    - Cookie 名称: ponyllm_jwt
//!    - 属性要求: HttpOnly, SameSite=Strict, Path=/, Secure (或遵从生产契约)
//! 2. POST /api/user/logout 响应需包含清理 Set-Cookie 头 (Max-Age=0 或 Expires 过期)
//! 3. 鉴权中间件支持从 Cookie: ponyllm_jwt=... 提取 JWT：
//!    - 当请求未带 Authorization: Bearer 头但带合法 Cookie 时：
//!      - GET /api/user/me 鉴权通过并返回 200 及对应 user profile
//!      - GET /api/user/tokens 鉴权通过并返回 200
//!    - 当 Cookie 携带非法/篡改 token 时返回 401
//!
//! 红相语义：当前实现无 Set-Cookie / 无 logout 路由 / 中间件只认 Bearer → 本文件断言预期 FAIL。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::StatusCode;
use serde_json::json;

const JWT_SECRET: &str = "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef";

fn seed_password(plain: &str) -> String {
    use ponyllm_core::password::{generate_salt, hash_password, PBKDF2_ITERATIONS};
    let salt = generate_salt();
    hash_password(plain, &salt, PBKDF2_ITERATIONS)
}

struct TestHarness {
    addr: SocketAddr,
    client: reqwest::Client,
}

impl TestHarness {
    async fn new() -> Self {
        std::env::set_var("PONYLLM_USER_TOKENS_ENABLED", "1");
        std::env::set_var("PONYLLM_JWT_SECRET", JWT_SECRET);

        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (_, e_admin) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let alice = UserEntry {
            id: "usr-alice".into(),
            name: "Alice".into(),
            enabled: true,
            allowed_models: Some(vec!["gpt-4o-mini".into()]),
            max_tokens: Some(100_000),
            created_at: 1000,
            username: Some("alice".into()),
            password_hash: Some(seed_password("alice-pass-1234")),
            role: UserRole::User,
            token_version: 0,
        };

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone()];
        cfg_file.gateway.users = vec![alice.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin];
        gw.users = vec![alice];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw).with_config_store(store));
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // 默认 client
        let client = reqwest::Client::new();

        Self { addr, client }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

#[tokio::test]
async fn test_red_login_issues_httponly_cookie() {
    let h = TestHarness::new().await;

    // Act: 登录
    let resp = h
        .client
        .post(h.url("/api/user/login"))
        .json(&json!({
            "username": "alice",
            "password": "alice-pass-1234"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Assert: 响应头必须包含 Set-Cookie 且带 ponyllm_jwt
    let set_cookies: Vec<String> = resp
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect();

    let jwt_cookie = set_cookies
        .iter()
        .find(|c| c.contains("ponyllm_jwt="))
        .expect("login response must contain Set-Cookie with ponyllm_jwt");

    let lower = jwt_cookie.to_lowercase();
    assert!(
        lower.contains("httponly"),
        "ponyllm_jwt cookie must be HttpOnly: {jwt_cookie}"
    );
    assert!(
        lower.contains("samesite=strict") || lower.contains("samesite=lax"),
        "ponyllm_jwt cookie must declare SameSite: {jwt_cookie}"
    );
    assert!(
        lower.contains("path=/"),
        "ponyllm_jwt cookie must declare Path=/: {jwt_cookie}"
    );
}

#[tokio::test]
async fn test_red_logout_clears_cookie() {
    let h = TestHarness::new().await;

    // Act: 调用 logout 端点
    let resp = h
        .client
        .post(h.url("/api/user/logout"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "logout endpoint must be mounted and return 200"
    );

    let set_cookies: Vec<String> = resp
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect();

    let clear_cookie = set_cookies
        .iter()
        .find(|c| c.contains("ponyllm_jwt="))
        .expect("logout response must contain Set-Cookie clearing ponyllm_jwt");

    let lower = clear_cookie.to_lowercase();
    assert!(
        lower.contains("max-age=0") || lower.contains("expires="),
        "logout Set-Cookie must invalidate cookie with Max-Age=0 or Expires: {clear_cookie}"
    );
}

#[tokio::test]
async fn test_red_auth_middleware_accepts_cookie_without_bearer_header() {
    let h = TestHarness::new().await;

    // 1. 正常登录拿到 JWT
    let resp = h
        .client
        .post(h.url("/api/user/login"))
        .json(&json!({
            "username": "alice",
            "password": "alice-pass-1234"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["access_token"].as_str().unwrap();

    // 2. 仅携带 Cookie: ponyllm_jwt=...，不携带 Authorization 头
    let cookie_header = format!("ponyllm_jwt={}", token);

    // 访问 /api/user/me
    let me_resp = h
        .client
        .get(h.url("/api/user/me"))
        .header(COOKIE, &cookie_header)
        .send()
        .await
        .unwrap();

    assert_eq!(
        me_resp.status(),
        StatusCode::OK,
        "GET /api/user/me must succeed with Cookie auth alone"
    );
    let me_body: serde_json::Value = me_resp.json().await.unwrap();
    assert_eq!(me_body["username"], "alice");

    // 访问 /api/user/tokens
    let tokens_resp = h
        .client
        .get(h.url("/api/user/tokens"))
        .header(COOKIE, &cookie_header)
        .send()
        .await
        .unwrap();

    assert_eq!(
        tokens_resp.status(),
        StatusCode::OK,
        "GET /api/user/tokens must succeed with Cookie auth alone"
    );

    // 3. 携带伪造/无效 Cookie 时必须 401
    let bad_cookie_resp = h
        .client
        .get(h.url("/api/user/me"))
        .header(COOKIE, "ponyllm_jwt=bad-forged-jwt-token")
        .send()
        .await
        .unwrap();

    assert_eq!(
        bad_cookie_resp.status(),
        StatusCode::UNAUTHORIZED,
        "Invalid cookie must yield 401"
    );
}
