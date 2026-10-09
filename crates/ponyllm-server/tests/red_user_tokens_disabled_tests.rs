//! B002 契约测试：`user_tokens_enabled` 默认 off（契约#17）。
//! off 时全部 /api/user/** 一律 404（含 /api/user/login），隐藏功能存在性 —— 不 401、不 200。
//! 本文件独立进程运行（独立 test 二进制），env `PONYLLM_USER_TOKENS_ENABLED` 保持未设置，
//! 与开启态的用例互不串扰。红相（路由天然不存在）下大部分断言已 404 通过（契约锚）；
//! login-off-404 在红相为 401（旧中间件兜底）→ 此项为红相主锚（B002 需给出显式 404）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, UserEntry, UserRole};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
use serde_json::json;

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
    admin_key: String,
}

impl Gh {
    async fn new() -> Self {
        // 注意：不设置 PONYLLM_USER_TOKENS_ENABLED —— 保持 B002 默认 off
        std::env::set_var(
            "PONYLLM_JWT_SECRET",
            "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef",
        );
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (admin_key, e_admin) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let admin = UserEntry {
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

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone()];
        cfg_file.gateway.users = vec![admin.clone()];
        cfg_file.providers = HashMap::new();
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = HashMap::new();
        gw.gateway_keys = vec![e_admin];
        gw.users = vec![admin];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw).with_config_store(store));
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, admin_key }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

/// off 时带合法机器 key 访问 /api/user/** → 一律 404（隐藏存在，不 200/401/403）。
#[tokio::test]
async fn user_plane_all_404_when_disabled() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let auth = format!("Bearer {}", h.admin_key);

    // GET 面
    for path in ["/api/user/me", "/api/user/tokens", "/api/user/admin/users"] {
        let resp = c
            .get(h.url(path))
            .header("Authorization", &auth)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "disabled user plane GET {path} must be 404, got {}",
            resp.status()
        );
    }

    // POST 面
    for (path, body) in [
        ("/api/user/tokens", json!({ "name": "x" })),
        (
            "/api/user/admin/users",
            json!({ "username": "x", "password": "y" }),
        ),
    ] {
        let resp = c
            .post(h.url(path))
            .header("Authorization", &auth)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "disabled user plane POST {path} must be 404, got {}",
            resp.status()
        );
    }
}

/// off 时 login 也 404（红相主锚：当前旧中间件对无凭据 POST 给 401，B002 需显式 404）。
#[tokio::test]
async fn login_also_404_when_disabled() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let resp = c
        .post(h.url("/api/user/login"))
        .json(&json!({ "username": "admin", "password": "admin-pass-1234" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "login must be hidden (404) while user_tokens_enabled=off, got {}",
        resp.status()
    );
}
