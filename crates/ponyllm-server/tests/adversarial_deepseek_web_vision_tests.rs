//! Wave-10 deepseek-web vision — L2-AT adversarial (SSRF zero-egress, malformed
//! payloads, content-type/extension mismatch, oversized payload).
//!
//! Contract: `.dev-team/contracts/wave-10-deepseek-web-vision.md` (R-VIS-06..10).
//! Every test is RED now (bridge unimplemented -> images silently dropped ->
//! 200 with degraded content instead of the required explicit 4xx/zero-egress)
//! and must go GREEN after task-6/7 land.
//!
//! Determinism: fixed payloads/URLs, canary listeners assert ZERO connections
//! (no egress), a short post-response drain is used only to collect an already
//! made connection attempt (never as a synchronisation primitive).

use axum::response::IntoResponse as _;
use base64::Engine as _;
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Serializes PONYLLM_* env mutations across tests (process-global env +
/// parallel harness threads would otherwise race).
static ENV_LOCK: Mutex<()> = Mutex::new(());

const CHALLENGE_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"challenge":{
"algorithm":"DeepSeekHashV1",
"challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a",
"salt":"s000",
"difficulty":8,
"signature":"sig2",
"expireAt":1729666800000
}},"biz_msg":"ok"}}"#;
const SESSION_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"chat_session":{"id":"sess-mock-1"}},"biz_msg":"ok"}}"#;
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[derive(Debug, Clone)]
struct Call {
    method: String,
    path: String,
    detail: String,
}

struct Upstream {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
    upload_calls: Arc<AtomicUsize>,
    completion_calls: Arc<AtomicUsize>,
}

async fn spawn_upstream() -> Arc<Upstream> {
    use axum::extract::Multipart;
    use axum::http::HeaderMap;
    use axum::response::Response;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        calls: Arc::new(Mutex::new(Vec::new())),
        upload_calls: Arc::new(AtomicUsize::new(0)),
        completion_calls: Arc::new(AtomicUsize::new(0)),
    });
    let up_upload = up.clone();
    let up_completion = up.clone();

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
            axum::routing::post(move |_h: HeaderMap, mut multipart: Multipart| {
                let up = up_upload.clone();
                async move {
                    let mut filename = String::new();
                    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
                        filename = field.file_name().unwrap_or_default().to_string();
                        let _ = field.bytes().await.unwrap_or_default();
                    }
                    up.upload_calls.fetch_add(1, Ordering::SeqCst);
                    up.calls.lock().unwrap().push(Call {
                        method: "POST".into(),
                        path: "/api/v0/file/upload_file".into(),
                        detail: format!("filename={filename}"),
                    });
                    axum::Json(json!({"code":0,"data":{"biz_code":0,"biz_data":{
                        "id":"file-1","status":"PENDING","file_name":filename,
                        "file_size":97,"model_kind":"VISION","is_image":true,"audit_result":"unknown"
                    }}}))
                    .into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_h: HeaderMap, body: axum::Json<serde_json::Value>| {
                let up = up_completion.clone();
                async move {
                    up.completion_calls.fetch_add(1, Ordering::SeqCst);
                    up.calls.lock().unwrap().push(Call {
                        method: "POST".into(),
                        path: "/api/v0/chat/completion".into(),
                        detail: format!("ref_file_ids={:?}", body["ref_file_ids"]),
                    });
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(
                            "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n",
                        ))
                        .unwrap()
                }
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
    config.request_body_limit = 512 * 1024 * 1024;
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

fn png_uri() -> String {
    format!("data:image/png;base64,{PNG_B64}")
}

/// Canary listener: accepts connections and counts them (zero = no egress).
async fn spawn_canary() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let c = conns.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok(_) => {
                    c.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://{addr}/x.png"), conns)
}

// ---------------------------------------------------------------------------
// R-VIS-10 SSRF zero-egress: every blocked address sees ZERO connections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_10_ssrf_zero_egress() {
    let (canary_url, canary_conns) = spawn_canary().await;
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST"); // zero-egress: strict

    let attack_urls = vec![
        canary_url.clone(),                        // http://127.0.0.1:<canary>/x.png
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "http://10.0.0.1/x.png".to_string(),
        "http://[::1]:1/x.png".to_string(),
        "file:///etc/passwd".to_string(),
    ];
    for url in attack_urls {
        let (status, text) = send_chat(&gw, &url).await;
        assert!(
            !status.is_success() && status.as_u16() < 500,
            "R-VIS-10: {url} -> 4xx rejection; got {status} {text}"
        );
        // Drain: collect any connection attempt already made (never a
        // synchronisation primitive).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            canary_conns.load(Ordering::SeqCst),
            0,
            "R-VIS-10: ZERO egress connections allowed to the canary ({url})"
        );
    }
    assert_eq!(
        up.upload_calls.load(Ordering::SeqCst),
        0,
        "R-VIS-10: no bridge/uploads happen for rejected image URLs"
    );
}

// ---------------------------------------------------------------------------
// L2-AT extras
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_ssrf_malformed_base64() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = send_chat(&gw, "data:image/png;base64,@@@@not-base64").await;
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "adv: malformed base64 -> explicit 4xx; got {status} {text}"
    );
    assert_eq!(up.upload_calls.load(Ordering::SeqCst), 0, "adv: no upload for malformed base64");
    assert!(
        !text.to_lowercase().contains("don't see"),
        "adv: no silent-drop degraded content; got {text}"
    );
}

#[tokio::test]
async fn adv_ssrf_content_type_extension_mismatch() {
    // With the test allowlist the local mock is reachable; the content-type /
    // extension pair must be validated (image/* content vs extension whitelist).
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

    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");
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
    assert_eq!(up.upload_calls.load(Ordering::SeqCst), 0, "adv: rejected images never uploaded");
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST");
}

#[tokio::test]
async fn adv_ssrf_oversized_payload() {
    // 100MiB+1 raw image via base64 request: gateway size check must 4xx with
    // zero upload (memory-heavy, run last).
    let big = "A".repeat(104_857_600 + 1);
    let b64 = base64::engine::general_purpose::STANDARD.encode(big.as_bytes());
    let uri = format!("data:image/png;base64,{b64}");
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = send_chat(&gw, &uri).await;
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "adv: >100MB payload -> 4xx; got {status} {text}"
    );
    assert_eq!(up.upload_calls.load(Ordering::SeqCst), 0, "adv: oversize -> zero uploads");
}

// Keep the `Value`/Call types referenced (shared with the acceptance suite's
// harness shape).
#[allow(dead_code)]
fn _type_refs(_: &Value, _: &Call) {}
