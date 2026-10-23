//! T2 红相验收测试：systemone 路由的 Sentry 上报过滤闸门
//! `should_report_sentry_exhaustion(kind: &GatewayErrorKind, last_error: &str) -> bool`
//!
//! 契约来源（Protocol Agent 冻结，以契约/ADR 为准）：
//! - ADR: /home/dm/pony-sentry/.agents/notes/implemented/process/2026-10-12-suppress-upstream-quota-errors-in-sentry.md
//! - 共享任务板 task-4「契约冻结存档」
//!
//! 判定语义 = OR（任中即不报）：
//! - 规则 A（kind 命中即 false）：kind ∈ { RateLimitExceeded{..}, QuotaExhausted,
//!   UpstreamUnavailable }；`CoreError::NoAvailableKey(_)` 经 `CoreError::kind()`
//!   （error.rs:133-135）折叠为 `RateLimitExceeded{retry_after:None}`，无需单独分支。
//!   `UpstreamUnavailable` 代表上游网络超时/连接失败/DNS/5xx，属于 provider 瞬态，
//!   不应创建 issue。
//! - 规则 B（文本命中即 false，与 kind 正交，大小写不敏感 `to_lowercase()` 子串）：
//!   `"no available key"` / `"freeusagelimiterror"` / `"rate limit"` / `"quota"` / `"429"`。
//! - 其余 kind（AuthInvalid / LockContention / ClientBadRequest / CapacityExhausted /
//!   ModelNotFound / Internal）→ true（报）。
//!
//! 分级：L1（纯函数单元）+ L2（systemone 路由级集成，可注入接缝 = `AppState::sentry`
//! 字段为 `pub`，可替换为指向本地 mock ingest 端点的 `SentryClient`）。
//!
//! 红相说明：`.dev-team/red-phase-systemone-filter.txt` 记录本文件当前阶段（函数不存在）
//! 的物理编译失败证据（Exit Code != 0）。
//!
//! 可见性依赖：本文件为标准集成测试（独立 crate，只能访问 ponyllm-server 的 `pub` 项）。
//! 契约中函数须以 `pub`（或等价再导出）对测试可见，否则绿相 E0603 无法验收；已向
//! Protocol Agent 同步该修正请求。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_core::sentry::{SentryClient, SentryConfig};
use ponyllm_server::routes::systemone::should_report_sentry_exhaustion;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};

// ---------------------------------------------------------------------------
// L1: 纯函数单元测试（判定语义与完整 kind 映射表）
// ---------------------------------------------------------------------------

/// NoAvailableKey 语义：kind 折叠为 RateLimitExceeded → 不报。
/// 同时锚定折叠契约本身（error.rs:133-135），防止上游行为漂移。
#[test]
fn no_available_key_folds_to_rate_limit_exceeded_and_never_reports() {
    // Arrange: 构造真实 NoAvailableKey 错误并取 kind 折叠结果
    let err = CoreError::NoAvailableKey("zen-jev".to_string());
    let folded_kind = err.kind();
    assert_eq!(
        folded_kind,
        GatewayErrorKind::RateLimitExceeded { retry_after: None },
        "CoreError::NoAvailableKey 必须折叠为 RateLimitExceeded{{retry_after:None}}（error.rs:133-135）"
    );

    // Act: 以折叠后 kind + 真实渲染文本（含 "No available key"）喂闸门
    let report = should_report_sentry_exhaustion(&folded_kind, &err.to_string());

    // Assert: 无可用 key（配额/冷却族）不得上报
    assert!(
        !report,
        "NoAvailableKey（→RateLimitExceeded）不得上报 Sentry"
    );
}

