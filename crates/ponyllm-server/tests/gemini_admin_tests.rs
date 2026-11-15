//! Gemini provider Admin-plane acceptance tests.
//!
//! Locks the wiring added for `UpstreamProtocol::Gemini` on the admin surface:
//! - create/read round-trip of a provider with `default_protocol = "gemini"`
//!   (`ProviderView::default_protocol` must render the lowercase token the
//!   console sends back, otherwise every edit round-trip silently drops it);
//! - model creation with `protocol = "gemini"`;
//! - key creation (plaintext once, then masked) — Gemini keys are plain static
//!   API keys, no OAuth envelope;
//! - `GET /api/admin/providers/{name}/upstream-models` against a mock Gemini
//!   `GET {base}/models?pageSize=1000` upstream: `x-goog-api-key` auth (never
//!   Bearer), `models/` prefix stripped, ids sorted+deduped;
//! - auth-failure path answers a clear 502 JSON envelope (no panic, no 200 with
//!   an empty list);
//! - `POST /api/admin/keys/{id}/test` probes with `GET {base}/models?pageSize=1`
//!   + `x-goog-api-key`, and reports upstream status/body on rejection.
//!
//! No real network: the upstream is a local axum mock allowed through the
//! loopback probe hatch (`PONYLLM_ALLOW_LOOPBACK_PROBE=1`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State as AxumState;
use axum::http::{HeaderMap, Uri};
use axum::routing::get;
use axum::{Json, Router};
use ponyllm_config::ConfigFile;
use ponyllm_server::admin_store::{ConfigStore, FileConfigStore};
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;

const GOOD_KEY: &str = "gemini-good-key-abc123";

// ---------------------------------------------------------------------------
// Mock Gemini upstream: only `GET {base}/models` exists.
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct GeminiMock {
    /// `(raw query string, x-goog-api-key header)` per request, in order.
    seen: Arc<Mutex<Vec<(String, Option<String>)>>>,
}

async fn gemini_models_handler(
    AxumState(mock): AxumState<GeminiMock>,
    headers: HeaderMap,
    uri: Uri,
) -> (StatusCode, Json<Value>) {
    let key = headers
        .get("x-goog-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    mock.seen
        .lock()
        .unwrap()
        .push((uri.query().unwrap_or_default().to_string(), key.clone()));

    // Reject anything that is not the expected static key: this is what proves
    // the admin probe sends `x-goog-api-key` instead of a Bearer token.
    if key.as_deref() != Some(GOOD_KEY) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": {
                    "code": 401,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "UNAUTHENTICATED"
                }
            })),
        );
    }

    // Native Gemini listing shape: `models[].name` is a resource name.
    (
        StatusCode::OK,
        Json(json!({
            "models": [
                {"name": "models/gemini-3.8-flash", "displayName": "Gemini 3.8 Flash"},
                {"name": "models/gemini-3.1-pro", "displayName": "Gemini 3.1 Pro"},
                {"name": "models/gemini-3.8-flash", "displayName": "Gemini 3.8 Flash"}
            ]
        })),
    )
}

