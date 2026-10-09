//! B002 红相契约测试：推理面双层配额闸（user + token 叠乘，fail-closed）。
//! 冻结契约 #16：绑定 user_id+quota 的 sk-pony token 超量 → 429 {code:token_quota_exhausted}；
//! token.model_limits 不含请求模型 → 403 {code:model_forbidden_for_user}。
//! 确定性手段：quota=Some(0)（used(0)>=limit(0) 立即超限，无需上游成功调用累计）。
//! 本 harness 无 providers，红相（B002 未实现）下这些请求到不了 token 闸 → 断言 FAIL（红）；
//! C/D 为既有 user 闸锚（现状已绿，防回归）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{
    generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole,
};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};

fn alice_user(allowed_models: Option<Vec<String>>, max_tokens: Option<u64>) -> UserEntry {
    UserEntry {
        id: "usr-alice".into(),
        name: "Alice".into(),
        enabled: true,
        allowed_models,
        max_tokens,
        created_at: 1000,
        username: Some("alice".into()),
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    }
}

struct Gh {
    addr: SocketAddr,
    plain_key: String,
}

impl Gh {
    /// 参数化 harness：user 闸与 token 闸的边界条件一次配齐。
    async fn new(
        alice_allowed: Option<Vec<String>>,
        alice_max: Option<u64>,
        key_model_limits: Option<Vec<String>>,
        key_quota: Option<u64>,
    ) -> Self {
        std::env::set_var(
            "PONYLLM_USER_TOKENS_ENABLED",
            "1",
        );
        std::env::set_var(
            "PONYLLM_JWT_SECRET",
            "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef",
        );
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (plain_key, mut e_key) =
            generate_scoped_gateway_key("alice-boot-token", KeyScope::Inference);
        e_key.user_id = Some("usr-alice".into());
        e_key.user_owned = true;
        e_key.model_limits = key_model_limits;
        e_key.quota = key_quota;
        let alice = alice_user(alice_allowed, alice_max);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_key.clone()];
        cfg_file.gateway.users = vec![alice.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_key];
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
        Self { addr, plain_key }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

async fn chat(c: &reqwest::Client, h: &Gh, model: &str) -> (StatusCode, Value) {
    let resp = c
        .post(h.url("/v1/chat/completions"))
        .header("Authorization", format!("Bearer {}", h.plain_key))
        .header("content-type", "application/json")
        .json(&json!({ "model": model, "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

/// 契约#16 主锚（token 闸模型白名单）：model_limits 不含请求模型 → 403 model_forbidden_for_user。
#[tokio::test]
async fn token_model_limits_forbid_outside_model_403() {
    let h = Gh::new(None, None, Some(vec!["gpt-4o-mini".into()]), None).await;
    let c = reqwest::Client::new();

    let (status, body) = chat(&c, &h, "claude-3-5-sonnet").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "model outside token.model_limits must be 403, got {status}"
    );
    assert_eq!(
        body["error"]["code"], "model_forbidden_for_user",
        "B002 token-gate code on forbidden model"
    );
}

/// 契约#16 主锚（token 闸配额）：quota=Some(0) → used(0)>=limit(0) 立即超限 → 429 token_quota_exhausted。
#[tokio::test]
async fn token_quota_exhausted_429() {
    let h = Gh::new(None, None, None, Some(0)).await; // 零预算：确定性超限，无需累计
    let c = reqwest::Client::new();

    let (status, body) = chat(&c, &h, "gpt-4o-mini").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "token with exhausted quota must 429, got {status}"
    );
    assert_eq!(
        body["error"]["code"], "token_quota_exhausted",
        "B002 token-gate code on exhausted quota"
    );
}

/// 既有 user 闸锚：max_tokens=Some(0) → 429 user_quota_exhausted（现状已绿，防回归）。
#[tokio::test]
async fn user_quota_exhausted_429_anchor() {
    let h = Gh::new(None, Some(0), None, None).await;
    let c = reqwest::Client::new();

    let (status, body) = chat(&c, &h, "gpt-4o-mini").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "user zero-budget must 429");
    assert_eq!(body["error"]["code"], "user_quota_exhausted");
}

/// 既有 user 闸锚：allowed_models 不含模型 → 403 model_forbidden_for_user（现状已绿）。
#[tokio::test]
async fn user_model_forbidden_403_anchor() {
    let h = Gh::new(Some(vec!["gpt-4o-mini".into()]), None, None, None).await;
    let c = reqwest::Client::new();

    let (status, body) = chat(&c, &h, "claude-3-5-sonnet").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "user-forbidden model must 403");
    assert_eq!(body["error"]["code"], "model_forbidden_for_user");
}