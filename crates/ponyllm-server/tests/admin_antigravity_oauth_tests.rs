//! Tests for Antigravity OAuth API in Admin routes (/api/admin/oauth/antigravity/*)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::ConfigFile;
use ponyllm_core::pool::{GatewayRoutingStrategy, UpstreamProtocol};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
use tempfile::TempDir;

struct OAuthHarness {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    pub _temp_dir: TempDir,
    pub config_path: String,
    pub api_key: String,
}

impl OAuthHarness {
    async fn new(admin_write_enabled: bool) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");
        let api_key = "admin-secret-token".to_string();

        let mut config_file = ConfigFile::default();
        config_file.gateway.bind = "127.0.0.1:8080".to_string();
        config_file.gateway.api_key = api_key.clone();
        config_file.gateway.default_strategy = GatewayRoutingStrategy::Economy;
        config_file.gateway.web_enabled = true;
        config_file.gateway.admin_write_enabled = admin_write_enabled;
        config_file.config_version = 0;

        config_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = api_key.clone();
        gw_config.default_strategy = GatewayRoutingStrategy::Economy;
        gw_config.web_enabled = true;
        gw_config.admin_write_enabled = admin_write_enabled;

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        let state = Arc::new(AppState::new(gw_config).with_config_store(store));

        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            state,
            _temp_dir: temp_dir,
            config_path: config_path.to_str().unwrap().to_string(),
            api_key,
        }
    }
}

#[tokio::test]
async fn test_antigravity_auth_url_endpoint() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/api/admin/oauth/antigravity/auth-url", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();

    assert!(json.get("auth_url").is_some());
    let auth_url = json["auth_url"].as_str().unwrap();
    assert!(auth_url.contains("accounts.google.com"));
    assert!(auth_url.contains("access_type=offline"));
    assert!(auth_url.contains("prompt=consent%20select_account"));

    let redirect_uri = json["redirect_uri"].as_str().unwrap();
    assert!(redirect_uri.contains("/oauth2callback"));
    assert!(json.get("state").is_some());
}

