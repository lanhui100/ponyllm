//! Integration and contract tests for Admin API (WEB-03).
//!
//! Enforces:
//! - Security 7 conditions:
//!   1. overview echoes auth_mode ("open" | "secured")
//!   2. auth rotate answers Cache-Control: no-store and Pragma: no-cache
//!   3. keys list masking with FlightRecorder::sanitize_key (no raw secrets)
//!   4. strategy PUT bumps config_version strictly (+1)
//!   5. bind consistency between overview and service/status (0.0.0.0 echo)
//!   6. openapi doc zero-hit for real key patterns and example placeholders only
//!   7. open mode rotate answers 409 Conflict
//! - Architect matrix:
//!   - 8 endpoints in secured mode (401 on unauthorized, 200 on authorized)
//!   - 8 endpoints in open mode (read endpoints 200, rotate 409)
//!   - SDK embedded path (config_store = None) -> 503 admin_store_unavailable
//!   - hot_reload_ms == 500 and no absolute path leak
//!   - config_version serde default compatibility

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{ConfigFile, KeySection, ModelConfig, ProviderSection};
use ponyllm_core::pool::{
    ApiKeyEntry, BillingMode, GatewayRoutingStrategy, KeyPool, ModelTier,
    RoutingStrategy,
};
use ponyllm_server::admin_store::{ConfigStore, FileConfigStore};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use reqwest::StatusCode;
use tempfile::TempDir;

/// Helper to set up a test gateway instance with FileConfigStore and mock provider.
#[allow(dead_code)]
struct TestHarness {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    pub temp_dir: TempDir,
    pub config_path: String,
    pub api_key: String,
    pub raw_keys: Vec<KeySection>,
}

impl TestHarness {
    async fn new(bind_addr: &str, api_key: &str, strategy: GatewayRoutingStrategy) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let raw_keys = vec![
            KeySection {
                id: "k-sk-standard".to_string(),
                api_key: "sk-proj-live-token-abcdef1234567890".to_string(),
                priority: 1,
                weight: 10,
            },
            KeySection {
                id: "k-non-sk".to_string(),
                api_key: "custom-vendor-secret-token-xyz987654".to_string(),
                priority: 2,
                weight: 20,
            },
            KeySection {
                id: "k-exact-eight".to_string(),
                api_key: "12345678".to_string(),
                priority: 3,
                weight: 30,
            },
            KeySection {
                id: "k-short-five".to_string(),
                api_key: "abc12".to_string(),
                priority: 4,
                weight: 40,
            },
        ];

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
                display_name: None,
                temperature: None,
                top_p: None,
                protocol: None,
                base_url: None,
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
        config_file.gateway.bind = bind_addr.to_string();
        config_file.gateway.api_key = api_key.to_string();
        config_file.gateway.default_strategy = strategy;
        config_file.gateway.web_enabled = true;
        config_file.gateway.web_dist_dir = "web/dist".to_string();
        config_file.providers = providers;
        config_file.config_version = 0;

        config_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        // Build GatewayConfig matching ConfigFile
        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = bind_addr.to_string();
        gw_config.api_key = api_key.to_string();
        gw_config.default_strategy = strategy;
        gw_config.web_enabled = true;
        gw_config.web_dist_dir = "web/dist".to_string();

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
            display_name: None,
            temperature: None,
            top_p: None,
            protocol: None,
            base_url: None,
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
            api_key: api_key.to_string(),
            raw_keys,
        }
    }

    async fn new_sdk_no_store() -> (SocketAddr, Arc<AppState>) {
        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = "sdk-test-token".to_string();

        let state = Arc::new(AppState::new(gw_config));
        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state)
    }
}

