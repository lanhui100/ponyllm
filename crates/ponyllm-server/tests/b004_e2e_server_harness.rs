//! B004 acceptance harness：以真实生产路由（`create_app` + `AppState` +
//! `FileConfigStore`，与 CLI 二进制同一套代码）拉起真实监听服务，供浏览器 E2E 与
//! curl 冒烟链路访问。
//!
//! 背景（B004 发现）：CLI `serve` 未把 config 文件里的 `users` 接线进运行时
//! `GatewayConfig`（login 读 `user_tracker`），且 `ponyllm-cli` lib 被 wave-2 在途
//! 重构打断无法编译（wizard.rs E0560）——故本 harness 以测试域内真实服务兜底交付
//! 验收收据；CLI serve 接线缺口已上报 Lead 由 executor 修复。
//!
//! 运行方式（显式 acceptance，默认短驻留避免拖慢 CI）：
//! ```bash
//! PONYLLM_B004_HOLD_SECS=600 cargo test -p ponyllm-server \
//!   --test b004_e2e_server_harness -- --exact b004_e2e_server_harness::b004_serve --nocapture
//! ```
//! 真实断言：绑定后内部探活 /health=200 且 /api/user/login（seed 用户）返回 200+JWT
//! —— 证明服务真实可用后驻留。驻留时长由 env 控制（默认 3s）。

use std::sync::Arc;

use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use ponyllm_config::{UserEntry, UserRole};

fn seed_admin_user() -> UserEntry {
    use ponyllm_core::password::{generate_salt, hash_password, PBKDF2_ITERATIONS};
    let salt = generate_salt();
    let phc = hash_password("admin-b004-pass", &salt, PBKDF2_ITERATIONS);
    UserEntry {
        id: "usr-admin".into(),
        name: "Root Admin".into(),
        enabled: true,
        allowed_models: None,
        max_tokens: None,
        created_at: 1739000000,
        username: Some("admin".into()),
        password_hash: Some(phc),
        role: UserRole::Admin,
        token_version: 0,
    }
}

#[tokio::test]
async fn b004_serve() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("ponyllm.toml");

    let mut cfg_file = ponyllm_config::ConfigFile::default();
    cfg_file.gateway.bind = "127.0.0.1:0".into();
    cfg_file.gateway.api_key = "sk-legacy-b004-admin-token-0123456789abcdef".into();
    cfg_file.gateway.admin_write_enabled = true;
    cfg_file.gateway.users = vec![seed_admin_user()];
    cfg_file.gateway.gateway_keys =
        vec![ponyllm_config::generate_scoped_gateway_key("boot-admin", ponyllm_config::KeyScope::Admin).1];
    cfg_file.save_to_path(config_path.to_str().unwrap()).unwrap();

    let mut gw = GatewayConfig::default();
    gw.bind_addr = "127.0.0.1:0".into();
    gw.api_key = "sk-legacy-b004-admin-token-0123456789abcdef".into();
    gw.web_enabled = true;
    gw.web_dist_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist").into();
    gw.admin_write_enabled = true;
    gw.user_tokens_enabled = true;
    gw.jwt_secret = Some("ponyllm-b004-jwt-secret-0123456789abcdef0123456789abcdef".into());
    gw.users = vec![seed_admin_user()];
    // 预置一枚"自助 token"（user_owned + 绑定 user_id + 模型白名单 + 额度）：
    // 模拟生产里"创建后经配置轮询热载生效"的终态 —— 真实鉴权/双闸路径与 API 创建的 token 完全一致。
    let (seed_plain, mut seed_key) =
        ponyllm_config::generate_scoped_gateway_key("seed-b004-token", ponyllm_config::KeyScope::Inference);
    seed_key.user_owned = true;
    seed_key.user_id = Some("usr-admin".into());
    seed_key.model_limits = Some(vec!["gpt-4o-mini".into()]);
    seed_key.quota = Some(1000);
    println!("B004_SEED_TOKEN {}", seed_plain);
    gw.gateway_keys = vec![
        ponyllm_config::generate_scoped_gateway_key("boot-admin", ponyllm_config::KeyScope::Admin).1,
        seed_key,
    ];
    gw.providers = Default::default();

    let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
    let state = Arc::new(AppState::new(gw).with_config_store(store));
    let app = create_app(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    println!("B004_HARNESS_READY http://{}", addr);

    // —— 真实断言：服务必须真实可用 ——
    let client = reqwest::Client::new();
    let health = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200, "harness server /health must be 200");
    let login = client
        .post(format!("http://{}/api/user/login", addr))
        .json(&serde_json::json!({ "username": "admin", "password": "admin-b004-pass" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        login.status(),
        200,
        "seeded admin login must be 200 (users seeding + pbkdf2 + jwt wiring verified)"
    );

    // —— 驻留：供浏览器 E2E 与 curl 冒烟访问（时长由 env 控制，默认 3s 防拖 CI）——
    let hold = std::env::var("PONYLLM_B004_HOLD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(3);
    tokio::time::sleep(std::time::Duration::from_secs(hold)).await;
    // tempdir 随测试结束清理；进程退出即回收 listener。
}