async fn spawn_gemini_mock(mock: GeminiMock) -> SocketAddr {
    let app = Router::new()
        .route("/v1beta/models", get(gemini_models_handler))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ---------------------------------------------------------------------------
// Harness: empty config store + admin writes enabled.
// ---------------------------------------------------------------------------

struct GeminiAdminHarness {
    addr: SocketAddr,
    state: Arc<AppState>,
    _temp_dir: TempDir,
    config_path: String,
    api_key: String,
}

impl GeminiAdminHarness {
    async fn new() -> Self {
        // Same hatch as the admin dial-test matrix: loopback mocks are
        // legitimate test traffic, private/metadata ranges stay blocked.
        std::env::set_var("PONYLLM_ALLOW_LOOPBACK_PROBE", "1");
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");
        let api_key = "admin-secret-token".to_string();

        let mut config_file = ConfigFile::default();
        config_file.gateway.bind = "127.0.0.1:8080".to_string();
        config_file.gateway.api_key = api_key.clone();
        config_file.gateway.web_enabled = true;
        config_file.gateway.admin_write_enabled = true;
        config_file.providers = HashMap::new();
        config_file.config_version = 0;
        config_file
            .save_to_path(config_path.to_str().unwrap())
            .unwrap();

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = api_key.clone();
        gw_config.web_enabled = true;
        gw_config.admin_write_enabled = true;

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

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

/// `If-Match` for the current config_version (optimistic concurrency).
fn if_match(ver: u64) -> String {
    format!("\"{}\"", ver)
}

// ---------------------------------------------------------------------------
// Main acceptance test: provider → list echo → model → key → upstream listing
// → key dial test, then the auth-failure path.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn gemini_provider_lifecycle_listing_and_dial_test() {
    let mock = GeminiMock::default();
    let upstream = spawn_gemini_mock(mock.clone()).await;
    let harness = GeminiAdminHarness::new().await;
    let client = reqwest::Client::new();
    let auth = harness.auth();
    let gemini_base = format!("http://{}/v1beta", upstream);

    // 1. Create the provider with default_protocol=gemini -> 201 + echo.
    let resp = client
        .post(harness.url("/api/admin/providers"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(0))
        .json(&json!({
            "name": "gemini",
            "base_url": gemini_base,
            "default_model": "gemini-3.8-flash",
            "strategy": "priority",
            "billing_mode": "metered",
            "default_protocol": "gemini"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created: Value = resp.json().await.unwrap();
    assert_eq!(created["name"], "gemini");
    assert_eq!(
        created["default_protocol"], "gemini",
        "create response must echo the gemini protocol: {created}"
    );
    assert_eq!(created["base_url"], gemini_base);
    assert_eq!(created["strategy"], "priority");

    // 2. GET /api/admin/providers echoes the same protocol (console round-trip).
    let resp = client
        .get(harness.url("/api/admin/providers"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let providers: Vec<Value> = resp.json().await.unwrap();
    let view = providers
        .iter()
        .find(|p| p["name"] == "gemini")
        .unwrap_or_else(|| panic!("gemini provider missing from list: {providers:?}"));
    assert_eq!(view["default_protocol"], "gemini");
    assert_eq!(view["strategy"], "priority");
    assert_eq!(view["base_url"], gemini_base);
    assert_eq!(view["billing_mode"], "Metered");

    // 3. Create a model bound to the gemini protocol -> 201 + protocol echo.
    let resp = client
        .post(harness.url("/api/admin/models"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(1))
        .json(&json!({
            "provider": "gemini",
            "name": "gemini-3.1-pro",
            "tier": "Flagship",
            "context_window": "1M",
            "input_types": ["text", "image"],
            "output_types": ["text"],
            "protocol": "gemini"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let model: Value = resp.json().await.unwrap();
    assert_eq!(model["provider"], "gemini");
    assert_eq!(model["name"], "gemini-3.1-pro");
    assert_eq!(model["protocol"], "gemini");
    assert_eq!(model["context_window"], "1M");
    assert_eq!(model["input_types"], json!(["text", "image"]));

    // 4. Create the static API key -> 201, plaintext returned once.
    let resp = client
        .post(harness.url("/api/admin/keys"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(2))
        .json(&json!({
            "provider": "gemini",
            "id": "gemini-key-1",
            "api_key": GOOD_KEY,
            "priority": 1,
            "weight": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-store");
    let key_view: Value = resp.json().await.unwrap();
    assert_eq!(key_view["id"], "gemini-key-1");
    assert_eq!(key_view["api_key"], GOOD_KEY);

    // 5. upstream-models: native Gemini listing, `models/` stripped, sorted,
    //    deduped, and pageSize=1000 on the wire.
    let resp = client
        .get(harness.url("/api/admin/providers/gemini/upstream-models"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listing: Value = resp.json().await.unwrap();
    assert_eq!(listing["provider"], "gemini");
    assert_eq!(listing["source"], "upstream");
    assert_eq!(
        listing["models"],
        json!([{"id": "gemini-3.1-pro"}, {"id": "gemini-3.8-flash"}]),
        "listing must strip the models/ prefix, dedupe and sort: {listing}"
    );
    {
        let seen = mock.seen.lock().unwrap();
        let (query, key) = seen.last().expect("listing must hit the mock upstream");
        assert_eq!(query, "pageSize=1000");
        assert_eq!(
            key.as_deref(),
            Some(GOOD_KEY),
            "Gemini listing must authenticate with x-goog-api-key"
        );
    }

    // 6. Key dial test: `GET {base}/models?pageSize=1` + x-goog-api-key -> ok.
    let resp = client
        .post(harness.url("/api/admin/keys/gemini-key-1/test"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dial: Value = resp.json().await.unwrap();
    assert_eq!(dial["success"], true, "dial test view: {dial}");
    assert_eq!(dial["http_status"], 200);
    assert!(dial["error_code"].is_null());
    assert_eq!(dial["message"], "probe ok");
    {
        let seen = mock.seen.lock().unwrap();
        let (query, key) = seen.last().expect("dial test must hit the mock upstream");
        assert_eq!(query, "pageSize=1");
        assert_eq!(key.as_deref(), Some(GOOD_KEY));
    }

    // 7. Auth-failure path: a provider whose key is rejected must answer a
    //    clear 502 JSON envelope (never 200 with an empty model list).
    let resp = client
        .post(harness.url("/api/admin/providers"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(3))
        .json(&json!({
            "name": "gemini-bad",
            "base_url": gemini_base,
            "default_model": "gemini-3.8-flash",
            "default_protocol": "gemini"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .post(harness.url("/api/admin/keys"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(4))
        .json(&json!({
            "provider": "gemini-bad",
            "id": "gemini-bad-key",
            "api_key": "definitely-not-the-key"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .get(harness.url("/api/admin/providers/gemini-bad/upstream-models"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "rejected Gemini key must surface as 502"
    );
    let err: Value = resp.json().await.unwrap();
    assert_eq!(err["error"]["code"], "upstream-auth-failed");
    let message = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("401"),
        "error message must carry the upstream status: {message}"
    );
    assert!(
        message.contains("API key not valid"),
        "error message must carry the upstream body snippet: {message}"
    );

    // 8. Same rejection through the dial test: 200 envelope, success=false,
    //    error_code=unauthorized, upstream message preserved.
    let resp = client
        .post(harness.url("/api/admin/keys/gemini-bad-key/test"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dial: Value = resp.json().await.unwrap();
    assert_eq!(dial["success"], false);
    assert_eq!(dial["http_status"], 401);
    assert_eq!(dial["error_code"], "unauthorized");
    assert!(
        dial["message"]
            .as_str()
            .unwrap_or_default()
            .contains("API key not valid"),
        "dial-test message must carry the upstream body: {dial}"
    );

    // 9. PUT round-trip: the console always sends default_protocol back, so the
    //    update path must parse and echo `gemini` (not silently drop it).
    let resp = client
        .put(harness.url("/api/admin/providers/gemini"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(5))
        .json(&json!({ "default_protocol": "gemini", "strategy": "priority" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["name"], "gemini");
    assert_eq!(
        updated["default_protocol"], "gemini",
        "PUT must echo the gemini protocol: {updated}"
    );

    // 10. The admin store really holds the gemini protocol (not just the view).
    let (stored, _ver) = FileConfigStore::new(&harness.config_path)
        .load()
        .await
        .unwrap();
    let stored_provider = &stored.providers["gemini"];
    assert_eq!(
        stored_provider.default_protocol,
        Some(ponyllm_core::pool::UpstreamProtocol::Gemini)
    );
    assert_eq!(stored_provider.keys.len(), 1);
    assert_eq!(stored_provider.keys[0].api_key, GOOD_KEY);
    assert!(stored_provider
        .model_configs
        .iter()
        .any(|m| m.name == "gemini-3.1-pro"
            && m.protocol == Some(ponyllm_core::pool::UpstreamProtocol::Gemini)));

    // 11. Hot state matches the store: the provider is routed as Gemini.
    let live = harness.state.config.read();
    assert_eq!(
        live.providers["gemini"].default_protocol,
        Some(ponyllm_core::pool::UpstreamProtocol::Gemini)
    );
}

// ---------------------------------------------------------------------------
// Provider with the gemini protocol but no key: no dial, clear 400 envelope.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn gemini_upstream_models_without_key_fails_fast() {
    let mock = GeminiMock::default();
    let upstream = spawn_gemini_mock(mock.clone()).await;
    let harness = GeminiAdminHarness::new().await;
    let client = reqwest::Client::new();
    let auth = harness.auth();

    let resp = client
        .post(harness.url("/api/admin/providers"))
        .header("Authorization", &auth)
        .header("If-Match", if_match(0))
        .json(&json!({
            "name": "gemini-nokey",
            "base_url": format!("http://{}/v1beta", upstream),
            "default_protocol": "gemini"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .get(harness.url("/api/admin/providers/gemini-nokey/upstream-models"))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    // 400 with a cause the operator can act on (a generic 404 would read as
    // "this provider has no listing interface", which is false for Gemini).
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err: Value = resp.json().await.unwrap();
    assert_eq!(err["error"]["code"], "provider_has_no_key");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no API key configured"),
        "message must name the cause: {err}"
    );
    assert!(
        mock.seen.lock().unwrap().is_empty(),
        "no key means no upstream dial"
    );
}
