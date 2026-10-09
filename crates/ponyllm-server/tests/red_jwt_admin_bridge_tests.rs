//! B005 红相契约测试：JWT admin 桥（管理面凭据家族扩展）。
//! 冻结契约：`.dev-team/contracts/wave-4-jwt-admin-bridge.md`（B1-B15 矩阵，B3 校准 201）。
//!
//! 红相锚定（当前实现 `auth_middleware` 对 `/api/admin/**` 与 `/v1/telemetry/**`
//! 只认 gateway-key 家族，JWT 一律按非网关 key 拒绝）：
//! - B1-B7 admin JWT × 管理面 → 当前得 401，期望 200/201 → **红 FAIL**
//! - B8-B9 user JWT × 管理面 → 当前得 401，期望 403（严禁 401） → **红 FAIL**
//! - B10-B11 垃圾/过期 JWT × 管理面 → 当前 401（回落 key 家族），期望 401 → 绿 PASS
//! - B12 机器 key × 管理面 → 200（存量回归） → 绿 PASS
//! - B13-B14 JWT × `/api/user/me` → 200（用户面不变） → 绿 PASS
//! - B15 机器 key × `/api/user/me` → 401（用户面零回落） → 绿 PASS
//!
//! 确定性优先：JWT 用 `ponyllm_core::jwt::sign` 直签（claims 与
//! `routes/user.rs::handle_user_login` L170-179 同构：iss=ponyllm、tv=0、sub 为种子
//! 用户 id），不依赖 login 端点与登录限流（test-expert 确定性原则）。
//! 每个契约项一个测试函数（1:1 对齐契约矩阵，红相逐项可见、绿相逐项验收）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, ConfigFile, KeyScope, ProviderSection, UserEntry, UserRole};
use ponyllm_core::jwt::{sign, Claims};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use reqwest::StatusCode;
use serde_json::json;

const JWT_SECRET: &str = "pony-llm-red-test-jwt-secret-0123456789abcdef-0123456789abcdef";
const JWT_ISSUER: &str = "ponyllm";

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

/// 种子 provider（B3 需要已存在的 provider；B1/B2/B4-B7 有 provider 更贴近生产态）。
fn seed_provider_section() -> ProviderSection {
    ProviderSection {
        base_url: "https://api.openai.com/v1".into(),
        default_model: "gpt-4o".into(),
        strategy: "round_robin".into(),
        billing_mode: ponyllm_core::pool::BillingMode::Metered,
        input_price: 2.5,
        cached_price: 1.25,
        output_price: 10.0,
        models: vec!["gpt-4o".into()],
        model_configs: vec![],
        keys: vec![],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
        rate_limits: None,
        egress_pool: vec![],
        egress_strategy: "round_robin".into(),
    }
}

fn seed_provider_config() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://api.openai.com/v1".into(),
        default_model: "gpt-4o".into(),
        strategy: "round_robin".into(),
        billing_mode: ponyllm_core::pool::BillingMode::Metered,
        input_price: 2.5,
        cached_price: 1.25,
        output_price: 10.0,
        models: vec!["gpt-4o".into()],
        model_specs: vec![],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
        rate_limits: None,
        egress_pool: vec![],
        egress_strategy: "round_robin".into(),
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

        let mut providers_file = HashMap::new();
        providers_file.insert("openai".to_string(), seed_provider_section());
        let mut providers_gw = HashMap::new();
        providers_gw.insert("openai".to_string(), seed_provider_config());

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = legacy.clone();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![e_admin.clone(), e_infer.clone(), e_read.clone()];
        cfg_file.gateway.users = vec![admin.clone(), alice.clone()];
        cfg_file.providers = providers_file;
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = legacy.clone();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.providers = providers_gw;
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 直签合法 HS256 JWT（claims 与 login 签发 L170-179 同构；iss=ponyllm、tv=0）。
/// `exp_delta_secs`：相对当前时间的过期偏移（负值=已过期）。
fn sign_jwt(sub: &str, username: &str, role: &str, exp_delta_secs: i64) -> String {
    let now = now_secs();
    let claims = Claims {
        sub: sub.into(),
        username: username.into(),
        role: role.into(),
        tv: 0,
        iat: now,
        exp: now + exp_delta_secs,
        iss: JWT_ISSUER.into(),
    };
    sign(&claims, JWT_SECRET.as_bytes()).unwrap()
}

/// admin JWT 请求辅助：GET path，期望状态码（红相：当前得 401 → 红 FAIL）。
async fn assert_admin_jwt_get(h: &Gh, path: &str, expected: StatusCode, label: &str) {
    let c = reqwest::Client::new();
    let admin_jwt = sign_jwt("usr-admin", "admin", "admin", 3600);
    let resp = c
        .get(h.url(path))
        .header("Authorization", bearer(&admin_jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        expected,
        "{label}: admin JWT × GET {path} must be {expected} (red: currently 401)"
    );
}

/// B1: GET /api/admin/overview（Resource::AdminRead）→ 200
#[tokio::test]
async fn b1_admin_jwt_overview_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/api/admin/overview", StatusCode::OK, "B1").await;
}

/// B2: GET /api/admin/providers（Resource::AdminRead）→ 200
#[tokio::test]
async fn b2_admin_jwt_providers_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/api/admin/providers", StatusCode::OK, "B2").await;
}