/// 规则 A：RateLimitExceeded 不看文本（含干净文本与空文本）恒不报。
#[test]
fn rate_limit_exceeded_never_reports_regardless_of_text() {
    // Arrange/Act/Assert: kind 命中即 false，文本任意
    let kind_none = GatewayErrorKind::RateLimitExceeded { retry_after: None };
    assert!(
        !should_report_sentry_exhaustion(&kind_none, "boom"),
        "RateLimitExceeded{{retry_after:None}} + 干净文本不得上报"
    );
    assert!(
        !should_report_sentry_exhaustion(&kind_none, ""),
        "RateLimitExceeded{{retry_after:None}} + 空文本不得上报（kind 判定独立于文本）"
    );
    let kind_some = GatewayErrorKind::RateLimitExceeded {
        retry_after: Some(Duration::from_secs(30)),
    };
    assert!(
        !should_report_sentry_exhaustion(&kind_some, "boom"),
        "RateLimitExceeded{{retry_after:Some(30s)}} 不得上报"
    );
}

/// 规则 A：QuotaExhausted 不看文本恒不报。
#[test]
fn quota_exhausted_never_reports_regardless_of_text() {
    let kind = GatewayErrorKind::QuotaExhausted;
    assert!(
        !should_report_sentry_exhaustion(&kind, "boom"),
        "QuotaExhausted + 干净文本不得上报"
    );
    assert!(
        !should_report_sentry_exhaustion(&kind, "upstream balance empty"),
        "QuotaExhausted + 配额措辞文本不得上报"
    );
}

/// 规则 B：文本标记 "no available key" 与 kind 正交 —— kind=Internal（本应报）也压住，
/// 且大小写不敏感（"No Available Key" 混合大小写）。
#[test]
fn text_marker_no_available_key_suppresses_report_orthogonal_to_kind() {
    let kind = GatewayErrorKind::Internal;
    let text = "No Available Key for provider 'zen-jev' (all keys cooling down)";
    assert!(
        !should_report_sentry_exhaustion(&kind, text),
        "文本兜底（no available key，混合大小写）命中时不得上报，无论 kind 为何"
    );
}

/// 规则 B：文本标记 "freeusagelimiterror"（zen 免费额度上游体）压住 kind=Internal。
#[test]
fn text_marker_freeusagelimiterror_suppresses_report_orthogonal_to_kind() {
    let kind = GatewayErrorKind::Internal;
    let text = "upstream responded FreeUsageLimitError: zen free tier daily cap reached";
    assert!(
        !should_report_sentry_exhaustion(&kind, text),
        "文本兜底（FreeUsageLimitError）命中时不得上报，无论 kind 为何"
    );
}

/// 规则 B：文本标记 "rate limit" 压住 kind=AuthInvalid（本应报）。
#[test]
fn text_marker_rate_limit_suppresses_report_orthogonal_to_kind() {
    let kind = GatewayErrorKind::AuthInvalid;
    let text = "Rate limit exceeded. Please try again later.";
    assert!(
        !should_report_sentry_exhaustion(&kind, text),
        "文本兜底（rate limit，大小写不敏感）命中时不得上报，无论 kind 为何"
    );
}

/// 规则 B：文本标记 "quota" 压住 kind=LockContention（本应报）。
#[test]
fn text_marker_quota_suppresses_report_orthogonal_to_kind() {
    let kind = GatewayErrorKind::LockContention;
    let text = "account quota exhausted for family antigravity";
    assert!(
        !should_report_sentry_exhaustion(&kind, text),
        "文本兜底（quota）命中时不得上报，无论 kind 为何"
    );
}

/// 规则 B：文本标记 "429"（状态文本，kind() 分类丢失率限信号时的兜底）压住 kind=Internal。
#[test]
fn text_marker_http_429_suppresses_report_orthogonal_to_kind() {
    let kind = GatewayErrorKind::Internal;
    let text = "HTTP 429 Too Many Requests: upstream throttled";
    assert!(
        !should_report_sentry_exhaustion(&kind, text),
        "文本兜底（429）命中时不得上报，无论 kind 为何"
    );
}

