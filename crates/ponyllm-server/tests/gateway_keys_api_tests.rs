//! Gateway credential management API tests (task-27; contract
//! `.agents/notes/web-users-api.md`).
//!
//! 只增不改：既有断言文件一字不动。覆盖三端点的门禁链（401 → 404 门控 →
//! 412 → 403）、一次性明文与零泄漏、last4 识别、吊销幂等、作用域矩阵。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use ponyllm_config::{ConfigFile, GatewayKeyEntry, KeyScope};
use ponyllm_core::pool::{BillingMode, GatewayRoutingStrategy};
use ponyllm_server::admin_store::FileConfigStore;
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;

struct Gh {
    addr: SocketAddr,
    admin: String,
    infer: String,
    readonly: String,
    admin_write: bool,
}

impl Gh {
    async fn new(admin_write: bool) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("ponyllm.toml");

        let (admin_plain, e_admin) =
            ponyllm_config::generate_scoped_gateway_key("boot-admin", KeyScope::Admin);
        let (infer_plain, e_infer) =
            ponyllm_config::generate_scoped_gateway_key("agent-x", KeyScope::Inference);
        let (read_plain, e_read) =
            ponyllm_config::generate_scoped_gateway_key("view-x", KeyScope::Readonly);

        let mut config_file = ConfigFile::default();
        config_file.gateway.bind = "127.0.0.1:8080".to_string();
        config_file.gateway.api_key = "legacy-gh-token-abcdef1234567890".to_string();
        config_file.gateway.admin_write_enabled = admin_write;
        config_file.gateway.gateway_keys =
            vec![e_admin.clone(), e_infer.clone(), e_read.clone()];
        config_file.providers = HashMap::new();
        config_file.save_to_path(config_path.to_str().unwrap()).unwrap();

        let mut gw = GatewayConfig::default();
        gw.bind_addr = "127.0.0.1:8080".to_string();
        gw.api_key = "legacy-gh-token-abcdef1234567890".to_string();
        gw.web_enabled = false;
        gw.admin_write_enabled = admin_write;
        gw.default_strategy = GatewayRoutingStrategy::Economy;
        gw.providers = HashMap::new();
        let _ = BillingMode::Metered;
        gw.gateway_keys = vec![e_admin, e_infer, e_read];

        let store = Arc::new(FileConfigStore::new(config_path.to_str().unwrap()));
        std::mem::forget(temp_dir);
        let state = Arc::new(AppState::new(gw).with_config_store(store));
        let app = create_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            admin: admin_plain,
            infer: infer_plain,
            readonly: read_plain,
            admin_write,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn bearer(t: &str) -> String {
    format!("Bearer {}", t)
}

/// Full happy path: list → issue (plaintext once) → list shows last4 → revoke
/// → revoked key 401 → re-revoke idempotent.
#[tokio::test]
async fn gateway_keys_issue_list_revoke_roundtrip() {
    let h = Gh::new(true).await;
    let c = reqwest::Client::new();

    // list: only the two boot keys, no hash/plaintext anywhere.
    let list: serde_json::Value = c
        .get(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 3, "3 boot keys, none issued yet: {list}");
    let raw = serde_json::to_string(&list).unwrap();
    assert!(!raw.contains("key_hash"), "hash must never be returned");
    assert!(!raw.contains("\"salt\""), "salt must never be returned");
    // NOTE: `prefix` (e.g. "sk-pony-infer-") is a frozen public identifier and
    // IS returned by design; the secret is the 32-hex suffix, so assert the
    // row shape instead of grepping for the prefix.
    for row in arr {
        let obj = row.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        for k in &keys {
            assert!(
                ["id", "scope", "prefix", "last4", "revoked", "expires_at", "config_version"]
                    .contains(k),
                "unexpected field '{k}' in credential list row"
            );
        }
        // last4 is exactly the public tail; a full plaintext would be 32 hex.
        let last4 = obj["last4"].as_str().unwrap();
        assert_eq!(last4.len(), 4, "last4 must be 4 chars, got {last4:?}");
    }

    // issue: 201 + one-time plaintext + no-store.
    let resp = c
        .post(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.admin))
        .header("If-Match", "0")
        .json(&serde_json::json!({"id": "ci-agent-1", "scope": "inference"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-store")
    );
    let issued: serde_json::Value = resp.json().await.unwrap();
    let plain = issued["api_key"].as_str().unwrap().to_string();
    assert!(plain.starts_with("sk-pony-infer-"));
    assert_eq!(issued["scope"], "inference");

    // the freshly issued key works immediately (memory mirror).
    let ok = c
        .get(h.url("/api/admin/quota"))
        .header("Authorization", bearer(&plain))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK, "issued key must work at once");

    // list now shows it with last4 identification, still no secrets.
    let list2: serde_json::Value = c
        .get(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = list2
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "ci-agent-1")
        .expect("issued key listed");
    assert_eq!(row["last4"], &plain[plain.len() - 4..]);
    assert_eq!(row["revoked"], false);
    assert!(list2.as_array().unwrap().len() >= 4);

    // revoke: 200, the entry is DELETED (hard delete since 2026-09-21),
    // and the key dies immediately (401).
    let ver = row["config_version"].as_i64().unwrap();
    let rev = c
        .post(h.url("/api/admin/gateway-keys/ci-agent-1/revoke"))
        .header("Authorization", bearer(&h.admin))
        .header("If-Match", ver.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(rev.status(), StatusCode::OK);
    let rev_body: serde_json::Value = rev.json().await.unwrap();
    assert_eq!(rev_body["id"], "ci-agent-1");

    // The deleted id is absent from the list: no tombstone, no trace.
    let list3: serde_json::Value = c
        .get(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list3.as_array().unwrap().iter().all(|v| v["id"] != "ci-agent-1"),
        "deleted key must vanish from the list: {list3}"
    );

    let dead = c
        .get(h.url("/api/admin/quota"))
        .header("Authorization", bearer(&plain))
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), StatusCode::UNAUTHORIZED, "deleted key fail-closed");

