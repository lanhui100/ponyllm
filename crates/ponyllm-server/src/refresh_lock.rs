//! Cross-replica antigravity refresh serialization (multi-node HA).
//!
//! [`RefreshGate`] is the trait seam defined in ponyllm-core; this module
//! provides the two concrete implementations:
//!
//! - [`PostgresRefreshLock`]: a session-level PostgreSQL advisory lock over a
//!   **dedicated connection**. The connection that acquired the lock is held
//!   by the returned guard until it is dropped (refresh + persist complete),
//!   then an unlock query runs and the connection is recycled or closed (a
//!   broken/closed connection releases the session lock server-side). PG
//!   unreachable ⇒ fail closed (skip the round, count `refresh_lock_error`).
//! - [`InMemoryRefreshLock`]: an in-memory test double that shares a held-key
//!   map so tests can simulate multiple replicas through the same gate.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateError, RefreshGateGuard};
use ponyllm_core::telemetry::MetricsCollector;
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

/// Advisory lock key text: `hashtext('ponyllm-antigravity-refresh')` is
/// evaluated server-side so every replica derives the same advisory lock id.
const REFRESH_LOCK_KEY: &str = "ponyllm-antigravity-refresh";
/// Upper bound for one lock round: refresh HTTP + write-back retries.
const REFRESH_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Idle PG connection cache inside the lock. At most one idle connection is
/// kept; concurrent acquisitions open additional connections so each lock
/// round owns its own session (advisory locks are session-scoped).
#[derive(Default)]
struct PgConnectionState {
    idle: Option<tokio_postgres::Client>,
}

/// PostgreSQL advisory-lock based [`RefreshGate`].
pub struct PostgresRefreshLock {
    state: Arc<Mutex<PgConnectionState>>,
    metrics: Option<Arc<MetricsCollector>>,
}

impl std::fmt::Debug for PostgresRefreshLock {
    // Never print the DSN (embedded credentials).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRefreshLock")
            .field("metrics", &self.metrics.as_ref().map(|_| "set"))
            .finish_non_exhaustive()
    }
}

impl PostgresRefreshLock {
    /// The DSN is never stored: it is read from `PONYLLM_LOCK_DATABASE_URL`
    /// at connect time (so it cannot leak through Debug/Display, and the
    /// gateway fails closed when the env var is absent). `metrics` is
    /// optional (tests pass `None`).
    pub fn new(metrics: Option<Arc<MetricsCollector>>) -> Self {
        let state = Arc::new(Mutex::new(PgConnectionState::default()));
        spawn_idle_warmup(state.clone());
        Self { state, metrics }
    }

    async fn connect(&self) -> Result<tokio_postgres::Client, String> {
        let dsn = std::env::var("PONYLLM_LOCK_DATABASE_URL").unwrap_or_default();
        if dsn.trim().is_empty() {
            return Err("PONYLLM_LOCK_DATABASE_URL is not set".to_string());
        }
        tokio_postgres::connect(&dsn, NoTls)
            .await
            .map(|(client, connection)| {
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::warn!("refresh lock PG connection error: {}", e);
                    }
                });
                client
            })
            .map_err(|e| format!("refresh lock PG connect failed: {}", e))
    }

    /// Unlock on `client`, then recycle it as the shared idle connection (only
    /// if none is already parked) or close it (closing releases session locks).
    async fn release_client(state: Arc<Mutex<PgConnectionState>>, client: tokio_postgres::Client) {
        // Best-effort unlock; on failure drop the connection so the server
        // releases the session lock automatically.
        let unlocked = client
            .query_one(
                "SELECT pg_advisory_unlock(hashtext($1))",
                &[&REFRESH_LOCK_KEY],
            )
            .await
            .map(|row| row.get::<_, bool>(0))
            .unwrap_or(false);
        if !unlocked {
            return; // dropping `client` closes the session → server releases
        }
        let mut guard = state.lock().await;
        if guard.idle.is_none() {
            guard.idle = Some(client);
        }
        // else: the extra connection drops and closes.
    }
}

/// Warm the idle connection eagerly at construction so the first lock round
/// does not pay a connect penalty. No-ops when `PONYLLM_LOCK_DATABASE_URL`
/// is absent.
fn spawn_idle_warmup(state: Arc<Mutex<PgConnectionState>>) {
    tokio::spawn(async move {
        let dsn = std::env::var("PONYLLM_LOCK_DATABASE_URL").unwrap_or_default();
        if dsn.trim().is_empty() {
            return;
        }
        if let Ok((client, connection)) = tokio_postgres::connect(&dsn, NoTls).await {
            tokio::spawn(async move {
                let _ = connection.await;
            });
            // Park it as idle (only if nothing else parked meanwhile).
            let mut guard = state.lock().await;
            if guard.idle.is_none() {
                guard.idle = Some(client);
            }
        }
    });
}

/// Guard holding the session that owns the advisory lock. Drop releases it.
struct PgLockGuard {
    state: Arc<Mutex<PgConnectionState>>,
    client: Option<tokio_postgres::Client>,
}

impl RefreshGateGuard for PgLockGuard {}

impl Drop for PgLockGuard {
    fn drop(&mut self) {
        let state = self.state.clone();
        if let Some(client) = self.client.take() {
            tokio::spawn(async move {
                PostgresRefreshLock::release_client(state, client).await;
            });
        }
    }
}

