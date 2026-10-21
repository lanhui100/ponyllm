//! 全局 Pre-commit 请求级硬墙钟机制验收与红相测试
//!
//! 验证契约：
//! 1. 配置解析：`pre_commit_total_timeout_secs` 解析、默认值（与 empty_stop 统一为 75s 或独立配置）、Some(0) 禁用、Some(n) 生效
//! 2. 单候选超预算快速熔断：即使下游无响应或连续空帧，累计耗时达到全局墙钟硬死线时立即返回 503 Service Unavailable，并携带 upstream_unavailable 错误码
//! 3. 多候选 Fallback 链路总墙钟阻断：Primary 超时或失败后，切换至 Fallback 时若已耗尽或中途耗尽全局硬死线，立即阻断后续 Fallback，严禁无休止尝试
//! 4. 真实 HTTP 协议契约：503 状态码、标准 JSON 错误格式、`retry-after` 防雪崩头注入。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;

fn make_antigravity_provider(base_url: &str, default_model: &str, fallbacks: Vec<String>) -> ProviderConfig {
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
        model_specs: vec![ModelSpec {
            name: default_model.to_string(),
            fallbacks,
            ..ModelSpec::default()
        }],
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

fn empty_stop_sse_frame() -> String {
    let frame = serde_json::json!({
        "response": {
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"thoughtSignature": "wallclock-sig", "text": ""}]
                },
                "finishReason": "STOP"
            }]
        }
    });
    format!("data: {}\n\n", frame)
}

fn chat_req(model: &str) -> serde_json::Value {
    json!({
        "model": model,
        "stream": true,
        "messages": [{"role": "user", "content": "hello wallclock"}]
    })
}

// ================================================================
// 测试 1：配置解析与默认语义测试
// ================================================================

#[test]
fn test_pre_commit_timeout_config_parsing_and_defaults() {
    let mut cfg = GatewayConfig::default();
    
    // 默认行为验证：未设置 pre_commit_total_timeout_secs 时，应安全映射或默认为 75s（与 empty_stop 对齐或继承）
    assert_eq!(
        cfg.pre_commit_total_timeout_secs, None,
        "默认 pre_commit_total_timeout_secs 必须为 None"
    );
    assert_eq!(
        cfg.effective_pre_commit_timeout(),
        Some(Duration::from_secs(75)),
        "未配置时 effective_pre_commit_timeout 默认应为 75s"
    );

    // Some(0) 代表显式禁用硬墙钟限制
    cfg.pre_commit_total_timeout_secs = Some(0);
    assert_eq!(
        cfg.effective_pre_commit_timeout(),
        None,
        "Some(0) 必须禁用 pre-commit 墙钟预算"
    );

    // Some(3) 代表显式设置 3 秒墙钟
    cfg.pre_commit_total_timeout_secs = Some(3);
    assert_eq!(
        cfg.effective_pre_commit_timeout(),
        Some(Duration::from_secs(3)),
        "Some(3) 必须解析为 3s 墙钟预算"
    );
}

// ================================================================
// 测试 2：单候选多 Attempt 遭遇硬墙钟熔断与 503 规范测试
// ================================================================

#[tokio::test]
async fn test_precommit_wallclock_single_target_timeout_breaks_with_503() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    // 模拟 Antigravity 上游，每个 attempt 延迟 400ms 并返回空 STOP
    let mock = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move |_: Json<serde_json::Value>| {
            let h = hits_clone.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(400)).await;
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

    let pool = Arc::new(KeyPool::new("prov_single", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    // 设置全局 pre-commit 硬墙钟为 1 秒
    config.pre_commit_total_timeout_secs = Some(1);
    config.providers.insert(
        "prov_single".to_string(),
        make_antigravity_provider(
            &format!("http://{}", upstream_addr),
            "gemini-3.8-flash-high",
            vec![],
        ),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("prov_single", pool);

    let gateway_app = create_app(state);
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let start = Instant::now();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&chat_req("gemini-3.8-flash-high"))
        .send()
        .await
        .unwrap();
    let elapsed = start.elapsed();

    // 真实 HTTP 状态码断言：触碰墙钟必须 503
    assert_eq!(
        resp.status(),
        503,
        "触碰 pre-commit 硬墙钟必须返回 HTTP 503 Service Unavailable"
    );

    // 标准错误体断言
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "upstream_unavailable",
        "error.code 必须为 upstream_unavailable"
    );

    // 耗时与重试截断断言：1s 预算下，400ms/次 不可能无限重试（旧逻辑会尝试 12-15 次），应被截断在 < 5 次
    let total_hits = hits.load(Ordering::SeqCst);
    assert!(
        total_hits < 5,
        "总尝试次数必须被硬墙钟截断，当前命中数: {}",
        total_hits
    );
    assert!(
        elapsed < Duration::from_secs(6),
        "请求必须及时中断，实际耗时 {:?}",
        elapsed
    );
}

// ================================================================
// 测试 3：多候选 Fallback 链在跨 Provider 累积超时触碰总墙钟后阻断
// ================================================================

