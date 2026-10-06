//! Admin-surface egress-pool tests (contract C7 + C8 regression).
//!
//! C7 — management plane:
//!     - `GET /api/admin/quota` provider rows expose an egress view
//!       (`egress` array: egress_id / state / cooldown_remaining_secs /
//!       cooldown_reset_at), reflecting live per-egress cooldowns;
//!     - `PUT /api/admin/providers/{name}` accepts `egress_pool` +
//!       `egress_strategy`; `egress_pool = []` clears the pool and falls back
//!       to the provider `proxy` semantics;
//!     - pool entries are validated with the SSRF guard (private / metadata /
//!       bad scheme ⇒ 400 `egress_blocked`);
//!     - the existing `admin_write_enabled` gate still guards these writes.
//! C8 — regression: a provider without `egress_pool` keeps today's behavior
//!     exactly (`proxy` resolution, empty pool, no egress view).
//!
//! Assumed frozen interface used by these tests:
//! - `AppState::egress_pools: parking_lot::RwLock<HashMap<String, Arc<EgressPool>>>`
//!   (pub, parallel to `AppState::pools`) — the quota handler reads it.
//! - `ponyllm_core::pool::{EgressPool, EgressEntry, EgressStrategy}`.
//! - `UpdateProviderPayload::{egress_pool: Option<Vec<String>>,
//!   egress_strategy: Option<String>}` and `ProviderView::{egress_pool,
//!   egress_strategy}` JSON keys.
//! - `QuotaKeyView::egress: Option<Vec<QuotaEgressView>>` (provider-level,
//!   repeated on every key row of that provider; `null`/absent for pool-less
//!   providers) with JSON keys `index: u32` (position in the pool),
//!   `entry: string` ("direct" or the raw proxy URL), `state: string`
//!   ("active" | "cooling"), `cooldown_reset_at: Option<string RFC3339>`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ponyllm_core::pool::{EgressEntry, EgressPool, EgressStrategy, KeyPool};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use reqwest::StatusCode;

/// Disk TOML truth: `zen` has an egress pool (+ a proxy fallback), `legacy`
/// has only the single proxy (zero-migration shape).
const DISK_TOML: &str = r#"
[gateway]
bind = "127.0.0.1:8080"
api_key = "egress-admin-secret"
admin_write_enabled = true
web_enabled = false

[providers.zen]
base_url = "https://opencode.ai/zen"
default_model = "zen-free"
strategy = "round_robin"
proxy = "http://127.0.0.1:8899"
egress_pool = ["direct", "http://127.0.0.1:8899"]
egress_strategy = "round_robin"
keys = [ { id = "zen-1", api_key = "sk-zen-token-0000111122223333", priority = 1, weight = 10 } ]

[providers.legacy]
base_url = "https://api.example.com/v1"
default_model = "legacy-model"
strategy = "round_robin"
proxy = "http://127.0.0.1:8899"
keys = [ { id = "legacy-1", api_key = "sk-legacy-token-9999888877776666", priority = 1, weight = 10 } ]
"#;

struct EgressAdminHarness {
    addr: SocketAddr,
    api_key: String,
    state: Arc<AppState>,
    zen_egress_pool: Arc<EgressPool>,
    config_path: String,
}

impl EgressAdminHarness {
    async fn new(admin_write_enabled: bool) -> Self {
        // Loopback pproxy / loopback mocks are legitimate test traffic.
        std::env::set_var("PONYLLM_ALLOW_LOOPBACK_PROBE", "1");
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");
        std::fs::write(&config_path, DISK_TOML).unwrap();

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = "egress-admin-secret".to_string();
        gw_config.web_enabled = false;
        gw_config.admin_write_enabled = admin_write_enabled;

        let mut zen = ProviderConfig::default();
        zen.base_url = "https://opencode.ai/zen".to_string();
        zen.default_model = "zen-free".to_string();
        zen.strategy = "round_robin".to_string();
        zen.proxy = Some("http://127.0.0.1:8899".to_string());
        let mut legacy = ProviderConfig::default();
        legacy.base_url = "https://api.example.com/v1".to_string();
        legacy.default_model = "legacy-model".to_string();
        legacy.strategy = "round_robin".to_string();
        legacy.proxy = Some("http://127.0.0.1:8899".to_string());
        gw_config.providers.insert("zen".to_string(), zen);
        gw_config.providers.insert("legacy".to_string(), legacy);

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir); // must outlive the server (test-only leak)
        let state = Arc::new(AppState::new(gw_config).with_config_store(store));

