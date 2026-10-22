//! Wave-9 deepseek-web — L2-AT adversarial client tests (RED phase).
//!
//! Three determinism-first adversarial classes against the frozen client API:
//! (a) network timeout/delay injection — delay = NFR baseline + margin, never
//!     a self-chosen threshold (baseline: NFR `external_call_timeout_ms`
//!     == 3000ms; streaming TTFB 90s is a frozen NON-GOAL constant guarded by
//!     the pre-existing `send_guarded` path, see contract §8);
//! (b) malformed payloads — broken JSON, missing fields, oversized fields;
//! (c) deterministic concurrency — fixed seed LCG + tokio Barrier + fixed
//!     iteration count, no sleeps used as synchronisation.
//!
//! Every test drives a frozen `todo!()` stub, so each is RED in this phase;
//! the injection parameters are the contract anchor (NFR baseline table).

use ponyllm_core::deepseek_web::{ChatSession, ClientError, CompletionRequest, DeepSeekWebClient};
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

type Outcome = (StatusCode, Vec<(String, String)>, String);

fn make_app(
    handler: Arc<dyn Fn(Method, String, HeaderMap, Bytes) -> Outcome + Send + Sync>,
) -> axum::Router {
    axum::Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| async move {
            let (status, hdrs, out) = handler(method, uri.path().to_string(), headers, body);
            let mut builder = Response::builder().status(status);
            for (k, v) in hdrs {
                builder = builder.header(k, v);
            }
            builder.body(axum::body::Body::from(out)).unwrap()
        },
    )
}

async fn spawn_app(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn client(base: &str, device_id: &str) -> DeepSeekWebClient {
    DeepSeekWebClient {
        http: reqwest::Client::new(),
        base_url: base.to_string(),
        token: Some("web-token-abc".to_string()),
        device_id: device_id.to_string(),
    }
}

fn challenge_envelope() -> Outcome {
    (
        StatusCode::OK,
        vec![("content-type".to_string(), "application/json".to_string())],
        json!({"data":{"biz_code":0,"biz_data":{"challenge":{
            "algorithm":"DeepSeekHashV1",
            "challenge":"eb1cc3ae55acef598a8054febbf4d6f03a4f97556e0b958f9e7ddbbf47928f22",
            "salt":"f9a1b2c3d4e5f60718293a4b5c6d7e8f",
            "difficulty":100000,
            "signature":"a1b2c3d4e5f60718293a4b5c6d7e8f90"
        }},"biz_msg":"ok"}})
        .to_string(),
    )
}

const TARGET_PATH: &str = "/api/v0/chat/completion";

// ---------------------------------------------------------------------------
// (a) network timeout injection: delay = 3000ms (NFR baseline) + 2000ms margin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_timeout_short_call_budget_exceeded() {
    // Arrange: the challenge endpoint stalls for baseline+margin. The client
    // must fail within its own budget instead of hanging or thrash-retrying.
    const NFR_SHORT_CALL_TIMEOUT_MS: u64 = 3000; // .dev-team/nfr-baseline-wave-9.json
    const MARGIN_MS: u64 = 2000; // mandatory margin above the frozen baseline
    let delay = std::time::Duration::from_millis(NFR_SHORT_CALL_TIMEOUT_MS + MARGIN_MS);

    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let app = make_app(Arc::new(move |_m, _p, _h, _b| {
        c2.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(delay); // inject upstream-side latency (blocking mock)
        challenge_envelope()
    }));
    let base = spawn_app(app).await;

    // Act: bounded observation window — budget + margin + slack.
    let started = std::time::Instant::now();
    let result = client(&base, "dev-1").create_pow_challenge(TARGET_PATH).await;
    let elapsed = started.elapsed();

    // Assert: explicit error within a bounded window (no hang, no avalanche).
    assert!(
        result.is_err(),
        "timeout injection ({}ms) must surface an explicit error, got {:?}",
        delay.as_millis(),
        result
    );
    assert!(
        elapsed < std::time::Duration::from_secs(6),
        "must fail within budget+margin; elapsed {:?}",
        elapsed
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no blind retry after timeout (exactly 1 upstream request)"
    );
}

// ---------------------------------------------------------------------------
// (b) malformed payloads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_malformed_upstream_json_errors_cleanly() {
    // Broken JSON from the upstream: must be an Err, never a panic/hang.
    let app = make_app(Arc::new(|_m, _p, _h, _b| {
        (
            StatusCode::OK,
            vec![("content-type".to_string(), "application/json".to_string())],
            "{this is not json".to_string(),
        )
    }));
    let base = spawn_app(app).await;
    let err = client(&base, "dev-1")
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("malformed upstream JSON must produce an explicit error");
    assert!(
        matches!(err, ClientError::Other(_) | ClientError::Network(_)),
        "malformed body -> Other/Network; got {err:?}"
    );
}

