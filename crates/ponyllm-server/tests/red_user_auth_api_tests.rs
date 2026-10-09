//! B002 红相契约测试：`/api/user` 认证链路（登录 / me / 改密）+ login 撞限流。
//! 冻结契约见 Lead B002 派单。测试用真实 listener（Gh 模式），先经 `/api/user/login`
//! 拿 JWT 再调端点。红相语义：B002 未实现 → login 落在旧中间件 401 invalid_api_key
//! 或 404，本文件断言(200/401 invalid_credentials/400 invalid_old_password/429)预期 FAIL。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
use serde_json::json;

/// B002 启动守卫的 JWT secret（非生产凭证）；B002完成由 env 读取。
const JWT_SECRET: &str = "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef";

/// 生成被管用户的 PHC 口令（B001/B002 绿相实现后走 Ok 分支；
/// 红相口令桩 `todo!()` panic —— catch_unwind 回退占位 hash，保证 harness 仍可启动），
/// 断言绝不依赖该回退：所有对登录/口令的断言都走真实 API + 真实断言。
fn seed_password(plain: &str) -> String {
    use ponyllm_core::password::{generate_salt, hash_password, PBKDF2_ITERATIONS};
    match std::panic::catch_unwind(|| {
        let salt = generate_salt();
        hash_password(plain, &salt, PBKDF2_ITERATIONS)
    }) {
        Ok(phc) => phc,
        Err(_) => format!(
            "$pbkdf2-sha256$i={PBKDF2_ITERATIONS}$c2FsdHlldm$0000000000000000000000000000000000000000000000000000000000000000"
        ),
    }
}

struct Gh {
    addr: SocketAddr,
    admin_gateway: String,
}

impl Gh {
    async fn new() -> Self {
        // B002 运行时开关：绿相据此挂载 /api/user/**；红相阶段这两个 env 未被读取，设置无害。
        std::env::set_var("PONYLLM_USER_TOKENS_ENABLED", "1");
        std::env::set_var("PONYLLM_JWT_SECRET", JWT_SECRET);

        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (admin_gateway, e_admin) =
            generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let admin_user = UserEntry {
            id: "usr-admin".into(),
            name: "Root Admin".into(),
            enabled: true,
            allowed_models: None,
            max_tokens: None,
            created_at: 1000,
            username: Some("admin".into()),
            password_hash: Some(seed_password("admin-pass-1234")),
            role: UserRole::Admin,
            token_version: 0,
        };
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
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone()];
        cfg_file.gateway.users = vec![admin_user.clone(), alice.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.auth_fail_limit = 5; // 便于红相一次性把登录限流阈值压低、测试确定且快速
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin];
        gw.users = vec![admin_user, alice];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw).with_config_store(store));
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            admin_gateway: admin_gateway,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// POST /api/user/login 原始响应。
async fn login(
    h: &Gh,
    c: &reqwest::Client,
    user: &str,
    pass: &str,
) -> reqwest::Response {
    c.post(h.url("/api/user/login"))
        .json(&json!({ "username": user, "password": pass }))
        .send()
        .await
        .expect("login request must send")
}

/// 断言登录成功并剥出 access_token（红相 B002 未实现 → 此处 404/401，test 红失败）。
async fn login_token(
    h: &Gh,
    c: &reqwest::Client,
    user: &str,
    pass: &str,
) -> String {
    let resp = login(h, c, user, pass).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "login {} must return 200 + access_token (B002 contract) — red-phase fails here",
        user
    );
    let v: serde_json::Value = resp.json().await.expect("login body json");
    v["access_token"]
        .as_str()
        .expect("access_token present")
        .to_string()
}

/// 契约#1 正常登录：200 + {access_token, user{id,username,role,name,enabled}}。
#[tokio::test]
async fn login_success_returns_token_and_public_user() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let resp = login(&h, &c, "alice", "alice-pass-1234").await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "valid login must be 200 (red-phase 404/401 → fail)"
    );
    let v: serde_json::Value = resp.json().await.expect("login body");
    assert!(
        v["access_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "access_token must be present & non-empty"
    );
    let u = &v["user"];
    assert_eq!(u["username"], "alice");
    assert_eq!(u["role"], "user");
    assert_eq!(u["id"], "usr-alice");
    assert_eq!(u["name"], "Alice");
    assert_eq!(u["enabled"], true);
}

