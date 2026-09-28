//! Real-cluster smoke test for the Kubernetes config backend (multi-node HA).
//!
//! `#[ignore]` by default — run via `scripts/k3d-smoke.sh` against a live
//! k3d (or k3s) cluster. Exercises the REAL apiserver optimistic-concurrency
//! semantics: a save carrying the loaded `resourceVersion` succeeds, and a
//! save carrying a stale `resourceVersion` is rejected with 409 Conflict →
//! [`ConfigStoreError::Conflict`]. This is the empirical confirmation behind
//! the Phase 2 RBAC verb decision (patch + resourceVersion CAS).
//!
//! Preconditions (created by the smoke script):
//! - a `ponyllm-live-config` Secret in namespace `ponyllm` with a valid
//!   `ponyllm.toml` data key;
//! - kube credentials resolvable via `kube::Config::infer()` (KUBECONFIG).

use std::sync::Arc;

use ponyllm_server::admin_store::{
    ConfigStore, ConfigStoreError, ConfigVersion, KubernetesConfigStore, KubeSecretApi,
    SecretApi,
};

async fn build_store() -> KubernetesConfigStore {
    let config = kube::Config::infer()
        .await
        .expect("kube config infer (KUBECONFIG)");
    let client = kube::Client::try_from(config).expect("kube client");
    let api: Arc<dyn SecretApi> = Arc::new(KubeSecretApi::new(client, "ponyllm"));
    KubernetesConfigStore::with_api(api, "ponyllm-live-config", "ponyllm.toml")
}

#[tokio::test]
#[ignore = "requires a real cluster (scripts/k3d-smoke.sh)"]
async fn real_apiserver_patch_cas_semantics() {
    let store = build_store().await;

    // 1) load: the Secret is present and parses.
    let (cfg, version) = store.load().await.expect("load ponyllm-live-config");
    assert!(cfg.gateway.web_enabled, "sample config must parse");

    // 2) save with the CURRENT resourceVersion: the apiserver must accept the
    //    merge patch (single-writer success path).
    store
        .save(&cfg, &version)
        .await
        .expect("save with current resourceVersion succeeds");

    // 3) save with the STALE resourceVersion (the version we loaded before the
    //    successful save bumped it): the apiserver must answer 409 Conflict,
    //    which the store maps to ConfigStoreError::Conflict.
    let err = store
        .save(&cfg, &version)
        .await
        .expect_err("stale resourceVersion must be rejected");
    assert!(
        matches!(err, ConfigStoreError::Conflict { .. }),
        "stale resourceVersion must map to Conflict, got {:?}",
        err
    );

    // 4) a fresh load yields the bumped version (truth-source round trip).
    let (_cfg2, version2) = store.load().await.expect("reload");
    assert_ne!(
        version2,
        version,
        "resourceVersion must advance after a successful save"
    );
}