#[async_trait]
impl RefreshGate for PostgresRefreshLock {
    async fn try_acquire(
        &self,
        key_id: &str,
    ) -> Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
        // Take the idle connection (if any) so every lock round owns one
        // distinct session.
        let mut client = {
            let mut guard = self.state.lock().await;
            guard.idle.take()
        };
        if client.is_none() {
            client = Some(
                self.connect()
                    .await
                    .map_err(|e| {
                        if let Some(m) = &self.metrics {
                            m.record_refresh_lock_error();
                        }
                        RefreshGateError::Unavailable(e)
                    })?,
            );
        }
        let client = client.expect("client established above");

        let query = tokio::time::timeout(
            REFRESH_LOCK_TIMEOUT,
            client.query_one(
                "SELECT pg_try_advisory_lock(hashtext($1))",
                &[&REFRESH_LOCK_KEY],
            ),
        )
        .await;

        match query {
            Ok(Ok(row)) => {
                let acquired: bool = row.get(0);
                if acquired {
                    if let Some(m) = &self.metrics {
                        m.record_refresh_lock_acquired();
                    }
                    tracing::debug!(key_id, "antigravity refresh lock acquired");
                    Ok(Some(Box::new(PgLockGuard {
                        state: self.state.clone(),
                        client: Some(client),
                    })))
                } else {
                    // Another session holds the lock — recycle the connection.
                    if let Some(m) = &self.metrics {
                        m.record_refresh_lock_skipped();
                    }
                    tracing::debug!(key_id, "antigravity refresh lock held by another replica");
                    let mut guard = self.state.lock().await;
                    if guard.idle.is_none() {
                        guard.idle = Some(client);
                    }
                    Ok(None)
                }
            }
            Ok(Err(e)) => {
                if let Some(m) = &self.metrics {
                    m.record_refresh_lock_error();
                }
                // Query failed: drop the connection (releases any session
                // locks it held) and fail closed.
                drop(client);
                Err(RefreshGateError::Unavailable(format!(
                    "advisory lock query failed: {}",
                    e
                )))
            }
            Err(_) => {
                if let Some(m) = &self.metrics {
                    m.record_refresh_lock_error();
                }
                drop(client);
                Err(RefreshGateError::Unavailable(format!(
                    "advisory lock query timed out after {:?}",
                    REFRESH_LOCK_TIMEOUT
                )))
            }
        }
    }
}

/// In-memory test double: shares a `held` map across gate instances so two
/// "replicas" (separate `InMemoryRefreshLock`s over the same map) serialize
/// deterministically. Never used in production.
#[derive(Debug)]
pub struct InMemoryRefreshLock {
    shared: Arc<Mutex<HashMap<String, bool>>>,
    metrics: Option<Arc<MetricsCollector>>,
}

impl InMemoryRefreshLock {
    pub fn new(
        shared: Arc<Mutex<HashMap<String, bool>>>,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        Self { shared, metrics }
    }

    pub fn fresh() -> Self {
        Self::new(Arc::new(Mutex::new(HashMap::new())), None)
    }
}

struct InMemoryGuard {
    shared: Arc<Mutex<HashMap<String, bool>>>,
    key: String,
}

impl RefreshGateGuard for InMemoryGuard {}

impl Drop for InMemoryGuard {
    fn drop(&mut self) {
        let shared = self.shared.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            shared.lock().await.remove(&key);
        });
    }
}

#[async_trait]
impl RefreshGate for InMemoryRefreshLock {
    async fn try_acquire(
        &self,
        key_id: &str,
    ) -> Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
        let mut map = self.shared.lock().await;
        if *map.get(key_id).unwrap_or(&false) {
            if let Some(m) = &self.metrics {
                m.record_refresh_lock_skipped();
            }
            return Ok(None);
        }
        map.insert(key_id.to_string(), true);
        if let Some(m) = &self.metrics {
            m.record_refresh_lock_acquired();
        }
        Ok(Some(Box::new(InMemoryGuard {
            shared: self.shared.clone(),
            key: key_id.to_string(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_lock_serializes_two_replicas() {
        let shared = Arc::new(Mutex::new(HashMap::new()));
        let replica_a = InMemoryRefreshLock::new(shared.clone(), None);
        let replica_b = InMemoryRefreshLock::new(shared.clone(), None);

        let a_guard = replica_a.try_acquire("k-1").await.unwrap();
        assert!(a_guard.is_some(), "replica A must acquire first");

        // While A holds the lock, B must be skipped.
        let b_guard = replica_b.try_acquire("k-1").await.unwrap();
        assert!(b_guard.is_none(), "replica B must skip while A holds the lock");

        // Different keys do not block each other.
        let b_other = replica_b.try_acquire("k-2").await.unwrap();
        assert!(b_other.is_some());

        // Drop A's guard (async drop task) and B can now acquire.
        drop(a_guard);
        drop(b_other);
        // Wait for drop-spawned release task to run.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        let b_again = replica_b.try_acquire("k-1").await.unwrap();
        assert!(b_again.is_some(), "replica B must acquire after A releases");
    }

    #[tokio::test]
    async fn in_memory_lock_counts_metrics() {
        let shared = Arc::new(Mutex::new(HashMap::new()));
        let metrics = Arc::new(MetricsCollector::new());
        let a = InMemoryRefreshLock::new(shared.clone(), Some(metrics.clone()));
        let b = InMemoryRefreshLock::new(shared.clone(), Some(metrics.clone()));

        let _g = a.try_acquire("k-1").await.unwrap().unwrap();
        let skipped = b.try_acquire("k-1").await.unwrap();
        assert!(skipped.is_none());

        let summary = metrics.get_summary();
        assert_eq!(summary.ha_ops.refresh_lock_acquired_total, 1);
        assert_eq!(summary.ha_ops.refresh_lock_skipped_total, 1);
    }
}