//! L2-T（Test Agent）红相验收测试：网关 pre-commit 空 STOP 透明重试墙钟总预算。
//!
//! 契约：`.dev-team/contracts/2026-10-07-empty-stop-budget-contract.md`（frozen，
//! Lead 一票裁决）。验收矩阵 C1–C8 以该文件为准。
//!
//! 红相语义（当前阶段，业务代码未实现）：
//! - C1 常量钉子：`MIN_EMPTY_STOP_ATTEMPTS`/`PER_KEY_EMPTY_STOP_MAX_ATTEMPTS`
//!   仍是旧值 15/5，断言 8/2 必须失败（红）；`MAX_EMPTY_STOP_ATTEMPTS_CAP=12` 与
//!   `MAX_EMPTY_STOP_TOTAL_DURATION=75s` 空桩已存在，断言为真绿，不许伪造红。
//! - C2/C3：`empty_stop_attempt_budget` / `effective_empty_stop_timeout` 为
//!   `unimplemented!()` 空桩 → panic（红）。
//! - C4/C5/C7：旧代码无请求级墙钟闸、per-key=5、且 empty-STOP break 不回填
//!   Retry-After → 命中数/响应头断言失败（红）。
//!
//! 纪律：只新增本文件，绝不修改既有测试或业务代码；测试绝不伪装通过。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::streaming::{
    empty_stop_attempt_budget, MAX_EMPTY_STOP_ATTEMPTS_CAP, MAX_EMPTY_STOP_TOTAL_DURATION,
    MIN_EMPTY_STOP_ATTEMPTS, PER_KEY_EMPTY_STOP_MAX_ATTEMPTS,
};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use serde_json::json;

/// Antigravity 协议的 provider（上游为空 STOP mock）。
fn antigravity_provider(base_url: &str, default_model: &str) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        base_url: base_url.to_string(),
        default_model: default_model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.1,
        cached_price: 0.01,
        output_price: 0.2,
        models: vec![default_model.to_string()],
        model_specs: vec![],
        default_protocol: Some(UpstreamProtocol::Antigravity),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
        rate_limits: None,
    }
}

/// 构造一个 antigravity 空 STOP SSE 帧（复用 `is_antigravity_empty_stop_frame`
/// 判定形状：finishReason=STOP 且 parts 无非空 text、无 functionCall；单帧即
/// frames=1，可触发 R3 first-frame 判定）。帧形状对齐 streaming.rs
/// `test_empty_stop_shape_signature_only_trailer`。
fn empty_stop_sse_frame() -> String {
    let frame = serde_json::json!({
        "response": {
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"thoughtSignature": "red-phase-sig", "text": ""}]
                },
                "finishReason": "STOP"
            }]
        }
    });
    format!("data: {}\n\n", frame)
}

struct Harness {
    base: String,
    /// mock 上游收到的请求数（= attempt 数）。
    hits: Arc<AtomicUsize>,
}

/// 网关 + 内存 mock antigravity 上游（每次请求都返回空 STOP SSE 帧）的集成
/// harness：`pool_keys` 控制密钥池大小，`budget_secs` 透传给
/// `empty_stop_total_timeout_secs`（None=不显式设置，契约默认 75s 语义），
/// `latency` 模拟每个 attempt 的上游响应延迟。
async fn spawn_empty_stop_harness(
    pool_keys: usize,
    budget_secs: Option<u64>,
    latency: Duration,
) -> Harness {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let mock = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move |_: Json<serde_json::Value>| {
            let hits = hits_clone.clone();
            let latency = latency;
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                if !latency.is_zero() {
                    tokio::time::sleep(latency).await;
                }
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(empty_stop_sse_frame()))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy_prov", RoutingStrategy::RoundRobin));
    for i in 1..=pool_keys {
        let id = format!("k{i}");
        let secret = format!("sk-{i}");
        pool.add_key(ApiKeyEntry::new(&id, &secret, 1, 10));
    }

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: 行为测试显式 opt-in
    config.empty_stop_total_timeout_secs = budget_secs;
    config.providers.insert(
        "agy_prov".to_string(),
        antigravity_provider(&format!("http://{}", upstream_addr), "gemini-3.8-flash-high"),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("agy_prov", pool);

    let gateway_app = create_app(state);
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw_listener, gateway_app).await.unwrap();
    });

    Harness {
        base: format!("http://{}", gw_addr),
        hits,
    }
}

