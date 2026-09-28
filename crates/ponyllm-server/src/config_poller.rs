//! Injectable configuration change poller (multi-node HA).
//!
//! The Kubernetes config backend cannot watch a file's mtime; it polls the
//! Secret content hash every [`KUBERNETES_POLL_INTERVAL_MS`] and atomically
//! rebuilds the gateway pools on change. The poller is split behind the
//! [`ConfigSource`] seam so it is unit-testable without any Kubernetes
//! infrastructure (three states: hash changed → callback fired; unchanged →
//! no action; source error → log only, traffic unaffected).

use std::time::Duration;

use async_trait::async_trait;
use ponyllm_config::ConfigFile;

/// Config backend polling interval (milliseconds), surfaced as the overview
/// `hot_reload_ms` contract: file backend 500ms (legacy mtime watcher),
/// kubernetes backend 2s (Secret content-hash poll).
pub const KUBERNETES_POLL_INTERVAL_MS: u64 = 2000;

/// A config truth source the poller can snapshot. For the Kubernetes backend
/// this is a thin wrapper over [`crate::admin_store::ConfigStore`].
#[async_trait]
pub trait ConfigSource: Send + Sync {
    /// Returns `(content_identity, config)`. `content_identity` must change
    /// exactly when the underlying configuration content changes (for the
    /// Secret backend: a hash of `data['ponyllm.toml']`), and stay stable
    /// for metadata-only churn.
    async fn snapshot(&self) -> Result<(String, ConfigFile), String>;
}

/// Run the poll loop until `shutdown` returns true. `on_change` is invoked
/// (with the freshly loaded config) exactly once per content change.
pub async fn run_config_poller(
    source: &dyn ConfigSource,
    interval: Duration,
    mut on_change: impl FnMut(ConfigFile),
    shutdown: impl Fn() -> bool,
) {
    let mut last_identity: Option<String> = None;
    loop {
        if shutdown() {
            tracing::info!("config poller stopped (draining)");
            return;
        }
        match source.snapshot().await {
            Ok((identity, config)) => {
                if last_identity.as_deref() != Some(identity.as_str()) {
                    if last_identity.is_some() {
                        tracing::info!(
                            "config change detected via content hash (identity={})",
                            &identity[..identity.len().min(12)]
                        );
                        on_change(config);
                    }
                    // First snapshot establishes the baseline without a reload.
                    last_identity = Some(identity);
                }
            }
            Err(e) => {
                // Read failure must never disturb live traffic: log and keep
                // polling.
                tracing::warn!("config poll failed (ignored): {}", e);
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Content-hash identity for a loaded config: canonical TOML serialization
/// hashed with SHA-256. Deterministic for identical content, so every replica
/// derives the same identity and metadata-only Secret changes do not churn.
pub fn content_hash(config: &ConfigFile) -> String {
    use sha2::{Digest, Sha256};
    let canonical = toml::to_string_pretty(config).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct FakeSource {
        state: Arc<tokio::sync::Mutex<Vec<(String, ConfigFile)>>>,
    }

    #[async_trait]
    impl ConfigSource for FakeSource {
        async fn snapshot(&self) -> Result<(String, ConfigFile), String> {
            let mut queue = self.state.lock().await;
            if queue.is_empty() {
                return Err("backend unavailable".to_string());
            }
            Ok(queue.remove(0))
        }
    }

    fn cfg_with_strategy(economy: bool) -> ConfigFile {
        let mut cfg = ConfigFile::default();
        // `ConfigFile::default()` randomizes the api_key: pin it so two
        // instances serialize identically (deterministic content hash).
        cfg.gateway.api_key = "sk-test-deterministic".to_string();
        cfg.gateway.default_strategy = if economy {
            ponyllm_core::pool::GatewayRoutingStrategy::Economy
        } else {
            ponyllm_core::pool::GatewayRoutingStrategy::Speed
        };
        cfg
    }

    #[tokio::test]
    async fn poller_three_states() {
        let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let counter = Arc::new(AtomicUsize::new(0));
        let identity_a = content_hash(&cfg_with_strategy(true));

        // State 1: initial snapshot establishes baseline (no change callback).
        // State 2: identical identity → no callback.
        // State 3: identity change → exactly one callback.
        let states = vec![
            (identity_a.clone(), cfg_with_strategy(true)),
            (identity_a.clone(), cfg_with_strategy(true)),
            (content_hash(&cfg_with_strategy(false)), cfg_with_strategy(false)),
        ];
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(states)),
        };
        let c = counter.clone();
        let ch = changes.clone();
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(5), move |cfg| {
                ch.lock().unwrap().push(cfg);
                c.fetch_add(1, Ordering::SeqCst);
            }, || false)
            .await;
        });

        // Wait until all three snapshots consumed, then cancel.
        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();
        let _ = handle.await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "exactly one change callback across baseline + unchanged + changed states"
        );
        let collected = changes.lock().unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(
            collected[0].gateway.default_strategy,
            ponyllm_core::pool::GatewayRoutingStrategy::Speed
        );
    }

    #[tokio::test]
    async fn poller_source_error_does_not_panic_or_fire_change() {
        // Only errors: loop keeps polling, no callback, no panic.
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(2), move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }, || false)
            .await;
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        handle.abort();
        let _ = handle.await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn poller_stops_when_shutdown_fires() {
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(vec![(
                content_hash(&cfg_with_strategy(true)),
                cfg_with_strategy(true),
            )])),
        };
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = shutdown.clone();
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(2), |_| {}, move || {
                stop.load(Ordering::SeqCst)
            })
            .await;
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        shutdown.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("poller must exit after shutdown fires")
            .unwrap();
    }

    #[test]
    fn content_hash_stable_for_same_config_and_sensitive_to_change() {
        let a = content_hash(&cfg_with_strategy(true));
        let a2 = content_hash(&cfg_with_strategy(true));
        assert_eq!(a, a2);
        let b = content_hash(&cfg_with_strategy(false));
        assert_ne!(a, b);
    }
}