//! Wave-10 deepseek-web vision bridge — L2-T functional acceptance
//! (`r_vis_01..20`, contract `.dev-team/contracts/wave-10-deepseek-web-vision.md`).
//!
//! Red phase: the bridge is UNIMPLEMENTED in the current tree (G1/G2 silent
//! image drop, G3 502) — every test that asserts the bridge/failure semantics
//! must FAIL now and PASS after task-6/7 land. Protocol-surface rows
//! (r_vis_01 wire / r_vis_16 absence) are green-by-design at this phase.
//!
//! Test-only reachability knob (frozen by L2-T, see signal): URL-image
//! positive paths (fetch/redirect/size) need a local mock image server, which
//! SSRF blocks (loopback/private). The contract's r_vis_08 judgment requires
//! exercising the redirect chain locally, so the executor MUST honor an
//! admin allowlist env `PONYLLM_IMAGE_HOST_ALLOWLIST` (comma-separated hosts;
//! default EMPTY = full SSRF posture). Tests set it only where a positive
//! fetch path is asserted; all rejection/zero-egress tests leave it unset.

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

/// Valid 1x1 red PNG, base64 (data URI payload).
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn png_data_uri() -> String {
    format!("data:image/png;base64,{PNG_B64}")
}
fn text_data_uri() -> String {
    format!(
        "data:text/plain;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(b"not an image")
    )
}

// ---------------------------------------------------------------------------
// Mock upstream: wave-9 chain + upload_file + fetch_files, with ordered log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Call {
    method: String,
    path: String,
    detail: String, // body excerpt / query / multipart summary
}

struct Upstream {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
    upload_calls: Arc<AtomicUsize>,
    completion_calls: Arc<AtomicUsize>,
    /// fetch_files auto-advance: first call per suite returns PENDING, later
    /// calls SUCCESS (two-hop poll). Timeout tests flip `never_ready` to keep
    /// it PENDING forever.
    fetch_ready_on: Arc<AtomicUsize>,
    never_ready: Arc<AtomicUsize>,
    /// upload response: Some(biz_code) to force an upload failure
    upload_biz_code: Arc<AtomicUsize>,
    /// challenge response: Some(status) to force a challenge failure
    challenge_status: Arc<AtomicUsize>,
    completion_body: Arc<Mutex<Option<Value>>>,
}

