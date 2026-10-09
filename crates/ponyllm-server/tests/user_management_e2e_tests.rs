use axum::http::StatusCode;
use ponyllm_config::{KeyScope, UserEntry, UserRole};
use ponyllm_server::app::create_app;
use ponyllm_server::config::GatewayConfig;
use ponyllm_server::state::AppState;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_user_model_restriction_and_quota_exhaustion_e2e() {
    // 1. Arrange Config with a user and bound key
    let mut config = GatewayConfig::default();
    config.api_key = "admin-secret".to_string();
    config.admin_write_enabled = true;

    let user = UserEntry {
        id: "usr_alice".to_string(),
        name: "Alice Developer".to_string(),
        enabled: true,
        allowed_models: Some(vec!["gpt-4o-mini".to_string()]),
        max_tokens: Some(50),
        created_at: 1000,
        username: None,
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    };
    config.users.push(user);

    let (plain_key, mut key_entry) =
        ponyllm_config::generate_scoped_gateway_key("alice-key", KeyScope::Inference);
    key_entry.user_id = Some("usr_alice".to_string());
    config.gateway_keys.push(key_entry);

    let state = Arc::new(AppState::new(config));
    let app = create_app(state.clone());

    // 2. Test forbidden model access
    let req_forbidden_model = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("authorization", format!("Bearer {}", plain_key))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "model": "claude-3-5-sonnet",
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.clone().oneshot(req_forbidden_model).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body_json["error"]["code"], "model_forbidden_for_user");

    // 3. Test quota exhaustion
    // Simulate consuming 60 tokens (max is 50)
    state.user_tracker.record_tokens("usr_alice", 60);

    let req_allowed_model = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("authorization", format!("Bearer {}", plain_key))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.clone().oneshot(req_allowed_model).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body_json["error"]["code"], "user_quota_exhausted");

    // 4. Test usage reset
    state.user_tracker.reset_usage("usr_alice");
    assert_eq!(state.user_tracker.get_used_tokens("usr_alice"), 0);

    // After reset, checking access succeeds
    assert!(state
        .user_tracker
        .check_access("usr_alice", "gpt-4o-mini")
        .is_ok());
}
