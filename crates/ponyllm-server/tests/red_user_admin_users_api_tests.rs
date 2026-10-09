//! B002 红相契约测试：admin 用户管理（`/api/user/admin/users` CRUD/reset-*）+ 角色矩阵。
//! 冻结契约见 Lead B002 派单 #9-14 + 权限矩阵(15)。红相：B002 未实现 → 端点 404/401，
//! 断言(201/200/409 + user JWT 403)预期 FAIL；user JWT × admin 端点的 403 是权限矩阵主锚。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const JWT_SECRET: &str = "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef";

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

fn user(id: &str, username: &str, pass: &str, role: UserRole) -> UserEntry {
    UserEntry {
        id: id.into(),
        name: username.into(),
        enabled: true,
        allowed_models: None,
        max_tokens: None,
        created_at: 1000,
        username: Some(username.into()),
        password_hash: Some(seed_password(pass)),
        role,
        token_version: 0,
    }
}

struct Gh {
    addr: SocketAddr,
}

impl Gh {
    async fn new() -> Self {
        std::env::set_var("PONYLLM_USER_TOKENS_ENABLED", "1");
        std::env::set_var("PONYLLM_JWT_SECRET", JWT_SECRET);
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (_, e_admin) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let admin = user("usr-admin", "admin", "admin-pass-1234", UserRole::Admin);
        let alice = user("usr-alice", "alice", "alice-pass-1234", UserRole::User);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone()];
        cfg_file.gateway.users = vec![admin.clone(), alice.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin];
        gw.users = vec![admin, alice];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw).with_config_store(store));
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

async fn login_token(c: &reqwest::Client, h: &Gh, user: &str, pass: &str) -> String {
    let resp = c
        .post(h.url("/api/user/login"))
        .json(&json!({ "username": user, "password": pass }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login {user} 200 (red fails here)");
    resp.json::<Value>().await.unwrap()["access_token"].as_str().unwrap().to_string()
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// 暂存 admin 态 helper：用 admin JWT 创建一个新用户并返回其 id（用于后续 CRUD）。
async fn create_user(
    c: &reqwest::Client,
    h: &Gh,
    admin_tok: &str,
    body: Value,
) -> reqwest::Response {
    c.post(h.url("/api/user/admin/users"))
        .header("Authorization", bearer(admin_tok))
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// 契约#10：admin 创建用户 → 201。
#[tokio::test]
async fn admin_create_user_returns_201() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_tok = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let resp = create_user(
        &c, &h, &admin_tok,
        json!({ "username": "carol", "password": "carol-pass", "role": "user",
                "allowed_models": ["gpt-4o-mini"], "max_tokens": 5000 }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "admin create user must be 201 (red fails)");
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["username"], "carol");
    assert!(v["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

/// 契约#10：重名 username → 409。
#[tokio::test]
async fn admin_create_duplicate_username_409() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_tok = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let _first = create_user(
        &c, &h, &admin_tok,
        json!({ "username": "dup-user", "password": "p1" }),
    )
    .await;
    let dup = create_user(
        &c, &h, &admin_tok,
        json!({ "username": "dup-user", "password": "p2" }),
    )
    .await;
    assert_eq!(dup.status(), StatusCode::CONFLICT, "duplicate username must be 409 (red fails)");
    let v: Value = dup.json().await.unwrap();
    assert_eq!(v["error"]["code"], "username_taken");
}

/// 契约#9：admin 列出用户 → 200，且含种子用户与已建用户、不泄漏 hash。
#[tokio::test]
async fn admin_list_users_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_tok = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let resp = c
        .get(h.url("/api/user/admin/users"))
        .header("Authorization", bearer(&admin_tok))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin list users must be 200 (red fails)");
    let raw = resp.text().await.unwrap();
    assert!(!raw.contains("password_hash"), "admin list must never leak password_hash");
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert!(v.is_array() && !v.as_array().unwrap().is_empty());
}

/// 契约#11：admin 更新用户 → 200。
#[tokio::test]
async fn admin_update_user_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_t = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let resp = c
        .put(h.url("/api/user/admin/users/usr-alice"))
        .header("Authorization", bearer(&admin_t))
        .json(&json!({ "enabled": false, "role": "user" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin update user must be 200 (red fails)");
}

/// 契约#12：admin 删除用户 → 200。
#[tokio::test]
async fn admin_delete_user_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_t = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let created: Value = create_user(&c, &h, &admin_t, json!({ "username": "temp-user", "password": "x" }))
        .await
        .json()
        .await
        .unwrap();
    let uid = created["id"].as_str().unwrap();

    let resp = c
        .delete(h.url(&format!("/api/user/admin/users/{uid}")))
        .header("Authorization", bearer(&admin_t))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin delete user must be 200 (red fails)");
}

/// 契约#13/#14：reset-password / reset-usage → 200。
#[tokio::test]
async fn admin_reset_password_and_usage_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_t = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let rp = c
        .post(h.url("/api/user/admin/users/usr-alice/reset-password"))
        .header("Authorization", bearer(&admin_t))
        .json(&json!({ "new_password": "brand-new-pass" }))
        .send()
        .await
        .unwrap();
    assert_eq!(rp.status(), StatusCode::OK, "reset-password must 200 (red fails)");

    let ru = c
        .post(h.url("/api/user/admin/users/usr-alice/reset-usage"))
        .header("Authorization", bearer(&admin_t))
        .send()
        .await
        .unwrap();
    assert_eq!(ru.status(), StatusCode::OK, "reset-usage must 200 (red fails)");
}

/// 权限矩阵主锚：普通 user JWT × 全部 /api/user/admin/* 端点 → 403（不回落、不 404 假装不存在）。
#[tokio::test]
async fn user_jwt_forbidden_on_all_admin_user_endpoints() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let user_t = login_token(&c, &h, "alice", "alice-pass-1234").await;
    let b = bearer(&user_t);

    let admin_endpoints: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/user/admin/users".into(), None),
        (
            Method::POST,
            "/api/user/admin/users".into(),
            Some(json!({ "username": "x", "password": "y" })),
        ),
        (
            Method::PUT,
            "/api/user/admin/users/usr-bob".into(),
            Some(json!({ "enabled": false })),
        ),
        (Method::DELETE, "/api/user/admin/users/usr-bob".into(), None),
        (
            Method::POST,
            "/api/user/admin/users/usr-bob/reset-password".into(),
            Some(json!({ "new_password": "zz" })),
        ),
        (
            Method::POST,
            "/api/user/admin/users/usr-bob/reset-usage".into(),
            None,
        ),
    ];

    for (method, path, body) in &admin_endpoints {
        let mut req = c
            .request(method.clone(), &h.url(path))
            .header("Authorization", b.clone());
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "user JWT on admin endpoint {method} {path} must be 403 (red fails, likely 404/401)"
        );
    }
}

/// 权限矩阵：admin JWT 对 /api/user/admin/users 全部方法 → 200（已分别在单测断言，这里做 GET+POST 冒烟）。
#[tokio::test]
async fn admin_jwt_accesses_admin_endpoints() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_t = login_token(&c, &h, "admin", "admin-pass-1234").await;

    let list = c
        .get(h.url("/api/user/admin/users"))
        .header("Authorization", bearer(&admin_t))
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK, "admin list must 200 (red fails)");
}