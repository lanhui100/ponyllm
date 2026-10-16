//! wave-6 红相契约测试：`GET /api/user/models`（用户面只读模型清单）。
//!
//! 冻结契约：`.dev-team/contracts/wave-6-user-models-picker.md` §1（后端）+ §3（T-B1..T-B6）。
//!
//! 红相基线（T1 空桩 `handle_user_models` 恒返回 501 Not Implemented）：
//! 本文件全部 200 类断言（T-B2 / T-B4 / T-B5 / T-B6）必须真实 FAIL（Exit≠0，断言失败而非编译错误）。
//! 绿相（task-3 实现后）：本文件必须全绿，且不得修改本文件。
//!
//! harness 事实基线（已用 `--nocapture` 探针物理核实，非推断）：
//! - 注入 providers `openai{gpt-4o, gpt-4o-mini}` + `anthropic{claude-3-5-sonnet}` 后，
//!   `AppState::list_all_models()` 原生顺序为：
//!   `auto` → `claude-3-5-sonnet` → `claude-3-5-sonnet[1m]` → `gpt-4o` → `gpt-4o[1m]`
//!   → `gpt-4o-mini` → `gpt-4o-mini[1m]`（默认 `context_window="1M"` 会派生 `[1m]` 别名）。
//! - 无凭据 401 由既有 `auth_middleware` 产出，信封 code 为 `invalid_api_key`
//!   （契约 §1.2 的 `{"message":"not authenticated","code":"invalid_token"}` 是 handler 侧
//!   `unauthorized_json()` 形态；中间件侧早于 handler 拦截，见 task-2 回报中的契约漂移说明）。
//! - 合法 `sk-pony-*` 自助 token 在推理面 `/v1/models` 返回 200，在 `/api/user/models` 返回 401。
//!
//! 纪律：本文件只做契约断言，不修改任何业务实现；写域仅本文件。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{
    generate_scoped_gateway_key, ConfigFile, KeyScope, ProviderSection, UserEntry, UserRole,
};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};

const JWT_SECRET: &str = "pony-llm-wave6-red-test-jwt-secret-0123456789abcdef-0123456789ab";

/// 注入 harness 的模型来源（红相断言据此锚定，不依赖真实上游配置）。
const OPENAI_DEFAULT: &str = "gpt-4o";
const OPENAI_EXTRA: &str = "gpt-4o-mini";
const ANTHROPIC_DEFAULT: &str = "claude-3-5-sonnet";

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

fn user(id: &str, username: &str, pass: &str, allowed_models: Option<Vec<String>>) -> UserEntry {
    UserEntry {
        id: id.into(),
        name: username.into(),
        enabled: true,
        allowed_models,
        max_tokens: None,
        created_at: 1000,
        username: Some(username.into()),
        password_hash: Some(seed_password(pass)),
        role: UserRole::User,
        token_version: 0,
    }
}

/// `ProviderSection` 无 `Default` 实现 → 经 serde 反序列化走其 `#[serde(default)]` 缺省值。
fn provider_section(base_url: &str, default_model: &str, models: &[&str]) -> ProviderSection {
    serde_json::from_value(json!({
        "base_url": base_url,
        "default_model": default_model,
        "models": models,
    }))
    .expect("ProviderSection 必须能从契约最小 JSON 反序列化（serde default 兜底）")
}

fn provider_runtime(section: &ProviderSection) -> ProviderConfig {
    ProviderConfig {
        base_url: section.base_url.clone(),
        default_model: section.default_model.clone(),
        models: section.models.clone(),
        ..Default::default()
    }
}

/// 测试网关：显式构造（不读 `PONYLLM_USER_TOKENS_ENABLED` env，消除并行测试环境竞态），
/// 随机端口真实 HTTP + 真实中间件 + 真实 JWT 登录。
struct Gh {
    addr: SocketAddr,
    /// 直连 AppState（用于取 `list_all_models()` 原生顺序作为契约基准）。
    state: Arc<AppState>,
    /// admin scope 网关 key 明文（`sk-pony-admin-*` 家族）。
    admin_key: String,
    /// legacy 网关 api_key（`gateway.api_key`）。
    legacy_key: String,
}

