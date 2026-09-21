//! P0 auth hardening tests (task-20; contract `.agents/notes/auth-eval.md` §3–§4).
//!
//! Additive-only: existing assertions in other test files are untouched.
//! Covers: `auth_compat` default/parse, open+non-loopback fail-fast,
//! weak-key guard, strict bare-token rejection, OpenAPI bearer declaration.

use ponyllm_config::{validate_bind_auth_combo, validate_gateway_key_strength, AuthCompat};
use ponyllm_server::GatewayConfig;

#[test]
fn p0_auth_compat_defaults_to_dual() {
    // Old configs without the field deserialize to `dual` (zero-migration).
    let cfg: GatewayConfig = serde_json::from_value(serde_json::json!({
        "bind_addr": "127.0.0.1:8080",
        "api_key": "sk-test",
        "providers": {},
        "max_retries": 3,
        "flight_recorder_capacity": 100
    }))
    .unwrap();
    assert_eq!(cfg.auth_compat, AuthCompat::Dual);
}

#[test]
fn p0_auth_compat_roundtrips_all_modes() {
    for (raw, want) in [
        ("legacy-only", AuthCompat::LegacyOnly),
        ("dual", AuthCompat::Dual),
        ("strict", AuthCompat::Strict),
    ] {
        let cfg: GatewayConfig = serde_json::from_value(serde_json::json!({
            "bind_addr": "127.0.0.1:8080",
            "api_key": "sk-test",
            "providers": {},
            "max_retries": 3,
            "flight_recorder_capacity": 100,
            "auth_compat": raw
        }))
        .unwrap();
        assert_eq!(cfg.auth_compat, want);
    }
}

#[test]
fn p0_auth_compat_unknown_value_fails_fast() {
    // Contract §3.1: unknown auth segments must fail-fast, never silently default.
    let res: Result<GatewayConfig, _> = serde_json::from_value(serde_json::json!({
        "bind_addr": "127.0.0.1:8080",
        "api_key": "sk-test",
        "providers": {},
        "max_retries": 3,
        "flight_recorder_capacity": 100,
        "auth_compat": "super-secure-v9"
    }));
    assert!(res.is_err(), "unknown auth_compat must fail-fast");
}

#[test]
fn p0_open_mode_non_loopback_refused() {
    // Open (empty key) on 0.0.0.0 refuses to start in every compat mode.
    for mode in [AuthCompat::LegacyOnly, AuthCompat::Dual, AuthCompat::Strict] {
        assert!(
            validate_bind_auth_combo("0.0.0.0:8080", "", mode).is_err(),
            "open+0.0.0.0 must refuse ({:?})",
            mode
        );
        assert!(
            validate_bind_auth_combo("0.0.0.0:8080", "none", mode).is_err(),
            "open(none)+0.0.0.0 must refuse ({:?})",
            mode
        );
        // Loopback stays allowed (local dev zero friction).
        assert!(validate_bind_auth_combo("127.0.0.1:8080", "", mode).is_ok());
        assert!(validate_bind_auth_combo("127.0.0.1:8080", "none", mode).is_ok());
        // Secured key on any bind is fine.
        assert!(validate_bind_auth_combo("0.0.0.0:8080", "sk-pony-secret-16chars-ok", mode).is_ok());
    }
}

#[test]
fn p0_weak_gateway_key_rejected() {
    assert!(validate_gateway_key_strength("123456").is_err());
    assert!(validate_gateway_key_strength("password").is_err());
    assert!(validate_gateway_key_strength("password12345678").is_err());
    assert!(validate_gateway_key_strength("short").is_err());
    assert!(validate_gateway_key_strength("1234567890123456").is_err());
    // A long key containing a weak substring as a random run still passes.
    assert!(validate_gateway_key_strength("sk-pony-0123456789abcdef0123456789ab").is_ok());
    assert!(validate_gateway_key_strength("sk-pony-9f8e7d6c5b4a7081a9f8e7d6c5b4a70").is_ok());
}

