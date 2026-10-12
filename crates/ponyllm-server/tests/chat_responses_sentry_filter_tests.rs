//! T2 扩展：chat/completions（chat.rs）与 responses（responses.rs）路由的
//! Sentry 上报过滤闸门 —— 红相验收测试。
//!
//! 背景（Lead 决策，2026-10-12）：已冻结的 `should_report_sentry_exhaustion`
//! 闸门（systemone 已绿）扩展覆盖 chat/responses 两个路由的 `GatewayExhaustedError`
//! 上报块（chat.rs:1593-1615、responses.rs:1151-1173），根治上游配额/限流类噪音
//! （deepseek-v4-flash 429 已证实 chat 路由同样漏报配置）。
//!
//! 契约（不变，v2）：pub fn should_report_sentry_exhaustion(kind, last_error) -> bool，
//! OR 语义（kind ∈ {RateLimitExceeded{..}, QuotaExhausted} 或文本标记
//! "no available key"/"freeusagelimiterror"/"rate limit"/"quota"/"429" 任一命中即不报）。
//! 函数位于 ponyllm_server::routes::systemone，已 pub 导出。
//!
//! 红相说明：当前 chat/responses 上报块尚未接入闸门（`is_client_bad_request` 过滤
//! 只拦 3 个客户端 4xx 文本标记，429/配额文本不受限）→ 本文件"配额类 0 上报"断言
//! 在现阶段应失败（红相锚定）。Executor 在闸门就位后本文件须全绿。
//! 红相证据追加至 /home/dm/pony-sentry/.dev-team/red-phase-systemone-filter.txt。
//!
//! 注入接缝（沿用 systemone_sentry_filter_tests.rs）：`AppState::sentry` 为 pub 字段，
//! 可替换为指向本地 mock ingest 端点的 `SentryClient`；每个场景独立网关（含独立 pool），
//! 确定性无跨请求 key 冷却耦合。分级：L2（路由级集成，配额类 0 上报 + 正控 1 上报）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_core::sentry::{SentryClient, SentryConfig};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};