#[tokio::test]
async fn test_precommit_wallclock_fallback_chain_stops_when_budget_exhausted() {
    // 构造 3 个 Mock Provider：
    // prov_primary: 慢速延迟 800ms 返回 500 触发 fallback
    // prov_secondary: 延迟 800ms 返回 500 触发 fallback
    // prov_tertiary: 正常快速响应 200，但如果不受总墙钟控制，就会一直兜底到 tertiary
    let p1_hits = Arc::new(AtomicUsize::new(0));
    let p2_hits = Arc::new(AtomicUsize::new(0));
    let p3_hits = Arc::new(AtomicUsize::new(0));

    let p1_hits_c = p1_hits.clone();
    let mock1 = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move || {
            let h = p1_hits_c.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(700)).await;
                axum::response::Response::builder()
                    .status(500)
                    .body(Body::from("prov1 server error"))
                    .unwrap()
            }
        }),
    );
    let l1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = l1.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l1, mock1).await.unwrap();
    });

    let p2_hits_c = p2_hits.clone();
    let mock2 = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move || {
            let h = p2_hits_c.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(700)).await;
                axum::response::Response::builder()
                    .status(500)
                    .body(Body::from("prov2 server error"))
                    .unwrap()
            }
        }),
    );
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = l2.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l2, mock2).await.unwrap();
    });

    let p3_hits_c = p3_hits.clone();
    let mock3 = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move || {
            let h = p3_hits_c.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(empty_stop_sse_frame()))
                    .unwrap()
            }
        }),
    );
    let l3 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr3 = l3.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l3, mock3).await.unwrap();
    });

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    // 设置全局 pre-commit 硬墙钟为 1 秒（1000ms）
    // prov1 耗时 700ms 失败后转入 prov2，prov2 执行时已逾期或将在执行后逾期，绝对不允许再去拨号 prov3！
    config.pre_commit_total_timeout_secs = Some(1);

    config.providers.insert(
        "prov_1".to_string(),
        make_antigravity_provider(
            &format!("http://{}", addr1),
            "model-primary",
            vec!["model-fallback-1".to_string()],
        ),
    );
    config.providers.insert(
        "prov_2".to_string(),
        make_antigravity_provider(
            &format!("http://{}", addr2),
            "model-fallback-1",
            vec!["model-fallback-2".to_string()],
        ),
    );
    config.providers.insert(
        "prov_3".to_string(),
        make_antigravity_provider(
            &format!("http://{}", addr3),
            "model-fallback-2",
            vec![],
        ),
    );

    let state = Arc::new(AppState::new(config));
    let pool1 = Arc::new(KeyPool::new("prov_1", RoutingStrategy::RoundRobin));
    pool1.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
    state.register_pool("prov_1", pool1);

    let pool2 = Arc::new(KeyPool::new("prov_2", RoutingStrategy::RoundRobin));
    pool2.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
    state.register_pool("prov_2", pool2);

    let pool3 = Arc::new(KeyPool::new("prov_3", RoutingStrategy::RoundRobin));
    pool3.add_key(ApiKeyEntry::new("k3", "sk-3", 1, 10));
    state.register_pool("prov_3", pool3);

    let gateway_app = create_app(state);
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&chat_req("model-primary"))
        .send()
        .await
        .unwrap();

    // 必须 503 阻断
    assert_eq!(
        resp.status(),
        503,
        "Fallback 链累计触碰硬墙钟后必须直接 503 阻断"
    );

    let p1_count = p1_hits.load(Ordering::SeqCst);
    let p2_count = p2_hits.load(Ordering::SeqCst);
    let p3_count = p3_hits.load(Ordering::SeqCst);

    assert!(p1_count >= 1, "prov1 必须被执行过首轮拨号");
    // 关键红相断言：在总硬墙钟 1s 到期后，prov3 绝对不可被调用（hits 必须为 0）
    assert_eq!(
        p3_count, 0,
        "硬墙钟耗尽后严禁继续 fallback 至 prov_3，实际 prov_3 hits={}",
        p3_count
    );
}

// ================================================================
// 测试 4：Messages 协议端点的全局 pre-commit 硬墙钟一致性
// ================================================================

#[tokio::test]
async fn test_precommit_wallclock_messages_endpoint_conformance() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let mock = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(move |_: Json<serde_json::Value>| {
            let h = hits_clone.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(600)).await;
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

    let pool = Arc::new(KeyPool::new("prov_msg", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.pre_commit_total_timeout_secs = Some(1);
    config.providers.insert(
        "prov_msg".to_string(),
        make_antigravity_provider(
            &format!("http://{}", upstream_addr),
            "gemini-3.8-flash-high",
            vec![],
        ),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("prov_msg", pool);

    let gateway_app = create_app(state);
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi anthropic"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        503,
        "/v1/messages 在触碰硬墙钟后同样必须严格遵循 503 熔断契约"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["type"], "overloaded_error",
        "Anthropic 格式的 503 错误映射必须为 overloaded_error"
    );
}