#[tokio::test]
async fn test_antigravity_authorize_invalid_input() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    // 1. Missing / empty code
    let resp = client
        .post(format!("http://{}/api/admin/oauth/antigravity/authorize", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({
            "code_or_url": "   "
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 2. Readonly mode gate
    let ro_harness = OAuthHarness::new(false).await;
    let ro_resp = client
        .post(format!("http://{}/api/admin/oauth/antigravity/authorize", ro_harness.addr))
        .header("Authorization", format!("Bearer {}", ro_harness.api_key))
        .json(&serde_json::json!({
            "code_or_url": "mock-code"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(ro_resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_antigravity_authorize_success_and_hot_load() {
    // 1. Mock Google OAuth Token server
    let mock_oauth = axum::Router::new().route(
        "/token",
        axum::routing::post(|body: axum::extract::Form<HashMap<String, String>>| async move {
            let map = body.0;
            assert_eq!(map.get("grant_type").map(String::as_str), Some("authorization_code"));
            assert_eq!(map.get("code").map(String::as_str), Some("4/0A-mock-success-auth-code"));

            // Payload with dummy email JWT
            // Header: {"alg":"none","typ":"JWT"} -> eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0
            // Claims: {"email":"antigravity-user@example.com"} -> eyJlbWFpbCI6ImFudGlncmF2aXR5LXVzZXJAZXhhbXBsZS5jb20ifQ
            let mock_id_token = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6ImFudGlncmF2aXR5LXVzZXJAZXhhbXBsZS5jb20ifQ.";

            axum::Json(serde_json::json!({
                "access_token": "mock-access-token-12345",
                "refresh_token": "1//0mock-refresh-token-xyz987",
                "expires_in": 3600,
                "token_type": "Bearer",
                "id_token": mock_id_token,
            }))
        }),
    );
    let oauth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_port = oauth_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(oauth_listener, mock_oauth).await.unwrap();
    });

    std::env::set_var(
        "ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE",
        format!("http://127.0.0.1:{}/token", oauth_port),
    );

    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    // 2. Submit authorization with redirected URL
    let resp = client
        .post(format!("http://{}/api/admin/oauth/antigravity/authorize", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({
            "code_or_url": "http://localhost:51121/oauth2callback?code=4%2F0A-mock-success-auth-code&state=xyz",
            "provider": "antigravity",
            "priority": 1,
            "weight": 20
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "Response error: {}", text);
    let auth_res: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(auth_res["provider"], "antigravity");
    assert_eq!(auth_res["id"], "ag-antigravity-user@example.com");
    assert_eq!(auth_res["email"], "antigravity-user@example.com");

    // 3. Verify on disk config
    let reloaded = ConfigFile::load_or_default(Some(&harness.config_path)).unwrap();
    let agy_provider = reloaded.providers.get("antigravity").expect("antigravity provider created");
    assert_eq!(agy_provider.default_protocol, Some(UpstreamProtocol::Antigravity));
    let key = agy_provider.keys.iter().find(|k| k.id == "ag-antigravity-user@example.com").expect("key exists");
    assert_eq!(key.api_key, "1//0mock-refresh-token-xyz987");
    assert_eq!(key.priority, 1);
    assert_eq!(key.weight, 20);

    // 4. Verify pool registered in memory
    assert!(harness.state.pools.read().contains_key("antigravity"));

    std::env::remove_var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE");
}

#[tokio::test]
async fn test_oauth2_callback_exempt_and_pending_flow() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    // 1. First get auth-url to generate and register a pending state
    let auth_url_resp = client
        .get(format!("http://{}/api/admin/oauth/antigravity/auth-url", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(auth_url_resp.status(), StatusCode::OK);
    let auth_json: serde_json::Value = auth_url_resp.json().await.unwrap();
    let state_key = auth_json["state"].as_str().unwrap();

    // 2. Poll pending before callback -> ready: false
    let poll_resp = client
        .get(format!("http://{}/api/admin/oauth/antigravity/pending?state={}", harness.addr, state_key))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(poll_resp.status(), StatusCode::OK);
    let poll_json: serde_json::Value = poll_resp.json().await.unwrap();
    assert_eq!(poll_json["ready"], false);
    assert!(poll_json.get("code").is_none());

    // 3. Request /oauth2callback WITHOUT Authorization header (exempt from auth)
    let callback_resp = client
        .get(format!(
            "http://{}/oauth2callback?code=test-callback-code-999&state={}",
            harness.addr, state_key
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(callback_resp.status(), StatusCode::OK);
    let content_type = callback_resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/html"));
    let html_body = callback_resp.text().await.unwrap();
    assert!(html_body.contains("Google 授权成功"));
    assert!(html_body.contains("test-callback-code-999"));
    assert!(html_body.contains("postMessage"));

    // 4. Poll pending after callback -> ready: true, code received!
    let poll_resp2 = client
        .get(format!("http://{}/api/admin/oauth/antigravity/pending?state={}", harness.addr, state_key))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(poll_resp2.status(), StatusCode::OK);
    let poll_json2: serde_json::Value = poll_resp2.json().await.unwrap();
    assert_eq!(poll_json2["ready"], true);
    assert_eq!(poll_json2["code"], "test-callback-code-999");
}

#[tokio::test]
async fn test_oauth2_callback_google_error_propagation() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    let state_key = "test-error-state-123";
    // Pre-insert pending state
    {
        let mut map = harness.state.pending_antigravity_oauth.write();
        map.insert(
            state_key.to_string(),
            ponyllm_server::state::PendingAntigravityOAuth {
                created_at: std::time::Instant::now(),
                code: None,
                error: None,
                redirect_uri: Some("http://localhost:8080/oauth2callback".to_string()),
            },
        );
    }

    // Hit /oauth2callback with error
    let resp = client
        .get(format!(
            "http://{}/oauth2callback?error=access_denied&error_description=User+declined+permission&state={}",
            harness.addr, state_key
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let html_body = resp.text().await.unwrap();
    assert!(html_body.contains("Google 授权失败"));
    assert!(html_body.contains("User declined permission"));

    // Poll pending
    let poll_resp = client
        .get(format!("http://{}/api/admin/oauth/antigravity/pending?state={}", harness.addr, state_key))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    let poll_json: serde_json::Value = poll_resp.json().await.unwrap();
    assert_eq!(poll_json["ready"], true);
    assert_eq!(poll_json["error"], "User declined permission");
}

#[tokio::test]
async fn test_proxy_status_endpoint() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/api/admin/proxy/status", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(json.get("available").is_some());
    assert!(json.get("proxy_type").is_some());
    assert!(json.get("description").is_some());
    assert!(json.get("hint").is_some());
}

#[tokio::test]
async fn test_oauth2_callback_xss_prevention_and_security_headers() {
    let harness = OAuthHarness::new(true).await;
    let client = reqwest::Client::new();

    let state_key = "test-xss-state-456";
    {
        let mut map = harness.state.pending_antigravity_oauth.write();
        map.insert(
            state_key.to_string(),
            ponyllm_server::state::PendingAntigravityOAuth {
                created_at: std::time::Instant::now(),
                code: None,
                error: None,
                redirect_uri: Some("http://127.0.0.1:8080/oauth2callback".to_string()),
            },
        );
    }

    let malicious_payload = "<script>alert('XSS')</script><img src=x onerror=alert(1)>\"hello\"";
    let resp = client
        .get(format!("http://{}/oauth2callback", harness.addr))
        .query(&[
            ("error", "access_denied"),
            ("error_description", malicious_payload),
            ("state", state_key),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // 1. Verify Security Headers
    let headers = resp.headers();
    let csp = headers.get("content-security-policy").expect("CSP header present").to_str().unwrap();
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));

    let x_frame = headers.get("x-frame-options").expect("X-Frame-Options present").to_str().unwrap();
    assert_eq!(x_frame, "DENY");

    let x_content = headers.get("x-content-type-options").expect("X-Content-Type-Options present").to_str().unwrap();
    assert_eq!(x_content, "nosniff");

    // 2. Verify HTML escaping (No raw unescaped script tag in DOM)
    let body = resp.text().await.unwrap();
    assert!(!body.contains("<script>alert('XSS')</script>"));
    assert!(body.contains("&lt;script&gt;alert"));
    assert!(body.contains("&lt;img src=x onerror=alert(1)&gt;"));

    // 3. Verify TargetOrigin is strictly http://127.0.0.1:8080 (not wildcard '*')
    assert!(!body.contains("postMessage(payload, '*')"));
    assert!(body.contains("http://127.0.0.1:8080"));
}

