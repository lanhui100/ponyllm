//! Read-only quota snapshot tests (`GET /api/admin/quota`).
//!
//! 门禁：内存三态 + provider/key 过滤 + refresh=false 零上游 +
//! 未授权 401 + 响应零 key 原文。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ponyllm_config::{ConfigFile, KeySection, ModelConfig, ProviderSection};
use ponyllm_core::pool::{
    ApiKeyEntry, BillingMode, GatewayRoutingStrategy, KeyPool, ModelTier, PoolErrorType,
    RoutingStrategy,
};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use reqwest::StatusCode;

struct QuotaHarness {
    addr: SocketAddr,
    api_key: String,
}

impl QuotaHarness {
    async fn new() -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let raw_keys = vec![
            KeySection {
                id: "q-active".to_string(),
                api_key: "sk-probe-active-token-abcdef1234567890".to_string(),
                priority: 1,
                weight: 10,
            },
            KeySection {
                id: "q-cool".to_string(),
                api_key: "sk-probe-cooling-token-xyz9876543210".to_string(),
                priority: 2,
                weight: 10,
            },
        ];

        let provider_sec = ProviderSection {
            base_url: "https://api.example.com/v1".to_string(),
            default_model: "probe-model".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.0,
            cached_price: 0.5,
            output_price: 2.0,
            models: vec!["probe-model".to_string()],
            model_configs: vec![ModelConfig {
                name: "probe-model".to_string(),
                tier: ModelTier::Standard,
                billing_mode: Some(BillingMode::Metered),
                context_window: "128K".to_string(),
                max_output: "16K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(1.0),
                cached_price: Some(0.5),
                output_price: Some(2.0),
                pricing_mode: None,
                pricing_periods: Vec::new(),
                display_name: None,
                temperature: None,
                top_p: None,
                protocol: None,
                base_url: None,
                thinking_default: None,
                thinking_max: None,
                proxy: None,
                timeout_secs: None,
            }],
            keys: raw_keys.clone(),
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
        };

        let mut providers = HashMap::new();
        providers.insert("prober".to_string(), provider_sec);

        let mut config_file = ConfigFile::default();
        config_file.gateway.bind = "127.0.0.1:8080".to_string();
        config_file.gateway.api_key = "quota-test-secret".to_string();
        config_file.gateway.default_strategy = GatewayRoutingStrategy::Economy;
        config_file.gateway.web_enabled = false;
        config_file.gateway.admin_write_enabled = false;
        config_file.providers = providers;
        config_file.config_version = 0;
        config_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = "quota-test-secret".to_string();
        gw_config.default_strategy = GatewayRoutingStrategy::Economy;
        gw_config.web_enabled = false;
        gw_config.admin_write_enabled = false;
        gw_config.providers.insert(
            "prober".to_string(),
            ProviderConfig {
                base_url: "https://api.example.com/v1".to_string(),
                default_model: "probe-model".to_string(),
                strategy: "round_robin".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: 1.0,
                cached_price: 0.5,
                output_price: 2.0,
                models: vec!["probe-model".to_string()],
                model_specs: vec![ModelSpec {
                    name: "probe-model".to_string(),
                    tier: ModelTier::Standard,
                    context_window: "128K".to_string(),
                    max_output: "16K".to_string(),
                    input_types: vec!["text".to_string()],
                    output_types: vec!["text".to_string()],
                    billing_mode: Some(BillingMode::Metered),
                    input_price: Some(1.0),
                    cached_price: Some(0.5),
                    output_price: Some(2.0),
                    pricing_mode: None,
                    pricing_periods: Vec::new(),
                    display_name: None,
                    temperature: None,
                    top_p: None,
                    protocol: None,
                    base_url: None,
                    thinking_default: None,
                    thinking_max: None,
                    proxy: None,
                    timeout_secs: None,
                }],
                default_protocol: None,
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
            },
        );

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        // temp_dir must outlive the server: leak it (test-only).
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw_config).with_config_store(store));

        let pool = Arc::new(KeyPool::new("prober", RoutingStrategy::RoundRobin));
        for k in &raw_keys {
            pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
        }
        // One key into cooldown: memory L2 signal must surface.
        pool.set_key_cooldown("q-cool", Duration::from_secs(600));
        // A transient-only key must NOT cool (record_transient_failure path).
        pool.add_key(ApiKeyEntry::new(
            "q-transient",
            "sk-probe-transient-token-0000111122223333",
            3,
            10,
        ));
        if let Some(k) = pool
            .snapshot_keys()
            .into_iter()
            .find(|e| e.id == "q-transient")
        {
            k.record_failure(PoolErrorType::RateLimit { retry_after: None });
            k.clear_cooldown();
        }
        state.register_pool("prober", pool);

        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            api_key: "quota-test-secret".to_string(),
        }
    }

    fn auth(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        b.header("Authorization", format!("Bearer {}", self.api_key))
    }
}

#[tokio::test]
async fn quota_lists_memory_tri_state() {
    let h = QuotaHarness::new().await;
    let client = reqwest::Client::new();
    let body: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/quota", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 3, "expected 3 keys, got: {body}");
    let by_id = |id: &str| {
        items
            .iter()
            .find(|v| v["key_id"] == id)
            .unwrap_or_else(|| panic!("missing {id}: {body}"))
            .clone()
    };
    let active = by_id("q-active");
    assert_eq!(active["state"], "active");
    assert_eq!(active["schedulable"], true);
    assert_eq!(active["source"], "probe_only");
    let cool = by_id("q-cool");
    assert_eq!(cool["state"], "cooling_down");
    assert_eq!(cool["schedulable"], false);
    assert!(cool["cooldown_remaining_secs"].as_u64().unwrap() > 0);
    assert!(cool["cooldown_reset_at"].as_str().is_some());
    // No key material anywhere in the response.
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("sk-probe-"), "key material leaked: {raw}");
}

#[tokio::test]
async fn quota_filters_and_unknown_is_empty() {
    let h = QuotaHarness::new().await;
    let client = reqwest::Client::new();
    // provider filter hit
    let hit: serde_json::Value = h
        .auth(
            client.get(format!("http://{}/api/admin/quota?provider=prober", h.addr)),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hit.as_array().unwrap().len(), 3);
    // unknown provider -> empty list, not 404
    let miss = h
        .auth(
            client.get(format!("http://{}/api/admin/quota?provider=nope", h.addr)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(miss.status(), StatusCode::OK);
    let miss_body: serde_json::Value = miss.json().await.unwrap();
    assert_eq!(miss_body.as_array().unwrap().len(), 0);
    // key filter hit
    let one: serde_json::Value = h
        .auth(
            client.get(format!("http://{}/api/admin/quota?key_id=q-cool", h.addr)),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one.as_array().unwrap().len(), 1);
    assert_eq!(one[0]["key_id"], "q-cool");
}

#[tokio::test]
async fn quota_requires_auth_and_refresh_false_is_upstream_free() {
    let h = QuotaHarness::new().await;
    let client = reqwest::Client::new();
    // no token -> 401
    let unauth = client
        .get(format!("http://{}/api/admin/quota", h.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    // refresh=false (default): non-agy provider, no quota buckets, not stale
    let body: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/quota?refresh=false", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for v in body.as_array().unwrap() {
        assert!(v.get("quota").is_none(), "refresh=false must not probe: {v}");
        assert_eq!(v["stale"], false);
        assert!(v.get("usage").is_some(), "usage metrics must be present: {v}");
        assert!(v["usage"]["window_5h"].is_object());
        assert!(v["usage"]["window_weekly"].is_object());
    }
}
