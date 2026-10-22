//! Wave-10 vision — URL-fetch acceptance isolation binary.
//!
//! `PONYLLM_IMAGE_HOST_ALLOWLIST` is PROCESS-global: tests that need the
//! allowlist (to reach a local mock image server for positive URL-fetch
//! checks) live in a dedicated binary whose env is set ONCE; the zero-egress /
//! SSRF-rejection tests in `adversarial_deepseek_web_vision_tests.rs` keep
//! the strict (allowlist-absent) posture in their own process.

use axum::response::IntoResponse as _;
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

/// Set once for this binary: the local mock image server is reachable so the
/// content-type / extension validation of the SSRF-safe fetcher can be
/// exercised positively. Never removed (process exits).
static ALLOWLIST_ONCE: Once = Once::new();
fn ensure_allowlist() {
    ALLOWLIST_ONCE.call_once(|| {
        std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");
    });
}

const CHALLENGE_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"challenge":{
"algorithm":"DeepSeekHashV1",
"challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a",
"salt":"s000",
"difficulty":8,
"signature":"sig2",
"expireAt":1729666800000
}},"biz_msg":"ok"}}"#;
const SESSION_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"chat_session":{"id":"sess-mock-1"}},"biz_msg":"ok"}}"#;

struct Upstream {
    base: String,
    upload_calls: Arc<AtomicUsize>,
}

async fn spawn_upstream() -> Arc<Upstream> {
    use axum::extract::Multipart;
    use axum::response::Response;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        upload_calls: Arc::new(AtomicUsize::new(0)),
    });
    let up_upload = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(|axum::Json(_): axum::Json<serde_json::Value>| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
                    .into_response()
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
                    .into_response()
            }),
        )
        .route(
            "/api/v0/file/upload_file",
            axum::routing::post(move |_h: axum::http::HeaderMap, mut mp: Multipart| {
                let up = up_upload.clone();
                async move {
                    while let Some(f) = mp.next_field().await.unwrap_or(None) {
                        let _ = f.bytes().await.unwrap_or_default();
                    }
                    up.upload_calls.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"code":0,"data":{"biz_code":0,"biz_data":{
                        "id":"file-1","status":"SUCCESS","file_name":"x.png","file_size":3,
                        "model_kind":"VISION","is_image":true,"audit_result":"unknown"}}})).into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_h: axum::http::HeaderMap, _b: axum::Json<serde_json::Value>| async move {
                Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(
                        "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n",
                    ))
                    .unwrap()
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    up
}

async fn spawn_gateway(upstream: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek-web".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: upstream.to_string(),
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
                input_types: vec!["text".to_string(), "image".to_string()],
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
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn send_chat(gw: &str, url: &str) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({
            "model": "deepseek-chat",
            "messages": [{"role":"user","content":[
                {"type":"text","text":"what color"},
                {"type":"image_url","image_url":{"url": url}}
            ]}]
        }))
        .send()
        .await
        .unwrap();
    (resp.status(), resp.text().await.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// content-type / extension mismatch on the SSRF-safe fetcher (positive local
// fetch via the allowlist): both must be rejected explicitly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_ssrf_content_type_extension_mismatch() {
    ensure_allowlist();
    // img.txt served with image/png (extension mismatch) and img.png served
    // with text/html (mime masquerade) — both must be rejected with 4xx.
    let app = axum::Router::new()
        .route(
            "/img.txt",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(axum::body::Body::from("png"))
                    .unwrap()
            }),
        )
        .route(
            "/img.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "text/html")
                    .body(axum::body::Body::from("<html></html>"))
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    for (label, url) in [
        ("ext-mismatch", format!("{base}/img.txt")),
        ("mime-masquerade", format!("{base}/img.png")),
    ] {
        let (status, text) = send_chat(&gw, &url).await;
        assert!(
            !status.is_success() && status.as_u16() < 500,
            "adv: {label} -> explicit 4xx; got {status} {text}"
        );
    }
    assert_eq!(
        up.upload_calls.load(Ordering::SeqCst),
        0,
        "adv: rejected images never uploaded"
    );
}