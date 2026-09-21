//! Auth compatibility + scoped-key matrix tests (P1, task-21; contract
//! `auth-eval.md` §4 T1–T6, migration §4).
//!
//! 只增不改：旧断言文件一字不动，本文件独立覆盖三态×端点类、凭证形态、
//! 403 新码、名称空间隔离、失败体不泄漏。

use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{generate_scoped_gateway_key, hash_gateway_key, GatewayKeyEntry, KeyScope};
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;

struct AuthHarness {
    addr: SocketAddr,
    legacy: String,
    admin_plain: String,
    infer_plain: String,
    read_plain: String,
}

impl AuthHarness {
    async fn new(auth_compat: ponyllm_config::AuthCompat) -> Self {
        Self::new_with_entries(auth_compat, true).await
    }

    async fn new_with_entries(auth_compat: ponyllm_config::AuthCompat, with_scoped: bool) -> Self {
        let legacy = "legacy-master-token-abcdef1234567890".to_string();
        let (admin_plain, e_admin) = generate_scoped_gateway_key("adm-1", KeyScope::Admin);
        let (infer_plain, e_infer) = generate_scoped_gateway_key("agt-1", KeyScope::Inference);
        let (read_plain, e_read) = generate_scoped_gateway_key("view-1", KeyScope::Readonly);

        let mut config = GatewayConfig::default();
        config.bind_addr = "127.0.0.1:8080".to_string();
        config.api_key = legacy.clone();
        config.web_enabled = false;
        config.auth_compat = auth_compat;
        if with_scoped {
            config.gateway_keys = vec![e_admin, e_infer, e_read];
        }

        let state = Arc::new(AppState::new(config));
        let pool = Arc::new(ponyllm_core::pool::KeyPool::new(
            "prober",
            ponyllm_core::pool::RoutingStrategy::RoundRobin,
        ));
        state.register_pool("prober", pool);
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            legacy,
            admin_plain,
            infer_plain,
            read_plain,
        }
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {}", token)
}

// T1: compat tri-state × endpoint class (legacy token).
#[tokio::test]
async fn t1_legacy_token_dual_full_but_strict_dead() {
    let client = reqwest::Client::new();
    for (compat, expect_ok) in [
        (
            ponyllm_config::AuthCompat::LegacyOnly,
            true,
        ),
        (ponyllm_config::AuthCompat::Dual, true),
        (ponyllm_config::AuthCompat::Strict, false),
    ] {
        let h = AuthHarness::new(compat).await;
        // NOTE: /api/admin/overview needs a config store (503 without one);
        // use store-free endpoints so this file tests AUTH, not storage.
        for path in ["/api/admin/providers", "/api/admin/quota", "/v1/models"] {
            let resp = client
                .get(format!("http://{}{}", h.addr, path))
                .header("Authorization", bearer(&h.legacy))
                .send()
                .await
                .unwrap();
            if expect_ok {
                assert_eq!(resp.status(), StatusCode::OK, "{:?} {} must pass", compat, path);
            } else {
                assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{:?} {} must 401", compat, path);
                let body: serde_json::Value = resp.json().await.unwrap();
                assert_eq!(body["error"]["code"], "invalid_api_key");
                assert!(body["error"]["message"].as_str().unwrap().contains("scoped key"));
            }
        }
    }
}

// T2a: scoped admin survives strict; bare scoped token dies in strict.
#[tokio::test]
async fn t2_scoped_admin_survives_strict_bare_dies() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Strict).await;
    // Bearer scoped admin: full pass incl. rotate-adjacent read.
    let ok = client
        .get(format!("http://{}/api/admin/providers", h.addr))
        .header("Authorization", bearer(&h.admin_plain))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    // Bare scoped admin (no Bearer scheme): strict 401.
    let bare = client
        .get(format!("http://{}/api/admin/providers", h.addr))
        .header("Authorization", h.admin_plain.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), StatusCode::UNAUTHORIZED);
}

