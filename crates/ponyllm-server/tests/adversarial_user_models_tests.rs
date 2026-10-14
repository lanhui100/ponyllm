//! wave-6 L2-AT 对抗测试：`GET /api/user/models` 的确定性 / 无竞态脏读验证。
//!
//! 契约：`.dev-team/contracts/wave-6-user-models-picker.md` §1.5（单次 `check_access` 为纯内存读，
//! 无外部调用、无锁竞争、无 await 阻塞点）+ §1.4（顺序 = `list_all_models()` 原生顺序）。
//!
//! 对抗设计（确定性，零 sleep）：
//! - `tokio::sync::Barrier` 强制 50 个并发请求同时起跑（确定性竞态注入，非盲猜）；
//! - 固定迭代次数 50，无随机种子依赖 → 结果可复现；
//! - 断言：全部 200 + 全部响应体与基线**字节级一致** + 无 JoinError（无 panic / 无未处理失败）。
//!
//! 红相基线（T1 空桩 501）：本文件必须真实 FAIL（Exit≠0，断言失败而非编译错误）。
//! 纪律：本文件只做契约断言，不修改任何业务实现。

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

const JWT_SECRET: &str = "pony-llm-wave6-adv-test-jwt-secret-0123456789abcdef-0123456789abc";
/// 固定并发度（契约要求固定迭代 50 次）。
const CONCURRENCY: usize = 50;

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

fn user(id: &str, username: &str, pass: &str) -> UserEntry {
    UserEntry {
        id: id.into(),
        name: username.into(),
        enabled: true,
        allowed_models: None,
        max_tokens: None,
        created_at: 1000,
        username: Some(username.into()),
        password_hash: Some(seed_password(pass)),
        role: UserRole::User,
        token_version: 0,
    }
}

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

struct Gh {
    addr: SocketAddr,
}

impl Gh {
    async fn new() -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (_, admin_entry) = generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let alice = user("usr-alice", "alice", "alice-pass-1234");

        let openai_section = provider_section("http://127.0.0.1:9/v1", "gpt-4o", &["gpt-4o-mini"]);
        let anthropic_section = provider_section("http://127.0.0.1:9", "claude-3-5-sonnet", &[]);

        let mut cfg_file = ConfigFile::default();
        cfg_file.gateway.bind = "127.0.0.1:8080".into();
        cfg_file.gateway.api_key = "legacy-gh-token".into();
        cfg_file.gateway.admin_write_enabled = true;
        cfg_file.gateway.gateway_keys = vec![admin_entry.clone()];
        cfg_file.gateway.users = vec![alice.clone()];
        cfg_file.providers = HashMap::from([
            ("openai".to_string(), openai_section.clone()),
            ("anthropic".to_string(), anthropic_section.clone()),
        ]);
        cfg_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".into();
        gw.api_key = "legacy-gh-token".into();
        gw.web_enabled = false;
        gw.admin_write_enabled = true;
        gw.user_tokens_enabled = true;
        gw.jwt_secret = Some(JWT_SECRET.to_string());
        gw.providers = HashMap::from([
            ("openai".to_string(), provider_runtime(&openai_section)),
            ("anthropic".to_string(), provider_runtime(&anthropic_section)),
        ]);
        gw.gateway_keys = vec![admin_entry];
        gw.users = vec![alice];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let mut state = AppState::new(gw);
        state.user_plane_enabled = true;
        let state = Arc::new(state.with_config_store(store));
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

async fn login_token(c: &reqwest::Client, h: &Gh) -> String {
    let resp = c
        .post(h.url("/api/user/login"))
        .json(&json!({ "username": "alice", "password": "alice-pass-1234" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "harness precondition: login 必须 200"
    );
    let v: Value = resp.json().await.unwrap();
    v["access_token"].as_str().unwrap().to_string()
}

/// 50 路 barrier 同步并发 GET → 全部 200 且响应体字节级一致（无竞态脏读 / 无重排 / 无丢项）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_get_models_is_deterministic_and_byte_identical() {
    // Arrange
    let h = Gh::new().await;
    let c = reqwest::Client::new();
    let tok = login_token(&c, &h).await;

    // 基线（单请求）
    let baseline_resp = c
        .get(h.url("/api/user/models"))
        .header("Authorization", format!("Bearer {tok}"))
        .send()
        .await
        .unwrap();
    let baseline_status = baseline_resp.status();
    let baseline_body = baseline_resp.text().await.unwrap();
    assert_eq!(
        baseline_status,
        StatusCode::OK,
        "契约 §1.3：单请求必须 200（红相基线：T1 空桩 501）；raw = {baseline_body:?}"
    );

    // Act：固定 50 路并发 + Barrier 强制同时起跑（确定性竞态注入，零 sleep）
    let barrier = Arc::new(tokio::sync::Barrier::new(CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for i in 0..CONCURRENCY {
        let c = c.clone();
        let url = h.url("/api/user/models");
        let tok = tok.clone();
        let barrier = barrier.clone();
        set.spawn(async move {
            barrier.wait().await;
            let resp = c
                .get(&url)
                .header("Authorization", format!("Bearer {tok}"))
                .send()
                .await
                .unwrap_or_else(|e| panic!("并发请求 #{i} 传输失败：{e}"));
            let status = resp.status();
            let body = resp.text().await.unwrap();
            (i, status, body)
        });
    }

    let mut results = Vec::with_capacity(CONCURRENCY);
    while let Some(joined) = set.join_next().await {
        // Assert：无 panic / 无任务失败（JoinError 即"未处理失败"）
        let (i, status, body) = joined.expect("并发任务不得 panic（JoinError）");
        results.push((i, status, body));
    }

    // Assert
    assert_eq!(
        results.len(),
        CONCURRENCY,
        "固定 {CONCURRENCY} 路并发必须全部返回结果"
    );
    for (i, status, body) in &results {
        assert_eq!(
            *status,
            StatusCode::OK,
            "并发请求 #{i} 必须 200（红相基线 501）；raw = {body:?}"
        );
        assert_eq!(
            body, &baseline_body,
            "并发请求 #{i} 的响应体必须与基线字节级一致（确定性、无竞态脏读、无重排）"
        );
    }

    // 契约 §1.3：并发路径下响应形态依然合法（顶层仅 models、元素恰三键）
    let v: Value = serde_json::from_str(&baseline_body).expect("响应体必须是合法 JSON");
    let top = v
        .as_object()
        .expect("顶层必须是对象")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(top, vec!["models".to_string()], "顶层有且仅有 models 一个键");
    let models = v["models"].as_array().expect("models 必须是数组");
    assert!(
        !models.is_empty(),
        "注入 providers 后清单必须非空；raw = {baseline_body:?}"
    );
    for m in models {
        let mut keys = m
            .as_object()
            .expect("元素必须是对象")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "display_name".to_string(),
                "id".to_string(),
                "provider".to_string()
            ],
            "元素有且仅有 id/provider/display_name 三键；got {m}"
        );
    }
}