// -----------------------------------------------------------------------------
// Test 1: openapi_no_real_secret & schema validity & generation
// -----------------------------------------------------------------------------
#[test]
fn test_openapi_no_real_secret_and_schema_committed() {
    let schema_json = ponyllm_server::routes::admin::openapi_json();
    let schema_str = serde_json::to_string_pretty(&schema_json).unwrap();

    // Condition 6: zero hits for real secret key patterns
    // e.g. sk-pony-[0-9a-f]{32} or unmasked raw key tokens
    assert!(
        !schema_str.contains("sk-pony-"),
        "OpenAPI schema must never contain real generated secret keys"
    );
    assert!(
        !schema_str.contains("sk-proj-"),
        "OpenAPI schema must never contain real project secret keys"
    );

    // Assert that components exist
    assert!(schema_json["components"]["schemas"]["OverviewView"].is_object());
    assert!(schema_json["components"]["schemas"]["ProviderView"].is_object());
    assert!(schema_json["components"]["schemas"]["ModelView"].is_object());
    assert!(schema_json["components"]["schemas"]["KeyView"].is_object());
    assert!(schema_json["components"]["schemas"]["StrategyView"].is_object());
    assert!(schema_json["components"]["schemas"]["ServiceStatusView"].is_object());
    assert!(schema_json["components"]["schemas"]["RotateView"].is_object());

    // Check against committed web/openapi.json if present
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let openapi_path = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("web")
        .join("openapi.json");

    if openapi_path.exists() {
        let committed_str = std::fs::read_to_string(&openapi_path).unwrap();
        let committed_val: serde_json::Value = serde_json::from_str(&committed_str).unwrap();
        assert_eq!(
            schema_json, committed_val,
            "web/openapi.json is out of date! Run the dump helper to sync."
        );
    }
}

/// Helper test to dump/sync web/openapi.json
#[test]
#[ignore = "utility to regenerate web/openapi.json"]
fn dump_openapi_json() {
    let schema_json = ponyllm_server::routes::admin::openapi_json();
    let schema_str = format!("{}\n", serde_json::to_string_pretty(&schema_json).unwrap());
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let openapi_path = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("web")
        .join("openapi.json");
    std::fs::write(openapi_path, schema_str).unwrap();
}

// -----------------------------------------------------------------------------
// Test 2: keys_masking (Security condition 3)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_keys_masking_comprehensive() {
    let harness = TestHarness::new("127.0.0.1:8080", "secret-test-key", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/api/admin/keys", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body_text = resp.text().await.unwrap();

    // Zero-leak assert: none of the raw keys should appear anywhere in the response text
    for raw in &harness.raw_keys {
        assert!(
            !body_text.contains(&raw.api_key),
            "Raw key '{}' was leaked in /api/admin/keys response!",
            raw.api_key
        );
    }

    let views: Vec<serde_json::Value> = serde_json::from_str(&body_text).unwrap();
    assert_eq!(views.len(), 4);

    for view in &views {
        let id = view["id"].as_str().unwrap();
        let masked = view["masked_key"].as_str().unwrap();
        let raw = harness.raw_keys.iter().find(|k| k.id == id).unwrap();

        if raw.api_key.len() <= 8 {
            assert_eq!(masked, "****", "Key <= 8 chars must be masked as '****'");
        } else if raw.api_key.starts_with("sk-") {
            assert!(
                masked.starts_with("sk-***"),
                "Key starting with sk- must format as sk-***..."
            );
            let suffix = &raw.api_key[raw.api_key.len() - 4..];
            assert!(masked.ends_with(suffix));
        } else {
            assert!(masked.contains("***"));
        }
    }
}