#[tokio::test]
async fn adv_malformed_challenge_missing_fields_errors_cleanly() {
    // Challenge object without required fields: explicit Err, no panic.
    let app = make_app(Arc::new(|_m, _p, _h, _b| {
        (
            StatusCode::OK,
            vec![("content-type".to_string(), "application/json".to_string())],
            json!({"data":{"biz_code":0,"biz_data":{"challenge":{"algorithm":"DeepSeekHashV1"}},"biz_msg":"ok"}})
                .to_string(),
        )
    }));
    let base = spawn_app(app).await;
    let _ = client(&base, "dev-1")
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("challenge missing required fields -> explicit error, not panic");
}

#[tokio::test]
async fn adv_oversized_prompt_roundtrips_without_breakage() {
    // 1 MiB prompt through the completion wire: must not truncate/panic; the
    // mock returns a well-formed SSE stream regardless.
    let big = "a".repeat(1024 * 1024);
    let req = CompletionRequest {
        chat_session_id: "s1".into(),
        parent_message_id: None,
        model_type: "deepseek-chat".into(),
        prompt: big.clone(),
        ref_file_ids: vec![],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".into(),
        action: None,
        preempt: false,
    };
    // Wire round-trip must preserve the payload byte-exactly.
    let wire = serde_json::to_vec(&req).unwrap();
    let back: CompletionRequest = serde_json::from_slice(&wire).unwrap();
    assert_eq!(back.prompt.len(), big.len(), "1MiB prompt must round-trip");

    let pow = ponyllm_core::deepseek_web::PowResponse {
        algorithm: "DeepSeekHashV1".into(),
        challenge: "c".into(),
        salt: "s".into(),
        answer: 0,
        signature: "g".into(),
        target_path: TARGET_PATH.into(),
    };
    let app = make_app(Arc::new(|_m, _p, _h, _b| {
        (
            StatusCode::OK,
            vec![("content-type".to_string(), "text/event-stream".to_string())],
            "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n".to_string(),
        )
    }));
    let base = spawn_app(app).await;
    let resp = client(&base, "dev-1")
        .stream_completion(&req, &pow)
        .await
        .expect("stub panics -> red");
    assert_eq!(
        resp.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );
}

// ---------------------------------------------------------------------------
// (c) deterministic concurrency race: fixed seed LCG + Barrier + fixed count
// ---------------------------------------------------------------------------

/// Tiny deterministic LCG (fixed seed) — no thread_local RNG, reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn adv_concurrency_no_cross_request_bleed() {
    const N: usize = 8; // fixed iteration count (deterministic)
    let mut rng = Lcg(0x9E3779B97F4A7C15); // fixed seed
    let device_ids: Vec<String> = (0..N).map(|i| format!("dev-{:04x}", rng.next() ^ (i as u64))).collect();

    // Mock echoes the caller's x-device-id into the challenge `salt` field so
    // each response is provably tied to its own request.
    let app = make_app(Arc::new(move |_m, _p, headers, _b| {
        let dev = headers
            .get("x-device-id")
            .map(|v| v.to_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        (
            StatusCode::OK,
            vec![("content-type".to_string(), "application/json".to_string())],
            json!({"data":{"biz_code":0,"biz_data":{"challenge":{
                "algorithm":"DeepSeekHashV1",
                "challenge":"deadbeef",
                "salt":dev,
                "difficulty":1,
                "signature":"sig"
            }},"biz_msg":"ok"}})
            .to_string(),
        )
    }));
    let base = spawn_app(app).await;

    let barrier = Arc::new(Barrier::new(N));
    let mut handles = Vec::new();
    for dev in &device_ids {
        let dev = dev.clone();
        let base = base.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait(); // simultaneous start — no sleeps
            let ch = client(&base, &dev)
                .create_pow_challenge(TARGET_PATH)
                .await
                .expect("stub panics -> red");
            (dev, ch.salt)
        }));
    }
    for h in handles {
        let (dev, salt) = h.await.unwrap();
        assert_eq!(
            salt, dev,
            "concurrency: each response must carry its own request's identity (no cross-bleed)"
        );
    }
}

// Keep the `ChatSession` import used (session path is part of the client
// surface exercised under concurrency in later waves; here it documents the
// frozen type).
#[allow(dead_code)]
fn _session_type_ref(_s: ChatSession) {}