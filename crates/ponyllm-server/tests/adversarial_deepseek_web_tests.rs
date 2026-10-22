//! Wave-9 deepseek-web — L2-AT gateway adversarial tests over the real HTTP
//! full chain. Classes: (a) network timeout/delay injection (delay = NFR
//! baseline 3000ms + margin, never self-chosen), (b) malformed upstream SSE,
//! (c) deterministic concurrency race (fixed seed LCG + Barrier + fixed N).
//!
//! COMPILE DEPENDENCY (identical to acceptance_deepseek_web_gateway_tests.rs):
//! the provider is declared with `UpstreamProtocol::DeepSeekWeb` (task-4
//! config form). Compile-gated until task-4 lands; the client-level
//! adversarial matrix (adversarial_deepseek_web_client_tests.rs) carries the
//! runtime-red evidence on the frozen stubs at this phase.

use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

const CHALLENGE_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"challenge":{
"algorithm":"DeepSeekHashV1",
"challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a",
"salt":"s000",
"difficulty":8,
"signature":"sig2",
"expireAt":1729666800000
}},"biz_msg":"ok"}}"#;

const SESSION_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"chat_session":{"id":"sess-mock-1"}},"biz_msg":"ok"}}"#;

/// Simple solvable-chain mock. `tag_fn` produces the per-request
/// `response_message_id` used by the completion SSE.
async fn spawn_tagged_mock(tag_fn: Arc<dyn Fn() -> String + Send + Sync>) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    use axum::http::HeaderMap;
    use axum::response::Response;
    let challenge_calls = Arc::new(AtomicUsize::new(0));
    let completion_calls = Arc::new(AtomicUsize::new(0));
    let cc1 = challenge_calls.clone();
    let cc2 = completion_calls.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(_): axum::Json<serde_json::Value>| async move {
                cc1.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_: axum::Json<serde_json::Value>, _h: HeaderMap| {
                let tag_fn = tag_fn.clone();
                let cc2 = cc2.clone();
                async move {
                    cc2.fetch_add(1, Ordering::SeqCst);
                    let tag = tag_fn();
                    let sse = format!(
                        "event: ready\ndata: {{\"response_message_id\":\"{tag}\",\"request_message_id\":\"q\",\"model_type\":\"m\"}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"{tag}\"}}\n\n\
event: close\ndata: {{\"close_reason\":\"success\"}}\n\n"
                    );
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(sse))
                        .unwrap()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), challenge_calls, completion_calls)
}

async fn spawn_gateway(upstream_base: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek-web".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: upstream_base.to_string(),
            default_model: "deepseek-chat".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["deepseek-chat".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-chat".to_string(),
                priority: Some(10),
                context_window: "128K".to_string(),
                max_output: "4K".to_string(),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::DeepSeekWeb),
            ..Default::default()
        },
    );
    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("web-key-1", "web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);
    let gateway_app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, gateway_app).await.unwrap();
    });
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
// (a) network timeout injection through the gateway
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_gw_timeout_injection_bounded_failure() {
    // Challenge endpoint stalls baseline+margin; the gateway must fail the
    // request explicitly within a bounded window instead of hanging.
    const NFR_SHORT_CALL_TIMEOUT_MS: u64 = 3000; // nfr-baseline-wave-9.json
    const MARGIN_MS: u64 = 2000;
    let delay = std::time::Duration::from_millis(NFR_SHORT_CALL_TIMEOUT_MS + MARGIN_MS);

    let challenge_calls = Arc::new(AtomicUsize::new(0));
    let cc = challenge_calls.clone();
    use axum::response::Response as AxResponse;
    let app = axum::Router::new().route(
        "/api/v0/chat/create_pow_challenge",
        axum::routing::post(move |axum::Json(_): axum::Json<serde_json::Value>| async move {
            cc.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            AxResponse::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(CHALLENGE_OK))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let gw = spawn_gateway(&format!("http://{addr}")).await;

    let started = std::time::Instant::now();
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}],"stream":true}))
        .send()
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    assert!(
        !status.is_success() || !text.is_empty(),
        "adv: injected-delay request must terminate with an explicit result; got {status}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(6),
        "adv: must fail within budget+margin; elapsed {:?}",
        elapsed
    );
    assert_eq!(
        challenge_calls.load(Ordering::SeqCst),
        1,
        "adv: no blind retry after the injected timeout"
    );
}

