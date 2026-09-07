//! Integration and contract tests for Admin API write path & governance (WEB-06).
//!
//! Enforces:
//! - Governance 4 items:
//!   1. admin_write_enabled gate (returns 404 when false)
//!   2. If-Match optimistic concurrency control (missing/mismatch -> 412, match -> success)
//!   3. Write-before-backup to .bak file on disk
//!   4. Write queue lock serialization (no race conditions)
//! - Provider CUD (POST / DELETE, 409 conflict, 404 not found)
//! - Model CUD (POST / PUT / DELETE, 409 conflict, 404 not found)
//! - Key CUD:
//!   - Plaintext api_key returned ONCE in creation response with Cache-Control: no-store
//!   - Subsequent GET /api/admin/keys strictly desensitized (masked_key)
//!   - Hot-pool sync on DELETE
//! - Key dial-test:
//!   - 3s hard timeout
//!   - 200 OK -> success
//!   - 401 Unauthorized -> unauthorized error
//!   - 429 Rate Limit -> rate_limited error
//!   - >3s timeout -> timeout error
//!   - 404 on missing key

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ponyllm_config::{ConfigFile, KeySection, ModelConfig, ProviderSection};
use ponyllm_core::pool::{
    ApiKeyEntry, BillingMode, GatewayRoutingStrategy, KeyPool, ModelTier, RoutingStrategy,
};
use ponyllm_server::admin_store::{ConfigStore, FileConfigStore};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use reqwest::StatusCode;
use tempfile::TempDir;

#[allow(dead_code)]
struct WriteTestHarness {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    pub temp_dir: TempDir,
    pub config_path: String,
    pub api_key: String,
}

impl WriteTestHarness {
    async fn new(admin_write_enabled: bool) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");
        let api_key = "admin-secret-token".to_string();

        let raw_keys = vec![KeySection {
            id: "key-1".to_string(),
            api_key: "sk-proj-live-token-abcdef1234567890".to_string(),
            priority: 1,
            weight: 10,
        }];

        let provider_sec = ProviderSection {
            base_url: "https://api.openai.com/v1".to_string(),
            default_model: "gpt-4o".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 2.5,
            cached_price: 1.25,
            output_price: 10.0,
            models: vec!["gpt-4o".to_string()],
            model_configs: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                tier: ModelTier::Standard,
                billing_mode: Some(BillingMode::Metered),
                context_window: "128K".to_string(),
                max_output: "16K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(2.5),
                cached_price: Some(1.25),
                output_price: Some(10.0),
                protocol: None,
                thinking_default: None,
                thinking_max: None,
                proxy: None,
            }],
            keys: raw_keys.clone(),
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
        };

        let mut providers = HashMap::new();
        providers.insert("openai".to_string(), provider_sec);

        let mut config_file = ConfigFile::default();
        config_file.gateway.bind = "127.0.0.1:8080".to_string();
        config_file.gateway.api_key = api_key.clone();
        config_file.gateway.default_strategy = GatewayRoutingStrategy::Economy;
        config_file.gateway.web_enabled = true;
        config_file.gateway.admin_write_enabled = admin_write_enabled;
        config_file.providers = providers;
        config_file.config_version = 0;

        config_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = api_key.clone();
        gw_config.default_strategy = GatewayRoutingStrategy::Economy;
        gw_config.web_enabled = true;
        gw_config.admin_write_enabled = admin_write_enabled;

        let model_spec = ModelSpec {
            name: "gpt-4o".to_string(),
            tier: ModelTier::Standard,
            context_window: "128K".to_string(),
            max_output: "16K".to_string(),
            input_types: vec!["text".to_string()],
            output_types: vec!["text".to_string()],
            billing_mode: Some(BillingMode::Metered),
            input_price: Some(2.5),
            cached_price: Some(1.25),
            output_price: Some(10.0),
            protocol: None,
            thinking_default: None,
            thinking_max: None,
            proxy: None,
        };

        gw_config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                base_url: "https://api.openai.com/v1".to_string(),
                default_model: "gpt-4o".to_string(),
                strategy: "round_robin".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: 2.5,
                cached_price: 1.25,
                output_price: 10.0,
                models: vec!["gpt-4o".to_string()],
                model_specs: vec![model_spec],
                default_protocol: None,
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
            },
        );

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        let state = Arc::new(AppState::new(gw_config).with_config_store(store));

        let pool = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
        for k in &raw_keys {
            pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
        }
        state.register_pool("openai", pool);

        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            state,
            temp_dir,
            config_path: config_path.to_str().unwrap().to_string(),
            api_key,
        }
    }
}