/// 完整 kind 映射表（true 侧）：可报告 kind + 干净文本（以及空文本）必须上报。
#[test]
fn reportable_kinds_still_report_with_clean_text() {
    let cases: [(GatewayErrorKind, &str); 7] = [
        (GatewayErrorKind::AuthInvalid, "401 invalid api key"),
        (
            GatewayErrorKind::LockContention,
            "refresh lock held by another replica",
        ),
        (
            GatewayErrorKind::ClientBadRequest,
            "400 invalid parameter 'temperature'",
        ),
        (
            GatewayErrorKind::CapacityExhausted,
            "required context window exceeds capacity",
        ),
        (GatewayErrorKind::ModelNotFound, "model 'x' does not exist"),
        (GatewayErrorKind::Internal, "internal core error: panic"),
        (GatewayErrorKind::Internal, ""), // 空输入边界：无标记文本，仍应上报
    ];
    for (kind, text) in cases {
        assert!(
            should_report_sentry_exhaustion(&kind, text),
            "kind={kind:?} 文本={text:?} 应上报（非配额/限流族）"
        );
    }
}

/// 契约边界：同一"Antigravity stream collect failed"文本，
/// kind=UpstreamUnavailable 与 kind=QuotaExhausted 均不得上报（按 kind 判）。
#[test]
fn antigravity_stream_collect_failure_does_not_report_transient_or_quota() {
    let transient = "Antigravity stream collect failed: upstream connection reset";
    assert!(
        !should_report_sentry_exhaustion(&GatewayErrorKind::UpstreamUnavailable, transient),
        "上游瞬态（UpstreamUnavailable + 干净文本）不得上报"
    );
    let quota = "Antigravity stream collect failed: Resource has been exhausted (e.g. check quota)";
    assert!(
        !should_report_sentry_exhaustion(&GatewayErrorKind::QuotaExhausted, quota),
        "上游配额帧（QuotaExhausted）不得上报"
    );
}

/// 上游网络不可用（连接失败/超时/DNS/5xx）无论文本如何都不创建 issue。
#[test]
fn upstream_unavailable_never_reports_regardless_of_text() {
    for text in [
        "connection reset by peer",
        "error sending request for url: dns error",
        "upstream returned 500 Internal Server Error",
        "",
    ] {
        assert!(
            !should_report_sentry_exhaustion(&GatewayErrorKind::UpstreamUnavailable, text),
            "UpstreamUnavailable 文本={text:?} 不得上报"
        );
    }
}

/// 正控：Internal（网关代码/不变量类错误）仍必须创建 issue。
#[test]
fn internal_error_still_reports() {
    assert!(
        should_report_sentry_exhaustion(
            &GatewayErrorKind::Internal,
            "internal invariant violated: unexpected state"
        ),
        "Internal 属于代码类错误，必须上报"
    );
}

// ---------------------------------------------------------------------------
// L2: systemone 路由级集成测试 —— 可注入接缝成立（AppState::sentry 为 pub 字段，
// 可替换为指向本地 mock ingest 的 SentryClient），断言配额/限流场景不产生任何上报。
// ---------------------------------------------------------------------------

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

/// 拉起一个独立 systemone 测试网关：
/// - 上游 mock 固定返回 `upstream_status` + `upstream_body`（max_retries=0，单次尝试，
///   确定性无 pool-wait / 退避延迟）；
/// - 注入接缝：`AppState::sentry` 替换为指向本地 mock ingest 的客户端。
/// 返回 (网关地址, ingest 计数)。
async fn spawn_systemone_gateway(
    upstream_status: StatusCode,
    upstream_body: serde_json::Value,
) -> (SocketAddr, Arc<AtomicUsize>) {
    spawn_systemone_gateway_with_pool(upstream_status, upstream_body, true).await
}

async fn spawn_systemone_gateway_with_pool(
    upstream_status: StatusCode,
    upstream_body: serde_json::Value,
    register_pool: bool,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let upstream = Router::new().route(
        "/systemone",
        // 零提取器处理器（axum 0.8 合法）：上游响应只由 status+body 决定，忽略请求体
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
        "zen-jev".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{upstream_addr}"),
            default_model: "jev-1.13-free".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Free,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["jev-1.13-free".to_string()],
            model_specs: vec![],
            default_protocol: Some(UpstreamProtocol::Systemone),
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
    if register_pool {
        let pool = Arc::new(KeyPool::new("zen-jev", RoutingStrategy::RoundRobin));
        pool.add_key(ApiKeyEntry::new("jev-test", "test-key", 1, 10));
        state.register_pool("zen-jev", pool);
    }

    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, counter)
}