impl Gh {
    /// `alice_allowed`：alice 的 `allowed_models`（None = 全量）。
    /// `plane_enabled`：显式锁定 user plane 开关（不读 env）。
    async fn new(alice_allowed: Option<Vec<String>>, plane_enabled: bool) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (admin_key, admin_entry) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let legacy_key = "legacy-gh-token".to_string();
        let alice = user("usr-alice", "alice", "alice-pass-1234", alice_allowed);
        let bob = user("usr-bob", "bob", "bob-pass-1234", None);

        let openai_section = provider_section("http://127.0.0.1:9/v1", OPENAI_DEFAULT, &[OPENAI_EXTRA]);
        let anthropic_section = provider_section("http://127.0.0.1:9", ANTHROPIC_DEFAULT, &[]);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = legacy_key.clone();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![admin_entry.clone()];
        cfg_file.gateway.users = vec![alice.clone(), bob.clone()];
        cfg_file.providers = HashMap::from([
            ("openai".to_string(), openai_section.clone()),
            ("anthropic".to_string(), anthropic_section.clone()),
        ]);
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = legacy_key.clone();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        // user plane 开关与 JWT secret 显式注入（配置优先，零 env 依赖）。
        gw.user_tokens_enabled = plane_enabled;
        gw.jwt_secret = Some(JWT_SECRET.to_string());
        gw.providers = HashMap::from([
            ("openai".to_string(), provider_runtime(&openai_section)),
            ("anthropic".to_string(), provider_runtime(&anthropic_section)),
        ]);
        gw.gateway_keys = vec![admin_entry];
        gw.users = vec![alice, bob];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let mut state = AppState::new(gw);
        // 显式锁定 plane 开关：宿主进程 env 即使设置了 PONYLLM_USER_TOKENS_ENABLED 也不受影响。
        state.user_plane_enabled = plane_enabled;
        let state = Arc::new(state.with_config_store(store));
        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            state,
            admin_key,
            legacy_key,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// `list_all_models()` 原生 id 顺序 = 契约 §1.4 的顺序基准。
    fn native_model_ids(&self) -> Vec<String> {
        self.state
            .list_all_models()
            .into_iter()
            .map(|(id, _, _, _)| id)
            .collect()
    }
}

