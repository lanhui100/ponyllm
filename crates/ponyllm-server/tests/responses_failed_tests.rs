//! Upstream `response.failed` gateway behavior: downstream must observe an
//! error, never `finish_reason: other` (streaming) or a fake success
//! (non-streaming must fail over / surface the upstream detail).
//!
//! - Streaming: headers are already committed when the failure frame arrives,
//!   so no key swap or failover is possible — the stream ends with an error
//!   (no `other`/`stop` synthesis) and retry relies on the client resending.
//! - Non-streaming: the Conversion error maps to `UpstreamUnavailable`
//!   (failover-eligible) and the gateway tries the next routed target; with a
//!   single candidate the client gets a 5xx carrying the upstream detail.
//! - `/v1/messages` mirrors `/v1/chat/completions`: a failed Responses
//!   upstream fails over instead of returning an empty success message.

use std::sync::Arc;
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};

fn provider(base_url: String, model: &str, proto: UpstreamProtocol, price: f64) -> ProviderConfig {
    ProviderConfig {
        base_url,
        default_model: model.to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: price,
        cached_price: 0.01,
        output_price: 0.20,
        models: vec![model.to_string()],
        model_specs: vec![],
        default_protocol: Some(proto),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
    }
}

async fn spawn_gateway(state: Arc<AppState>) -> std::net::SocketAddr {
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn failed_responses_object(model: &str) -> serde_json::Value {
    json!({
        "id": "resp-fail-1",
        "object": "response",
        "status": "failed",
        "model": model,
        "output": [],
        "error": {"code": "server_error", "message": "upstream exploded"}
    })
}

#[tokio::test]
async fn test_chat_streaming_responses_failed_is_error_not_other() {
    // The mock emits the failure frame only after a delay, so the gateway
    // commits downstream headers (and the pre-failure delta) first — the
    // same ordering a real paced upstream produces. An instantly-complete
    // mock body would let the abort win the race before headers flush.
    let part1 = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_f\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"spark-fail-stream\",\"output\":[]}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_f\",\"item_id\":\"it_0\",\"output_index\":0,\"content_index\":0,\"delta\":\"partial\"}\n\n",
    );
    let part2 = concat!(
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_f\",\"object\":\"response\",\"status\":\"failed\",\"model\":\"spark-fail-stream\",\"output\":[],\"error\":{\"code\":\"server_error\",\"message\":\"upstream exploded\"}}}\n\n",
    );
    let mock = Router::new().route(
        "/v1/responses",
        post(move || async move {
            let chunks = vec![
                bytes::Bytes::from_static(part1.as_bytes()),
                bytes::Bytes::from_static(part2.as_bytes()),
            ];
            let paced = futures_util::stream::iter(chunks).then(|chunk| async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Ok::<_, std::io::Error>(chunk)
            });
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                axum::body::Body::from_stream(paced),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("spark_fail", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "spark_fail".to_string(),
        provider(
            format!("http://{}", upstream_addr),
            "spark-fail-stream",
            UpstreamProtocol::Responses,
            0.10,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark_fail", pool);
    let gw_addr = spawn_gateway(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "spark-fail-stream",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    // Headers were committed before the failure frame arrived: status stays 200.
    assert_eq!(resp.status(), 200);
    // The stream error item aborts the SSE body mid-flight (axum turns a
    // stream error into a truncated body): collect what arrived before the
    // abort and assert it carries no fake success signal.
    let mut ok_text = String::new();
    let mut saw_error = false;
    let mut body_stream = resp.bytes_stream();
    while let Some(item) = body_stream.next().await {
        match item {
            Ok(chunk) => ok_text.push_str(&String::from_utf8_lossy(&chunk)),
            Err(_) => {
                saw_error = true;
                break;
            }
        }
    }
    assert!(
        ok_text.contains("\"content\":\"partial\""),
        "pre-failure delta must still flow: {ok_text}"
    );
    assert!(
        !ok_text.contains("\"finish_reason\":\"other\""),
        "failed upstream must never surface as finish_reason:other: {ok_text}"
    );
    assert!(
        !ok_text.contains("\"finish_reason\":\"stop\""),
        "failed upstream must never synthesize a success stop chunk: {ok_text}"
    );
    assert!(
        saw_error,
        "downstream must observe the stream abort (error), not a clean [DONE]: {ok_text}"
    );
}

#[tokio::test]
async fn test_chat_non_streaming_responses_failed_fails_over() {
    // Broken Responses-native upstream: HTTP 200 with a `status: failed` object.
    let broken = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("m")
                .to_string();
            Json(failed_responses_object(&m))
        }),
    );
    let broken_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let broken_addr = broken_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(broken_listener, broken).await.unwrap();
    });

    // Healthy Chat-native backup.
    let backup = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async {
            Json(json!({
                "id": "chatcmpl-backup",
                "object": "chat.completion",
                "created": 1,
                "model": "spark-failover",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "backup says hi"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
            }))
        }),
    );
    let backup_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr = backup_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(backup_listener, backup).await.unwrap();
    });

    let pool_broken = Arc::new(KeyPool::new("fail_broken", RoutingStrategy::RoundRobin));
    pool_broken.add_key(ApiKeyEntry::new("fb-k1", "sk-broken", 1, 10));
    let pool_backup = Arc::new(KeyPool::new("fail_backup", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("fk-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.max_retries = 1;
    config.providers.insert(
        "fail_broken".to_string(),
        provider(
            format!("http://{}", broken_addr),
            "spark-failover",
            UpstreamProtocol::Responses,
            0.10,
        ),
    );
    config.providers.insert(
        "fail_backup".to_string(),
        provider(
            format!("http://{}", backup_addr),
            "spark-failover",
            UpstreamProtocol::Chat,
            0.20,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("fail_broken", pool_broken);
    state.register_pool("fail_backup", pool_backup);
    let gw_addr = spawn_gateway(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "spark-failover",
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-ponyllm-provider").unwrap().to_str().unwrap(),
        "fail_backup",
        "failed Responses upstream must fail over to the backup provider"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "backup says hi");
}

#[tokio::test]
async fn test_chat_non_streaming_responses_failed_single_candidate_is_503_with_upstream_detail() {
    // Single Responses-native upstream fails: no failover target left, so the
    // client must get a retryable 5xx carrying the upstream detail.
    let mock = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("m")
                .to_string();
            Json(failed_responses_object(&m))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("solo_fail", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "solo_fail".to_string(),
        provider(
            format!("http://{}", upstream_addr),
            "spark-solo-fail",
            UpstreamProtocol::Responses,
            0.10,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("solo_fail", pool);
    let gw_addr = spawn_gateway(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "spark-solo-fail",
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    // UpstreamUnavailable projects to 503 (service unavailable, retryable).
    assert_eq!(resp.status(), 503, "failed upstream must map to a retryable 5xx");
    let body: serde_json::Value = resp.json().await.unwrap();
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("solo_fail"),
        "error must name the failing provider: {msg}"
    );
    assert!(
        msg.contains("server_error") && msg.contains("upstream exploded"),
        "error must carry the upstream code/message: {msg}"
    );
}

#[tokio::test]
async fn test_messages_non_streaming_responses_failed_fails_over() {
    // P1-1: /v1/messages mirrors /v1/chat/completions — a failed Responses
    // upstream must fail over to the next routed target, not return an
    // empty success message.
    let broken = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("m")
                .to_string();
            Json(failed_responses_object(&m))
        }),
    );
    let broken_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let broken_addr = broken_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(broken_listener, broken).await.unwrap();
    });

    // Healthy Anthropic-native backup.
    let backup = Router::new().route(
        "/v1/messages",
        post(|_: Json<serde_json::Value>| async {
            Json(json!({
                "id": "msg_backup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "backup anthropic hi"}],
                "model": "spark-msg-failover",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 2}
            }))
        }),
    );
    let backup_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr = backup_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(backup_listener, backup).await.unwrap();
    });

    let pool_broken = Arc::new(KeyPool::new("msg_fail_broken", RoutingStrategy::RoundRobin));
    pool_broken.add_key(ApiKeyEntry::new("fb-k1", "sk-broken", 1, 10));
    let pool_backup = Arc::new(KeyPool::new("msg_fail_backup", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("fk-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.max_retries = 1;
    config.providers.insert(
        "msg_fail_broken".to_string(),
        provider(
            format!("http://{}", broken_addr),
            "spark-msg-failover",
            UpstreamProtocol::Responses,
            0.10,
        ),
    );
    config.providers.insert(
        "msg_fail_backup".to_string(),
        provider(
            format!("http://{}", backup_addr),
            "spark-msg-failover",
            UpstreamProtocol::Anthropic,
            0.20,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("msg_fail_broken", pool_broken);
    state.register_pool("msg_fail_backup", pool_backup);
    let gw_addr = spawn_gateway(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "spark-msg-failover",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-ponyllm-provider").unwrap().to_str().unwrap(),
        "msg_fail_backup",
        "failed Responses upstream must fail over to the backup provider"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["content"][0]["text"], "backup anthropic hi");
}

#[tokio::test]
async fn test_messages_non_streaming_responses_failed_single_candidate_is_503() {
    // Single Responses-native upstream fails: client gets a retryable 503
    // carrying the provider name and upstream detail (Anthropic envelope).
    let mock = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("m")
                .to_string();
            Json(failed_responses_object(&m))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("msg_solo_fail", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "msg_solo_fail".to_string(),
        provider(
            format!("http://{}", upstream_addr),
            "spark-msg-solo-fail",
            UpstreamProtocol::Responses,
            0.10,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("msg_solo_fail", pool);
    let gw_addr = spawn_gateway(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "spark-msg-solo-fail",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503, "failed upstream must map to a retryable 5xx");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "error");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("msg_solo_fail"),
        "error must name the failing provider: {msg}"
    );
    assert!(
        msg.contains("server_error") && msg.contains("upstream exploded"),
        "error must carry the upstream code/message: {msg}"
    );
}