/// 契约#1：admin 用户登录，role=admin。
#[tokio::test]
async fn login_admin_role_is_admin() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let resp = login(&h, &c, "admin", "admin-pass-1234").await;
    assert_eq!(resp.status(), StatusCode::OK, "admin login 200");
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["user"]["role"], "admin");
}

/// 契约#1：错口令 → 401 {code:invalid_credentials}。
#[tokio::test]
async fn login_wrong_password_401_invalid_credentials() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let resp = login(&h, &c, "alice", "WRONG-pass").await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "wrong password must be 401"
    );
    let v: serde_json::Value = resp.json().await.expect("error body");
    assert_eq!(v["error"]["code"], "invalid_credentials");
}

/// 契约#1：未知用户 → 与错口令同一信封（防用户名枚举）。
#[tokio::test]
async fn login_unknown_user_receives_same_unified_401() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let unknown = login(&h, &c, "no-such-user", "whatever").await;
    let wrong = login(&h, &c, "alice", "WRONG-pass").await;
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    let uv: serde_json::Value = unknown.json().await.unwrap();
    let wv: serde_json::Value = wrong.json().await.unwrap();
    assert_eq!(
        uv["error"]["code"], wv["error"]["code"],
        "unknown-user must not be distinguishable from wrong-password (no user enumeration)"
    );
    assert_eq!(uv["error"]["code"], "invalid_credentials");
}

/// 契约#13(mq)覆盖对抗：错口令连撞 → 429（登录限流 check-before-hash）。
/// 本用例断言"重复错误登录最终 429"，是真实契约锚；红相可能经既有中间件 429 意外通过，
/// 故属契约不变量而非红相主锚。
#[tokio::test]
async fn login_repeated_wrong_password_eventually_429() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let mut saw_429 = false;
    for i in 0..12 {
        let resp = login(&h, &c, "alice", &format!("try-{i}")).await;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
        assert!(resp.status().is_client_error(), "expected 4xx transient: {}", resp.status());
    }
    assert!(saw_429, "repeated login failures must eventually be rate-limited (429)");
}

/// 契约#2：GET /api/user/me（带 JWT）→ 200 当前用户 + used_tokens。
#[tokio::test]
async fn me_with_jwt_returns_profile_and_usage() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&h, &c, "alice", "alice-pass-1234").await;

    let resp = c
        .get(h.url("/api/user/me"))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "me must be 200 with JWT");
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["username"], "alice");
    assert_eq!(v["id"], "usr-alice");
    assert!(v.get("used_tokens").is_some(), "me must expose used_tokens");
}

/// 契约#2：未登录访问 /api/user/me → 401（不回落）。
#[tokio::test]
async fn me_unauthenticated_401() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let resp = c.get(h.url("/api/user/me")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "unauthenticated me must 401");
}

/// 契约#3：改密旧口令错 → 400 {code:invalid_old_password}。
#[tokio::test]
async fn change_password_wrong_old_400() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&h, &c, "alice", "alice-pass-1234").await;

    let resp = c
        .put(h.url("/api/user/me/password"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "old_password": "WRONG-OLD", "new_password": "new-pass-99" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "wrong old password must be 400"
    );
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["code"], "invalid_old_password");
}

/// 契约#3：改密成功 200，且旧 JWT 因 token_version+1 立即失效（旧 token 调 /me → 401）。
#[tokio::test]
async fn change_password_ok_then_old_jwt_invalidated() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let old_tok = login_token(&h, &c, "alice", "alice-pass-1234").await;

    let resp = c
        .put(h.url("/api/user/me/password"))
        .header("Authorization", bearer(&old_tok))
        .json(&json!({ "old_password": "alice-pass-1234", "new_password": "new-pass-99" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "successful password change must be 200");

    let me = c
        .get(h.url("/api/user/me"))
        .header("Authorization", bearer(&old_tok))
        .send()
        .await
        .unwrap();
    assert_eq!(
        me.status(),
        StatusCode::UNAUTHORIZED,
        "old JWT (pre token_version bump) must be rejected after password change"
    );
}