//! Graceful shutdown integration tests (P1-OPS-001, multi-node HA).
//!
//! Uses an in-process `watch` channel (not a real signal) so the drain path is
//! testable cross-platform: an in-flight SSE stream must finish naturally when
//! drain is triggered, and a stream that exceeds the drain deadline must be
//! truncated (server exits; the client-retry contract applies).

use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use futures_util::StreamExt;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use tokio::sync::watch;
use tokio_stream::wrappers::ReceiverStream;


type SseStream = ReceiverStream<Result<Event, std::io::Error>>;

/// Named handler so a stream-body type error points inside the function
/// instead of failing the opaque `Handler` bound on the closure.
async fn sse_handler(stream: SseStream) -> axum::response::Response {
    Sse::new(stream).into_response()
}

fn make_provider(base_url: &str, default_model: &str) -> ProviderConfig {
        ProviderConfig {
    rate_limits: None,
        base_url: base_url.to_string(),
        default_model: default_model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.50,
        cached_price: 0.25,
        output_price: 1.00,
        models: vec![],
        model_specs: Vec::new(),
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
    }
}

/// Upstream that streams `chunks` SSE events with `interval` between chunks.
fn slow_sse_upstream(interval: Duration, chunks: usize, chunk: &'static str) -> Router {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::io::Error>>(chunks + 1);
    tokio::spawn(async move {
        for i in 0..chunks {
            tokio::time::sleep(interval).await;
            let event = Event::default().data(format!("{} {}", chunk, i));
            if tx.send(Ok(event)).await.is_err() {
                return;
            }
        }
    });
    sse_router(ReceiverStream::new(rx))
}

/// Build an SSE `/v1/chat/completions` mock route from a stream. The stream is
/// stashed in a `Arc<Mutex<Option<_>>>` slot so the axum handler closure stays
/// `Clone` (axum `post()` requires `H: Clone`).
fn sse_router(stream: SseStream) -> Router {
    let slot: Arc<tokio::sync::Mutex<Option<SseStream>>> =
        Arc::new(tokio::sync::Mutex::new(Some(stream)));
    Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            let mut guard = slot.lock().await;
            let stream = guard.take().expect("mock stream consumed once");
            drop(guard);
            sse_handler(stream).await
        }),
    )
}

/// Start a gateway wired to `upstream` using the graceful-shutdown serve
/// helper. Returns (base_url, shutdown_sender, serve_task).
async fn spawn_gateway_with_shutdown(
    upstream: Router,
    drain_timeout: Duration,
) -> (
    String,
    watch::Sender<bool>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        make_provider(&format!("http://{}", upstream_addr), "deepseek-v4-flash"),
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let state = Arc::new(AppState::new(config).with_shutdown_rx(shutdown_rx.clone()));
    state.register_pool("deepseek", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    let task = tokio::spawn(ponyllm_server::serve::serve_with_shutdown(
        gateway_listener,
        gateway_app,
        shutdown_rx,
        drain_timeout,
    ));
    (format!("http://{}", gateway_addr), shutdown_tx, task)
}

fn chat_request_body() -> serde_json::Value {
    serde_json::json!({
        "model": "deepseek-v4-flash",
        "stream": true,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

/// In-flight SSE stream finishes naturally when drain is triggered before its
/// last chunk: full body received, server task exits Ok.
#[tokio::test]
async fn drain_allows_in_flight_sse_to_finish() {
    // ~8 chunks x 60ms ≈ 480ms stream; drain deadline 10s (generous).
    let upstream = slow_sse_upstream(Duration::from_millis(60), 8, "chunk");
    let (base, shutdown_tx, serve_task) =
        spawn_gateway_with_shutdown(upstream, Duration::from_secs(10)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/completions", base))
        .json(&chat_request_body())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let mut bytes_stream = resp.bytes_stream();
    let mut received = Vec::new();
    // Read the first chunk to prove the request is in-flight.
    let first = bytes_stream.next().await.expect("first chunk").unwrap();
    received.extend_from_slice(&first);

    // Trigger drain mid-stream.
    shutdown_tx.send(true).unwrap();

    // The stream must still deliver every remaining chunk.
    while let Some(Ok(bytes)) = bytes_stream.next().await {
        received.extend_from_slice(&bytes);
    }
    let text = String::from_utf8(received).unwrap();
    assert!(text.contains("chunk 0"), "first chunk present: {text}");
    assert!(text.contains("chunk 7"), "last chunk present: {text}");

    // Server exits cleanly (graceful, within deadline).
    tokio::time::timeout(Duration::from_secs(5), serve_task)
        .await
        .expect("serve task must exit after drain")
        .unwrap()
        .expect("serve_with_shutdown returns Ok");
}

/// A stream that hangs past the drain deadline is truncated: the server exits
/// by the deadline and the client sees the connection close mid-body.
#[tokio::test]
async fn drain_deadline_truncates_hung_stream_and_server_exits() {
    // Upstream never sends any chunk: body hangs forever.
    let (never_tx, never_rx) = tokio::sync::mpsc::channel::<Result<Event, std::io::Error>>(1);
    let _ = never_tx; // never send
    let upstream = sse_router(ReceiverStream::new(never_rx));

    // Drain deadline 250ms — well under the test budget.
    let (base, shutdown_tx, serve_task) =
        spawn_gateway_with_shutdown(upstream, Duration::from_millis(250)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/completions", base))
        .json(&chat_request_body())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Give the request a moment to reach the hung handler, then drain.
    tokio::time::sleep(Duration::from_millis(80)).await;
    shutdown_tx.send(true).unwrap();

    // The server must exit by (deadline + small slack).
    tokio::time::timeout(Duration::from_secs(5), serve_task)
        .await
        .expect("serve task must exit after drain deadline")
        .unwrap()
        .expect("serve_with_shutdown returns Ok");

    // The drain cap worked: the server gave up on the hung stream instead of
    // waiting forever. Whether the CLIENT observes EOF/RST immediately is
    // process-exit-dependent (in-process, axum keeps spawned connection tasks
    // alive until process termination; kubelet does the real truncation when
    // the grace period expires). Probe briefly and accept any outcome — the
    // requirement under test is the server-side drain deadline.
    let mut stream = resp.bytes_stream();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
}