use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};

#[tokio::test]
async fn systemone_passthrough_preserves_body_and_records_usage() {
    let upstream = Router::new().route(
        "/systemone",
        post(|Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["model"], "jev-1.13-free");
            assert_eq!(body["state"], "payment failed");
            assert_eq!(body["questions"]["urgent"]["type"], "noul");
            Json(json!({
                "model": "jev-1.13-free",
                "answers": {"urgent": {"type": "noul", "noul": 0.98}},
                "usage": {"input_tokens": 17, "output_tokens": 9}
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "chat-only".to_string(),
        ProviderConfig {
    rate_limits: None,
            base_url: format!("http://{}", addr),
            default_model: "chat-only-model".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Free,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["chat-only-model".to_string()],
            model_specs: vec![],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
        },
    );
    config.providers.insert(
        "zen-jev".to_string(),
        ProviderConfig {
    rate_limits: None,
            base_url: format!("http://{}", addr),
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
        },
    );
    config.gateway_keys = Vec::new();
    config.api_key = "none".to_string();
    let state = Arc::new(AppState::new(config));
    let pool = Arc::new(KeyPool::new("zen-jev", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("jev-test", "test-key", 1, 10));
    state.register_pool("zen-jev", pool);
    let chat_pool = Arc::new(KeyPool::new("chat-only", RoutingStrategy::RoundRobin));
    chat_pool.add_key(ApiKeyEntry::new("chat-test", "chat-key", 1, 10));
    state.register_pool("chat-only", chat_pool);

    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = reqwest::Client::new();
    let missing = client
        .post(format!("http://{gateway_addr}/v1/systemone"))
        .json(&json!({"state": "missing model", "questions": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400);
    let wrong_provider = client
        .post(format!("http://{gateway_addr}/v1/systemone"))
        .json(&json!({"model": "chat-only-model", "state": "must not route", "questions": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_provider.status(), 404);

    let response = client
        .post(format!("http://{gateway_addr}/v1/systemone"))
        .json(&json!({
            "model": "jev-1.13-free",
            "state": "payment failed",
            "questions": {"urgent": {"type": "noul", "instructions": "urgent?"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["answers"]["urgent"]["noul"], 0.98);
    assert_eq!(body["usage"]["input_tokens"], 17);

    let metrics = state.metrics.get_summary();
    assert_eq!(metrics.successful_requests, 1);
    assert_eq!(metrics.prompt_tokens, 17);
    assert_eq!(metrics.completion_tokens, 9);
}

async fn spawn_systemone_test_gateway() -> (SocketAddr, Arc<AppState>) {
    let upstream = Router::new().route(
        "/systemone",
        post(|Json(_body): Json<serde_json::Value>| async move {
            Json(json!({"model":"jev-1.13-free","answers":{},"usage":{"input_tokens":1,"output_tokens":1}}))
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });
    let mut config = GatewayConfig::default();
    config.api_key = "none".to_string();
    config.providers.insert("zen-jev".to_string(), ProviderConfig {
    rate_limits: None,
        base_url: format!("http://{upstream_addr}"), default_model: "jev-1.13-free".to_string(),
        strategy: "round_robin".to_string(), billing_mode: BillingMode::Free,
        input_price: 0.0, cached_price: 0.0, output_price: 0.0,
        models: vec!["jev-1.13-free".to_string()], model_specs: vec![],
        default_protocol: Some(UpstreamProtocol::Systemone), chat_url: None,
        responses_url: None, messages_url: None, proxy: None, timeout_secs: None,
    });
    let state = Arc::new(AppState::new(config));
    let pool = Arc::new(KeyPool::new("zen-jev", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("jev-test", "test-key", 1, 10));
    state.register_pool("zen-jev", pool);
    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, state)
}

#[tokio::test]
async fn systemone_accepts_unicode_empty_state_and_200_questions() {
    let (addr, _state) = spawn_systemone_test_gateway().await;
    let mut questions = serde_json::Map::new();
    for i in 0..200 {
        questions.insert(format!("q{i}"), json!({"type":"noul","instructions":"判断🚀🙂"}));
    }
    let state = "中文简历：🚀🙂\n\n".repeat(200);
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/systemone"))
        .json(&json!({"model":"jev-1.13-free","state":state,"questions":questions}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let empty_state = reqwest::Client::new()
        .post(format!("http://{addr}/v1/systemone"))
        .json(&json!({"model":"jev-1.13-free","state":"","questions":{}}))
        .send().await.unwrap();
    assert_eq!(empty_state.status(), StatusCode::OK);
}

#[tokio::test]
async fn systemone_rejects_body_over_512_kib() {
    let (addr, _state) = spawn_systemone_test_gateway().await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/systemone"))
        .json(&json!({"model":"jev-1.13-free","state":"x".repeat(600 * 1024),"questions":{}}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
