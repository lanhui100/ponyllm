//! B002 红相契约测试：用户自助 Token 生命周期（`/api/user/tokens` CRUD/rotate/归属隔离）。
//! 冻结契约见 Lead B002 派单 #4-8。红相：B002 未实现 → 端点 404/401，断言(201/200/404)预期 FAIL。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
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
        let alice = user("usr-alice", "alice", "alice-pass-1234", UserRole::User);
        let bob = user("usr-bob", "bob", "bob-pass-1234", UserRole::User);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone()];
        cfg_file.gateway.users = vec![alice.clone(), bob.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin];
        gw.users = vec![alice, bob];

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
    assert_eq!(resp.status(), StatusCode::OK, "login {user} must 200 (red fails here)");
    let v: Value = resp.json().await.unwrap();
    v["access_token"].as_str().unwrap().to_string()
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// 契约#5：POST /api/user/tokens → 201 {key_id, api_key(明文一次), ...}，且明文仅此一次。
#[tokio::test]
async fn create_token_returns_201_with_one_time_plaintext() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    let resp = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "ci-token", "model_limits": ["gpt-4o-mini"], "quota": 1000 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "token creation must be 201 (red fails)");
    let v: Value = resp.json().await.unwrap();
    let key_id = v["key_id"].as_str().expect("key_id present");
    let api_key = v["api_key"].as_str().expect("one-time plaintext api_key present");
    assert!(api_key.starts_with("sk-pony-"), "self-service token must be sk-pony-*: {api_key}");
    assert!(!key_id.is_empty(), "key_id must be a non-empty value given by B002");
    assert!(v["created_by"].is_null() || v["created_by"].as_str().is_some());
    let list: Value = c
        .get(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let raw = serde_json::to_string(&list).unwrap();
    assert!(
        !raw.contains(api_key),
        "plaintext must never be re-surfaced after issuance"
    );
    assert!(!raw.contains("\"key_hash\""), "hash must never be returned");
}

/// 契约#4：GET /api/user/tokens → 自己的 token 列表（含 used_tokens）。
#[tokio::test]
async fn list_own_tokens_includes_used_tokens() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    c.post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "t1" }))
        .send()
        .await
        .unwrap();

    let resp = c
        .get(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "list must be 200");
    let v: Value = resp.json().await.unwrap();
    let arr = v.as_array().expect("tokens list is a JSON array");
    assert!(!arr.is_empty(), "alice must see her created token");
    for row in arr {
        assert!(row.get("used_tokens").is_some(), "each token row must expose used_tokens");
        assert!(row.get("name").is_some());
    }
}

/// 契约#5+#3：创建、更新（改名/额度/停用）、删除同一 token → 200。
#[tokio::test]
async fn update_own_token_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    let created: Value = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "rename-me" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key_id = created["key_id"].as_str().unwrap();

    let resp = c
        .put(h.url(&format!("/api/user/tokens/{key_id}")))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "renamed", "quota": 500 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "update own token must be 200 (red fails)");
}

/// 契约#7：删除自己的 token → 200。
#[tokio::test]
async fn delete_own_token_returns_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    let created: Value = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "to-delete" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key_id = created["key_id"].as_str().unwrap();

    let resp = c
        .delete(h.url(&format!("/api/user/tokens/{key_id}")))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "delete own token must be 200 (red fails)");
}

/// 契约#8：rotate → 200 {api_key 新明文}（旧 key 立即失效由 B002 保证，这里断言新明文）。
#[tokio::test]
async fn rotate_token_returns_new_plaintext() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    let created: Value = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&tok))
        .json(&json!({ "name": "rotate-me" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key_id = created["key_id"].as_str().unwrap();
    let old_plain = created["api_key"].as_str().unwrap();

    let resp = c
        .post(h.url(&format!("/api/user/tokens/{key_id}/rotate")))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "rotate must be 200 (red fails)");
    let v: Value = resp.json().await.unwrap();
    let new_plain = v["api_key"].as_str().expect("new plaintext api_key");
    assert_ne!(new_plain, old_plain, "rotation must issue a NEW plaintext");
    assert!(new_plain.starts_with("sk-pony-"));
}

/// 契约#6/#7 归属隔离：他人 token 删除/更新 → 404（bob 不能动 alice 的 token）。
#[tokio::test]
async fn cannot_touch_other_users_token_404() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let alice_tok = login_token(&c, &h, "alice", "alice-pass-1234").await;
    let bob_tok = login_token(&c, &h, "bob", "bob-pass-1234").await;

    let created: Value = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&alice_tok))
        .json(&json!({ "name": "alice-only" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key_id = created["key_id"].as_str().unwrap();

    let del = c
        .delete(h.url(&format!("/api/user/tokens/{key_id}")))
        .header("Authorization", bearer(&bob_tok))
        .send()
        .await
        .unwrap();
    assert_eq!(
        del.status(),
        StatusCode::NOT_FOUND,
        "deleting another user's token must be 404 (red fails: no route yet)"
    );

    let upd = c
        .put(h.url(&format!("/api/user/tokens/{key_id}")))
        .header("Authorization", bearer(&bob_tok))
        .json(&json!({ "name": "hijacked" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        upd.status(),
        StatusCode::NOT_FOUND,
        "updating another user's token must be 404"
    );

    // bob 的列表绝不该包含 alice 的 token
    let bob_list: Value = c
        .get(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&bob_tok))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let raw = serde_json::to_string(&bob_list).unwrap();
    assert!(!raw.contains(key_id), "token ownership must be strictly per-user");
}