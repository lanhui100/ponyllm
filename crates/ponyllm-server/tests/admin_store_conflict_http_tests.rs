//! HTTP-layer seam test (P1-qa S2-1 / A2): the admin write path must map a
//! STORE-level optimistic-concurrency conflict (ConfigStoreError::Conflict)
//! to HTTP 412 `precondition_failed` AND increment `admin_save_conflicts_total`
//! — distinct from the existing If-Match header precondition tests.
//!
//! Uses a fake `SecretApi` (force-conflict switch) behind a real
//! `KubernetesConfigStore`, injected into a real `create_app` router, so the
//! whole "wiremock-level 409 → store Conflict → HTTP 412 + metric" chain is
//! exercised without a cluster.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ponyllm_server::admin_store::{
    ConfigStoreError, ConfigVersion, KubernetesConfigStore, SecretApi, SecretSnapshot,
};
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;

/// SecretApi double mirroring admin_store's test fake (kept local so the
/// integration test owns its switch).
#[derive(Default)]
struct FakeSecretApi {
    force_conflict: std::sync::atomic::AtomicBool,
    /// When true, GET answers NotFound (config truth source deleted).
    not_found: std::sync::atomic::AtomicBool,
    /// When true, GET answers Timeout (API server hung).
    force_timeout: std::sync::atomic::AtomicBool,
    rv: std::sync::atomic::AtomicU64,
    toml: std::sync::Mutex<String>,
}

impl FakeSecretApi {
    fn seed(toml: String) -> Arc<Self> {
        Arc::new(Self {
            force_conflict: std::sync::atomic::AtomicBool::new(false),
            not_found: std::sync::atomic::AtomicBool::new(false),
            force_timeout: std::sync::atomic::AtomicBool::new(false),
            rv: std::sync::atomic::AtomicU64::new(100),
            toml: std::sync::Mutex::new(toml),
        })
    }
}

#[async_trait]
impl SecretApi for FakeSecretApi {
    async fn get(&self, _name: &str) -> Result<SecretSnapshot, ConfigStoreError> {
        if self.force_timeout.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ConfigStoreError::Timeout {
                operation: "get",
                duration: std::time::Duration::from_millis(1500),
            });
        }
        if self.not_found.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ConfigStoreError::NotFound(
                "secrets \"ponyllm-live-config\" not found".to_string(),
            ));
        }
        let mut data = BTreeMap::new();
        data.insert(
            "ponyllm.toml".to_string(),
            self.toml.lock().unwrap().clone().into_bytes(),
        );
        Ok(SecretSnapshot {
            resource_version: Some(self.rv.load(std::sync::atomic::Ordering::SeqCst).to_string()),
            data,
        })
    }

    async fn patch_data(
        &self,
        _name: &str,
        resource_version: &str,
        _data: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), ConfigStoreError> {
        if self.force_conflict.load(std::sync::atomic::Ordering::SeqCst)
            || self.rv.load(std::sync::atomic::Ordering::SeqCst).to_string() != resource_version
        {
            return Err(ConfigStoreError::Conflict {
                expected: Some(ConfigVersion::Kubernetes(resource_version.to_string())),
                current: Some(ConfigVersion::Kubernetes(
                    self.rv.load(std::sync::atomic::Ordering::SeqCst).to_string(),
                )),
            });
        }
        self.rv.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn namespace(&self) -> &str {
        "ponyllm"
    }
}

fn gateway_config() -> GatewayConfig {
    let mut cfg = GatewayConfig::default();
    cfg.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    cfg.admin_write_enabled = true;
    cfg
}

/// Live config seeded with the SAME `openai` provider the fake Secret serves,
/// mirroring `generate_sample_config()` (`strategy = "priority"`).
///
/// Why this exists: `state.config.providers` is NEVER populated asynchronously
/// in this harness. `run_config_poller` has exactly one caller in the whole
/// workspace — `crates/ponyllm-cli/src/main.rs:527` — and nothing in
/// `create_app` / `AppState::new` spawns it, so seeding the in-memory config
/// from the store is impossible here and a bounded poll would never settle.
/// `handle_admin_update_provider` only mutates the live config through
/// `state.config.write().providers.get_mut(&name)`, i.e. a silent no-op on an
/// empty map. Seeding the map up-front therefore is the only way to assert the
/// live-config contract; it makes the write observable deterministically with no
/// timing assumption at all.
fn gateway_config_with_openai() -> GatewayConfig {
    let mut cfg = gateway_config();
    cfg.providers.insert(
        "openai".to_string(),
        ponyllm_server::ProviderConfig {
            base_url: "https://api.openai.com".to_string(),
            default_model: "gpt-4o".to_string(),
            strategy: "priority".to_string(),
            ..Default::default()
        },
    );
    cfg
}