async fn login_token(c: &reqwest::Client, h: &Gh, user: &str, pass: &str) -> String {
    let resp = c
        .post(h.url("/api/user/login"))
        .json(&json!({ "username": user, "password": pass }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "harness precondition: login {user} 必须 200（否则断言与被测对象无关）"
    );
    let v: Value = resp.json().await.unwrap();
    v["access_token"].as_str().unwrap().to_string()
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// 发一次 `GET /api/user/models`，返回 (status, raw body)。
async fn get_user_models(
    c: &reqwest::Client,
    h: &Gh,
    auth: Option<&str>,
) -> (StatusCode, String) {
    let mut req = c.get(h.url("/api/user/models"));
    if let Some(token) = auth {
        req = req.header("Authorization", bearer(token));
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    (status, body)
}

/// 断言 200 并把 body 解析成 JSON（status 断言先于解析，红相失败原因保持为契约断言）。
fn expect_200_json(status: StatusCode, body: &str) -> Value {
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/user/models 必须 200（红相基线：T1 空桩返回 501）；raw body = {body:?}"
    );
    serde_json::from_str::<Value>(body)
        .unwrap_or_else(|e| panic!("200 响应体必须是合法 JSON（{e}）；raw body = {body:?}"))
}

fn model_ids(v: &Value) -> Vec<String> {
    v["models"]
        .as_array()
        .expect("`models` 必须是数组")
        .iter()
        .map(|m| m["id"].as_str().expect("`id` 必须是非空字符串").to_string())
        .collect()
}

/// 契约 §1.3：顶层有且仅有 `models`；元素有且仅有 id/provider/display_name 三键。
fn assert_models_envelope(v: &Value) {
    let obj = v.as_object().expect("顶层必须是 JSON 对象");
    let mut top_keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    top_keys.sort_unstable();
    assert_eq!(
        top_keys,
        vec!["models"],
        "契约 §1.3：顶层有且仅有 `models` 一个键（禁止 total/ok/config_version）"
    );

    let arr = v["models"].as_array().expect("`models` 必须是数组");
    for m in arr {
        let o = m.as_object().expect("每个模型元素必须是 JSON 对象");
        let mut keys: Vec<&str> = o.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["display_name", "id", "provider"],
            "契约 §1.3：元素有且仅有 id/provider/display_name 三键（禁止 protocol 等）；got {m}"
        );
        assert!(
            m["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "`id` 必须是非空字符串；got {m}"
        );
        assert!(
            m["provider"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "`provider` 必须是非空字符串；got {m}"
        );
        assert!(
            m["display_name"].is_null() || m["display_name"].is_string(),
            "`display_name` 必须是 string|null（缺失时必须显式 null，不得省略）；got {m}"
        );
    }
}

fn assert_401_envelope(body: &str) {
    let v: Value = serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("401 响应体必须是 JSON 错误信封（{e}）；raw = {body:?}"));
    let code = v["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("401 信封必须带 error.code；raw = {body:?}"));
    assert!(!code.is_empty(), "401 信封 error.code 不得为空；raw = {body:?}");
    let msg = v["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("401 信封必须带 error.message；raw = {body:?}"));
    assert!(!msg.is_empty(), "401 信封 error.message 不得为空；raw = {body:?}");
}

// ---------------------------------------------------------------------------
// T-B1：无凭据 → 401（既有中间件把守 `/api/user/**`，零 key 家族回落）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb1_no_credentials_returns_401() {
    // Arrange
    let h = Gh::new(None, true).await;
    let c = reqwest::Client::new();

    // Act
    let (status, body) = get_user_models(&c, &h, None).await;

    // Assert
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "契约 §1.2：无凭据访问 /api/user/models 必须 401（不得 404/200）；raw = {body:?}"
    );
    assert_401_envelope(&body);
}

// ---------------------------------------------------------------------------
// T-B2：合法 user JWT → 200 + 唯一允许的响应形态 + 既有 no-store 响应层
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb2_valid_user_jwt_returns_200_models_envelope() {
    // Arrange
    let h = Gh::new(None, true).await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    // Act
    let resp = c
        .get(h.url("/api/user/models"))
        .header("Authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let cache_control = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.text().await.unwrap();

    // Assert
    let v = expect_200_json(status, &body);
    assert_models_envelope(&v);
    assert_eq!(
        cache_control.as_deref(),
        Some("no-store"),
        "契约 §1.1：user_routes() 既有响应层必须为 /api/user/** 注入 Cache-Control: no-store"
    );
    let ids = model_ids(&v);
    assert!(!ids.is_empty(), "合法用户必须看到非空模型清单；raw = {body:?}");
    assert_eq!(
        ids.first().map(String::as_str),
        Some("auto"),
        "契约 §1.4：`auto` 恒首位，不得重排；got {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == &format!("openai/{OPENAI_DEFAULT}")),
        "注入的 provider 模型 openai/{OPENAI_DEFAULT} 必须出现（harness 注入生效）；got {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == &format!("anthropic/{ANTHROPIC_DEFAULT}")),
        "注入的 provider 模型 anthropic/{ANTHROPIC_DEFAULT} 必须出现；got {ids:?}"
    );
    // display_name 显式 null（不得省略键、不得空串代替）
    let gpt4o = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == format!("openai/{OPENAI_DEFAULT}"))
        .expect("openai/gpt-4o 必须在清单内");
    assert!(
        gpt4o.get("display_name").is_some() && gpt4o["display_name"].is_null(),
        "契约 §1.3：无 display_name 的模型必须显式输出 null；got {gpt4o}"
    );
}

// ---------------------------------------------------------------------------
// T-B3：user plane 关闭 → 404（既有中间件在鉴权之前决定，不得回落 401）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb3_plane_disabled_returns_404() {
    // Arrange: 先在 plane 开启的 harness 上登录拿到合法 JWT（两 harness 同 secret/同用户，
    // 该 JWT 在 plane 关闭的 harness 上依然是合法凭据 → 404 只能来自 plane gating）。
    let c = reqwest::Client::new();
    let enabled = Gh::new(None, true).await;
    let tok = login_token(&c, &enabled, "alice", "alice-pass-1234").await;
    let h = Gh::new(None, false).await;

    // precondition：该 JWT 必须能通过中间件鉴权（红相期为 501，实现后为 200；绝不 401）
    let (enabled_status, enabled_body) = get_user_models(&c, &enabled, Some(&tok)).await;
    assert_ne!(
        enabled_status,
        StatusCode::UNAUTHORIZED,
        "harness precondition: JWT 必须通过中间件（否则 404 断言与 plane gating 无关）；raw = {enabled_body:?}"
    );

    // Act
    let (with_jwt, body_jwt) = get_user_models(&c, &h, Some(&tok)).await;
    let (no_cred, body_no_cred) = get_user_models(&c, &h, None).await;

    // Assert
    assert_eq!(
        with_jwt,
        StatusCode::NOT_FOUND,
        "契约 §1.1：plane 关闭时整个 /api/user/** 隐藏为 404；raw = {body_jwt:?}"
    );
    assert_eq!(
        no_cred,
        StatusCode::NOT_FOUND,
        "契约 §1.1：plane gating 必须先于 key 家族鉴权决定（无凭据也 404，不得 401）；raw = {body_no_cred:?}"
    );
}

// ---------------------------------------------------------------------------
// T-B4：allowed_models = None → 全量 + 原生顺序（逐元素保序，不得重排）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb4_allowed_models_none_returns_full_list_in_native_order() {
    // Arrange
    let h = Gh::new(None, true).await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;
    let expected = h.native_model_ids();

    // Act
    let (status, body) = get_user_models(&c, &h, Some(&tok)).await;

    // Assert
    let v = expect_200_json(status, &body);
    assert_models_envelope(&v);
    let ids = model_ids(&v);
    assert_eq!(
        ids, expected,
        "契约 §1.4/§1.5：allowed_models=None 必须返回 list_all_models() 全量且逐元素保序（不得排序/去重/丢项）"
    );
    // In wave 8, models are auto + anthropic/claude-3-5-sonnet + openai/gpt-4o + openai/gpt-4o-mini (no [1m])
    assert!(ids.len() >= 4, "至少 auto + 3 个 provider/model 项；got {ids:?}");
    assert!(!ids.iter().any(|id| id.contains("[1m]")), "no [1m] in user models");
}