async fn post_systemone(addr: &SocketAddr) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/systemone"))
        .json(&serde_json::json!({"model": "jev-1.13-free", "state": "test", "questions": {}}))
        .send()
        .await
        .expect("systemone 请求必须发出")
}

/// 场景 A（被测）：上游 429 → kind=RateLimitExceeded → 不得上报；
/// 干净 500 → kind=UpstreamUnavailable → 不得上报。
/// 正控（同测试内第三网关）：无 key pool 触发 Internal 代码类错误 → 必须上报，
/// 证明 ingest 管道活着、计数器非真空。
#[tokio::test]
async fn systemone_rate_limit_exhaustion_skips_sentry_report() {
    // ---- 被测场景：429（限流/配额族）----
    let (addr, counter) = spawn_systemone_gateway(
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({"error":{"message":"HTTP 429 FreeUsageLimitError: rate limit exceeded"}}),
    )
    .await;
    let resp = post_systemone(&addr).await;
    // 路由确实走完了耗尽路径（而非 4xx 提前返回）——429 对 429
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    // 沉降窗口上界 1.5s（本地管道事件 ~ms 级到达）：实现漏过滤时事件必在此窗口内出现
    assert!(
        !wait_ingest_count(&counter, 1, Duration::from_millis(1500)).await,
        "429 限流/配额耗尽不得产生 Sentry 上报"
    );

    // ---- 上游瞬态负控：干净 500 → UpstreamUnavailable → 不得上报 ----
    let (addr2, counter2) = spawn_systemone_gateway(
        StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({"error":{"message":"internal boom"}}),
    )
    .await;
    let resp2 = post_systemone(&addr2).await;
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !wait_ingest_count(&counter2, 1, Duration::from_millis(1500)).await,
        "UpstreamUnavailable（干净 500）不得上报"
    );

    // ---- 正控：Internal（代码类错误）仍上报，证明 ingest 管道存活 ----
    let (addr3, counter3) =
        spawn_systemone_gateway_with_pool(StatusCode::OK, serde_json::json!({}), false).await;
    let resp3 = post_systemone(&addr3).await;
    assert_eq!(resp3.status(), StatusCode::BAD_GATEWAY);
    assert!(
        wait_ingest_count(&counter3, 1, Duration::from_millis(3000)).await,
        "Internal 代码类错误必须上报（正控，证明管道存活）"
    );
    assert_eq!(
        counter3.load(Ordering::SeqCst),
        1,
        "正控应恰好 1 条 ingest 事件"
    );
}

/// 场景 B（被测）：上游 500 + FreeUsageLimitError 文本 → UpstreamUnavailable
/// （且文本命中）→ 不得上报。正控同测试内第二网关：无 key pool 触发 Internal →
/// 必须上报（证明 ingest 管道存活）。
#[tokio::test]
async fn systemone_upstream_quota_text_fallback_skips_sentry_report() {
    // ---- 被测场景：500 + FreeUsageLimitError 文本 ----
    let (addr, counter) = spawn_systemone_gateway(
        StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({"error":{"message":"FreeUsageLimitError: daily limit reached"}}),
    )
    .await;
    let resp = post_systemone(&addr).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !wait_ingest_count(&counter, 1, Duration::from_millis(1500)).await,
        "文本兜底（FreeUsageLimitError）命中时不得上报，无论 kind 为何"
    );

    // ---- 正控：Internal（代码类错误）仍上报，证明 ingest 管道存活 ----
    let (addr2, counter2) =
        spawn_systemone_gateway_with_pool(StatusCode::OK, serde_json::json!({}), false).await;
    let resp2 = post_systemone(&addr2).await;
    assert_eq!(resp2.status(), StatusCode::BAD_GATEWAY);
    assert!(
        wait_ingest_count(&counter2, 1, Duration::from_millis(3000)).await,
        "Internal 代码类错误必须上报（正控，证明管道存活）"
    );
}