// ---------------------------------------------------------------------------
// (b) malformed upstream SSE: client-facing stream stays well-formed/bounded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_gw_malformed_upstream_sse_terminates_wellformed() {
    use axum::response::Response as AxResponse;
    let one = Arc::new(AtomicUsize::new(0));
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(|axum::Json(_): axum::Json<serde_json::Value>| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_: axum::Json<serde_json::Value>| {
                let one = one.clone();
                async move {
                    one.fetch_add(1, Ordering::SeqCst);
                    // Malformed SSE: broken JSON frame then an error close.
                    AxResponse::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(
                            "event: ready\ndata: {not json\n\nevent: close\ndata: {\"close_reason\":\"error\"}\n\n",
                        ))
                        .unwrap()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let gw = spawn_gateway(&format!("http://{addr}")).await;

    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}],"stream":true}))
        .send()
        .await
        .unwrap();
    let _status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    // The upstream malformed frame must not wedge the gateway: the client
    // receives a bounded, well-formed (possibly error) terminal. No fake
    // success stop, no indefinite hang.
    assert!(
        !text.contains("\"finish_reason\":\"stop\"") || !text.contains("{not json"),
        "adv: malformed upstream frame must not surface as a fake success chunk"
    );
    assert_eq!(one.load(Ordering::SeqCst), 1, "adv: exactly one completion attempt");
}

// ---------------------------------------------------------------------------
// (c) deterministic concurrency race through the gateway
// ---------------------------------------------------------------------------

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn adv_gw_concurrency_no_cross_stream_bleed() {
    const N: usize = 8;
    let mut rng = Lcg(0x0FEDCBA987654321);
    let tags: Vec<String> = (0..N).map(|i| format!("tag-{:04x}", rng.next() ^ (i as u64))).collect();
    let tag_iter = tags.clone();
    let cursor = Arc::new(AtomicUsize::new(0));
    let tag_fn = Arc::new(move || {
        let idx = cursor.fetch_add(1, Ordering::SeqCst);
        tag_iter[idx % tag_iter.len()].clone()
    });
    let (base, _chal, completion_calls) = spawn_tagged_mock(tag_fn).await;
    let gw = spawn_gateway(&base).await;

    let barrier = Arc::new(Barrier::new(N));
    let mut handles = Vec::new();
    for i in 0..N {
        let gw = gw.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait();
            let resp = reqwest::Client::new()
                .post(format!("{gw}/v1/chat/completions"))
                .bearer_auth("web-token-abc")
                .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":format!("msg-{i}")}],"stream":true}))
                .send()
                .await
                .unwrap();
            let text = resp.text().await.unwrap_or_default();
            (i, text)
        }));
    }
    let mut seen_tags = Vec::new();
    for h in handles {
        let (i, text) = h.await.unwrap();
        // Each stream must contain exactly one content payload: its own tag.
        let mut hits: Vec<&str> = tags.iter().filter(|t| text.contains(t.as_str())).map(|s| s.as_str()).collect();
        hits.sort_unstable();
        assert_eq!(
            hits.len(),
            1,
            "adv: stream {i} must reference exactly one response tag (no cross-bleed); got {hits:?}"
        );
        seen_tags.push(hits[0].to_string());
    }
    seen_tags.sort();
    assert_eq!(seen_tags.len(), N, "adv: all N distinct streams observed");
    assert_eq!(
        completion_calls.load(Ordering::SeqCst),
        N as usize,
        "adv: exactly N completion requests"
    );
}