// T2b: role matrix — inference key: inference+quota 200, admin-read/write 403.
#[tokio::test]
async fn t2_inference_key_matrix_403_on_admin() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Dual).await;
    let auth = bearer(&h.infer_plain);
    // quota: allowed
    let q = client
        .get(format!("http://{}/api/admin/quota", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(q.status(), StatusCode::OK);
    // models (inference class): allowed
    let m = client
        .get(format!("http://{}/v1/models", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(m.status(), StatusCode::OK);
    // admin read: 403 with the NEW envelope (never 401).
    let r = client
        .get(format!("http://{}/api/admin/providers", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let rb: serde_json::Value = r.json().await.unwrap();
    assert_eq!(rb["error"]["code"], "forbidden");
    assert_eq!(rb["error"]["type"], "insufficient_scope");
    // admin write: 403 (authz before the write-gate 404 — order iron rule).
    let w = client
        .post(format!("http://{}/api/admin/auth/rotate", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(w.status(), StatusCode::FORBIDDEN);
}

// T2c: readonly key: admin-read 200, inference 403.
#[tokio::test]
async fn t2_readonly_key_reads_but_no_inference() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Dual).await;
    let auth = bearer(&h.read_plain);
    let r = client
        .get(format!("http://{}/api/admin/providers", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let m = client
        .get(format!("http://{}/v1/models", h.addr))
        .header("Authorization", auth.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(m.status(), StatusCode::FORBIDDEN);
    let mb: serde_json::Value = m.json().await.unwrap();
    assert_eq!(mb["error"]["code"], "forbidden");
}

// T2d: human-machine isolation — gateway (inference) token can never rotate.
#[tokio::test]
async fn t2_gateway_token_never_rotates() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Dual).await;
    for token in [&h.infer_plain, &h.read_plain] {
        let resp = client
            .post(format!("http://{}/api/admin/auth/rotate", h.addr))
            .header("Authorization", bearer(token))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "rotate is admin-only");
    }
}

// T5a: namespace isolation — scoped-prefix secret never falls to legacy;
// upstream-style secrets stay invalid.
#[tokio::test]
async fn t5_namespaces_never_cross() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Dual).await;
    // right prefix, wrong secret -> 401, not legacy pass
    let r = client
        .get(format!("http://{}/api/admin/quota", h.addr))
        .header("Authorization", "Bearer sk-pony-infer-wrongsecret0000")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // legacy value with a scoped prefix must not verify as scoped either
    let r2 = client
        .get(format!("http://{}/api/admin/quota", h.addr))
        .header("Authorization", bearer("sk-pony-admin-unknownvalue0000"))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::UNAUTHORIZED);
}

// T6: failure bodies never echo the credential; revoked entries fail closed.
#[tokio::test]
async fn t6_no_echo_and_revoked_closed() {
    let client = reqwest::Client::new();
    let h = AuthHarness::new(ponyllm_config::AuthCompat::Dual).await;
    let secret = "Bearer sk-pony-infer-echo-probe-secret-zzz";
    let resp = client
        .get(format!("http://{}/api/admin/providers", h.addr))
        .header("Authorization", secret)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let raw = resp.text().await.unwrap();
    assert!(!raw.contains("echo-probe-secret"), "credential echoed: {raw}");

    // revoked entry: build a second harness via hash round-trip
    let (plain, mut entry): (String, GatewayKeyEntry) =
        generate_scoped_gateway_key("rev-1", KeyScope::Inference);
    entry.revoked = true;
    assert_eq!(hash_gateway_key(&entry.salt, &plain), entry.key_hash);
    let mut config = GatewayConfig::default();
    config.api_key = "another-legacy-token-1234567890abcdef".to_string();
    config.web_enabled = false;
    config.gateway_keys = vec![entry];
    let state = Arc::new(AppState::new(config));
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let r = client
        .get(format!("http://{}/api/admin/quota", addr))
        .header("Authorization", bearer(&plain))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