// -----------------------------------------------------------------------------
// Test 1: admin_write_enabled gate (returns 404 on all write endpoints when off)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_admin_write_disabled_gate() {
    let harness = WriteTestHarness::new(false).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    let endpoints = vec![
        ("POST", "/api/admin/providers", serde_json::json!({"name": "test", "base_url": "http://example.com"})),
        ("DELETE", "/api/admin/providers/openai", serde_json::json!({})),
        ("POST", "/api/admin/models", serde_json::json!({"provider": "openai", "name": "m1"})),
        ("PUT", "/api/admin/models/gpt-4o", serde_json::json!({"context_window": "256K"})),
        ("DELETE", "/api/admin/models/gpt-4o", serde_json::json!({})),
        ("POST", "/api/admin/keys", serde_json::json!({"provider": "openai", "id": "k2", "api_key": "sec"})),
        ("DELETE", "/api/admin/keys/key-1", serde_json::json!({})),
        ("POST", "/api/admin/keys/key-1/test", serde_json::json!({})),
    ];

    for (method, path, body) in endpoints {
        let url = format!("http://{}{}", harness.addr, path);
        let req = match method {
            "POST" => client.post(&url).json(&body),
            "PUT" => client.put(&url).json(&body),
            "DELETE" => client.delete(&url),
            _ => unreachable!(),
        };
        let resp = req
            .header("Authorization", &auth)
            .header("If-Match", "\"0\"")
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "Expected 404 for {} {} when write is disabled",
            method,
            path
        );
        let err: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(err["error"]["code"], "admin_write_disabled");
    }
}

// -----------------------------------------------------------------------------
// Test 2: If-Match optimistic concurrency control
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_if_match_validation() {
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);
    let url = format!("http://{}/api/admin/providers", harness.addr);
    let payload = serde_json::json!({
        "name": "anthropic",
        "base_url": "https://api.anthropic.com"
    });

    // 1. Missing If-Match header -> 412
    let resp = client
        .post(&url)
        .header("Authorization", &auth)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    let err: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(err["error"]["code"], "precondition_failed");

    // 2. Mismatched If-Match header -> 412
    let resp = client
        .post(&url)
        .header("Authorization", &auth)
        .header("If-Match", "\"99\"")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);

    // 3. Matching If-Match header ("0") -> 201 Created
    let resp = client
        .post(&url)
        .header("Authorization", &auth)
        .header("If-Match", "\"0\"")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