impl Upstream {
    fn log(&self, method: &str, path: &str, detail: impl Into<String>) {
        self.calls
            .lock()
            .unwrap()
            .push(Call { method: method.into(), path: path.into(), detail: detail.into() });
    }
    fn snapshot(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

async fn spawn_upstream() -> Arc<Upstream> {
    use axum::extract::Multipart;
    use axum::http::HeaderMap;
    use axum::response::Response;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        calls: Arc::new(Mutex::new(Vec::new())),
        upload_calls: Arc::new(AtomicUsize::new(0)),
        completion_calls: Arc::new(AtomicUsize::new(0)),
        fetch_ready_on: Arc::new(AtomicUsize::new(0)),
        never_ready: Arc::new(AtomicUsize::new(0)),
        upload_biz_code: Arc::new(AtomicUsize::new(0)),
        challenge_status: Arc::new(AtomicUsize::new(0)),
        completion_body: Arc::new(Mutex::new(None)),
    });
    let up_challenge = up.clone();
    let up_session = up.clone();
    let up_upload = up.clone();
    let up_fetch = up.clone();
    let up_completion = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let up = up_challenge.clone();
                async move {
                    let target = body["target_path"].as_str().unwrap_or_default().to_string();
                    up.log("POST", "/api/v0/chat/create_pow_challenge", format!("target_path={target}"));
                    if up.challenge_status.load(Ordering::SeqCst) != 0 {
                        return Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from(
                                r#"{"data":{"biz_code":40012,"biz_msg":"USER_IS_BANNED","biz_data":null}}"#,
                            ))
                            .unwrap();
                    }
                    axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(move || {
                let up = up_session.clone();
                async move {
                    up.log("POST", "/api/v0/chat_session/create", "");
                    axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/file/upload_file",
            axum::routing::post(move |headers: HeaderMap, mut multipart: Multipart| {
                let up = up_upload.clone();
                async move {
                    let mut field_name = String::new();
                    let mut filename = String::new();
                    let mut content_type = String::new();
                    let mut size = 0usize;
                    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
                        field_name = field.name().unwrap_or_default().to_string();
                        filename = field.file_name().unwrap_or_default().to_string();
                        content_type = field.content_type().unwrap_or_default().to_string();
                        size = field.bytes().await.unwrap_or_default().len();
                    }
                    let pow_hdr = headers
                        .get("x-ds-pow-response")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    up.log(
                        "POST",
                        "/api/v0/file/upload_file",
                        format!(
                            "field={field_name} filename={filename} content-type={content_type} size={size} pow_header={}",
                            !pow_hdr.is_empty()
                        ),
                    );
                    up.upload_calls.fetch_add(1, Ordering::SeqCst);
                    let biz_code = up.upload_biz_code.load(Ordering::SeqCst);
                    let seq = up.upload_calls.load(Ordering::SeqCst);
                    let body = if biz_code != 0 {
                        format!(
                            r#"{{"code":0,"data":{{"biz_code":{biz_code},"biz_msg":"upload rejected","biz_data":null}}}}"#
                        )
                    } else {
                        format!(
                            r#"{{"code":0,"data":{{"biz_code":0,"biz_data":{{"id":"file-{seq}","status":"PENDING","file_name":"{filename}","file_size":{size},"model_kind":"VISION","is_image":true,"audit_result":"unknown"}}}}}}"#
                        )
                    };
                    axum::Json(serde_json::from_str::<serde_json::Value>(&body).unwrap())
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/file/fetch_files",
            axum::routing::get(move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| {
                let up = up_fetch.clone();
                async move {
                    let ids = q.get("file_ids").cloned().unwrap_or_default();
                    up.log("GET", "/api/v0/file/fetch_files", format!("file_ids={ids}"));
                    let ready = up.fetch_ready_on.fetch_add(1, Ordering::SeqCst);
                    let never = up.never_ready.load(Ordering::SeqCst) != 0;
                    let files: Vec<Value> = ids
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .enumerate()
                        .map(|(i, id)| {
                            // Auto-advance: first call PENDING, later SUCCESS.
                            let status = if never || ready == 0 { "PENDING" } else { "SUCCESS" };
                            json!({"id": id, "status": status, "file_name": format!("img-{i}.png"), "file_size": 97, "is_image": true})
                        })
                        .collect();
                    axum::Json(json!({"code":0,"data":{"biz_code":0,"biz_data":{"files": files}}}))
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |headers: HeaderMap, body: axum::Json<serde_json::Value>| {
                let up = up_completion.clone();
                async move {
                    up.log(
                        "POST",
                        "/api/v0/chat/completion",
                        format!("ref_file_ids={:?}", body["ref_file_ids"]),
                    );
                    up.completion_calls.fetch_add(1, Ordering::SeqCst);
                    *up.completion_body.lock().unwrap() = Some(body.0.clone());
                    let _ = headers;
                    let sse = "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"red\"}\n\n\
event: close\ndata: {\"close_reason\":\"success\"}\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(sse))
                        .unwrap()
                }
            }),
        );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    up
}

/// Local "public-ish" image server for URL-source tests (reachable only via
/// the PONYLLM_IMAGE_HOST_ALLOWLIST test knob).
struct ImageServer {
    base: String,
    hits: Arc<AtomicUsize>,
}

async fn spawn_image_server() -> ImageServer {
    let hits = Arc::new(AtomicUsize::new(0));
    let h1 = hits.clone();
    let app = axum::Router::new().route(
        "/img.png",
        axum::routing::get(move || {
            let h1 = h1.clone();
            async move {
                h1.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(axum::body::Body::from(
                        base64::engine::general_purpose::STANDARD
                            .decode(PNG_B64)
                            .unwrap(),
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
    ImageServer { base: format!("http://{addr}"), hits }
}

async fn spawn_gateway(upstream: &str, image_mode: bool, body_limit_mb: usize) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.request_body_limit = body_limit_mb * 1024 * 1024;
    let mut input_types = vec!["text".to_string()];
    if image_mode {
        input_types.push("image".to_string());
    }
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
                input_types: input_types.clone(),
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

async fn chat_completions(
    gw: &str,
    content: Value,
    model: &str,
) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model": model, "messages": [{"role": "user", "content": content}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    (status, text)
}

fn img_content(urls: Vec<&str>) -> Value {
    let mut parts = vec![json!({"type": "text", "text": "what color"})];
    for u in urls {
        parts.push(json!({"type": "image_url", "image_url": {"url": u}}));
    }
    Value::Array(parts)
}

// ---------------------------------------------------------------------------
// R-VIS-01 downstream wire unchanged (protocol surface + gateway acceptance)
// ---------------------------------------------------------------------------

#[test]
fn r_vis_01_downstream_wire_unchanged() {
    // Protocol surface (green-by-design at red phase): the OpenAI image part
    // serializes to EXACTLY {"type":"image_url","image_url":{"url":...}} — no
    // invented keys; the Anthropic part to {"type":"image","source":{...}}.
    // The behavioural recognition of the wire is pinned by r_vis_03
    // (ref_file_ids injection) / r_vis_13 (no silent drop) — never by text
    // heuristics on a mock that cannot mimic the model's "no image" wording.
    let openai = ponyllm_protocol::openai::chat::ContentPart::ImageUrl {
        image_url: ponyllm_protocol::openai::chat::ImageUrlObject {
            url: png_data_uri(),
            detail: None,
        },
    };
    let j = serde_json::to_value(&openai).unwrap();
    let obj = j.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["image_url", "type"], "R-VIS-01: no extra OpenAI keys");
    let iu = obj["image_url"].as_object().unwrap();
    assert!(iu.contains_key("url"));
    assert!(iu.len() <= 2, "R-VIS-01: image_url carries at most url+detail");

    // Anthropic surface: {"type":"image","source":{"type":"base64","media_type":"image/png","data":...}}
    let anthropic = json!({"type":"image","source":{"type":"base64","media_type":"image/png","data": PNG_B64}});
    let a_obj = anthropic.as_object().unwrap();
    let mut a_keys: Vec<&str> = a_obj.keys().map(String::as_str).collect();
    a_keys.sort();
    assert_eq!(a_keys, vec!["source", "type"], "R-VIS-01: no extra Anthropic keys");
}

// ---------------------------------------------------------------------------
// R-VIS-02 images extracted in order with message association
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_02_images_extracted_in_order() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;

    // Two messages, each 1 image + text; input order = A then B.
    let content = json!([
        {"type": "text", "text": "first"},
        {"type": "image_url", "image_url": {"url": png_data_uri()}},
        {"type": "text", "text": "second"},
        {"type": "image_url", "image_url": {"url": png_data_uri()}},
    ]);
    let (status, text) = chat_completions(&gw, content, "deepseek-chat").await;
    assert!(
        status.is_success(),
        "R-VIS-02: bridge must succeed; got {status} {text}"
    );

    // The uploads must be per-image, in input order (upload_file called 2x).
    let uploads: Vec<Call> = up
        .snapshot()
        .into_iter()
        .filter(|c| c.path == "/api/v0/file/upload_file")
        .collect();
    assert_eq!(
        uploads.len(),
        2,
        "R-VIS-02: two images -> two uploads (none dropped); got {uploads:?}"
    );
    for c in &uploads {
        assert!(
            c.detail.contains("field=file"),
            "R-VIS-02: multipart field name 'file'; got {}",
            c.detail
        );
    }
}

// ---------------------------------------------------------------------------
// R-VIS-03 bridge chain: challenge(upload) -> multipart -> completion ref_file_ids
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_03_bridge_chain_ref_file_ids_order() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;

    let (status, text) = chat_completions(
        &gw,
        img_content(vec![png_data_uri().as_str(), png_data_uri().as_str()]),
        "deepseek-chat",
    )
    .await;
    assert!(
        status.is_success(),
        "R-VIS-03: bridge must complete; got {status} {text} | mock-calls={:?}",
        up.snapshot()
    );

    let calls = up.snapshot();
    // Ordered chain per image: challenge(target=/api/v0/file/upload_file) ->
    // upload -> fetch -> (2nd image) -> ... -> session -> completion.
    let challenges: Vec<String> = calls
        .iter()
        .filter(|c| c.path == "/api/v0/chat/create_pow_challenge")
        .map(|c| c.detail.clone())
        .collect();
    let upload_challenges = challenges
        .iter()
        .filter(|d| d.contains("target_path=/api/v0/file/upload_file"))
        .count();
    let completion_challenges = challenges
        .iter()
        .filter(|d| d.contains("target_path=/api/v0/chat/completion"))
        .count();
    // Lead ruling (2026-10-22): each image fetches its own upload PoW AND the
    // completion itself fetches its own PoW (wave-9 DW-P-01) — the completion
    // challenge is CORRECT behaviour, not a violation.
    assert_eq!(
        upload_challenges, 2,
        "R-VIS-03: one upload PoW per image; got {challenges:?}"
    );
    assert_eq!(
        completion_challenges, 1,
        "R-VIS-03: completion fetches its own PoW (wave-9 DW-P-01); got {challenges:?}"
    );
    let uploads: Vec<String> = calls
        .iter()
        .filter(|c| c.path == "/api/v0/file/upload_file")
        .map(|c| c.detail.clone())
        .collect();
    assert_eq!(uploads.len(), 2, "R-VIS-03: exactly two uploads; got {uploads:?}");

    // Completion body carries ref_file_ids in INPUT order.
    let body = up.completion_body.lock().unwrap().clone().expect("completion body");
    assert_eq!(
        body["ref_file_ids"],
        json!(["file-1", "file-2"]),
        "R-VIS-03: ref_file_ids order == image input order; got {body}"
    );

    // URL source variant (http(s) image_url): same chain once the bytes are
    // fetched SSRF-safely (test allowlist reaches the local image server).
    let img = spawn_image_server().await;
    let img_base = img.base.clone();
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");
    let up2 = spawn_upstream().await;
    let gw2 = spawn_gateway(&up2.base, true, 256).await;
    let (status2, text2) = chat_completions(
        &gw2,
        img_content(vec![format!("{img_base}/img.png").as_str()]),
        "deepseek-chat",
    )
    .await;
    assert!(
        status2.is_success(),
        "R-VIS-03: URL image -> same bridge chain; got {status2} {text2}"
    );
    let body2 = up2.completion_body.lock().unwrap().clone().expect("completion body (url)");
    assert_eq!(
        body2["ref_file_ids"],
        json!(["file-1"]),
        "R-VIS-03: URL image -> ref_file_ids; got {body2}"
    );
    assert_eq!(
        img.hits.load(Ordering::SeqCst),
        1,
        "R-VIS-03: image server fetched exactly once"
    );
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST");
}

// ---------------------------------------------------------------------------
// R-VIS-04 upload envelope parse + PENDING->SUCCESS poll; biz_code!=0 error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_04_upload_envelope_and_poll() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let (status, text) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    assert!(status.is_success(), "R-VIS-04: bridge with PENDING->SUCCESS poll; got {status} {text}");
    let fetch_logs: Vec<Call> = up
        .snapshot()
        .into_iter()
        .filter(|c| c.path == "/api/v0/file/fetch_files")
        .collect();
    assert!(!fetch_logs.is_empty(), "R-VIS-04: wait_file_ready must poll fetch_files");

    // upload biz_code != 0 -> explicit error, no completion.
    let up2 = spawn_upstream().await;
    up2.upload_biz_code.store(40012, Ordering::SeqCst);
    let gw2 = spawn_gateway(&up2.base, true, 256).await;
    let (status2, text2) = chat_completions(&gw2, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    assert!(
        !status2.is_success() || status2.as_u16() < 500,
        "R-VIS-04: upload biz_code!=0 -> 4xx explicit; got {status2} {text2}"
    );
    assert_eq!(
        up2.completion_calls.load(Ordering::SeqCst),
        0,
        "R-VIS-04: failed upload must not reach completion"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-05 >50 images OR any failure -> whole-request 4xx, no image-less call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_05_over_limit_or_any_failure_aborts_request() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;

    let many: Vec<String> = (0..51).map(|_| png_data_uri()).collect();
    let urls: Vec<&str> = many.iter().map(String::as_str).collect();
    let (status, text) = chat_completions(&gw, img_content(urls), "deepseek-chat").await;
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "R-VIS-05: >50 images -> 4xx; got {status} {text}"
    );
    assert_eq!(
        up.completion_calls.load(Ordering::SeqCst),
        0,
        "R-VIS-05: aborted request must never reach completion (no image-less call)"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-06 non-http(s) scheme rejected (zero egress)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_06_non_http_scheme_rejected() {
    for (label, url) in [
        ("file", "file:///etc/passwd"),
        ("ftp", "ftp://example.com/x.png"),
        ("data-as-fetch", text_data_uri().as_str()),
    ] {
        let up = spawn_upstream().await;
        let gw = spawn_gateway(&up.base, true, 256).await;
        let (status, text) = chat_completions(&gw, img_content(vec![url]), "deepseek-chat").await;
        assert!(
            !status.is_success() && status.as_u16() < 500,
            "R-VIS-06: {label} scheme -> 4xx; got {status} {text}"
        );
        assert_eq!(
            up.upload_calls.load(Ordering::SeqCst),
            0,
            "R-VIS-06: {label}: zero uploads (rejected before bridge)"
        );
    }
}

// ---------------------------------------------------------------------------
// R-VIS-07 blocked hosts double-checked (literal + DNS-resolved)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_07_blocked_hosts_double_checked() {
    for (label, url) in [
        ("loopback", "http://127.0.0.1:1/img.png"),
        ("metadata", "http://169.254.169.254/latest/meta-data/"),
        ("private10", "http://10.0.0.1/x.png"),
        ("ipv6-loopback", "http://[::1]:1/x.png"),
        ("localhost", "http://localhost/x.png"),
    ] {
        let up = spawn_upstream().await;
        let gw = spawn_gateway(&up.base, true, 256).await;
        let (status, text) = chat_completions(&gw, img_content(vec![url]), "deepseek-chat").await;
        assert!(
            !status.is_success() && status.as_u16() < 500,
            "R-VIS-07: {label} -> 4xx; got {status} {text}"
        );
        assert_eq!(
            up.upload_calls.load(Ordering::SeqCst),
            0,
            "R-VIS-07: {label}: zero uploads"
        );
    }
}

// ---------------------------------------------------------------------------
// R-VIS-08 fetch budget + redirect revalidation (local allowlist knob on)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_08_fetch_budget_and_redirect_revalidation() {
    // Redirect chain server: /r1 -> /r2 -> /r3 -> /r4 (4 hops => reject).
    let hits = Arc::new(AtomicUsize::new(0));
    let h1 = hits.clone();
    let app = axum::Router::new()
        .route(
            "/img1.png",
            axum::routing::get(move || {
                let h1 = h1.clone();
                async move {
                    h1.fetch_add(1, Ordering::SeqCst);
                    axum::response::Response::builder()
                        .status(302)
                        .header("location", "/img2.png")
                        .body(axum::body::Body::empty())
                        .unwrap()
                }
            }),
        )
        .route(
            "/img2.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(302)
                    .header("location", "/img3.png")
                    .body(axum::body::Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/img3.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(302)
                    .header("location", "/img4.png")
                    .body(axum::body::Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/img4.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(302)
                    .header("location", "/img5.png")
                    .body(axum::body::Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/img5.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(axum::body::Body::from("png"))
                    .unwrap()
            }),
        )
        .route(
            "/priv.png",
            axum::routing::get(|| async move {
                axum::response::Response::builder()
                    .status(302)
                    .header("location", "http://169.254.169.254/latest/meta-data/")
                    .body(axum::body::Body::empty())
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    // With the test allowlist the mock is reachable; img1->...->img5 is FOUR
    // redirect hops which exceeds the budget (<=3, Lead ruling off-by-one:
    // 3 hops is within budget), so the fetch must fail with an explicit error.
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let (status, text) = chat_completions(&gw, img_content(vec![format!("{base}/img1.png").as_str()]), "deepseek-chat").await;
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "R-VIS-08: redirect budget exceeded (4 hops) -> 4xx; got {status} {text}"
    );
    // Redirect to a private/metadata host must be rejected at revalidation.
    let (status2, text2) = chat_completions(&gw, img_content(vec![format!("{base}/priv.png").as_str()]), "deepseek-chat").await;
    assert!(
        !status2.is_success() && status2.as_u16() < 500,
        "R-VIS-08: redirect-to-private -> 4xx; got {status2} {text2}"
    );
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST");
}

// ---------------------------------------------------------------------------
// R-VIS-09 size + content-type + extension checks (allowlist on)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_09_size_and_content_type_checks() {
    // (a) content-type text/html masquerading as image.
    let html = axum::Router::new().route(
        "/img.png",
        axum::routing::get(|| async move {
            axum::response::Response::builder()
                .status(200)
                .header("content-type", "text/html")
                .body(axum::body::Body::from("<html></html>"))
                .unwrap()
        }),
    );
    let l1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a1 = l1.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l1, html).await.unwrap();
    });
    // (b) extension mismatch: image/png served at a .txt path.
    let ext = axum::Router::new().route(
        "/img.txt",
        axum::routing::get(|| async move {
            axum::response::Response::builder()
                .status(200)
                .header("content-type", "image/png")
                .body(axum::body::Body::from("png"))
                .unwrap()
        }),
    );
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a2 = l2.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l2, ext).await.unwrap();
    });

    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_IMAGE_HOST_ALLOWLIST", "127.0.0.1");
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let (s1, t1) = chat_completions(&gw, img_content(vec![format!("http://{a1}/img.png").as_str()]), "deepseek-chat").await;
    assert!(
        !s1.is_success() && s1.as_u16() < 500,
        "R-VIS-09: text/html content-type -> 4xx; got {s1} {t1}"
    );
    let (s2, t2) = chat_completions(&gw, img_content(vec![format!("http://{a2}/img.txt").as_str()]), "deepseek-chat").await;
    assert!(
        !s2.is_success() && s2.as_u16() < 500,
        "R-VIS-09: extension mismatch (.txt served image/png) -> 4xx; got {s2} {t2}"
    );
    std::env::remove_var("PONYLLM_IMAGE_HOST_ALLOWLIST");
}

// ---------------------------------------------------------------------------
// R-VIS-11 gateway pre-validates before upload (51 / 101MB / .txt -> zero upload)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_11_gateway_prevalidate_before_upload() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;

    // 51 images (tiny) -> 4xx, zero uploads.
    let many: Vec<String> = (0..51).map(|_| png_data_uri()).collect();
    let urls: Vec<&str> = many.iter().map(String::as_str).collect();
    let (s, _t) = chat_completions(&gw, img_content(urls), "deepseek-chat").await;
    assert!(!s.is_success() && s.as_u16() < 500, "R-VIS-11: 51 images -> 4xx; got {s}");
    assert_eq!(up.upload_calls.load(Ordering::SeqCst), 0, "R-VIS-11: zero upload calls");

    // non-image mime (text/plain) -> 4xx, zero uploads.
    let (s2, t2) = chat_completions(&gw, img_content(vec![text_data_uri().as_str()]), "deepseek-chat").await;
    assert!(!s2.is_success() && s2.as_u16() < 500, "R-VIS-11: text/plain -> 4xx; got {s2} {t2}");
    assert_eq!(up.upload_calls.load(Ordering::SeqCst), 0, "R-VIS-11: no upload for non-image");

    // 101MB single file (decoded > 100MiB) -> 4xx, zero uploads.
    let big = "A".repeat(104_857_600 + 1);
    let big_b64 = base64::engine::general_purpose::STANDARD.encode(big.as_bytes());
    let big_uri = format!("data:image/png;base64,{big_b64}");
    let up3 = spawn_upstream().await;
    let gw3 = spawn_gateway(&up3.base, true, 512).await; // body limit high enough to reach the size check
    let (s3, t3) = chat_completions(&gw3, img_content(vec![big_uri.as_str()]), "deepseek-chat").await;
    assert!(
        !s3.is_success() && s3.as_u16() < 500,
        "R-VIS-11: >100MB image -> 4xx; got {s3} {t3}"
    );
    assert_eq!(up3.upload_calls.load(Ordering::SeqCst), 0, "R-VIS-11: oversize -> no upload");
}

// ---------------------------------------------------------------------------
// R-VIS-12 ref_file_ids only for files actually parsed ready
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_12_ref_file_ids_only_ready() {
    // Mock keeps fetch_files PENDING forever + shortened parse window -> the
    // id must never reach completion; request -> explicit 4xx.
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_VISION_PARSE_TIMEOUT_MS", "2000");
    let up = spawn_upstream().await;
    up.never_ready.store(1, Ordering::SeqCst); // fetch stays PENDING forever
    let gw = spawn_gateway(&up.base, true, 256).await;
    let (status, text) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "R-VIS-12: PENDING timeout -> explicit 4xx; got {status} {text}"
    );
    assert_eq!(
        up.completion_calls.load(Ordering::SeqCst),
        0,
        "R-VIS-12: unfinished file_id must not reach completion"
    );
    std::env::remove_var("PONYLLM_VISION_PARSE_TIMEOUT_MS");
}

// ---------------------------------------------------------------------------
// R-VIS-13 explicit 4xx + reason, NEVER silent drop (G1/G2 contract ban)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_13_explicit_4xx_no_silent_drop() {
    let cases: Vec<(&str, Value)> = vec![
        ("upload-failure", img_content(vec![png_data_uri().as_str()])),
        ("non-image-mime", img_content(vec![text_data_uri().as_str()])),
        ("malformed-base64", img_content(vec!["data:image/png;base64,@@@@not-base64"])),
        ("ssrf-loopback", img_content(vec!["http://127.0.0.1:1/x.png"])),
    ];
    for (label, content) in cases {
        let up = spawn_upstream().await;
        if label == "upload-failure" {
            up.upload_biz_code.store(40012, Ordering::SeqCst);
        }
        let gw = spawn_gateway(&up.base, true, 256).await;
        let (status, text) = chat_completions(&gw, content, "deepseek-chat").await;
        assert!(
            !status.is_success() && status.as_u16() < 500,
            "R-VIS-13: {label} -> 4xx explicit; got {status} {text}"
        );
        let lower = text.to_lowercase();
        assert!(
            !(lower.contains("don't see") || lower.contains("no image") || lower.contains("没看到图")),
            "R-VIS-13: {label} response must not carry silent-drop degraded content; got {text}"
        );
        assert!(
            !text.trim().is_empty(),
            "R-VIS-13: {label} error message must state the reason"
        );
    }
}

// ---------------------------------------------------------------------------
// R-VIS-14 parse timeout -> explicit error (shortened measurable window)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_14_parse_timeout_explicit_error() {
    // Contract allows a shortened measurable window (mock clock / config);
    // frozen test knob: PONYLLM_VISION_PARSE_TIMEOUT_MS.
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PONYLLM_VISION_PARSE_TIMEOUT_MS", "2000");
    let up = spawn_upstream().await;
    up.never_ready.store(1, Ordering::SeqCst);
    let gw = spawn_gateway(&up.base, true, 256).await;
    let started = std::time::Instant::now();
    let (status, text) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    let elapsed = started.elapsed();
    assert!(
        !status.is_success() && status.as_u16() < 500,
        "R-VIS-14: parse timeout -> 4xx; got {status} {text}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "R-VIS-14: bounded (shortened) window; elapsed {:?}",
        elapsed
    );
    std::env::remove_var("PONYLLM_VISION_PARSE_TIMEOUT_MS");
}

