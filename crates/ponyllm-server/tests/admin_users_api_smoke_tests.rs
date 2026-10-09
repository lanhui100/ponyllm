use axum::http::StatusCode;
use ponyllm_server::app::create_app;
use ponyllm_server::config::GatewayConfig;
use ponyllm_server::state::AppState;
use std::sync::Arc;

#[tokio::test]
async fn test_admin_users_crud_and_quota_smoke() {
    let mut config = GatewayConfig::default();
    config.bind_addr = "127.0.0.1:8080".to_string();
    config.api_key = "admin-master-key".to_string();
    config.admin_write_enabled = true;
    config.web_enabled = false;

    // Use a temp file for Admin Store
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("ponyllm.toml");
    let mut cfg_file = ponyllm_config::ConfigFile::default();
    cfg_file.gateway.api_key = "admin-master-key".to_string();
    cfg_file.gateway.admin_write_enabled = true;
    cfg_file.gateway.gateway_keys = Vec::new();
    let serialized = toml::to_string(&cfg_file).unwrap();
    std::fs::write(&config_path, serialized).unwrap();

    let store = Arc::new(ponyllm_server::admin_store::FileConfigStore::new(
        config_path.to_str().unwrap(),
    ));
    std::mem::forget(temp_dir);

    let state = Arc::new(AppState::new(config).with_config_store(store));
    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let base_url = format!("http://{}", addr);

    // Test a basic overview call first
    let res = client
        .get(format!("{}/api/admin/overview", base_url))
        .header("authorization", "Bearer admin-master-key")
        .send()
        .await
        .unwrap();
    println!("Overview status: {}", res.status());

    // 1. Create a user via POST /api/admin/users
    let resp = client
        .post(format!("{}/api/admin/users", base_url))
        .header("authorization", "Bearer admin-master-key")
        .header("if-match", "*")
        .json(&serde_json::json!({
            "id": "smoke_usr_bob",
            "name": "Bob Tester",
            "enabled": true,
            "allowed_models": ["gpt-4o-mini"],
            "max_tokens": 1000
        }))
        .send()
        .await
        .unwrap();

    println!("Create user status: {}", resp.status());
    assert_eq!(resp.status(), StatusCode::CREATED);
    let user_view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(user_view["id"], "smoke_usr_bob");
    assert_eq!(user_view["max_tokens"], 1000);

    // 2. Get user via GET /api/admin/users/smoke_usr_bob
    println!("Sending GET user...");
    let resp = client
        .get(format!("{}/api/admin/users/smoke_usr_bob", base_url))
        .header("authorization", "Bearer admin-master-key")
        .send()
        .await
        .unwrap();

    println!("GET user status: {}", resp.status());
    assert_eq!(resp.status(), StatusCode::OK);

    // 3. Update user via PUT /api/admin/users/smoke_usr_bob
    println!("Sending PUT user...");
    let req_body = serde_json::json!({
        "max_tokens": 2000
    });
    let resp = client
        .put(format!("{}/api/admin/users/smoke_usr_bob", base_url))
        .header("authorization", "Bearer admin-master-key")
        .header("if-match", "*")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    println!("PUT user status: {}", resp.status());
    assert_eq!(resp.status(), StatusCode::OK);
    let updated_view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(updated_view["max_tokens"], 2000);

    // 4. Simulate token usage and reset
    state.user_tracker.record_tokens("smoke_usr_bob", 1500);
    assert_eq!(state.user_tracker.get_used_tokens("smoke_usr_bob"), 1500);

    let resp = client
        .post(format!(
            "{}/api/admin/users/smoke_usr_bob/reset-usage",
            base_url
        ))
        .header("authorization", "Bearer admin-master-key")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(state.user_tracker.get_used_tokens("smoke_usr_bob"), 0);

    // 5. Delete user via DELETE /api/admin/users/smoke_usr_bob
    let resp = client
        .delete(format!("{}/api/admin/users/smoke_usr_bob", base_url))
        .header("authorization", "Bearer admin-master-key")
        .header("if-match", "*")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}