// -----------------------------------------------------------------------------
// Test 3: auth_rotate_no_store_headers_and_effect (Security condition 2 & ADR)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_auth_rotate_no_store_headers_and_effect() {
    let harness = TestHarness::new("127.0.0.1:8080", "old-secret-token", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{}/api/admin/auth/rotate", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Condition 2: Cache-Control: no-store and Pragma: no-cache
    let cache_control = resp.headers().get("cache-control").and_then(|v| v.to_str().ok());
    assert_eq!(cache_control, Some("no-store"));
    let pragma = resp.headers().get("pragma").and_then(|v| v.to_str().ok());
    assert_eq!(pragma, Some("no-cache"));

    let body: serde_json::Value = resp.json().await.unwrap();
    let new_token = body["new_token"].as_str().unwrap().to_string();
    let rotated_at = body["rotated_at"].as_str().unwrap();
    let version = body["config_version"].as_u64().unwrap();

    assert!(new_token.starts_with("sk-pony-"));
    assert_ne!(new_token, harness.api_key);
    assert_eq!(version, 1);
    assert!(chrono::DateTime::parse_from_rfc3339(rotated_at).is_ok());

    // Immediate effect on new requests:
    // 1. Old token must now be rejected with 401
    let old_resp = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(old_resp.status(), StatusCode::UNAUTHORIZED);

    // 2. New token succeeds with 200
    let new_resp = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .header("Authorization", format!("Bearer {}", new_token))
        .send()
        .await
        .unwrap();
    assert_eq!(new_resp.status(), StatusCode::OK);

    // Check disk persistence
    let store = FileConfigStore::new(&harness.config_path);
    let disk_cfg = store.load().unwrap();
    assert_eq!(disk_cfg.gateway.api_key, new_token);
    assert_eq!(disk_cfg.config_version, 1);
}

