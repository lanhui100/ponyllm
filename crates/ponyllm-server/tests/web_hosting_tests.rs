//! WEB-01 `serve` web-console hosting contract (acceptance 4).
//!
//! - dist present  → `/app/dashboard` deep-link serves `index.html` (200, html),
//!   real asset served with its content-type, `/v1/models` NOT swallowed,
//!   static needs no auth token, `..` escape contained (404).
//! - dist missing   → `/app` + `/app/*` deterministic 503 JSON (`web_dist_missing`),
//!   gateway routes (`/health`, `/v1/models`-shape) unaffected.
//! - `--no-web`     → `/app` + `/app/*` 404 JSON (`web_disabled`), gateway unaffected.

use std::sync::Arc;
use ponyllm_server::{create_app, AppState, GatewayConfig};

fn test_config_with_web(web_enabled: bool, web_dist_dir: &str) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    // Empty api_key => auth_middleware open mode: no token needed for API probes.
    config.api_key = String::new();
    config.web_enabled = web_enabled;
    config.web_dist_dir = web_dist_dir.to_string();
    config
}

async fn spawn_gateway(config: GatewayConfig) -> std::net::SocketAddr {
    let state = Arc::new(AppState::new(config));
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn write_fake_dist(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("index.html"), "<html>pony console</html>").unwrap();
    std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
}

/// Minimal valid ICO (single 16x16 PNG-compressed entry), produced by
/// scripts/gen-favicon.sh's pipeline; used to assert the `/favicon.ico` route
/// serves a REAL ICO (never SVG bytes mislabeled as ICO).
const FAKE_ICO: &[u8] = b"\x00\x00\x01\x00\x01\x00\x10\x10\x00\x00\x00\x00\
\x20\x00\x56\x00\x00\x00\x16\x00\x00\x00\x89\x50\
\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\
\x44\x52\x00\x00\x00\x10\x00\x00\x00\x10\x08\x06\
\x00\x00\x00\x1f\xf3\xff\x61\x00\x00\x00\x1d\x49\
\x44\x41\x54\x78\x9c\x63\xe4\x17\xd7\xfa\xcf\x40\
\x01\x60\xa2\x44\xf3\xa8\x01\xa3\x06\x8c\x1a\x30\
\x98\x0c\x00\x00\x91\x94\x01\x6f\x85\x33\xe4\xf0\
\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";

const FAVICON_SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 32 32\"><rect width=\"32\" height=\"32\" fill=\"#0F172A\"/></svg>";

fn write_favicon_dist(dir: &std::path::Path, with_ico: bool) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("index.html"), "<html>pony console</html>").unwrap();
    std::fs::write(dir.join("favicon.svg"), FAVICON_SVG).unwrap();
    if with_ico {
        std::fs::write(dir.join("favicon.ico"), FAKE_ICO).unwrap();
    }
}

fn assert_cache_control(resp: &reqwest::Response) {
    assert_eq!(
        resp.headers().get("cache-control").unwrap().to_str().unwrap(),
        "public, max-age=86400"
    );
}