/// B3: POST /api/admin/models（Resource::AdminWrite）→ 201（校准：成功分支实际 201 CREATED；
/// If-Match:* 免版本耦合）。红相：当前 401。
#[tokio::test]
async fn b3_admin_jwt_create_model_201() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let admin_jwt = sign_jwt("usr-admin", "admin", "admin", 3600);
    let resp = c
        .post(h.url("/api/admin/models"))
        .header("Authorization", bearer(&admin_jwt))
        .header("If-Match", "*")
        .json(&json!({ "provider": "openai", "name": "red-b3-model" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "B3: admin JWT × POST /api/admin/models must be 201 (red: currently 401)"
    );
}

/// B4: GET /v1/telemetry/metrics（Resource::TeleSummary）→ 200
#[tokio::test]
async fn b4_admin_jwt_metrics_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/v1/telemetry/metrics", StatusCode::OK, "B4").await;
}

/// B5: GET /v1/telemetry/history（Resource::TeleSummary）→ 200
#[tokio::test]
async fn b5_admin_jwt_history_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/v1/telemetry/history", StatusCode::OK, "B5").await;
}

/// B6: GET /v1/telemetry/recorder?full=true（Resource::TeleFull；admin_write_enabled=true）→ 200
#[tokio::test]
async fn b6_admin_jwt_recorder_full_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/v1/telemetry/recorder?full=true", StatusCode::OK, "B6").await;
}

/// B7: GET /v1/telemetry/stream（Resource::TeleSummary；契约：200 即断言通过）→ 200
#[tokio::test]
async fn b7_admin_jwt_stream_200() {
    let h = Gh::new().await;
    assert_admin_jwt_get(&h, "/v1/telemetry/stream", StatusCode::OK, "B7").await;
}

/// B8: user JWT × GET /api/admin/overview → 403（严禁 401：角色拒绝是授权失败而非认证失败）。
/// 红相主锚：当前 user JWT 非网关 key → 401 → 红 FAIL。
#[tokio::test]
async fn b8_user_jwt_overview_403() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let user_jwt = sign_jwt("usr-alice", "alice", "user", 3600);
    let resp = c
        .get(h.url("/api/admin/overview"))
        .header("Authorization", bearer(&user_jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "B8: user JWT × GET /api/admin/overview must be 403 (red: currently 401)"
    );
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "B8: user JWT must NEVER yield 401 on the admin plane (role denial is 403)"
    );
}

/// B9: user JWT × GET /v1/telemetry/metrics → 403（严禁 401）。
#[tokio::test]
async fn b9_user_jwt_metrics_403() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let user_jwt = sign_jwt("usr-alice", "alice", "user", 3600);
    let resp = c
        .get(h.url("/v1/telemetry/metrics"))
        .header("Authorization", bearer(&user_jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "B9: user JWT × GET /v1/telemetry/metrics must be 403 (red: currently 401)"
    );
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "B9: user JWT must NEVER yield 401 on the admin plane (role denial is 403)"
    );
}

/// B10：垃圾 JWT × 管理面 → 401（验签失败回落既有 key 家族 → invalid_api_key）。
/// 绿相（契约 L64）：验签失败 → authenticate 路径 → 401；红相当前即 401 → 保持 PASS。
#[tokio::test]
async fn b10_garbage_jwt_falls_back_401() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    for bad in [
        "garbage-not-a-jwt",
        "abc.def.ghi",
        "eyJhbGciOiJIUzI1NiJ9.e30.invalidsig",
        "e30.e30.e30",
    ] {
        let resp = c
            .get(h.url("/api/admin/overview"))
            .header("Authorization", bearer(bad))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "B10: garbage JWT {bad:?} × /api/admin/overview must 401 (key-family fallback)"
        );
    }
}

/// B11：过期 JWT（直签 exp=now-60，合法签名）× 管理面 → 401。
/// 绿相（契约 L64）：验签 Expired → 回落 key 家族 → 401；红相当前即 401 → 保持 PASS。
#[tokio::test]
async fn b11_expired_jwt_falls_back_401() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let expired = sign_jwt("usr-admin", "admin", "admin", -60);
    let resp = c
        .get(h.url("/api/admin/overview"))
        .header("Authorization", bearer(&expired))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "B11: expired JWT × /api/admin/overview must 401 (key-family fallback)"
    );
}

/// B12：admin 机器 key × 管理面 → 200（存量回归，key 家族矩阵不受 JWT 桥影响）。
#[tokio::test]
async fn b12_machine_key_admin_plane_regression_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();

    let resp = c
        .get(h.url("/api/admin/overview"))
        .header("Authorization", bearer(&h.admin_key))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "B12: admin machine key × GET /api/admin/overview must 200 (existing key-family behavior)"
    );
}

/// B13：user JWT × GET /api/user/me → 200（用户面 `/api/user/**` JWT-only 分支不变）。
#[tokio::test]
async fn b13_user_jwt_me_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let jwt = sign_jwt("usr-alice", "alice", "user", 3600);
    let resp = c
        .get(h.url("/api/user/me"))
        .header("Authorization", bearer(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "B13: user JWT × GET /api/user/me must 200 (user plane unchanged)"
    );
}

/// B14：admin JWT × GET /api/user/me → 200（用户面不变）。
#[tokio::test]
async fn b14_admin_jwt_me_200() {
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let jwt = sign_jwt("usr-admin", "admin", "admin", 3600);
    let resp = c
        .get(h.url("/api/user/me"))
        .header("Authorization", bearer(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "B14: admin JWT × GET /api/user/me must 200 (user plane unchanged)"
    );
}

/// B15：机器 key（admin/inference/readonly）与 legacy × GET /api/user/me → 401
/// （用户面零回落 key 家族，冻结矩阵不变）。
#[tokio::test]
async fn b15_machine_keys_never_enter_user_plane_401() {
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
            "B15: {label} × /api/user/me must 401 (zero fallback to key family)"
        );
    }
}
