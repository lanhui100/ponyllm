//! deepseek-web vision bridge integration (T4, wave-10 contract R-VIS-*).
//!
//! Full chain against a mock chat.deepseek.com upstream (axum, random port):
//! downstream `image_url` (base64 data URI) must be decoded → uploaded
//! (multipart `file` + PoW for `/api/v0/file/upload_file`) → `fetch_files`
//! polled to SUCCESS → `ref_file_ids` injected IN ORDER into the completion
//! body; SSRF rejections surface as explicit 4xx with zero egress. Owned by
//! T4 (exec-integration): `deepseek_web_` prefix. The frozen T2 acceptance
//! files are separate and untouched.
//!
//! The mock resolves the PoW challenge per `target_path` (upload vs
//! completion) so both the image upload and the completion solve cleanly with
//! the golden-vector digest (salt `abc`, expireAt 12345, difficulty 1).

use axum::extract::{Multipart, Query};
use axum::routing::{get, post};
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn model_spec(name: &str) -> ModelSpec {
    ModelSpec {
        rate_limits: None,
        priority: None,
        name: name.to_string(),
        tier: ModelTier::Standard,
        context_window: "128K".to_string(),
        max_output: "16K".to_string(),
        input_types: vec!["text".to_string(), "image".to_string()],
        output_types: vec!["text".to_string()],
        ..Default::default()
    }
}