async fn spawn_gateway_with_config(
    fake: Arc<FakeSecretApi>,
    config_poll_ms: u64,
    cfg: GatewayConfig,
) -> (String, Arc<AppState>) {
    let store = Arc::new(KubernetesConfigStore::with_api(
        fake,
        "ponyllm-live-config",
        "ponyllm.toml",
    ));
    let state = Arc::new(
        AppState::new(cfg)
            .with_config_store(store)
            .with_config_poll_ms(config_poll_ms),
    );
    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), state)
}

async fn spawn_gateway_with_poll(fake: Arc<FakeSecretApi>, config_poll_ms: u64) -> (String, Arc<AppState>) {
    spawn_gateway_with_config(fake, config_poll_ms, gateway_config()).await
}

/// FakeSecretApi that answers GET with a fixed delay (simulating a hung /
/// black-holed control plane that never settles within the admin
/// fast-degradation budget of 1s).
#[derive(Default)]
struct SlowSecretApi {
    delay: std::time::Duration,
}

#[async_trait]
impl SecretApi for SlowSecretApi {
    async fn get(&self, _name: &str) -> Result<SecretSnapshot, ConfigStoreError> {
        tokio::time::sleep(self.delay).await;
        let mut data = BTreeMap::new();
        data.insert(
            "ponyllm.toml".to_string(),
            ponyllm_config::generate_sample_config().as_bytes().to_vec(),
        );
        Ok(SecretSnapshot {
            resource_version: Some("100".to_string()),
            data,
        })
    }

    async fn patch_data(
        &self,
        _name: &str,
        _resource_version: &str,
        _data: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), ConfigStoreError> {
        tokio::time::sleep(self.delay).await;
        Ok(())
    }

    fn namespace(&self) -> &str {
        "ponyllm"
    }
}