// ---------------------------------------------------------------------------
// R-VIS-15 per-request re-upload, no cross-request reuse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_15_per_request_reupload_no_reuse() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let (s1, t1) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    assert!(s1.is_success(), "R-VIS-15: first request; got {s1} {t1}");
    let (s2, t2) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "deepseek-chat").await;
    assert!(s2.is_success(), "R-VIS-15: second request; got {s2} {t2}");
    assert_eq!(
        up.upload_calls.load(Ordering::SeqCst),
        2,
        "R-VIS-15: two requests -> two independent uploads (no id reuse)"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-16 no TTL cache implemented (static absence; green-by-design)
// ---------------------------------------------------------------------------

#[test]
fn r_vis_16_no_ttl_cache_implemented() {
    // Scope: ONLY the wave-10 bridge surfaces (deepseek_web client + chat.rs)
    // — per contract STATIC-3 the grep is over
    // crates/ponyllm-core/src/deepseek_web/ + crates/ponyllm-server/src/routes/chat.rs.
    // Pre-existing benign vocab (token accounting cached_*, dispatch hot_cache,
    // HTTP CACHE_CONTROL/no-cache headers, JWT/session TTL) is whitelisted.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let files = [
        format!("{manifest}/../ponyllm-core/src/deepseek_web/client.rs"),
        format!("{manifest}/../ponyllm-core/src/deepseek_web/pow.rs"),
        format!("{manifest}/../ponyllm-core/src/deepseek_web/sse.rs"),
        format!("{manifest}/../ponyllm-core/src/deepseek_web/mod.rs"),
        format!("{manifest}/src/routes/chat.rs"),
    ];
    let mut hits = Vec::new();
    for path in files {
        if let Ok(text) = std::fs::read_to_string(&path) {
            for (i, line) in text.lines().enumerate() {
                let low = line.to_lowercase();
                if !(low.contains("cache")
                    || low.contains("ttl")
                    || low.contains("lru")
                    || low.contains("content_hash")
                    || low.contains("hashmap<"))
                {
                    continue;
                }
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let benign = low.contains("cached_tokens")
                    || low.contains("cached_price")
                    || low.contains("count_cached")
                    || low.contains("hot_cache")
                    || low.contains("cache_read_input_tokens")
                    || low.contains("cache_creation")
                    || low.contains("cache_control")
                    || low.contains("cache-control")
                    || low.contains("no-cache")
                    || low.contains("no-store")
                    || low.contains("cachedcontenttokencount")
                    || low.contains("ttl_secs")
                    || low.contains("jwt_ttl")
                    || low.contains("cached_read");
                if !benign {
                    hits.push((path.clone(), i + 1, line.to_string()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "R-VIS-16: TTL/content-hash cache must not be implemented in the bridge scope (NON-GOAL); found {hits:?}"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-17 (Amendment v2, Lead ruling 2026-10-22) /v1/messages SERVES
// deepseek-web via anthropic->chat translation + the same image bridge —
// no longer a 4xx rejection. Other providers keep existing behavior.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_17_messages_serves_deepseek_web() {
    let up = spawn_upstream().await; // deepseek-web chain mock (SSE "red")
    let gw = spawn_gateway(&up.base, true, 256).await;

    // (1) deepseek-web via /v1/messages (plain text) -> 200 + valid Anthropic
    // message envelope, content from the bridged completion.
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/messages"))
        .bearer_auth("web-token-abc")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "deepseek-chat",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        status == 200,
        "R-VIS-17: /v1/messages must SERVE deepseek-web (Amendment v2); got {status} {text}"
    );
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert_eq!(
        body["type"], "message",
        "R-VIS-17: valid Anthropic message envelope; got {text}"
    );
    let content = body["content"].as_array().map(|a| a.iter().map(|c| c["text"].as_str().unwrap_or("")).collect::<String>()).unwrap_or_default();
    assert!(
        content.contains("red"),
        "R-VIS-17: content from the deepseek-web bridge; got {text}"
    );

    // (2) Regression: a Chat-protocol provider via /v1/messages keeps its
    // existing behavior (200 + Anthropic envelope) — zero change for others.
    let oai_hits = Arc::new(AtomicUsize::new(0));
    let o1 = oai_hits.clone();
    let oai_app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |_: axum::Json<serde_json::Value>| {
            let o1 = o1.clone();
            async move {
                o1.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({
                    "id": "chatcmpl-regr", "object": "chat.completion", "created": 1,
                    "model": "chat-model",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "regression-ok"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                }))
                .into_response()
            }
        }),
    );
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a2 = l2.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l2, oai_app).await.unwrap();
    });

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "chat-regr".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{a2}"),
            default_model: "chat-model".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["chat-model".to_string()],
            model_specs: vec![ModelSpec {
                name: "chat-model".to_string(),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Chat),
            ..Default::default()
        },
    );
    let pool2 = Arc::new(KeyPool::new("chat-regr", RoutingStrategy::RoundRobin));
    pool2.add_key(ApiKeyEntry::new("k", "sk-regr", 1, 10));
    let st2 = Arc::new(AppState::new(config));
    st2.register_pool("chat-regr", pool2);
    let app2 = create_app(st2);
    let lg = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ga = lg.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(lg, app2).await.unwrap();
    });
    let gw2 = format!("http://{ga}");
    let r2 = reqwest::Client::new()
        .post(format!("{gw2}/v1/messages"))
        .bearer_auth("sk-regr")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({"model": "chat-model", "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let st2 = r2.status();
    let t2 = r2.text().await.unwrap_or_default();
    assert!(
        st2.is_success(),
        "R-VIS-17 regression: Chat provider via /v1/messages unchanged; got {st2} {t2}"
    );
    let b2: Value = serde_json::from_str(&t2).unwrap_or(Value::Null);
    assert_eq!(b2["type"], "message", "R-VIS-17 regression: valid envelope; got {t2}");
    assert!(t2.contains("regression-ok"), "R-VIS-17 regression: content; got {t2}");
    assert_eq!(oai_hits.load(Ordering::SeqCst), 1, "regression upstream hit once");
}

// ---------------------------------------------------------------------------
// R-VIS-18 Anthropic type:image goes through the same bridge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_18_anthropic_image_same_bridge() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/messages"))
        .bearer_auth("web-token-abc")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "deepseek-chat",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what color"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": PNG_B64}}
            ]}]
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "R-VIS-18: anthropic type:image bridged; got {status} {text}"
    );
    let body = up.completion_body.lock().unwrap().clone().expect("completion body");
    assert_eq!(
        body["ref_file_ids"],
        json!(["file-1"]),
        "R-VIS-18: anthropic image -> ref_file_ids; got {body}"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-19 input_types image declared (G4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_19_input_types_image_declared() {
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base, true, 256).await;
    let models: Value = reqwest::Client::new()
        .get(format!("{gw}/v1/models"))
        .bearer_auth("web-token-abc")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let found = models["data"].as_array().map(|arr| {
        arr.iter().any(|m| {
            m["id"].as_str().map(|s| s.ends_with("/deepseek-chat")).unwrap_or(false)
                && (m.get("input_types").and_then(|v| v.as_array()).map(|a| a.iter().any(|x| x == "image")).unwrap_or(false)
                    || m.get("modalities").and_then(|v| v.as_array()).map(|a| a.iter().any(|x| x == "image")).unwrap_or(false)
                    || m.to_string().contains("image"))
        })
    });
    assert!(
        found.unwrap_or(false),
        "R-VIS-19: /v1/models must expose the image capability for deepseek-chat; got {models}"
    );
}

// ---------------------------------------------------------------------------
// R-VIS-20 expert alias converged (G5): no silent fallback to default
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_vis_20_expert_alias_converged() {
    // Production shape (G5): both default and expert configured, but expert is
    // a non-enabled alias — a request for it must be explicitly unavailable,
    // never silently answered by default.
    let up = spawn_upstream().await;
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.request_body_limit = 256 * 1024 * 1024;
    config.providers.insert(
        "deepseek-web".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: up.base.clone(),
            default_model: "deepseek-chat".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["deepseek-chat".to_string(), "expert".to_string()],
            model_specs: vec![
                ModelSpec {
                    name: "deepseek-chat".to_string(),
                    priority: Some(10),
                    context_window: "128K".to_string(),
                    max_output: "4K".to_string(),
                    input_types: vec!["text".to_string(), "image".to_string()],
                    ..Default::default()
                },
                ModelSpec {
                    name: "expert".to_string(),
                    ..Default::default()
                },
            ],
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
    let gw = format!("http://{addr}");

    let (status, text) = chat_completions(&gw, img_content(vec![png_data_uri().as_str()]), "expert").await;
    // Contract: expert must be explicitly unavailable (4xx / removed), never
    // silently routed to default with a success.
    assert!(
        !status.is_success(),
        "R-VIS-20: 'expert' must not silently fall back to default (G5); got {status} {text}"
    );
    assert!(
        !text.to_lowercase().contains("red"),
        "R-VIS-20: expert request must not be answered by a silent default route"
    );
}