    // Deleting twice is 404 (nothing left to be idempotent over).
    let again = c
        .post(h.url("/api/admin/gateway-keys/ci-agent-1/revoke"))
        .header("Authorization", bearer(&h.admin))
        .header("If-Match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    let ab: serde_json::Value = again.json().await.unwrap();
    assert_eq!(ab["error"]["code"], "gateway_key_not_found");

    // The freed id may be re-issued later.
    let reissue = c
        .post(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.admin))
        .header("If-Match", "*")
        .json(&serde_json::json!({"id": "ci-agent-1", "scope": "inference"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reissue.status(), StatusCode::CREATED);

    // unknown id -> 404 gateway_key_not_found (distinct from gate 404).
    let unknown = c
        .post(h.url("/api/admin/gateway-keys/nope/revoke"))
        .header("Authorization", bearer(&h.admin))
        .header("If-Match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let ub: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(ub["error"]["code"], "gateway_key_not_found");
}

/// Scope matrix: list is AdminRead (readonly ok, inference 403);
/// issue/revoke are AdminWrite (both 403).
#[tokio::test]
async fn gateway_keys_scope_matrix() {
    let h = Gh::new(true).await;
    let c = reqwest::Client::new();

    // readonly can list.
    let ro = c
        .get(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.readonly))
        .send()
        .await
        .unwrap();
    assert_eq!(ro.status(), StatusCode::OK, "readonly lists credentials");

    // inference cannot even list (no admin-read).
    let inf = c
        .get(h.url("/api/admin/gateway-keys"))
        .header("Authorization", bearer(&h.infer))
        .send()
        .await
        .unwrap();
    assert_eq!(inf.status(), StatusCode::FORBIDDEN);
    let ib: serde_json::Value = inf.json().await.unwrap();
    assert_eq!(ib["error"]["code"], "forbidden");

    // issue: readonly + inference both 403.
    for (who, token) in [("readonly", &h.readonly), ("inference", &h.infer)] {
        let resp = c
            .post(h.url("/api/admin/gateway-keys"))
            .header("Authorization", bearer(token))
            .header("If-Match", "*")
            .json(&serde_json::json!({"id": format!("nope-{who}"), "scope": "readonly"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{who} must not issue");
    }

    // revoke: readonly 403.
    let rev = c
        .post(h.url("/api/admin/gateway-keys/boot-admin/revoke"))
        .header("Authorization", bearer(&h.readonly))
        .header("If-Match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(rev.status(), StatusCode::FORBIDDEN);

    // unauthenticated: 401.
    let anon = c.get(h.url("/api/admin/gateway-keys")).send().await.unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
}

/// Validation + concurrency: bad scope/id/expiry 400, duplicate 409,
/// missing/wrong If-Match 412.
#[tokio::test]
async fn gateway_keys_validation_and_if_match() {
    let h = Gh::new(true).await;
    let c = reqwest::Client::new();
    let post = |body: serde_json::Value, ver: Option<&str>| {
        let mut r = c
            .post(h.url("/api/admin/gateway-keys"))
            .header("Authorization", bearer(&h.admin))
            .json(&body);
        if let Some(v) = ver {
            r = r.header("If-Match", v);
        }
        r.send()
    };

    // missing If-Match -> 412
    let no_match = post(serde_json::json!({"id": "a", "scope": "admin"}), None)
        .await
        .unwrap();
    assert_eq!(no_match.status(), StatusCode::PRECONDITION_FAILED);

    // unknown scope -> 400 invalid_scope
    let bad_scope = post(serde_json::json!({"id": "a", "scope": "superuser"}), Some("*"))
        .await
        .unwrap();
    assert_eq!(bad_scope.status(), StatusCode::BAD_REQUEST);
    let bs: serde_json::Value = bad_scope.json().await.unwrap();
    assert_eq!(bs["error"]["code"], "invalid_scope");

    // empty id -> 400 invalid_key_id
    let bad_id = post(serde_json::json!({"id": "   ", "scope": "admin"}), Some("*"))
        .await
        .unwrap();
    assert_eq!(bad_id.status(), StatusCode::BAD_REQUEST);

    // past expiry -> 400 invalid_expiry
    let past = post(
        serde_json::json!({"id": "a", "scope": "admin", "expires_at": 1}),
        Some("*"),
    )
    .await
    .unwrap();
    assert_eq!(past.status(), StatusCode::BAD_REQUEST);
    let pb: serde_json::Value = past.json().await.unwrap();
    assert_eq!(pb["error"]["code"], "invalid_expiry");

    // duplicate id (boot-admin exists) -> 409
    let dup = post(serde_json::json!({"id": "boot-admin", "scope": "admin"}), Some("*"))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);
    let db: serde_json::Value = dup.json().await.unwrap();
    assert_eq!(db["error"]["code"], "gateway_key_already_exists");

    // wrong If-Match -> 412
    let stale = post(serde_json::json!({"id": "fresh-1", "scope": "readonly"}), Some("9999"))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
}

/// Gate: admin_write_enabled=false closes the whole surface with 404
/// (read list included, matching GET /api/admin/keys).
#[tokio::test]
async fn gateway_keys_gated_when_admin_write_disabled() {
    let h = Gh::new(false).await;
    let c = reqwest::Client::new();
    for (method, path) in [
        ("GET", "/api/admin/gateway-keys"),
        ("POST", "/api/admin/gateway-keys"),
        ("POST", "/api/admin/gateway-keys/boot-admin/revoke"),
    ] {
        let req = match method {
            "GET" => c.get(h.url(path)),
            // JSON body required: the extractor runs before the handler gate
            // (otherwise axum answers 415 instead of the 404 we assert).
            _ => c
                .post(h.url(path))
                .header("If-Match", "*")
                .json(&serde_json::json!({"id": "x", "scope": "admin"})),
        };
        let resp = req
            .header("Authorization", bearer(&h.admin))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method} {path} gated");
        let b: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(b["error"]["code"], "admin_write_disabled");
    }
    assert!(!h.admin_write);
}

/// Legacy default `last4` for pre-existing entries (disk compat).
#[test]
fn gateway_key_entry_last4_defaults_for_legacy_rows() {
    let legacy: GatewayKeyEntry = serde_json::from_value(serde_json::json!({
        "id": "old-1",
        "scope": "readonly",
        "prefix": "sk-pony-read-",
        "salt": "abc",
        "key_hash": "def"
    }))
    .unwrap();
    assert_eq!(legacy.last4, "****");
    assert!(!legacy.revoked);
}

/// Adversarial (task-29): an expired key fails closed even though its hash
/// matches; a key expiring in the future still works.
#[tokio::test]
async fn expired_gateway_key_fails_closed() {
    use ponyllm_config::KeyScope;
    use ponyllm_server::{create_app, AppState, GatewayConfig};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let (expired_plain, mut expired) =
        ponyllm_config::generate_scoped_gateway_key("expired-1", KeyScope::Inference);
    expired.expires_at = Some(now - 60);
    let (future_plain, mut future) =
        ponyllm_config::generate_scoped_gateway_key("future-1", KeyScope::Inference);
    future.expires_at = Some(now + 3600);

    let mut cfg = GatewayConfig::default();
    cfg.bind_addr = "127.0.0.1:8080".to_string();
    cfg.api_key = "legacy-expiry-probe-token-1234567890".to_string();
    cfg.web_enabled = false;
    cfg.gateway_keys = vec![expired, future];
    let state = Arc::new(AppState::new(cfg));
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let c = reqwest::Client::new();

    let dead = c
        .get(format!("http://{}/api/admin/quota", addr))
        .header("Authorization", bearer(&expired_plain))
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), StatusCode::UNAUTHORIZED, "expired key must 401");

    let alive = c
        .get(format!("http://{}/api/admin/quota", addr))
        .header("Authorization", bearer(&future_plain))
        .send()
        .await
        .unwrap();
    assert_eq!(alive.status(), StatusCode::OK, "future-dated key must work");
}