fn provider_config(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url: base_url.to_string(),
        default_model: "deepseek-v4-flash".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.14,
        cached_price: 0.014,
        output_price: 0.28,
        models: vec!["deepseek-v4-flash".to_string()],
        model_specs: vec![model_spec("deepseek-v4-flash")],
        default_protocol: Some(ponyllm_core::pool::UpstreamProtocol::DeepSeekWeb),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

const GOLDEN_CHALLENGE: &str =
    "f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb";

fn challenge_for(_target_path: &str) -> serde_json::Value {
    // Difficulty 1, salt "abc", expireAt 12345 → answer 0 solves on any target
    // (the worker golden vector is target-independent at the hash level).
    json!({
        "biz_code": 0,
        "biz_data": {
            "challenge": {
                "algorithm": "DeepSeekHashV1",
                "challenge": GOLDEN_CHALLENGE,
                "salt": "abc",
                "difficulty": 1,
                "signature": "sig1",
                "expireAt": 12345,
                "expireAfter": 3600
            }
        }
    })
}

/// Set up a stateful mock upstream exposing the full vision bridge surface.
/// Returns `(base_url, upload_calls, completion_bodies)` where the counters
/// let tests assert the upload chain and the injected `ref_file_ids`.
async fn spawn_vision_mock() -> (
    String,
    Arc<Mutex<Vec<(String, String)>>>,  // (filename, bytes_len) per upload
    Arc<Mutex<Vec<serde_json::Value>>>, // completion request bodies
) {
    let uploads: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let completions: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let up = uploads.clone();
    let comp = completions.clone();

    let app = Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            post(|Json(req): Json<serde_json::Value>| async move {
                Json(challenge_for(req["target_path"].as_str().unwrap_or("")))
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            post(|Json(_): Json<serde_json::Value>| async move {
                Json(json!({
                    "biz_code": 0,
                    "biz_data": { "chat_session": { "id": "sess-vis-1" } }
                }))
            }),
        )
        .route(
            "/api/v0/file/upload_file",
            post(move |req: Multipart| {
                let up = up.clone();
                async move {
                    let mut req = req;
                    let mut filename = String::new();
                    let mut bytes = Vec::new();
                    while let Some(field) = req.next_field().await.unwrap() {
                        filename = field.file_name().unwrap_or("").to_string();
                        let data = field.bytes().await.unwrap();
                        bytes = data.to_vec();
                    }
                    up.lock().unwrap().push((filename.clone(), format!("{}b", bytes.len())));
                    Json(json!({
                        "code": 0,
                        "data": {
                            "biz_code": 0,
                            "biz_data": {
                                "id": format!("file-uploaded-{}", up.lock().unwrap().len()),
                                "status": "PENDING",
                                "file_name": filename,
                                "file_size": bytes.len(),
                                "model_kind": "VISION",
                                "is_image": true,
                                "audit_result": "ok"
                            }
                        }
                    }))
                }
            }),
        )
        .route(
            "/api/v0/file/fetch_files",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let ids = q.get("file_ids").cloned().unwrap_or_default();
                let items: Vec<serde_json::Value> = ids
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|id| json!({
                        "id": id,
                        "status": "SUCCESS",
                        "file_name": "image.png",
                        "file_size": 97,
                        "is_image": true,
                    }))
                    .collect();
                Json(json!({
                    "code": 0,
                    "data": { "biz_code": 0, "biz_data": { "file_infos": items } }
                }))
            }),
        )
        .route(
            "/api/v0/chat/completion",
            post(move |Json(body): Json<serde_json::Value>| {
                let comp = comp.clone();
                async move {
                    comp.lock().unwrap().push(body.clone());
                    let sse = format!(
                        "event: ready\ndata: {}\n\n\
                         event: delta\ndata: {}\n\n\
                         event: finish\ndata: {}\n\n\
                         event: close\ndata: {}\n\n",
                        json!({"response_message_id":"r-vis","request_message_id":"q-vis","model_type":"deepseek-v4-flash"}),
                        json!({"p":"response/fragments/-1/content","o":"APPEND","v":"red"}),
                        json!({"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}),
                        json!({"close_reason":"success"})
                    );
                    axum::response::Response::new(axum::body::Body::from(sse))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), uploads, completions)
}

async fn spawn_gateway_for(base: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert("deepseek-web".to_string(), provider_config(base));
    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-web-k1", "user-web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

/// R-VIS-03/05/11/12: a base64-data-URI image in the user message is decoded,
/// uploaded (multipart) and its file_id injected into `ref_file_ids` IN ORDER,
/// then the completion SSE flows to the downstream as OpenAI chunks.
#[tokio::test]
async fn deepseek_web_vision_base64_upload_injects_ref_file_ids() {
    let (upstream, uploads, completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    // Unblock 127.0.0.1 for the completion upstream (test-only knob).
    std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");

    let red_png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let client = reqwest::Client::new();
    // Set a short parse-wait budget so the poll resolves fast (mock returns
    // SUCCESS immediately).
    std::env::set_var("PONYLLM_VISION_PARSE_TIMEOUT_MS", "2000");
    let resp = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What color is this?"},
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{red_png}")}}
                ]
            }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "vision request must succeed");

    // The image was uploaded exactly once (R-VIS-15 per-request fresh upload).
    let ups = uploads.lock().unwrap();
    assert_eq!(ups.len(), 1, "exactly one upload expected, got {ups:?}");
    assert_eq!(ups[0].0, "image.png", "upload filename must be image.png");
    drop(ups);

    // The completion body carries ref_file_ids in order, and no __web_images
    // side channel leaks onto the wire (Lead ruling).
    let bodies = completions.lock().unwrap();
    let body = bodies.last().expect("completion body captured");
    assert!(
        body.get("__web_images").is_none(),
        "internal __web_images must never reach the wire"
    );
    let ids = body["ref_file_ids"].as_array().expect("ref_file_ids array");
    assert_eq!(ids.len(), 1, "one file id injected");
    assert!(
        ids[0].as_str().unwrap_or("").starts_with("file-uploaded-"),
        "uploaded file id injected, got {ids:?}"
    );
    drop(bodies);
}

/// R-VIS-07: SSRF blocked hosts (loopback / link-local / metadata / private /
/// ::1) are rejected with an explicit 4xx and ZERO egress (no upload attempted).
#[tokio::test]
async fn deepseek_web_vision_ssrf_rejections() {
    let (upstream, uploads, _completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    // Ensure strict blocking (no allowlist).
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST");
    let client = reqwest::Client::new();
    // SSRF: image_url with a blocked host must fail the whole request (4xx).
    let resp = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "http://169.254.169.254/latest/meta-data"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    // SSRF: image_url with a blocked host must fail the whole request with an
    // explicit 4xx — NEVER a 5xx/upstream fault (defect A: R-VIS-13/10).
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        status.is_client_error(),
        "SSRF image must fail with 4xx, got status {} (body: {text})",
        status,
    );
    // Production UX: the message must surface the RAW cause, without gateway
    // internals (no "exhausted / across keys / Last error / upstream-side").
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("image host is blocked (SSRF policy)"),
        "message must carry the raw SSRF reason, got: {msg}"
    );
    assert!(
        !msg.contains("exhausted") && !msg.contains("across keys") && !msg.contains("Last error"),
        "message must not leak gateway internals, got: {msg}"
    );
    assert!(
        !msg.contains("upstream-side failure"),
        "client error must not be labeled upstream-side, got: {msg}"
    );
    // Zero uploads reached the mock (blocked before any egress).
    assert_eq!(
        uploads.lock().unwrap().len(),
        0,
        "SSRF rejection must cause zero upload attempts"
    );
}
/// Defect A (R-VIS-13): a malformed image (bad base64 / non-image mime /
/// unsupported type) must fail with an explicit **4xx**, never a 5xx/upstream
/// fault, and never trigger a silent no-image fallback.
#[tokio::test]
async fn deepseek_web_vision_bad_image_rejected_4xx() {
    let (upstream, uploads, completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    let client = reqwest::Client::new();
    // Bad base64 payload (not valid base64) in a data URI.
    let resp = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,!!!not-base64!!!"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text1 = resp.text().await.unwrap_or_default();
    assert!(
        status.is_client_error(),
        "bad base64 must fail with 4xx, got status {} (body: {text1})",
        status,
    );
    // Raw cause only, no gateway internals.
    let body1: serde_json::Value = serde_json::from_str(&text1).unwrap_or(serde_json::json!({}));
    let msg1 = body1["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg1.contains("base64 image decode failed") || msg1.contains("decode"),
        "message must carry the raw decode reason, got: {msg1}"
    );
    assert!(
        !msg1.contains("exhausted") && !msg1.contains("across keys"),
        "message must not leak gateway internals, got: {msg1}"
    );
    assert_eq!(uploads.lock().unwrap().len(), 0, "bad image must not upload");
    assert_eq!(completions.lock().unwrap().len(), 0, "bad image must not reach completion");

    // Non-image mime (text/plain) in a data URI → 4xx, zero upload.
    let resp2 = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": "data:text/plain;base64,aGVsbG8="}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    let status2 = resp2.status();
    let text2 = resp2.text().await.unwrap_or_default();
    assert!(
        status2.is_client_error(),
        "non-image mime must fail with 4xx, got status {} (body: {text2})",
        status2,
    );
    let body2: serde_json::Value = serde_json::from_str(&text2).unwrap_or(serde_json::json!({}));
    let msg2 = body2["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg2.contains("not an image") || msg2.contains("not in the allowed image whitelist"),
        "message must carry the raw mime reason, got: {msg2}"
    );
    assert!(
        !msg2.contains("exhausted") && !msg2.contains("across keys"),
        "message must not leak gateway internals, got: {msg2}"
    );
    assert_eq!(uploads.lock().unwrap().len(), 0, "non-image mime must not upload");
}

/// Defect C (R-VIS-19): `/v1/models` entries expose declared input/output
/// modalities — a deepseek-web provider with `input_types=["text","image"]`
/// must surface `"image"` in the listing so clients can discover vision
/// capability.
#[tokio::test]
async fn deepseek_web_vision_models_listing_exposes_image() {
    let (upstream, _uploads, _completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{gw}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entry = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| {
            m["id"].as_str().unwrap_or("").ends_with("deepseek-v4-flash")
                && m["owned_by"].as_str() == Some("deepseek-web")
        })
        .expect("deepseek-web model entry present");
    assert!(
        entry.get("input_types").is_some(),
        "entry must expose input_types, got {entry:?}"
    );
    let inputs = entry["input_types"].as_array().unwrap();
    assert!(
        inputs.iter().any(|t| t.as_str() == Some("image")),
        "input_types must include image, got {inputs:?}"
    );
    assert!(
        inputs.iter().any(|t| t.as_str() == Some("text")),
        "input_types must include text, got {inputs:?}"
    );
}

/// Defect A follow-up (R-VIS-11/13): >50 images must fail with an explicit
/// 4xx (never 502 upstream_exhausted), and the completion must never be
/// reached (no silent no-image fallback).
#[tokio::test]
async fn deepseek_web_vision_over_limit_51_images_4xx() {
    let (upstream, uploads, completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    let client = reqwest::Client::new();
    let red_png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let content: Vec<serde_json::Value> = (0..51)
        .map(|_i| json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/png;base64,{}", red_png)}
        }))
        .collect();
    let mut content = content;
    content.insert(0, json!({"type": "text", "text": "look"}));
    let resp = client
        .post(format!("{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": content}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_client_error(),
        ">50 images must fail with 4xx, got status {} (body: {})",
        status,
        resp.text().await.unwrap_or_default()
    );
    assert_eq!(uploads.lock().unwrap().len(), 0, "over-limit must not upload any");
    assert_eq!(completions.lock().unwrap().len(), 0, "over-limit must not reach completion");
}

/// r_vis_18: Anthropic `/v1/messages` with `type:image` (base64) must be
/// bridged — the Anthropic image block is translated to a chat image_url,
/// uploaded, and injected into the completion `ref_file_ids` (defect 2).
#[tokio::test]
async fn deepseek_web_vision_messages_anthropic_image_bridged() {
    let (upstream, uploads, completions) = spawn_vision_mock().await;
    let gw = spawn_gateway_for(&upstream).await;
    let client = reqwest::Client::new();
    let red_png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let resp = client
        .post(format!("{gw}/v1/messages"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "max_tokens": 256,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What color is this?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": red_png}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "/v1/messages vision must succeed, got status {} (body: {body_text})",
        status
    );
    // One image uploaded; completion body carries the file id.
    assert_eq!(uploads.lock().unwrap().len(), 1, "one upload expected");
    let bodies = completions.lock().unwrap();
    let body = bodies.last().expect("completion body captured");
    assert!(
        body.get("__web_images").is_none(),
        "internal side channel must not reach wire"
    );
    let ids = body["ref_file_ids"].as_array().expect("ref_file_ids array");
    assert_eq!(ids.len(), 1, "one file id injected (r_vis_18)");
    assert!(
        ids[0].as_str().unwrap_or("").starts_with("file-uploaded-"),
        "uploaded file id injected, got {ids:?}"
    );
}