#[tokio::test]
async fn web_hosting_serves_spa_and_keeps_api_priority() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_dist(tmp.path());
    let addr = spawn_gateway(test_config_with_web(
        true,
        tmp.path().to_str().unwrap(),
    ))
    .await;
    let client = reqwest::Client::new();

    // Deep-link refresh serves index.html.
    let deep = client
        .get(format!("http://{}/app/dashboard", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(deep.status(), 200);
    let ct = deep
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "unexpected content-type: {ct}");
    let body = deep.text().await.unwrap();
    assert!(body.contains("pony console"));

    // Root direct access (http://127.0.0.1:port/) serves index.html directly.
    for root_path in ["/", "/dashboard", "/connect", "/recorder", "/governance"] {
        let resp = client
            .get(format!("http://{}{}", addr, root_path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "Path {} failed to return 200", root_path);
        let r_body = resp.text().await.unwrap();
        assert!(r_body.contains("pony console"));
    }

    // Real asset keeps its content-type.
    let asset = client
        .get(format!("http://{}/app/app.js", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    let act = asset
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        act.contains("javascript"),
        "unexpected asset content-type: {act}"
    );

    // API routes are NOT swallowed by the SPA fallback.
    let api = client
        .get(format!("http://{}/v1/models", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(api.status(), 200);
    let api_body: serde_json::Value = api.json().await.unwrap();
    assert_eq!(api_body["object"], "list");

    // `..` escape is contained (404, no file outside dist leaks).
    let escape = client
        .get(format!("http://{}/app/../Cargo.toml", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(escape.status(), 404);

    // Bare prefix paths serve the SPA entry (ServeDir maps the empty remainder
    // to the directory; append_index(false) 404s and the fallback serves index).
    for path in ["/app", "/app/"] {
        let resp = client
            .get(format!("http://{}{}", addr, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "path: {path}");
        let text = resp.text().await.unwrap();
        assert!(text.contains("pony console"), "path: {path}");
    }
}

#[tokio::test]
async fn web_hosting_missing_dist_is_deterministic_503() {
    let missing = std::env::temp_dir().join("ponyllm-web-dist-definitely-missing");
    let _ = std::fs::remove_dir_all(&missing);
    let addr = spawn_gateway(test_config_with_web(
        true,
        missing.to_str().unwrap(),
    ))
    .await;
    let client = reqwest::Client::new();

    for path in ["/app", "/app/", "/app/dashboard"] {
        let resp = client
            .get(format!("http://{}{}", addr, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 503, "path: {path}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "web_dist_missing", "path: {path}");
    }

    // Gateway forwarding chain unaffected: health + models shape still answer.
    let health = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let models = client
        .get(format!("http://{}/v1/models", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), 200);
}

#[tokio::test]
async fn web_hosting_no_web_flag_disables_mount() {
    let addr = spawn_gateway(test_config_with_web(false, "web/dist")).await;
    let client = reqwest::Client::new();

    for path in ["/app", "/app/", "/app/dashboard"] {
        let resp = client
            .get(format!("http://{}{}", addr, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "path: {path}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "web_disabled", "path: {path}");
    }

    let health = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
}

/// Secured mode (api_key set): static `/app/*` serves WITHOUT any token while
/// auth-covered API routes reject tokenless callers with 401. This is the core
/// "static bypasses auth, API does not" property (reviewer-A missing test 1).
#[tokio::test]
async fn web_hosting_secured_static_bypass_but_api_guarded() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_dist(tmp.path());
    let mut config = test_config_with_web(true, tmp.path().to_str().unwrap());
    config.api_key = "sk-pony-secured-test".to_string();
    let addr = spawn_gateway(config).await;
    let client = reqwest::Client::new();

    // Static: no token, 200.
    let page = client
        .get(format!("http://{}/app/dashboard", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let asset = client
        .get(format!("http://{}/app/app.js", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);

    // API: no token => 401 with machine code; wrong token => 401; right token => 200.
    let anon = client
        .get(format!("http://{}/v1/models", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
    let anon_body: serde_json::Value = anon.json().await.unwrap();
    assert_eq!(anon_body["error"]["code"], "invalid_api_key");

    let wrong = client
        .get(format!("http://{}/v1/models", addr))
        .header("Authorization", "Bearer wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    let authed = client
        .get(format!("http://{}/v1/models", addr))
        .header("Authorization", "Bearer sk-pony-secured-test")
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 200);
}

/// Favicon contract: real ICO served with `image/x-icon` + explicit cache
/// header; `/favicon.svg` serves SVG with `image/svg+xml`; the `?v=` cache-bust
/// query does not change routing.
#[tokio::test]
async fn web_hosting_favicon_real_ico_and_cache_headers() {
    let tmp = tempfile::tempdir().unwrap();
    write_favicon_dist(tmp.path(), true);
    let addr = spawn_gateway(test_config_with_web(true, tmp.path().to_str().unwrap())).await;
    let client = reqwest::Client::new();

    let svg = client
        .get(format!("http://{}/favicon.svg", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(svg.status(), 200);
    assert!(svg
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("image/svg+xml"));
    assert_cache_control(&svg);
    assert_eq!(svg.bytes().await.unwrap().as_ref(), FAVICON_SVG);

    // Versioned URL (index.html links carry ?v=) must resolve identically.
    let versioned = client
        .get(format!("http://{}/favicon.svg?v=2", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(versioned.status(), 200);

    // HEAD follows the same route (tower-http ServeFile handles it) with headers.
    let head = client
        .head(format!("http://{}/favicon.svg", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_cache_control(&head);

    let ico = client
        .get(format!("http://{}/favicon.ico", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(ico.status(), 200);
    assert!(ico
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("image/x-icon"));
    assert_cache_control(&ico);
    let body = ico.bytes().await.unwrap();
    assert_eq!(&body[..4], b"\x00\x00\x01\x00", "/favicon.ico must be a real ICO");
    assert_eq!(body.as_ref(), FAKE_ICO);

    // Versioned .ico URL too.
    let ico_versioned = client
        .get(format!("http://{}/favicon.ico?v=2", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(ico_versioned.status(), 200);
    assert!(ico_versioned
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("image/x-icon"));
}

/// Legacy dists (no favicon.ico shipped): `/favicon.ico` falls back to the SVG
/// bytes so the implicit browser request never 404s (old behavior preserved).
#[tokio::test]
async fn web_hosting_favicon_fallback_svg_when_no_ico() {
    let tmp = tempfile::tempdir().unwrap();
    write_favicon_dist(tmp.path(), false);
    let addr = spawn_gateway(test_config_with_web(true, tmp.path().to_str().unwrap())).await;
    let client = reqwest::Client::new();

    let ico = client
        .get(format!("http://{}/favicon.ico", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(ico.status(), 200);
    assert!(ico
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("image/svg+xml"));
    assert_cache_control(&ico);
    assert_eq!(ico.bytes().await.unwrap().as_ref(), FAVICON_SVG);
}

/// No favicon shipped at all: routes are unregistered and browsers get a plain
/// 404 (never the SPA fallback, never an auth error).
#[tokio::test]
async fn web_hosting_favicon_missing_is_404() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_dist(tmp.path());
    let addr = spawn_gateway(test_config_with_web(true, tmp.path().to_str().unwrap())).await;
    let client = reqwest::Client::new();

    for path in ["/favicon.svg", "/favicon.ico"] {
        let resp = client
            .get(format!("http://{}{}", addr, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "path: {path}");
    }
}