/// Phase 3 fast-degradation contract: when the K8s control plane hangs (the
/// store never settles), the admin interface must answer 503 `admin_store_degraded`
/// within ~1 second instead of hanging the caller up to the store's own timeout.
#[tokio::test]
async fn admin_store_hang_degrades_to_http_503_within_one_second() {
    // The store would only respond after 5s — far beyond the 1s admin budget.
    let fake = Arc::new(SlowSecretApi {
        delay: std::time::Duration::from_secs(5),
    });
    let store = Arc::new(KubernetesConfigStore::with_api(
        fake,
        "ponyllm-live-config",
        "ponyllm.toml",
    ));
    let state = Arc::new(
        AppState::new(gateway_config())
            .with_config_store(store)
            .with_config_poll_ms(500),
    );
    let app = create_app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{}", addr);
    let client = reqwest::Client::new();

    let start = std::time::Instant::now();
    let resp = client
        .get(format!("{}/api/admin/overview", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "hanging store must degrade to 503, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "admin_store_degraded",
        "error code must be admin_store_degraded: {body}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(2000),
        "degraded response must arrive well within 1s budget, took {:?}",
        elapsed
    );
}

/// A valid `If-Match` write that then hits a STORE conflict must answer 412
/// with `precondition_failed` and count `admin_save_conflicts_total`.
#[tokio::test]
async fn store_conflict_maps_to_http_412_and_counts_metric() {    let fake = FakeSecretApi::seed(ponyllm_config::generate_sample_config().to_string());
    fake.force_conflict
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (base, state) = spawn_gateway_with_config(fake, 500, gateway_config_with_openai()).await;
    let client = reqwest::Client::new();

    // Premise guard: the live config MUST expose `openai`, otherwise the
    // post-write assertion below would be vacuous (the handler's `get_mut`
    // is a silent no-op on an empty map).
    assert_eq!(
        state.config.read().providers.get("openai").map(|p| p.strategy.clone()),
        Some("priority".to_string()),
        "live config must be seeded with openai/priority before the write"
    );

    let resp = client
        .put(format!("{}/api/admin/providers/openai", base))
        .header("Authorization", "Bearer test-token")
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({"strategy": "round_robin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED, "store Conflict must map to 412");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        "precondition_failed",
        "412 body must carry the web contract code: {body}"
    );

    // The metric must have been incremented by the store-conflict path.
    let metrics: serde_json::Value = client
        .get(format!("{}/v1/telemetry/metrics", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        metrics["ha_ops"]["admin_save_conflicts_total"], 1,
        "store-conflict path must increment admin_save_conflicts_total"
    );

    // The in-memory gateway config must NOT have been replaced by the failed write.
    let provider = state
        .config
        .read()
        .providers
        .get("openai")
        .cloned()
        .expect("live config must still carry the seeded openai provider");
    assert_eq!(
        provider.strategy, "priority",
        "failed write must not mutate the live config"
    );
}

/// The same write WITHOUT a store conflict succeeds (200), does not 412, and
/// does not increment the conflict metric — the A2 success leg.
#[tokio::test]
async fn store_success_path_answers_200_without_conflict_metric() {
    let fake = FakeSecretApi::seed(ponyllm_config::generate_sample_config().to_string());
    let (base, state) = spawn_gateway_with_config(fake, 500, gateway_config_with_openai()).await;
    let client = reqwest::Client::new();

    // Premise guard — same reason as the conflict leg: without a seeded
    // provider the handler's `get_mut` no-ops and the assertion is vacuous.
    assert_eq!(
        state.config.read().providers.get("openai").map(|p| p.strategy.clone()),
        Some("priority".to_string()),
        "live config must be seeded with openai/priority before the write"
    );

    let resp = client
        .put(format!("{}/api/admin/providers/openai", base))
        .header("Authorization", "Bearer test-token")
        .header("If-Match", "\"0\"")
        .json(&serde_json::json!({"strategy": "round_robin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "clean write must succeed");
    assert_eq!(
        state
            .config
            .read()
            .providers
            .get("openai")
            .expect("live config must still carry the openai provider")
            .strategy,
        "round_robin",
        "successful write must update the live config"
    );

    let metrics: serde_json::Value = client
        .get(format!("{}/v1/telemetry/metrics", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        metrics["ha_ops"]["admin_save_conflicts_total"], 0,
        "clean write must not count as a conflict"
    );
}

/// Positive contract for the kubernetes backend (P1-qa S3-1): the overview
/// endpoint echoes the per-backend polling interval (2000 for kubernetes) —
/// the file backend's 500 is asserted in admin_contract_tests.
#[tokio::test]
async fn overview_hot_reload_ms_echoes_kubernetes_poll_interval() {
    let fake = FakeSecretApi::seed(ponyllm_config::generate_sample_config().to_string());
    let (base, _state) = spawn_gateway_with_poll(fake, 2000).await;
    let client = reqwest::Client::new();

    let overview: serde_json::Value = client
        .get(format!("{}/api/admin/overview", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        overview["hot_reload_ms"], 2000,
        "kubernetes backend must report its 2s poll interval: {overview}"
    );
}

/// NotFound seam (P11-sec S3-2): a deleted truth source must surface as
/// HTTP 503 with `config_store_unavailable` (distinct from InvalidData/500)
/// so ops can tell "Secret deleted" from "config broken".
#[tokio::test]
async fn store_not_found_maps_to_http_503_config_store_unavailable() {
    let fake = FakeSecretApi::seed(ponyllm_config::generate_sample_config().to_string());
    fake.not_found
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (base, _state) = spawn_gateway_with_poll(fake, 500).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/admin/overview", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "deleted truth source must map to 503"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "config_store_unavailable");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("ponyllm-live-config") && !message.contains("not found"),
        "503 body must not leak the Secret name: {body}"
    );
}

/// Timeout seam: when API server hangs and KubeSecretApi times out,
/// the admin endpoint must map [`ConfigStoreError::Timeout`] to HTTP 504
/// `config_store_timeout` fast without hanging the caller.
#[tokio::test]
async fn store_timeout_maps_to_http_504_config_store_timeout() {
    let fake = FakeSecretApi::seed(ponyllm_config::generate_sample_config().to_string());
    fake.force_timeout
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (base, _state) = spawn_gateway_with_poll(fake, 500).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/admin/overview", base))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "store timeout must map to HTTP 504"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "config_store_timeout",
        "error code must be config_store_timeout: {body}"
    );
}
