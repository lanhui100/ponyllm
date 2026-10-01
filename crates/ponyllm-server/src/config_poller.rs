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

/// Config backend base polling interval (milliseconds), surfaced as the overview
/// `hot_reload_ms` contract: file backend 500ms (legacy mtime watcher),
/// kubernetes backend 2s (Secret content-hash poll).
pub const KUBERNETES_POLL_INTERVAL_MS: u64 = 2000;

/// Default exponential backoff steps (in milliseconds) when the config source
/// fails (e.g. Kubernetes API server offline / partitioned / timing out).
/// 2s -> 4s -> 8s -> 16s -> 30s cap.
pub const DEFAULT_BACKOFF_STEPS_MS: &[u64] = &[2000, 4000, 8000, 16000, 30000];

/// Tracks consecutive failure counts and calculates the backoff duration.
#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    steps: Vec<Duration>,
    consecutive_failures: usize,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::new(
            DEFAULT_BACKOFF_STEPS_MS
                .iter()
                .map(|&ms| Duration::from_millis(ms))
                .collect(),
        )
    }
}

impl BackoffPolicy {
    pub fn new(steps: Vec<Duration>) -> Self {
        assert!(!steps.is_empty(), "backoff steps must not be empty");
        Self {
            steps,
            consecutive_failures: 0,
        }
    }

    /// Reset consecutive failures on success.
    pub fn on_success(&mut self) {
        if self.consecutive_failures > 0 {
            tracing::info!(
                recovered_after_failures = self.consecutive_failures,
                "config source recovered; resetting backoff interval to base"
            );
            self.consecutive_failures = 0;
        }
    }

    /// Record a failure and return the sleep duration to wait before the next attempt.
    pub fn on_failure(&mut self) -> Duration {
        let idx = self.consecutive_failures.min(self.steps.len() - 1);
        let duration = self.steps[idx];
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        duration
    }

    pub fn consecutive_failures(&self) -> usize {
        self.consecutive_failures
    }

    pub fn base_interval(&self) -> Duration {
        self.steps[0]
    }
}

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
///
/// `initial_identity` seeds the change-detection baseline: pass the identity
/// of the config the process loaded at startup so a Secret change that lands
/// between startup and the first poll is still detected (P1-arch S3-1).
///
/// On snapshot failure (e.g. apiserver unreachable or timed out), the poller
/// silently falls back to the in-memory last known good configuration without
/// interrupting data plane forwarding, and backs off according to `interval`
/// or exponential backoff steps (2s -> 4s -> 8s -> 16s -> 30s) to prevent
/// apiserver storming.
pub async fn run_config_poller(
    source: &dyn ConfigSource,
    interval: Duration,
    initial_identity: Option<String>,
    on_change: impl FnMut(ConfigFile),
    shutdown: impl Fn() -> bool,
) {
    let policy = BackoffPolicy::new(vec![
        interval,
        interval.saturating_mul(2),
        interval.saturating_mul(4),
        interval.saturating_mul(8),
        Duration::from_millis(30000).max(interval.saturating_mul(15)),
    ]);
    run_config_poller_with_backoff(source, policy, initial_identity, on_change, shutdown).await
}

/// Run the poll loop with an explicit [`BackoffPolicy`].
pub async fn run_config_poller_with_backoff(
    source: &dyn ConfigSource,
    mut backoff: BackoffPolicy,
    initial_identity: Option<String>,
    mut on_change: impl FnMut(ConfigFile),
    shutdown: impl Fn() -> bool,
) {
    let mut last_identity = initial_identity;
    loop {
        if shutdown() {
            tracing::info!("config poller stopped (draining)");
            return;
        }
        let next_sleep = match source.snapshot().await {
            Ok((identity, config)) => {
                backoff.on_success();
                if last_identity.as_deref() != Some(identity.as_str()) {
                    if last_identity.is_some() {
                        tracing::info!(
                            "config change detected via content identity (identity={})",
                            &identity[..identity.len().min(12)]
                        );
                        on_change(config);
                    }
                    // First snapshot establishes the baseline without a reload.
                    last_identity = Some(identity);
                }
                backoff.base_interval()
            }
            Err(e) => {
                let failures = backoff.consecutive_failures() + 1;
                let sleep_dur = backoff.on_failure();
                // Read failure must never disturb live traffic: fallback to last
                // known good in-memory config, log and backoff.
                tracing::warn!(
                    consecutive_failures = failures,
                    backoff_delay_ms = sleep_dur.as_millis() as u64,
                    "config poll failed (fallback to last known good in-memory config): {}",
                    e
                );
                sleep_dur
            }
        };
        tokio::time::sleep(next_sleep).await;
    }
}