fn chat_streaming_req() -> serde_json::Value {
    json!({
        "model": "gemini-3.8-flash-high",
        "stream": true,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

// ================================================================
// C1：常量钉子（8/2 红，12/75s 绿）
// ================================================================

#[test]
fn test_c1_empty_stop_constant_nails() {
    // 契约目标值：MIN=8（当前 15，红）、PER_KEY=2（当前 5，红）。
    assert_eq!(
        MIN_EMPTY_STOP_ATTEMPTS, 8,
        "C1: MIN_EMPTY_STOP_ATTEMPTS 必须收紧到 8（当前 {}）",
        MIN_EMPTY_STOP_ATTEMPTS
    );
    assert_eq!(
        PER_KEY_EMPTY_STOP_MAX_ATTEMPTS, 2,
        "C1: PER_KEY_EMPTY_STOP_MAX_ATTEMPTS 必须收紧到 2（当前 {}）",
        PER_KEY_EMPTY_STOP_MAX_ATTEMPTS
    );
    // 空桩已落：真绿断言（不许伪造红）。
    assert_eq!(
        MAX_EMPTY_STOP_ATTEMPTS_CAP, 12,
        "C1: MAX_EMPTY_STOP_ATTEMPTS_CAP=12"
    );
    assert_eq!(
        MAX_EMPTY_STOP_TOTAL_DURATION,
        Duration::from_secs(75),
        "C1: MAX_EMPTY_STOP_TOTAL_DURATION=75s"
    );
    // 结构约束：上界必须 ≥ 下界，且 per-key 必须 < 总预算下限。
    assert!(
        MAX_EMPTY_STOP_ATTEMPTS_CAP >= MIN_EMPTY_STOP_ATTEMPTS,
        "cap 不得小于 min"
    );
}

// ================================================================
// C2：empty_stop_attempt_budget 表驱动（空桩 panic → 红）
// ================================================================

#[test]
fn test_c2_empty_stop_attempt_budget_table() {
    // (pool_keys, max_retries) -> 期望预算，公式
    // max(max_retries, pool_keys.saturating_mul(2)).clamp(8, 12)。
    // 契约修订（Lead 一票裁决，公式为准）：(5,0)→10。
    let cases: &[(usize, usize, usize)] = &[
        (0, 0, 8),
        (1, 3, 8),
        (5, 0, 10),
        (6, 3, 12),
        (8, 5, 12),
        (20, 2, 12),
        (1, 20, 12), // max_retries 封顶为 CAP
    ];
    for &(pool_keys, max_retries, expected) in cases {
        assert_eq!(
            empty_stop_attempt_budget(pool_keys, max_retries),
            expected,
            "C2: budget({pool_keys}, {max_retries}) 应为 {expected}"
        );
    }
}

// ================================================================
// C3：effective_empty_stop_timeout 配置语义（方法为空桩 panic → 红）
// ================================================================

#[test]
fn test_c3_empty_stop_total_timeout_config() {
    let mut cfg = GatewayConfig::default();
    // 字段默认 None（空桩已落，真绿）。
    assert_eq!(
        cfg.empty_stop_total_timeout_secs, None,
        "C3: 默认 empty_stop_total_timeout_secs 必须为 None"
    );
    // None → 默认 75s（空桩 unimplemented → 红）。
    assert_eq!(
        cfg.effective_empty_stop_timeout(),
        Some(Duration::from_secs(75)),
        "C3: None 必须解析为 75s"
    );
    // Some(0) → 禁用（None）。
    cfg.empty_stop_total_timeout_secs = Some(0);
    assert_eq!(
        cfg.effective_empty_stop_timeout(),
        None,
        "C3: Some(0) 必须禁用墙钟预算"
    );
    // Some(5) → 5s。
    cfg.empty_stop_total_timeout_secs = Some(5);
    assert_eq!(
        cfg.effective_empty_stop_timeout(),
        Some(Duration::from_secs(5)),
        "C3: Some(5) 必须解析为 5s"
    );
}

// ================================================================
// C4：请求级墙钟预算快速失败（503 + 命中数截断 + 不挂死）
// ================================================================

#[tokio::test]
async fn test_c4_wall_clock_budget_fast_fails() {
    // short-budget（1s）+ mock 每个 attempt 延迟 500ms 的持续空 STOP + 单 key。
    // 旧代码（无墙钟）无界地打满 15 次 attempts；有墙钟后命中数应被截断。
    let harness = spawn_empty_stop_harness(1, Some(1), Duration::from_millis(500)).await;
    let client = reqwest::Client::new();
    let start = Instant::now();

    let resp = client
        .post(format!("{}/v1/chat/completions", harness.base))
        .json(&chat_streaming_req())
        .send()
        .await
        .unwrap();
    let elapsed = start.elapsed();

    // 快速失败：503 + upstream_unavailable。
    assert_eq!(resp.status(), 503, "C4: 必须 503 快速失败");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "upstream_unavailable",
        "C4: error.code 必须为 upstream_unavailable，body={body}"
    );
    // 命中数必须远小于旧实现（无墙钟会打满 15 次）。
    let hits = harness.hits.load(Ordering::SeqCst);
    assert!(
        hits < 8,
        "C4: 墙钟预算必须截断 attempt 数，当前命中 {hits}（旧代码无墙钟会 ≥15）"
    );
    // 不挂死：pre-commit 阶段总耗时远小于下游 300s idle 上限。
    assert!(
        elapsed < Duration::from_secs(300),
        "C4: 请求不得挂死，耗时 {elapsed:?}"
    );
}

// ================================================================
// C5：deterministic 早收敛（per-key=2 + 阈 3 → 约第 6 次 break）
// ================================================================

#[tokio::test]
async fn test_c5_deterministic_early_convergence() {
    // 3-key 池 + 即时空 STOP（每 attempt 都返回 frames=1 空 STOP）。
    // per-key=2 时：k1×2→k2×2→k3×2，第 6 次 attempt 触发 deterministic break；
    // 旧 per-key=5 需 15 次才 break。
    let harness = spawn_empty_stop_harness(3, None, Duration::ZERO).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", harness.base))
        .json(&chat_streaming_req())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 503, "C5: deterministic 早收敛必须 503（UpstreamUnavailable）");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "upstream_unavailable", "C5: body={body}");

    // 早收敛：命中数须 ≤10（旧代码 per-key=5 需 ~15 次=红）。
    let hits = harness.hits.load(Ordering::SeqCst);
    assert!(
        hits <= 10,
        "C5: deterministic 早收敛应把 attempt 数压到 ≤10，当前命中 {hits}（旧 per-key=5 需 ~15）"
    );
}