/// 本地 mock PonySentry ingest：只计数 POST 事件，绝不外发。
async fn spawn_ingest_probe() -> (SocketAddr, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    let router = Router::new().route(
        "/ingest",
        post(move || async move {
            c.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (addr, counter)
}

/// 条件轮询（防 Flaky：显式有界轮询，禁止裸 sleep 盲猜）：等待 ingest 计数 >= expected。
async fn wait_ingest_count(counter: &Arc<AtomicUsize>, expected: usize, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if counter.load(Ordering::SeqCst) >= expected {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 拉起一个独立路由网关：
/// - 上游 mock 在 `/v1/chat/completions` 与 `/v1/responses` 均挂载同一失败响应
///   （`upstream_status` + `upstream_body`，max_retries=0 单次尝试，确定性无
///   pool-wait/退避延迟）；只有与 `protocol` 匹配的路径会被调用。
/// - 注入接缝：`AppState::sentry` 替换为指向本地 mock ingest 的客户端。
/// 返回 (网关地址, ingest 计数)。
async fn spawn_route_gateway(
    upstream_status: StatusCode,
    upstream_body: serde_json::Value,
    protocol: UpstreamProtocol,
    model: &str,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let body = upstream_body.clone();
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(move || async move { (upstream_status, Json(body.clone())) }),
        )
        .route(
            "/v1/responses",
            post(move || async move { (upstream_status, Json(upstream_body.clone())) }),
        );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: 测试显式 opt-in 开放模式
    config.api_key = "none".to_string();
    config.gateway_keys = Vec::new();
    config.max_retries = 0; // 单次尝试，避免跨尝试退避/pool-wait 不确定性
    config.providers.insert(
        "zen-test".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{upstream_addr}"),
            default_model: model.to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.10,
            cached_price: 0.01,
            output_price: 0.20,
            models: vec![model.to_string()],
            model_specs: vec![],
            default_protocol: Some(protocol),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );

    // 注入接缝：先构造 AppState，替换 sentry 为指向本地 ingest mock 的客户端，再 Arc 化
    let mut state = AppState::new(config);
    let (probe_addr, counter) = spawn_ingest_probe().await;
    state.sentry = Arc::new(SentryClient::new(SentryConfig {
        endpoint: format!("http://{probe_addr}/ingest"),
        client_token: None,
        environment: Some("test".to_string()),
        release: None,
        buffer_capacity: 16,
    }));
    let state = Arc::new(state);
    let pool = Arc::new(KeyPool::new("zen-test", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("test-key", "sk-test", 1, 10));
    state.register_pool("zen-test", pool);

    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, counter)
}

async fn post_chat(addr: &SocketAddr, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .expect("chat 请求必须发出")
}

async fn post_responses(addr: &SocketAddr, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .json(&serde_json::json!({"model": model, "input": "Hi"}))
        .send()
        .await
        .expect("responses 请求必须发出")
}

/// 场景（被测）：/v1/chat/completions 上游 429 → kind=RateLimitExceeded →
/// 不得产生 Sentry 上报（红相：当前实现未接闸门 → 断言失败）。
/// 正控（同测试内第二网关）：上游 500 + 干净文本 → kind=UpstreamUnavailable →
/// 必须上报，证明 ingest 管道活着、计数器非真空。
#[tokio::test]
async fn chat_completions_quota_rate_limit_skips_sentry_report() {
    // ---- 被测场景：429（限流/配额族）----
    let (addr, counter) = spawn_route_gateway(
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({"error":{"message":"rate limit exceeded. please retry later"}}),
        UpstreamProtocol::Chat,
        "jev-chat",
    )
    .await;
    let resp = post_chat(&addr, "jev-chat").await;
    // 路由确实走完了耗尽路径（project_openai_error(RateLimitExceeded) → 429）
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        !wait_ingest_count(&counter, 1, Duration::from_millis(1500)).await,
        "[chat] 429 限流/配额耗尽不得产生 Sentry 上报（should_report_sentry_exhaustion 闸门缺失=红相）"
    );

    // ---- 正控：同一环境模式的干净 500 必须上报 ----
    let (addr2, counter2) = spawn_route_gateway(
        StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({"error":{"message":"internal boom"}}),
        UpstreamProtocol::Chat,
        "jev-chat",
    )
    .await;
    let resp2 = post_chat(&addr2, "jev-chat").await;
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        wait_ingest_count(&counter2, 1, Duration::from_millis(3000)).await,
        "[chat] 非配额类上游故障（UpstreamUnavailable）必须上报（正控，证明管道存活）"
    );
    assert_eq!(counter2.load(Ordering::SeqCst), 1, "[chat] 正控应恰好 1 条 ingest 事件");
}

/// 场景（被测）：/v1/responses 上游 429 → kind=RateLimitExceeded →
/// 不得产生 Sentry 上报（红相：当前实现未接闸门 → 断言失败）。
/// 正控（同测试内第二网关）：上游 500 + 干净文本 → 必须上报，证明管道存活。
#[tokio::test]
async fn responses_route_quota_rate_limit_skips_sentry_report() {
    // ---- 被测场景：429（限流/配额族）----
    let (addr, counter) = spawn_route_gateway(
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({"error":{"message":"rate limit exceeded. please retry later"}}),
        UpstreamProtocol::Responses,
        "jev-resp",
    )
    .await;
    let resp = post_responses(&addr, "jev-resp").await;
    // 路由确实走完了耗尽路径（project_openai_error(RateLimitExceeded) → 429）
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        !wait_ingest_count(&counter, 1, Duration::from_millis(1500)).await,
        "[responses] 429 限流/配额耗尽不得产生 Sentry 上报（should_report_sentry_exhaustion 闸门缺失=红相）"
    );

    // ---- 正控：同一环境模式的干净 500 必须上报 ----
    let (addr2, counter2) = spawn_route_gateway(
        StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({"error":{"message":"internal boom"}}),
        UpstreamProtocol::Responses,
        "jev-resp",
    )
    .await;
    let resp2 = post_responses(&addr2, "jev-resp").await;
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        wait_ingest_count(&counter2, 1, Duration::from_millis(3000)).await,
        "[responses] 非配额类上游故障（UpstreamUnavailable）必须上报（正控，证明管道存活）"
    );
    assert_eq!(counter2.load(Ordering::SeqCst), 1, "[responses] 正控应恰好 1 条 ingest 事件");
}