// -----------------------------------------------------------------------------
// Test 3: Write-before-backup to .bak
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_write_before_backup_created() {
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    let backup_path = std::path::Path::new(&harness.config_path).with_extension("toml.bak");
    assert!(!backup_path.exists(), "Backup should not exist initially");

    // Perform a valid write operation
    let resp = client
        .post(format!("http://{}/api/admin/providers", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({
            "name": "deepseek",
            "base_url": "https://api.deepseek.com"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Verify backup exists and contains initial provider "openai"
    assert!(backup_path.exists(), "Backup file must be created on write");
    let backup_content = std::fs::read_to_string(backup_path).unwrap();
    assert!(backup_content.contains("openai"));
    assert!(!backup_content.contains("deepseek"));
}

// -----------------------------------------------------------------------------
// Test 4: Provider CUD operations
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_provider_cud() {
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    // 1. Create Provider
    let create_resp = client
        .post(format!("http://{}/api/admin/providers", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({
            "name": "google",
            "base_url": "https://generativelanguage.googleapis.com",
            "default_model": "gemini-1.5-pro",
            "billing_mode": "free"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json().await.unwrap();
    assert_eq!(created["name"], "google");
    assert_eq!(created["billing_mode"], "Free");

    // 2. Duplicate Provider returns 409 Conflict
    let dup_resp = client
        .post(format!("http://{}/api/admin/providers", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"1\"")
        .json(&serde_json::json!({
            "name": "google",
            "base_url": "https://generativelanguage.googleapis.com"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup_resp.status(), StatusCode::CONFLICT);

    // 3. Delete Provider
    let del_resp = client
        .delete(format!("http://{}/api/admin/providers/google", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);

    // 4. Delete non-existent provider returns 404
    let not_found_resp = client
        .delete(format!("http://{}/api/admin/providers/non_existent", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"2\"")
        .send()
        .await
        .unwrap();
    assert_eq!(not_found_resp.status(), StatusCode::NOT_FOUND);
}

// -----------------------------------------------------------------------------
// Test 5: Model CUD operations
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_model_cud() {
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    // 1. Create Model
    let create_resp = client
        .post(format!("http://{}/api/admin/models", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({
            "provider": "openai",
            "name": "gpt-4o-mini",
            "tier": "Light",
            "context_window": "128K",
            "thinking_default": "low"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let m: serde_json::Value = create_resp.json().await.unwrap();
    assert_eq!(m["name"], "gpt-4o-mini");
    assert_eq!(m["tier"], "Light");

    // 2. Duplicate Model returns 409
    let dup_resp = client
        .post(format!("http://{}/api/admin/models", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"1\"")
        .json(&serde_json::json!({
            "provider": "openai",
            "name": "gpt-4o-mini"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup_resp.status(), StatusCode::CONFLICT);

    // 3. Update Model
    let update_resp = client
        .put(format!("http://{}/api/admin/models/gpt-4o-mini", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"1\"")
        .json(&serde_json::json!({
            "provider": "openai",
            "context_window": "256K"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update_resp.status(), StatusCode::OK);
    let updated: serde_json::Value = update_resp.json().await.unwrap();
    assert_eq!(updated["context_window"], "256K");

    // 4. Delete Model
    let del_resp = client
        .delete(format!("http://{}/api/admin/models/gpt-4o-mini?provider=openai", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"2\"")
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);
}

// -----------------------------------------------------------------------------
// Test 6: Key CUD with one-time plaintext and subsequent masking
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_key_cud_one_time_plaintext_and_masking() {
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    let raw_secret = "sk-proj-super-secret-key-12345678";

    // 1. Create Key returns plaintext once with no-store headers
    let create_resp = client
        .post(format!("http://{}/api/admin/keys", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({
            "provider": "openai",
            "id": "key-brand-new",
            "api_key": raw_secret,
            "priority": 2,
            "weight": 50
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(create_resp.status(), StatusCode::CREATED);
    assert_eq!(
        create_resp.headers().get("cache-control").unwrap(),
        "no-store"
    );
    assert_eq!(create_resp.headers().get("pragma").unwrap(), "no-cache");

    let created: serde_json::Value = create_resp.json().await.unwrap();
    assert_eq!(created["id"], "key-brand-new");
    assert_eq!(created["api_key"], raw_secret, "Plaintext must be echoed once upon creation");

    // 2. Subsequent GET /api/admin/keys returns desensitized masked_key
    let list_resp = client
        .get(format!("http://{}/api/admin/keys", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let keys: Vec<serde_json::Value> = list_resp.json().await.unwrap();
    let found = keys.iter().find(|k| k["id"] == "key-brand-new").unwrap();
    let masked = found["masked_key"].as_str().unwrap();

    assert_ne!(masked, raw_secret, "GET must never return plaintext secret");
    assert!(masked.contains("***"));
    assert!(masked.starts_with("sk-***"));

    // 3. Delete Key
    let del_resp = client
        .delete(format!("http://{}/api/admin/keys/key-brand-new?provider=openai", harness.addr))
        .header("Authorization", &auth)
        .header("If-Match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);

    // Verify key removed from pool
    let list_again = client
        .get(format!("http://{}/api/admin/keys", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    let keys_after: Vec<serde_json::Value> = list_again.json().await.unwrap();
    assert!(!keys_after.iter().any(|k| k["id"] == "key-brand-new"));
}

// -----------------------------------------------------------------------------
// Test 7: Key Dial-Test with Mock Upstream Server
// -----------------------------------------------------------------------------
async fn spawn_mock_upstream() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    use axum::routing::get;
    let app = axum::Router::new()
        .route("/ok/models", get(|| async { (StatusCode::OK, "models ok") }))
        .route(
            "/auth_fail/models",
            get(|| async { (StatusCode::UNAUTHORIZED, "auth fail") }),
        )
        .route(
            "/rate_limited/models",
            get(|| async { (StatusCode::TOO_MANY_REQUESTS, "rate limited") }),
        )
        .route(
            "/slow/models",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(3500)).await;
                (StatusCode::OK, "slow response")
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

#[tokio::test]
async fn test_key_dial_test_matrix() {
    let (mock_addr, _mock_handle) = spawn_mock_upstream().await;
    let harness = WriteTestHarness::new(true).await;
    let client = reqwest::Client::new();
    let auth = format!("Bearer {}", harness.api_key);

    // Setup providers pointing to mock upstream
    let test_cases = vec![
        ("prov-ok", format!("http://{}/ok", mock_addr), "k-ok"),
        ("prov-auth", format!("http://{}/auth_fail", mock_addr), "k-auth"),
        ("prov-rate", format!("http://{}/rate_limited", mock_addr), "k-rate"),
        ("prov-slow", format!("http://{}/slow", mock_addr), "k-slow"),
    ];

    let mut ver = 0;
    for (p_name, base_url, k_id) in &test_cases {
        // Add provider
        client
            .post(format!("http://{}/api/admin/providers", harness.addr))
            .header("Authorization", &auth)
            .header("If-Match", format!("\"{ver}\""))
            .json(&serde_json::json!({ "name": p_name, "base_url": base_url }))
            .send()
            .await
            .unwrap();
        ver += 1;

        // Add key
        client
            .post(format!("http://{}/api/admin/keys", harness.addr))
            .header("Authorization", &auth)
            .header("If-Match", format!("\"{ver}\""))
            .json(&serde_json::json!({
                "provider": p_name,
                "id": k_id,
                "api_key": "test-key-material"
            }))
            .send()
            .await
            .unwrap();
        ver += 1;
    }

    // 1. OK probe
    let resp = client
        .post(format!("http://{}/api/admin/keys/k-ok/test", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(view["success"], true);
    assert_eq!(view["http_status"], 200);
    assert!(view["error_code"].is_null());

    // 2. Auth fail probe
    let resp = client
        .post(format!("http://{}/api/admin/keys/k-auth/test", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(view["success"], false);
    assert_eq!(view["http_status"], 401);
    assert_eq!(view["error_code"], "unauthorized");

    // 3. Rate limited probe
    let resp = client
        .post(format!("http://{}/api/admin/keys/k-rate/test", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(view["success"], false);
    assert_eq!(view["http_status"], 429);
    assert_eq!(view["error_code"], "rate_limited");

    // 4. Timeout probe (>=3s)
    let resp = client
        .post(format!("http://{}/api/admin/keys/k-slow/test", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(view["success"], false);
    assert_eq!(view["error_code"], "timeout");
    assert!(view["latency_ms"].as_u64().unwrap() >= 3000);

    // 5. Non-existent key probe returns 404
    let resp = client
        .post(format!("http://{}/api/admin/keys/ghost-key/test", harness.addr))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// -----------------------------------------------------------------------------
// Test 8: Write queue serialization & concurrency safety
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_write_queue_concurrency() {
    let harness = WriteTestHarness::new(true).await;
    let auth = format!("Bearer {}", harness.api_key);
    let addr = harness.addr;

    // Concurrently add 10 models with If-Match: "*" wildcard
    let mut handles = Vec::new();
    for i in 0..10 {
        let auth = auth.clone();
        let handle = tokio::spawn(async move {
            let client = reqwest::Client::new();
            client
                .post(format!("http://{}/api/admin/models", addr))
                .header("Authorization", auth)
                .header("If-Match", "*")
                .json(&serde_json::json!({
                    "provider": "openai",
                    "name": format!("model-concurrent-{i}")
                }))
                .send()
                .await
                .unwrap()
        });
        handles.push(handle);
    }

    for h in handles {
        let resp = h.await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // Verify all 10 models exist and config_version reached 10
    let store = FileConfigStore::new(&harness.config_path);
    let final_cfg = store.load().unwrap();
    assert_eq!(final_cfg.config_version, 10);
    assert_eq!(final_cfg.providers["openai"].models.len(), 11); // 1 initial + 10 added
}