        // Live key pools (quota rows) + the live egress pool (quota egress view).
        let zen_keys = KeyPool::new("zen", ponyllm_core::pool::RoutingStrategy::RoundRobin);
        zen_keys.add_key(ponyllm_core::pool::ApiKeyEntry::new(
            "zen-1",
            "sk-zen-token-0000111122223333",
            1,
            10,
        ));
        state.register_pool("zen", Arc::new(zen_keys));
        let legacy_keys = KeyPool::new("legacy", ponyllm_core::pool::RoutingStrategy::RoundRobin);
        legacy_keys.add_key(ponyllm_core::pool::ApiKeyEntry::new(
            "legacy-1",
            "sk-legacy-token-9999888877776666",
            1,
            10,
        ));
        state.register_pool("legacy", Arc::new(legacy_keys));

        let egress_pool = Arc::new(EgressPool::new("zen", EgressStrategy::RoundRobin));
        egress_pool.add_egress(EgressEntry::direct("direct"));
        egress_pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        state.egress_pools.write().insert("zen".to_string(), egress_pool.clone());

        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            api_key: "egress-admin-secret".to_string(),
            state,
            zen_egress_pool: egress_pool,
            config_path: config_path.to_str().unwrap().to_string(),
        }
    }

    fn auth(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        b.header("Authorization", format!("Bearer {}", self.api_key))
    }
}

// ---------- C7: quota view exposes live egress state ----------

#[tokio::test]
async fn c7_quota_view_exposes_egress_state_and_cooldown() {
    let h = EgressAdminHarness::new(true).await;
    let client = reqwest::Client::new();

    let body: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/quota?provider=zen", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1, "one zen key row expected: {body}");
    let row = &rows[0];
    let egress = row["egress"].as_array().expect("quota row must expose egress view: {row}");
    assert_eq!(egress.len(), 2, "two egresses expected: {egress:?}");
    // Executor-frozen shape: index (pool position) + entry ("direct" or the
    // raw proxy URL) + state ("active" | "cooling") + cooldown_reset_at.
    let by_entry = |entry: &str| {
        egress
            .iter()
            .find(|e| e["entry"] == entry)
            .unwrap_or_else(|| panic!("missing egress entry {entry}: {egress:?}"))
    };
    let direct = by_entry("direct");
    assert_eq!(direct["index"], 0);
    assert_eq!(direct["state"], "active");
    let vps = by_entry("http://127.0.0.1:8899");
    assert_eq!(vps["index"], 1);
    assert_eq!(vps["state"], "active");

    // Live cooldown write → the view must flip that egress to cooling and
    // expose the wall-clock reset.
    h.zen_egress_pool
        .record_quota_exhausted("vps", Some(Duration::from_secs(1800)));
    let body2: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/quota?provider=zen", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let egress2 = body2[0]["egress"].as_array().expect("egress view must persist");
    let vps2 = egress2
        .iter()
        .find(|e| e["entry"] == "http://127.0.0.1:8899")
        .expect("vps egress row");
    assert_eq!(vps2["state"], "cooling");
    assert!(vps2["cooldown_reset_at"].as_str().is_some());
    let direct2 = egress2
        .iter()
        .find(|e| e["entry"] == "direct")
        .expect("direct egress row");
    assert_eq!(direct2["state"], "active", "direct must stay independent of vps's cooldown");

    // No key material anywhere in the response.
    let raw = serde_json::to_string(&body2).unwrap();
    assert!(!raw.contains("sk-"), "key material leaked: {raw}");
}

#[tokio::test]
async fn c7_quota_view_has_no_egress_array_for_pool_less_provider() {
    // C8 regression on the admin surface: a legacy provider must not gain an
    // egress view.
    let h = EgressAdminHarness::new(true).await;
    let client = reqwest::Client::new();
    let body: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/quota?provider=legacy", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for row in body.as_array().unwrap() {
        assert!(
            row.get("egress").is_none(),
            "pool-less provider must not expose an egress view: {row}"
        );
    }
}

// ---------- C7: PUT writes the pool; empty array clears back to proxy ----------

