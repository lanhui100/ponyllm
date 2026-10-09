//! B002 红相契约测试：凭据家族隔离矩阵（`/api/user/**` 只认 JWT）。
//! 冻结契约 #15：sk-pony-* 机器 key（admin/inference/readonly）访问任何 /api/user/** → 401
//! （验签失败 401 绝不回落 key 家族，绝不把 /api/user/me 当推理面）；旧 api_key 同理 401；
//! 伪造/垃圾 JWT → 401。红相主锚：当前 sk-pony key 过中间件后落在 404（无路由），断言 401 预期 FAIL。

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
    admin_key: String,
    infer_key: String,
    readonly_key: String,
    legacy: String,
}

impl Gh {
    async fn new() -> Self {
        std::env::set_var("PONYLLM_USER_TOKENS_ENABLED", "1");
        std::env::set_var("PONYLLM_JWT_SECRET", JWT_SECRET);
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (admin_key, e_admin) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let (infer_key, e_infer) = generate_scoped_gateway_key("agent-x", KeyScope::Inference);
        let (readonly_key, e_read) = generate_scoped_gateway_key("view-x", KeyScope::Readonly);
        let legacy = "legacy-gh-token-abcdef1234567890".to_string();
        let admin = user("usr-admin", "admin", "admin-pass-1234", UserRole::Admin);
        let alice = user("usr-alice", "alice", "alice-pass-1234", UserRole::User);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = legacy.clone();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone(), e_infer.clone(), e_read.clone()];
        cfg_file.gateway.users = vec![admin.clone(), alice.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = legacy.clone();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin, e_infer, e_read];
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
        Self {
            addr,
            admin_key,
            infer_key,
            readonly_key,
            legacy,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// 所有机器 key（admin/inference/readonly）与旧 api_key 访问 /api/user/me → 一律 401。
#[tokio::test]
async fn machine_keys_never_enter_user_plane() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let candidates = vec![
        ("admin-key", h.admin_key.clone()),
        ("infer-key", h.infer_key.clone()),
        ("readonly-key", h.readonly_key.clone()),
        ("legacy-token", h.legacy.clone()),
    ];
    for (label, key) in &candidates {
        let resp = c
            .get(h.url("/api/user/me"))
            .header("Authorization", bearer(key))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/api/user/me with {label} must be 401 (zero fallback to key family), got {}",
            resp.status()
        );
    }
}

/// 机器 key × /api/user/tokens 写面（POST/GET）→ 401。
#[tokio::test]
async fn machine_keys_rejected_on_token_surface() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    for key in [&h.admin_key, &h.infer_key, &h.readonly_key] {
        let list = c
            .get(h.url("/api/user/tokens"))
            .header("Authorization", bearer(key))
            .send()
            .await
            .unwrap();
        assert_eq!(
            list.status(),
            StatusCode::UNAUTHORIZED,
            "GET /api/user/tokens with key must 401"
        );

        let create = c
            .post(h.url("/api/user/tokens"))
            .header("Authorization", bearer(key))
            .json(&json!({ "name": "x" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            create.status(),
            StatusCode::UNAUTHORIZED,
            "POST /api/user/tokens with key must 401"
        );
    }
}

/// 机器 key × /api/user/admin/users（管理面）→ 401（不是 403：key 家族连"进入"都不许）。
#[tokio::test]
async fn machine_keys_rejected_on_admin_user_surface() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    for key in [&h.admin_key, &h.infer_key, &h.readonly_key] {
        let resp = c
            .get(h.url("/api/user/admin/users"))
            .header("Authorization", bearer(key))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "admin-scope machine key hitting /api/user/admin/users must STILL 401 (JWT-only plane)"
        );
    }
}

/// 伪造/垃圾 JWT → 401，且绝不回落 key 家族（B002 必须拒绝而非尝试按网关 key 验）。
#[tokio::test]
async fn forged_or_garbage_jwt_rejected_401() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    for bad in [
        "garbage-not-a-jwt",
        "abc.def.ghi",
        "eyJhbGciOiJIUzI1NiJ9.e30.invalidsig",
        "e30.e30.e30",
    ] {
        let resp = c
            .get(h.url("/api/user/me"))
            .header("Authorization", bearer(bad))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "malformed bearer {bad:?} must 401"
        );
    }
}

/// 合法 JWT 全 200（admin 与 user 各自 /api/user/me 冒烟；红相因 login 未实现在此红失败）。
#[tokio::test]
async fn valid_jwt_enters_user_plane() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    for (u, p) in [("admin", "admin-pass-1234"), ("alice", "alice-pass-1234")] {
        let login = c
            .post(h.url("/api/user/login"))
            .json(&json!({ "username": u, "password": p }))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK, "login {u} (red fails here)");
        let tok: Value = login.json().await.unwrap();
        let jwt = tok["access_token"].as_str().unwrap();

        let me = c
            .get(h.url("/api/user/me"))
            .header("Authorization", bearer(jwt))
            .send()
            .await
            .unwrap();
        assert_eq!(
            me.status(),
            StatusCode::OK,
            "/api/user/me with valid JWT must 200"
        );
    }
}