// ================================================================
// C6：三路由统一走 empty_stop_attempt_budget（源码审查占位）
// ================================================================

/// C6 属"实现审查 + 集成观测"，不在本文件设独立红测试（Lead 决定）：
/// Executor 必须让 chat.rs / messages.rs / responses.rs 三处预算公式统一调用
/// `empty_stop_attempt_budget`（含修复 responses.rs 此前漏乘 PER_KEY 的偏差），
/// 由源码审查与 C2 表驱动共同担保证。绿相复核时人工 review 三处调用点。
#[allow(dead_code)]
fn _c6_placeholder() {}

// ================================================================
// C7：空 STOP 预算 break 的 503 响应必须带 Retry-After（>0）
// ================================================================

#[tokio::test]
async fn test_c7_retry_after_header_on_budget_break() {
    // 同 C4 的墙钟 break 场景（1s 预算 + 500ms/attempt 延迟 + 单 key）。
    // 旧代码 empty-STOP break 分支不回填 last_retry_after → 无 Retry-After 头（红）。
    let harness = spawn_empty_stop_harness(1, Some(1), Duration::from_millis(500)).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", harness.base))
        .json(&chat_streaming_req())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 503, "C7: 必须 503");
    let retry_after = resp.headers().get("retry-after");
    assert!(
        retry_after.is_some(),
        "C7: 空 STOP 预算 break 必须回填 Retry-After 头（防下游紧绑定重试）"
    );
    let secs: u64 = retry_after
        .unwrap()
        .to_str()
        .expect("retry-after 必须是 ASCII 秒数")
        .parse()
        .expect("retry-after 必须是合法 u64");
    assert!(secs > 0, "C7: Retry-After 必须 >0，当前 {secs}");
}

// ================================================================
// C8：既有测试不回归 —— 由全量门禁 `cargo test --workspace` 承担，
// 不在本文件重复（Red 阶段本就允许全量门禁其他用例继续绿）。
// ================================================================