#[tokio::test]
async fn c7_put_provider_writes_egress_pool_and_strategy() {
    let h = EgressAdminHarness::new(true).await;
    let client = reqwest::Client::new();

    let resp = h
        .auth(
            client
                .put(format!("http://{}/api/admin/providers/zen", h.addr))
                .header("If-Match", "\"0\"")
                .json(&serde_json::json!({
                    "egress_pool": ["direct", "http://egress-a.example.com:8899"],
                    "egress_strategy": "priority",
                })),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "PUT must succeed: {:?}", resp.text().await.unwrap());
    let view: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        view["egress_pool"],
        serde_json::json!(["direct", "http://egress-a.example.com:8899"]),
        "PUT response must echo the new pool"
    );
    assert_eq!(view["egress_strategy"], "priority");

    // The read surface agrees.
    let list: serde_json::Value = h
        .auth(client.get(format!("http://{}/api/admin/providers", h.addr)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let zen = list.as_array().unwrap().iter().find(|v| v["name"] == "zen").unwrap();
    assert_eq!(
        zen["egress_pool"],
        serde_json::json!(["direct", "http://egress-a.example.com:8899"]),
        "GET providers must reflect the written pool"
    );
}

#[tokio::test]
async fn c7_put_provider_empty_egress_pool_clears_and_falls_back_to_proxy() {
    let h = EgressAdminHarness::new(true).await;
    let client = reqwest::Client::new();

    let resp = h
        .auth(
            client
                .put(format!("http://{}/api/admin/providers/zen", h.addr))
                .header("If-Match", "\"0\"")
                .json(&serde_json::json!({ "egress_pool": [] })),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "PUT must succeed: {:?}", resp.text().await.unwrap());
    let view: serde_json::Value = resp.json().await.unwrap();
    let cleared = view.get("egress_pool").and_then(|p| p.as_array()).map(|a| a.is_empty());
    assert!(
        cleared.unwrap_or(true),
        "clearing must leave an empty (or omitted) egress_pool: {view}"
    );

    // Persisted truth: pool gone, single-proxy fallback intact.
    let disk = std::fs::read_to_string(&h.config_path).unwrap();
    assert!(
        !disk.contains("egress-a.example.com"),
        "cleared pool entry must not persist"
    );
    assert!(
        disk.contains("proxy = \"http://127.0.0.1:8899\""),
        "proxy fallback must survive the pool clear"
    );
}

// ---------- C7: entry validation (SSRF guard) on the write path ----------

#[tokio::test]
async fn c7_put_provider_rejects_private_and_invalid_egress_entries() {
    let h = EgressAdminHarness::new(true).await;
    let client = reqwest::Client::new();

    for (label, pool) in [
        ("private", serde_json::json!(["http://10.0.0.5:8080"])),
        ("metadata", serde_json::json!(["http://169.254.169.254:80"])),
        ("bad-scheme", serde_json::json!(["ftp://example.com:21"])),
        ("no-host", serde_json::json!(["http://"])),
    ] {
        let resp = h
            .auth(
                client
                    .put(format!("http://{}/api/admin/providers/zen", h.addr))
                    .header("If-Match", "\"0\"")
                    .json(&serde_json::json!({ "egress_pool": pool })),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "PUT with {label} egress entry must be refused"
        );
        let err: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            err["error"]["code"], "egress_blocked",
            "refusal must use the egress_blocked code (got {err})"
        );
    }
}

// ---------- C7 red line: admin_write_enabled gate ----------

#[tokio::test]
async fn c7_admin_write_gate_still_guards_egress_writes() {
    let h = EgressAdminHarness::new(false).await;
    let client = reqwest::Client::new();
    let resp = h
        .auth(
            client
                .put(format!("http://{}/api/admin/providers/zen", h.addr))
                .header("If-Match", "\"0\"")
                .json(&serde_json::json!({ "egress_pool": ["direct"] })),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "writes must be gated when admin_write_enabled=false");
}

// ---------- C8: regression — no pool ⇒ exactly today's behavior ----------

/// Unit-level regression on the runtime config resolution: with no
/// `egress_pool` configured, the effective proxy resolution is byte-for-byte
/// today's `effective_proxy_url_for` (model > provider > gateway default,
/// `direct`/`none`/empty ⇒ `None`). This test compiles and passes against the
/// current code — it pins the zero-migration guarantee.
#[test]
fn c8_provider_without_egress_pool_keeps_legacy_proxy_behavior() {
    let mut cfg = GatewayConfig::default();
    let mut p = ProviderConfig::default();
    p.base_url = "https://api.example.com/v1".to_string();
    p.proxy = Some("http://127.0.0.1:8899".to_string());
    cfg.providers.insert("legacy".to_string(), p);

    // Provider proxy honored (unchanged).
    assert_eq!(
        cfg.effective_proxy_url_for("legacy", "any-model"),
        Some("http://127.0.0.1:8899".to_string())
    );
    // Explicit "direct"/"none" still resolve to no proxy.
    let mut direct_p = ProviderConfig::default();
    direct_p.base_url = "https://api.example.com/v1".to_string();
    direct_p.proxy = Some("direct".to_string());
    cfg.providers.insert("direct-p".to_string(), direct_p);
    assert_eq!(cfg.effective_proxy_url_for("direct-p", "m"), None);
    // Unknown provider inherits the gateway default (None here).
    assert_eq!(cfg.effective_proxy_url_for("nope", "m"), None);
}