/// Raw-bytes content identity: SHA-256 over the config payload bytes, computed
/// BEFORE any parsing/serialization. This is the ONLY stable change signal for
/// the kubernetes poller — hashing a parsed `ConfigFile` is nondeterministic
/// because `providers: HashMap` serializes in per-instance random order
/// (P1-arch S1-1: an 8-provider config would fire a false "change" every 2s).
pub fn raw_bytes_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
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
        let identity_a = "identity-a".to_string();

        // State 1: initial snapshot establishes baseline (no change callback).
        // State 2: identical identity → no callback.
        // State 3: identity change → exactly one callback.
        let states = vec![
            (identity_a.clone(), cfg_with_strategy(true)),
            (identity_a.clone(), cfg_with_strategy(true)),
            ("identity-b".to_string(), cfg_with_strategy(false)),
        ];
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(states)),
        };
        let c = counter.clone();
        let ch = changes.clone();
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(5), None, move |cfg| {
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
            run_config_poller(&src, Duration::from_millis(2), None, move |_| {
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
                "identity".to_string(),
                cfg_with_strategy(true),
            )])),
        };
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = shutdown.clone();
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(2), None, |_| {}, move || {
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
    fn raw_bytes_hash_stable_for_same_content_and_sensitive_to_change() {
        let a = raw_bytes_hash(b"ponyllm.toml v1");
        let a2 = raw_bytes_hash(b"ponyllm.toml v1");
        assert_eq!(a, a2);
        let b = raw_bytes_hash(b"ponyllm.toml v2");
        assert_ne!(a, b);
    }

    /// S1 regression (P1-arch S1-1): the raw-byte identity is stable even when
    /// re-parsing the same TOML would serialize in a different order — i.e.
    /// the poller must NOT fire on identical raw content. Simulated by feeding
    /// the same raw identity twice then a changed one: exactly one callback.
    #[tokio::test]
    async fn poller_identity_is_raw_bytes_not_parsed_serialization() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(vec![
                ("same-raw-hash".to_string(), cfg_with_strategy(true)),
                ("same-raw-hash".to_string(), cfg_with_strategy(true)),
                ("changed-raw-hash".to_string(), cfg_with_strategy(false)),
            ])),
        };
        let handle = tokio::spawn(async move {
            run_config_poller(&src, Duration::from_millis(5), None, move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }, || false)
            .await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();
        let _ = handle.await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "identical raw content must not fire changes");
    }

    /// Startup baseline (P1-arch S3-1): seeding `initial_identity` means a
    /// change that lands right after startup is still detected (no silent
    /// baseline swallow).
    #[tokio::test]
    async fn poller_initial_identity_seeds_baseline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let src = FakeSource {
            state: Arc::new(tokio::sync::Mutex::new(vec![
                // First poll returns the SAME content the process started with:
                // seeded baseline -> no callback.
                ("startup-hash".to_string(), cfg_with_strategy(true)),
                // Content changed since startup -> callback fires even though
                // this is the poller's first observable snapshot.
                ("new-hash".to_string(), cfg_with_strategy(false)),
            ])),
        };
        let handle = tokio::spawn(async move {
            run_config_poller(
                &src,
                Duration::from_millis(5),
                Some("startup-hash".to_string()),
                move |_| { c.fetch_add(1, Ordering::SeqCst); },
                || false,
            )
            .await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();
        let _ = handle.await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn backoff_policy_steps_and_reset() {
        let mut policy = BackoffPolicy::new(vec![
            Duration::from_millis(2000),
            Duration::from_millis(4000),
            Duration::from_millis(8000),
            Duration::from_millis(16000),
            Duration::from_millis(30000),
        ]);

        assert_eq!(policy.consecutive_failures(), 0);
        assert_eq!(policy.base_interval(), Duration::from_millis(2000));

        assert_eq!(policy.on_failure(), Duration::from_millis(2000));
        assert_eq!(policy.consecutive_failures(), 1);

        assert_eq!(policy.on_failure(), Duration::from_millis(4000));
        assert_eq!(policy.consecutive_failures(), 2);

        assert_eq!(policy.on_failure(), Duration::from_millis(8000));
        assert_eq!(policy.consecutive_failures(), 3);

        assert_eq!(policy.on_failure(), Duration::from_millis(16000));
        assert_eq!(policy.consecutive_failures(), 4);

        assert_eq!(policy.on_failure(), Duration::from_millis(30000));
        assert_eq!(policy.consecutive_failures(), 5);

        // Clamped at 30s
        assert_eq!(policy.on_failure(), Duration::from_millis(30000));
        assert_eq!(policy.consecutive_failures(), 6);

        // Reset on success
        policy.on_success();
        assert_eq!(policy.consecutive_failures(), 0);
        assert_eq!(policy.on_failure(), Duration::from_millis(2000));
    }

    #[tokio::test]
    async fn poller_backoff_timing_and_fallback_to_last_known_good() {
        // Mock source: returns Initial, then fails 3 times, then recovers with New
        struct StepSource {
            step: Arc<AtomicUsize>,
            timestamps: Arc<tokio::sync::Mutex<Vec<std::time::Instant>>>,
        }

        #[async_trait]
        impl ConfigSource for StepSource {
            async fn snapshot(&self) -> Result<(String, ConfigFile), String> {
                self.timestamps.lock().await.push(std::time::Instant::now());
                let cur = self.step.fetch_add(1, Ordering::SeqCst);
                match cur {
                    0 => Ok(("v1".to_string(), cfg_with_strategy(true))),
                    1 | 2 | 3 => Err("control plane offline (simulated 504)".to_string()),
                    _ => Ok(("v2".to_string(), cfg_with_strategy(false))),
                }
            }
        }

        let step = Arc::new(AtomicUsize::new(0));
        let timestamps = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let src = StepSource {
            step: step.clone(),
            timestamps: timestamps.clone(),
        };

        let policy = BackoffPolicy::new(vec![
            Duration::from_millis(20),
            Duration::from_millis(40),
            Duration::from_millis(80),
        ]);

        let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ch = changes.clone();
        let handle = tokio::spawn(async move {
            run_config_poller_with_backoff(
                &src,
                policy,
                None,
                move |cfg| {
                    ch.lock().unwrap().push(cfg);
                },
                || false,
            )
            .await;
        });

        // Wait long enough for step 0 (v1), 1 (err -> sleep 20), 2 (err -> sleep 40), 3 (err -> sleep 80), 4 (v2)
        // Total expected sleep: 20 + 20 + 40 + 80 ≈ 160ms
        tokio::time::sleep(Duration::from_millis(350)).await;
        handle.abort();
        let _ = handle.await;

        let applied = changes.lock().unwrap();
        // v1 establishes baseline (0), then 3 failures fall back quietly, then v2 triggers exactly 1 change
        assert_eq!(applied.len(), 1, "only genuine change should fire callback");
        assert_eq!(
            applied[0].gateway.default_strategy,
            ponyllm_core::pool::GatewayRoutingStrategy::Speed
        );

        let ts = timestamps.lock().await;
        assert!(ts.len() >= 5, "must have at least 5 poll attempts, got {}", ts.len());
        // Verify that consecutive intervals expanded
        let diff1 = ts[2].duration_since(ts[1]); // after failure 1 (step 1)
        let diff2 = ts[3].duration_since(ts[2]); // after failure 2 (step 2)
        assert!(diff2 >= diff1, "backoff delay must increase: diff1={:?}, diff2={:?}", diff1, diff2);
    }
}