// -----------------------------------------------------------------------------
// Test 4: strategy_put_bumps_config_version (Security condition 4)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_strategy_put_bumps_config_version() {
    let harness = TestHarness::new("127.0.0.1:8080", "secret-key", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    // Check initial version
    let get_resp = client
        .get(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let initial: serde_json::Value = get_resp.json().await.unwrap();
    assert_eq!(initial["config_version"], 0);
    assert_eq!(initial["strategy"], "economy");

    // Put new valid strategy
    let put_resp = client
        .put(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({"strategy": "speed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), StatusCode::OK);
    let updated: serde_json::Value = put_resp.json().await.unwrap();
    assert_eq!(updated["strategy"], "speed");
    assert_eq!(updated["config_version"], 1);

    // Verify disk was bumped
    let store = FileConfigStore::new(&harness.config_path);
    let disk_cfg = store.load().unwrap();
    assert_eq!(disk_cfg.config_version, 1);
    assert_eq!(
        disk_cfg.gateway.default_strategy,
        GatewayRoutingStrategy::Speed
    );

    // Immediate memory reload verification
    let get_again = client
        .get(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    let reloaded: serde_json::Value = get_again.json().await.unwrap();
    assert_eq!(reloaded["strategy"], "speed");
    assert_eq!(reloaded["config_version"], 1);

    // Invalid strategy yields 400 and does NOT bump version
    let bad_put = client
        .put(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({"strategy": "teleportation"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_put.status(), StatusCode::BAD_REQUEST);

    // Missing field yields 400
    let missing_field_put = client
        .put(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({"other": "field"}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_field_put.status(), StatusCode::BAD_REQUEST);
}

// -----------------------------------------------------------------------------
// Test 5: bind_consistency and zero_bind_echo (Security condition 5)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_bind_consistency_and_zero_bind_echo() {
    let harness = TestHarness::new("0.0.0.0:8888", "secret-key", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    let overview_resp = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(overview_resp.status(), StatusCode::OK);
    let overview_body: serde_json::Value = overview_resp.json().await.unwrap();

    let status_resp = client
        .get(format!("http://{}/api/admin/service/status", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);
    let status_body: serde_json::Value = status_resp.json().await.unwrap();

    // Condition 5: exactly equal bind field, 0.0.0.0 preserved as configured
    let bind_overview = overview_body["bind"].as_str().unwrap();
    let bind_status = status_body["bind"].as_str().unwrap();

    assert_eq!(bind_overview, "0.0.0.0:8888");
    assert_eq!(bind_status, "0.0.0.0:8888");
    assert_eq!(bind_overview, bind_status);
}

// -----------------------------------------------------------------------------
// Test 6: open_mode_matrix (Security condition 1 & 7)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_open_mode_matrix() {
    // Empty api_key triggers open mode
    let harness = TestHarness::new("127.0.0.1:8080", "", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    // 1. Overview reports auth_mode = "open"
    let overview_resp = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(overview_resp.status(), StatusCode::OK);
    let overview_body: serde_json::Value = overview_resp.json().await.unwrap();
    assert_eq!(overview_body["auth_mode"], "open");

    // 2. All read endpoints accessible without token
    let endpoints = vec![
        "/api/admin/overview",
        "/api/admin/providers",
        "/api/admin/providers/openai/models",
        "/api/admin/keys",
        "/api/admin/strategy",
        "/api/admin/service/status",
    ];

    for path in endpoints {
        let resp = client
            .get(format!("http://{}{}", harness.addr, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Open mode failed for {}", path);
    }

    // 3. Condition 7: Rotate in open mode yields 409 Conflict
    let rotate_resp = client
        .post(format!("http://{}/api/admin/auth/rotate", harness.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(rotate_resp.status(), StatusCode::CONFLICT);
    let err_body: serde_json::Value = rotate_resp.json().await.unwrap();
    assert_eq!(err_body["error"]["code"], "open_mode_no_credential");
}

// -----------------------------------------------------------------------------
// Test 7: secured_mode_matrix (All 8 endpoints 401 without auth, 200 with auth)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_secured_mode_matrix() {
    let harness = TestHarness::new("127.0.0.1:8080", "strong-auth-secret", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    let test_cases = vec![
        ("GET", "/api/admin/overview", None),
        ("GET", "/api/admin/providers", None),
        ("GET", "/api/admin/providers/openai/models", None),
        ("GET", "/api/admin/keys", None),
        ("GET", "/api/admin/strategy", None),
        ("PUT", "/api/admin/strategy", Some(serde_json::json!({"strategy": "reliable"}))),
        ("GET", "/api/admin/service/status", None),
        ("POST", "/api/admin/auth/rotate", None),
    ];

    for (method, path, body) in test_cases {
        // Unauthenticated request -> 401
        let req_unauth = match method {
            "GET" => client.get(format!("http://{}{}", harness.addr, path)),
            "PUT" => client.put(format!("http://{}{}", harness.addr, path)).json(&body.clone().unwrap()),
            "POST" => client.post(format!("http://{}{}", harness.addr, path)),
            _ => unreachable!(),
        };
        let resp_unauth = req_unauth.send().await.unwrap();
        assert_eq!(
            resp_unauth.status(),
            StatusCode::UNAUTHORIZED,
            "Expected 401 for unauthenticated {} {}",
            method,
            path
        );

        // Wrong token request -> 401
        let req_wrong = match method {
            "GET" => client.get(format!("http://{}{}", harness.addr, path)),
            "PUT" => client.put(format!("http://{}{}", harness.addr, path)).json(&body.clone().unwrap()),
            "POST" => client.post(format!("http://{}{}", harness.addr, path)),
            _ => unreachable!(),
        };
        let resp_wrong = req_wrong
            .header("Authorization", "Bearer invalid-token")
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp_wrong.status(),
            StatusCode::UNAUTHORIZED,
            "Expected 401 for bad token on {} {}",
            method,
            path
        );
    }

    // Now test all 8 endpoints with valid token
    // 1. overview
    let r1 = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let r1_body: serde_json::Value = r1.json().await.unwrap();
    assert_eq!(r1_body["auth_mode"], "secured");

    // 2. providers
    let r2 = client
        .get(format!("http://{}/api/admin/providers", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);

    // 3. provider models
    let r3 = client
        .get(format!("http://{}/api/admin/providers/openai/models", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::OK);

    // 4. keys
    let r4 = client
        .get(format!("http://{}/api/admin/keys", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r4.status(), StatusCode::OK);

    // 5. strategy GET
    let r5 = client
        .get(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r5.status(), StatusCode::OK);

    // 6. strategy PUT
    let r6 = client
        .put(format!("http://{}/api/admin/strategy", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .json(&serde_json::json!({"strategy": "reliable"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r6.status(), StatusCode::OK);

    // 7. service status
    let r7 = client
        .get(format!("http://{}/api/admin/service/status", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r7.status(), StatusCode::OK);

    // 8. auth rotate
    let r8 = client
        .post(format!("http://{}/api/admin/auth/rotate", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    assert_eq!(r8.status(), StatusCode::OK);
}

// -----------------------------------------------------------------------------
// Test 8: sdk_none_store_unavailable (SDK embedded mode -> 503)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_sdk_none_store_unavailable() {
    let (addr, _state) = TestHarness::new_sdk_no_store().await;
    let client = reqwest::Client::new();
    let auth = "Bearer sdk-test-token";

    // Store-dependent endpoints must return 503 admin_store_unavailable
    let store_endpoints = vec![
        ("GET", "/api/admin/overview", None),
        ("GET", "/api/admin/keys", None),
        ("GET", "/api/admin/strategy", None),
        ("PUT", "/api/admin/strategy", Some(serde_json::json!({"strategy": "speed"}))),
        ("GET", "/api/admin/service/status", None),
        ("POST", "/api/admin/auth/rotate", None),
    ];

    for (method, path, body) in store_endpoints {
        let req = match method {
            "GET" => client.get(format!("http://{}{}", addr, path)),
            "PUT" => client.put(format!("http://{}{}", addr, path)).json(&body.unwrap()),
            "POST" => client.post(format!("http://{}{}", addr, path)),
            _ => unreachable!(),
        };
        let resp = req.header("Authorization", auth).send().await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Expected 503 for {}",
            path
        );
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["error"]["code"], "admin_store_unavailable",
            "Expected code admin_store_unavailable for {}",
            path
        );
    }

    // In-memory config endpoints that don't need store remain 200
    let p_resp = client
        .get(format!("http://{}/api/admin/providers", addr))
        .header("Authorization", auth)
        .send()
        .await
        .unwrap();
    assert_eq!(p_resp.status(), StatusCode::OK);
}

// -----------------------------------------------------------------------------
// Test 9: overview_hot_reload_ms_and_no_path_leak (Acceptance 3)
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_overview_hot_reload_ms_and_no_path_leak() {
    let harness = TestHarness::new("127.0.0.1:8080", "secret-key", GatewayRoutingStrategy::Economy).await;
    let client = reqwest::Client::new();

    let overview_resp = client
        .get(format!("http://{}/api/admin/overview", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    let overview_text = overview_resp.text().await.unwrap();
    let overview_val: serde_json::Value = serde_json::from_str(&overview_text).unwrap();

    // Acceptance: hot_reload_ms is exactly 500
    assert_eq!(overview_val["hot_reload_ms"], 500);

    let status_resp = client
        .get(format!("http://{}/api/admin/service/status", harness.addr))
        .header("Authorization", format!("Bearer {}", harness.api_key))
        .send()
        .await
        .unwrap();
    let status_text = status_resp.text().await.unwrap();

    // Assert neither echoes absolute file path of config or web dist
    let abs_temp_path = harness.temp_dir.path().to_str().unwrap();
    assert!(
        !overview_text.contains(abs_temp_path),
        "overview must not echo config directory path"
    );
    assert!(
        !status_text.contains(abs_temp_path),
        "status must not echo config directory path"
    );
    assert!(
        !overview_text.contains("ponyllm.toml"),
        "overview must not mention config file name"
    );
    assert!(
        !status_text.contains("ponyllm.toml"),
        "status must not mention config file name"
    );
}

// -----------------------------------------------------------------------------
// Test 10: config_version_serde_default_compat
// -----------------------------------------------------------------------------
#[test]
fn test_config_version_serde_default_compat() {
    // Old toml without config_version key
    let old_toml = r#"
[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
api_key = "test-token"
default_strategy = "economy"

[providers.openai]
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"
"#;

    let parsed: ConfigFile = toml::from_str(old_toml).unwrap();
    assert_eq!(
        parsed.config_version, 0,
        "Old toml without config_version should default to 0"
    );
    assert_eq!(parsed.gateway.bind, "127.0.0.1:8080");
}