// ---------------------------------------------------------------------------
// T-B5：allowed_models = Some(["auto","gpt-4o"]) → 仅字面命中项
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb5_allowed_models_literal_only_returns_hits() {
    // Arrange
    let h = Gh::new(Some(vec!["auto".into(), OPENAI_DEFAULT.into()]), true).await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;

    // Act
    let (status, body) = get_user_models(&c, &h, Some(&tok)).await;

    // Assert
    let v = expect_200_json(status, &body);
    assert_models_envelope(&v);
    let ids = model_ids(&v);
    assert_eq!(
        ids,
        vec!["auto".to_string(), format!("openai/{}", OPENAI_DEFAULT)],
        "契约 §1.5 / C10-04：字面白名单 gpt-4o 放行 openai/gpt-4o，auto 放行 auto"
    );
    // 被排除项必须确实不存在（逐项硬断言，防止"全量返回"式假绿）
    for excluded in [
        format!("openai/{}", OPENAI_EXTRA),
        format!("anthropic/{}", ANTHROPIC_DEFAULT),
        "gpt-4o".to_string(),
        "gpt-4o[1m]".to_string(),
        "gpt-4o-mini[1m]".to_string(),
        "claude-3-5-sonnet[1m]".to_string(),
    ] {
        assert!(
            !ids.iter().any(|id| id == &excluded),
            "契约 §1.5：未命中项 {excluded} 必须被过滤；got {ids:?}"
        );
    }
    let full = h.native_model_ids();
    assert!(
        full.len() > ids.len(),
        "harness 必须存在被过滤项才能证明过滤真实生效（全量 {} 项 vs 过滤后 {} 项）",
        full.len(),
        ids.len()
    );
}