#[test]
fn p0_openapi_declares_bearer_security() {
    let doc = ponyllm_server::routes::admin::openapi_json();
    // Global security requirement references bearerAuth.
    let security = &doc["security"];
    assert!(security.is_array(), "openapi must declare global security");
    assert!(
        security
            .as_array()
            .unwrap()
            .iter()
            .any(|req| req.get("bearerAuth").is_some()),
        "global security must reference bearerAuth"
    );
    // Scheme is HTTP bearer.
    let scheme = &doc["components"]["securitySchemes"]["bearerAuth"];
    assert_eq!(scheme["type"], "http");
    assert_eq!(scheme["scheme"], "bearer");
    // Rotate documents 401/404 (order: auth before gate).
    let rotate_resps = &doc["paths"]["/api/admin/auth/rotate"]["post"]["responses"];
    assert!(rotate_resps.get("401").is_some(), "rotate must document 401");
    assert!(rotate_resps.get("404").is_some(), "rotate must document 404");
}

// ---------------------------------------------------------------------------
// Live-gateway behavior: strict rejects bare tokens, dual accepts them.
// ---------------------------------------------------------------------------

async fn spawn_gateway(auth_compat: AuthCompat, api_key: &str) -> (String, tokio::task::JoinHandle<()>) {
    use ponyllm_core::pool::GatewayRoutingStrategy;
    use ponyllm_server::{AppState, GatewayConfig};
    use std::sync::Arc;

    let mut cfg = GatewayConfig::default();
    cfg.bind_addr = "127.0.0.1:0".to_string();
    cfg.api_key = api_key.to_string();
    cfg.auth_compat = auth_compat;
    let state = Arc::new(AppState::new(cfg));
    let app = ponyllm_server::create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Let the loopback hatch see GatewayRoutingStrategy used (avoid unused import in some cfgs).
    let _ = GatewayRoutingStrategy::Economy;
    (addr, handle)
}

#[tokio::test]
async fn p0_strict_rejects_bare_token_dual_accepts() {
    // P1 evolution (contract §3.3): in `strict` the LEGACY token is disabled
    // entirely (LegacyDisabled → 401 in every form); `dual` keeps full compat.
    // Scoped-key strict survival is covered by auth_compat_tests T2.
    let client = reqwest::Client::new();
    let key = "sk-pony-strict-bare-probe-0001";

    for (mode, legacy_ok) in [(AuthCompat::Dual, true), (AuthCompat::Strict, false)] {
        let (addr, handle) = spawn_gateway(mode, key).await;
        let url = format!("http://{}/v1/models", addr);

        // Bearer legacy form: passes in dual, 401 in strict (re-issue guidance).
        let bearer = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .unwrap();
        if legacy_ok {
            assert_eq!(bearer.status(), 200, "bearer must pass in {:?}", mode);
        } else {
            assert_eq!(bearer.status(), 401, "legacy bearer must 401 in {:?}", mode);
            let body: serde_json::Value = bearer.json().await.unwrap();
            assert_eq!(body["error"]["code"], "invalid_api_key");
            assert!(body["error"]["message"].as_str().unwrap().contains("scoped key"));
        }

        // Bare token (no `Bearer ` scheme): dual accepts, strict 401.
        let bare = client
            .get(&url)
            .header("Authorization", key)
            .send()
            .await
            .unwrap();
        if legacy_ok {
            assert_eq!(bare.status(), 200, "dual must accept bare token");
        } else {
            assert_eq!(bare.status(), 401, "strict must reject bare token");
            let body: serde_json::Value = bare.json().await.unwrap();
            assert_eq!(body["error"]["code"], "invalid_api_key");
        }

        // x-api-key keeps equal rights with Bearer in dual; in strict the
        // legacy value is disabled in every form (contract §3.3).
        let xkey = client
            .get(&url)
            .header("x-api-key", key)
            .send()
            .await
            .unwrap();
        if legacy_ok {
            assert_eq!(xkey.status(), 200, "x-api-key must pass in {:?}", mode);
        } else {
            assert_eq!(xkey.status(), 401, "legacy x-api-key must 401 in {:?}", mode);
        }

        handle.abort();
    }
}
