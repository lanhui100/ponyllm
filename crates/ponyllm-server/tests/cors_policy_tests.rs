//! H6 regression: CORS is same-origin-only by default.
//!
//! - Evil-origin preflight gets NO `access-control-allow-origin` (browser
//!   blocks the cross-site call even with a stolen token).
//! - `PONYLLM_CORS_ALLOWLIST` opts named origins back in.
//! - Allowed methods/headers are a fixed minimal set (no `*`).

use std::sync::{Arc, Mutex, OnceLock};
use ponyllm_server::app::create_app;
use ponyllm_server::state::AppState;
use ponyllm_server::GatewayConfig;

/// Process env is global to the test binary: serialize every app build so
/// an allowlist set/remove in one test can never interleave with another
/// test's build (cargo runs tests in one binary on multiple threads).
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn spawn_app() -> std::net::SocketAddr {
    spawn_app_with_allowlist_inner(None).await
}

/// Build an app with a one-shot env override. `create_app` reads env
/// synchronously under a process-wide mutex, so concurrent tests cannot
/// observe each other's allowlist.
async fn spawn_app_with_allowlist(value: &str) -> std::net::SocketAddr {
    spawn_app_with_allowlist_inner(Some(value)).await
}

async fn spawn_app_with_allowlist_inner(value: Option<&str>) -> std::net::SocketAddr {
    // Hold the guard across build only (no .await inside critical section:
    // build the Router first, then bind).
    let app = {
        let _guard = env_lock().lock().unwrap();
        // Defensive: make sure no leaked allowlist weakens the default-deny
        // assertions below.
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        if let Some(v) = value {
            std::env::set_var("PONYLLM_CORS_ALLOWLIST", v);
        }
        let mut config = GatewayConfig::default();
        config.api_key = "test-token".to_string();
        let state = Arc::new(AppState::new(config));
        let app = create_app(state);
        std::env::remove_var("PONYLLM_CORS_ALLOWLIST");
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn preflight(addr: &std::net::SocketAddr, origin: &str) -> reqwest::Response {
    reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{}/v1/models", addr),
        )
        .header("Origin", origin)
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "authorization")
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn test_evil_origin_gets_no_allow_header_by_default() {
    // Default test env: PONYLLM_CORS_ALLOWLIST unset -> same-origin-only.
    let addr = spawn_app().await;
    let resp = preflight(&addr, "https://evil.example").await;
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "evil origin must not be allowlisted by default"
    );
}

#[tokio::test]
async fn test_simple_get_evil_origin_has_no_allow_header() {
    // Non-preflight cross-origin GET is equally unreadable without ACAO.
    let addr = spawn_app().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/v1/models", addr))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "simple cross-origin GET must not carry ACAO by default"
    );
}

#[tokio::test]
async fn test_x_api_key_preflight_allowed_for_listed_origin() {
    // PONYLLM_CORS_ALLOWLIST opts a named origin back in; the second auth
    // header (x-api-key) must preflight cleanly there (H6 red-team).
    let addr = spawn_app_with_allowlist("https://console.example.com").await;

    let resp = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{}/v1/models", addr),
        )
        .header("Origin", "https://console.example.com")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "x-api-key")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://console.example.com")
    );

    // ...while an unlisted origin on the same app instance stays denied.
    let evil = preflight(&addr, "https://evil.example").await;
    assert!(evil
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn test_security_headers_present() {
    let addr = spawn_app().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    let h = resp.headers();
    assert_eq!(h.get("x-frame-options").unwrap(), "SAMEORIGIN");
    assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
    assert!(h
        .get("permissions-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("camera=()"));
}