// ---------------------------------------------------------------------------
// T-B6a：通配前缀 Some(["openai/*"]) → 仅前缀命中，auto 被过滤
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb6_prefix_wildcard_filters_out_auto() {
    // Arrange
    let h = Gh::new(Some(vec!["openai/*".into()]), true).await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h, "alice", "alice-pass-1234").await;
    let expected: Vec<String> = h
        .native_model_ids()
        .into_iter()
        .filter(|id| id.starts_with("openai/"))
        .collect();

    // Act
    let (status, body) = get_user_models(&c, &h, Some(&tok)).await;

    // Assert
    let v = expect_200_json(status, &body);
    assert_models_envelope(&v);
    let ids = model_ids(&v);
    assert_eq!(
        ids, expected,
        "契约 §1.5：`openai/*` 前缀通配必须命中全部 openai/ 前缀项且保序"
    );
    assert!(
        !ids.is_empty(),
        "前缀通配必须至少命中 openai/gpt-4o（否则 harness 无意义）"
    );
    for id in &ids {
        assert!(id.starts_with("openai/"), "前缀白名单下不得出现非 openai/ 项：{id}");
    }
    for excluded in ["auto", "anthropic/claude-3-5-sonnet", "claude-3-5-sonnet"] {
        assert!(
            !ids.iter().any(|id| id == excluded),
            "契约 §1.5：{excluded} 必须被 `openai/*` 过滤掉；got {ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// T-B6b：Some(["*"]) → 全量；Some([]) → {"models":[]}（不是 204/404/null）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb6_star_returns_all_and_empty_array_denies_all() {
    // Arrange: 通配 `*` 用户
    let h_star = Gh::new(Some(vec!["*".into()]), true).await;
    let c = reqwest::Client::new();
    let tok_star = login_token(&c, &h_star, "alice", "alice-pass-1234").await;
    let expected_full = h_star.native_model_ids();

    // Act
    let (status_star, body_star) = get_user_models(&c, &h_star, Some(&tok_star)).await;

    // Assert
    let v_star = expect_200_json(status_star, &body_star);
    assert_models_envelope(&v_star);
    assert_eq!(
        model_ids(&v_star),
        expected_full,
        "契约 §1.5：`*` 通配 = 全量且保序"
    );

    // Arrange: 空白名单用户（check_access 语义 = 全部拒绝）
    let h_empty = Gh::new(Some(vec![]), true).await;
    let tok_empty = login_token(&c, &h_empty, "alice", "alice-pass-1234").await;

    // Act
    let (status_empty, body_empty) = get_user_models(&c, &h_empty, Some(&tok_empty)).await;

    // Assert
    let v_empty = expect_200_json(status_empty, &body_empty);
    assert_models_envelope(&v_empty);
    assert_eq!(
        v_empty,
        json!({ "models": [] }),
        "契约 §1.5：allowed_models=Some([]) = 全部拒绝 → 200 {{\"models\":[]}}（不得 204/404/null）"
    );
}

// ---------------------------------------------------------------------------
// T-B7：网关 key 家族（legacy api_key / admin scope key / sk-pony 自助 token）
//       访问 /api/user/models 必须 401，绝不回落到 key 家族鉴权
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tb7_gateway_key_family_never_falls_back_on_user_plane() {
    // Arrange
    let h = Gh::new(None, true).await;
    let c = reqwest::Client::new();
    let user_jwt = login_token(&c, &h, "alice", "alice-pass-1234").await;
    let created: Value = c
        .post(h.url("/api/user/tokens"))
        .header("Authorization", bearer(&user_jwt))
        .json(&json!({ "name": "red-phase-key" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sk_pony = created["api_key"]
        .as_str()
        .expect("harness precondition: 自助 token 必须返回一次性明文")
        .to_string();
    assert!(
        sk_pony.starts_with("sk-pony-"),
        "harness precondition: 自助 token 必须是 sk-pony-* 家族；got {sk_pony}"
    );

    // precondition: 该 sk-pony key 在推理面是合法凭据（否则 401 不构成"不回落"证据）
    let infer = c
        .get(h.url("/v1/models"))
        .header("Authorization", bearer(&sk_pony))
        .send()
        .await
        .unwrap();
    assert_eq!(
        infer.status(),
        StatusCode::OK,
        "harness precondition: sk-pony token 在推理面 /v1/models 必须 200（证明它属于 key 家族合法凭据）"
    );

    // Act + Assert：三类网关凭据逐个不得通过用户面
    for (label, credential) in [
        ("legacy gateway.api_key", h.legacy_key.clone()),
        ("admin scoped gateway key", h.admin_key.clone()),
        ("self-service sk-pony token", sk_pony.clone()),
    ] {
        let (status, body) = get_user_models(&c, &h, Some(&credential)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "契约 §1.2：{label} 访问 /api/user/models 必须 401（JWT-only，零 key 家族回落）；raw = {body:?}"
        );
        assert_401_envelope(&body);
    }
}
