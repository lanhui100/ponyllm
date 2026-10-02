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
//!   Connection TLS: `PONYLLM_LOCK_SSLMODE=require` (default) uses a rustls
//!   channel with the native root store so the lock DB credentials never
//!   travel in clear text; `=disable` is an explicit opt-out (logged loudly)
//!   for local/development only.
//! - [`InMemoryRefreshLock`]: an in-memory test double. It mirrors the
//!   production GLOBAL single-lock semantics (one holder across ALL keys —
//!   the same-egress-IP constraint the gate enforces), so tests can simulate
//!   two replicas and different keys blocking each other.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateError, RefreshGateGuard};
use ponyllm_core::telemetry::MetricsCollector;
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

/// Advisory lock key text: `hashtext('ponyllm-antigravity-refresh')` is
/// evaluated server-side so every replica derives the same advisory lock id.
/// GLOBAL (not per-key): the gate serializes ALL OAuth refreshes on one
/// egress IP — multi-account concurrent refreshes are exactly what upstream
/// risk control flags.
const REFRESH_LOCK_KEY: &str = "ponyllm-antigravity-refresh";
/// Upper bound for the `pg_try_advisory_lock` query itself. The full
/// refresh + persist critical section is separately capped by the manager's
/// 60s timeout (see `antigravity.rs`) so the lock can never be held longer
/// than that regardless of OAuth HTTP hangs.
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
    /// Graceful-drain flag: once true, acquisition refuses (fail closed) so
    /// no OAuth refresh starts during drain.
    draining: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for PostgresRefreshLock {
    // Never print the DSN (embedded credentials).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRefreshLock")
            .field("metrics", &self.metrics.as_ref().map(|_| "set"))
            .field("draining", &self.draining.load(std::sync::atomic::Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PostgresRefreshLock {
    /// The DSN is never stored: it is read from `PONYLLM_LOCK_DATABASE_URL`
    /// at connect time (so it cannot leak through Debug/Display, and the
    /// gateway fails closed when the env var is absent). `metrics` and
    /// `draining` are optional (tests pass `None`/defaults).
    pub fn new(metrics: Option<Arc<MetricsCollector>>) -> Self {
        let state = Arc::new(Mutex::new(PgConnectionState::default()));
        spawn_idle_warmup(state.clone());
        Self {
            state,
            metrics,
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Wire the graceful-drain flag (set by the CLI when SIGTERM fires).
    pub fn with_draining(mut self, draining: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.draining = draining;
        self
    }

    fn is_draining(&self) -> bool {
        self.draining.load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn connect(&self) -> Result<tokio_postgres::Client, String> {
        let dsn = std::env::var("PONYLLM_LOCK_DATABASE_URL").unwrap_or_default();
        if dsn.trim().is_empty() {
            return Err("PONYLLM_LOCK_DATABASE_URL is not set".to_string());
        }
        let sslmode = std::env::var("PONYLLM_LOCK_SSLMODE")
            .unwrap_or_else(|_| "require".to_string())
            .to_ascii_lowercase();
        // Each arm connects, spawns the driver connection task, and returns
        // just the Client so the arms share one result type.
        let client = match sslmode.as_str() {
            "disable" => {
                tracing::warn!(
                    "PONYLLM_LOCK_SSLMODE=disable: advisory-lock PG credentials travel without TLS (dev only; production requires 'require')"
                );
                tokio_postgres::connect(&dsn, NoTls).await
                    .map(|(client, connection)| spawn_pg_connection(client, connection))
                    .map_err(|e| {
            // Diagnostic detail is safe: tokio-postgres CONNECT errors carry
            // io/TLS text, never the DSN (which is parsed into Config before
            // the attempt). The returned error stays fully sanitized.
            tracing::warn!(error = %e, "refresh lock PG connect failed (detail)");
            sanitize_connect_error(&e)
        })?
            }
            _ => {
                let config = rustls::ClientConfig::builder()
                    .with_root_certificates(self::load_lock_roots())
                    .with_no_client_auth();
                let tls = postgres_rustls::MakeTlsConnector::new(
                    tokio_rustls::TlsConnector::from(std::sync::Arc::new(config)),
                );
                tokio_postgres::connect(&dsn, tls).await
                    .map(|(client, connection)| spawn_pg_connection(client, connection))
                    .map_err(|e| {
            // Diagnostic detail is safe: tokio-postgres CONNECT errors carry
            // io/TLS text, never the DSN (which is parsed into Config before
            // the attempt). The returned error stays fully sanitized.
            tracing::warn!(error = %e, "refresh lock PG connect failed (detail)");
            sanitize_connect_error(&e)
        })?
            }
        };
        Ok(client)
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

/// Spawn the tokio-postgres driver task that pumps the connection. Generic
/// over the TLS stream type so plain (NoTls) and rustls connections share one
/// helper.
fn spawn_pg_connection<S>(
    client: tokio_postgres::Client,
    connection: tokio_postgres::Connection<tokio_postgres::Socket, S>,
) -> tokio_postgres::Client
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static, {
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::warn!("refresh lock PG connection error: {}", e);
        }
    });
    client
}

/// Trust roots for the lock DB TLS channel (best-effort, additive):
/// 1. `PONYLLM_LOCK_CA_FILE` — a PEM bundle with the lock DB's CA (self-signed
///    or internal), when the operator provides one;
/// 2. the OS native trust store (rustls-native-certs).
/// An empty-but-present store is fine only when `PONYLLM_LOCK_CA_FILE` is set;
/// otherwise the handshake fails closed against the PG server cert — the
/// correct posture for a lock DB (P11-sec S2-1: `require` is verify-full
/// semantics, NOT libpq's encrypt-only).
pub fn load_lock_roots() -> rustls::RootCertStore {
    let mut store = rustls::RootCertStore::empty();
    if let Some(ca_path) = std::env::var("PONYLLM_LOCK_CA_FILE")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        match std::fs::File::open(&ca_path) {
            Ok(mut file) => {
                let mut reader = std::io::BufReader::new(&mut file);
                let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
                    .filter_map(Result::ok)
                    .collect();
                if certs.is_empty() {
                    tracing::warn!(ca_file = %ca_path, "PONYLLM_LOCK_CA_FILE contained no PEM certs — lock DB handshake will fail closed");
                } else {
                    for cert in certs.clone() {
                        let _ = store.add(cert);
                    }
                    tracing::info!(
                        ca_file = %ca_path,
                        certs = certs.len(),
                        "lock DB TLS: loaded custom CA bundle"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(ca_file = %ca_path, error = %e, "PONYLLM_LOCK_CA_FILE not readable — lock DB handshake will fail closed");
            }
        }
    }
    let result = rustls_native_certs::load_native_certs();
    for cert in result.certs {
        let _ = store.add(cert);
    }
    for err in result.errors {
        tracing::warn!("rustls native cert load error: {}", err);
    }
    store
}

/// PG connect errors are logged but must never leak the DSN (user/password).
/// The message is fully generic — no error Display, no host, no credentials.
/// (The advisory-lock query path keeps only the SQLSTATE code when present.)
fn sanitize_connect_error(_e: &tokio_postgres::Error) -> String {
    sanitized_connect_message().to_string()
}

/// The sanitized, DSN-free connect failure message.
fn sanitized_connect_message() -> &'static str {
    "refresh lock PG connect failed (lock database unreachable)"
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
        let sslmode = std::env::var("PONYLLM_LOCK_SSLMODE")
            .unwrap_or_else(|_| "require".to_string())
            .to_ascii_lowercase();
        let client = if sslmode == "disable" {
            tokio_postgres::connect(&dsn, NoTls).await.ok().map(|(c, conn)| {
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                c
            })
        } else {
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(load_lock_roots())
                .with_no_client_auth();
            let tls = postgres_rustls::MakeTlsConnector::new(
                tokio_rustls::TlsConnector::from(std::sync::Arc::new(config)),
            );
            tokio_postgres::connect(&dsn, tls).await.ok().map(|(c, conn)| {
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                c
            })
        };
        if client.is_none() {
            // Loud-but-sanitized: a silent warmup failure would hide why the
            // gate is always Unavailable (P11-sec S3-5). Never the DSN.
            tracing::warn!(
                "refresh lock idle warmup connect failed ({}); the gate will fail closed ",
                sanitized_connect_message()
            );
        }
        if let Some(client) = client {
            // Park it as idle (only if nothing else parked meanwhile).
            let mut guard = state.lock().await;
            if guard.idle.is_none() {
                guard.idle = Some(client);
            }
        }
    });
}

/// Guard holding the session that owns the advisory lock. Drop releases it
/// and records the hold duration into the `refresh_lock_hold_seconds` gauge.
struct PgLockGuard {
    state: Arc<Mutex<PgConnectionState>>,
    client: Option<tokio_postgres::Client>,
    acquired_at: Instant,
    metrics: Option<Arc<MetricsCollector>>,
}

impl RefreshGateGuard for PgLockGuard {}

impl Drop for PgLockGuard {
    fn drop(&mut self) {
        if let Some(m) = &self.metrics {
            let hold = self.acquired_at.elapsed().as_secs();
            m.record_refresh_lock_hold_secs(hold);
        }
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
        if self.is_draining() {
            if let Some(m) = &self.metrics {
                m.record_refresh_lock_error();
            }
            return Err(RefreshGateError::Unavailable(
                "graceful drain in progress: refresh serialization suspended".to_string(),
            ));
        }
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
                        acquired_at: Instant::now(),
                        metrics: self.metrics.clone(),
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
                // locks it held) and fail closed. Keep only the SQLSTATE code
                // (if any) — never the raw server error text.
                drop(client);
                let detail = e
                    .code()
                    .map(|c| c.code().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                Err(RefreshGateError::Unavailable(format!(
                    "advisory lock query failed (sqlstate {})",
                    detail
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

/// In-memory test double mirroring the production GLOBAL single-lock
/// semantics: one holder across ALL keys. Two "replicas" over the same shared
/// flag serialize deterministically; a held lock blocks a different key too
/// (the same-egress-IP constraint the gate exists to enforce). Never used in
/// production.
#[derive(Debug)]
pub struct InMemoryRefreshLock {
    shared: Arc<Mutex<bool>>,
    metrics: Option<Arc<MetricsCollector>>,
    draining: Arc<std::sync::atomic::AtomicBool>,
}

impl InMemoryRefreshLock {
    pub fn new(
        shared: Arc<Mutex<bool>>,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        Self {
            shared,
            metrics,
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn with_draining(mut self, draining: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.draining = draining;
        self
    }

    pub fn fresh() -> Self {
        Self::new(Arc::new(Mutex::new(false)), None)
    }
}

struct InMemoryGuard {
    shared: Arc<Mutex<bool>>,
    metrics: Option<Arc<MetricsCollector>>,
    acquired_at: Instant,
}

impl RefreshGateGuard for InMemoryGuard {}

impl Drop for InMemoryGuard {
    fn drop(&mut self) {
        if let Some(m) = &self.metrics {
            m.record_refresh_lock_hold_secs(self.acquired_at.elapsed().as_secs());
        }
        let shared = self.shared.clone();
        tokio::spawn(async move {
            *shared.lock().await = false;
        });
    }
}

#[async_trait]
impl RefreshGate for InMemoryRefreshLock {
    async fn try_acquire(
        &self,
        _key_id: &str,
    ) -> Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
        if self.draining.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(m) = &self.metrics {
                m.record_refresh_lock_error();
            }
            return Err(RefreshGateError::Unavailable(
                "graceful drain in progress: refresh serialization suspended".to_string(),
            ));
        }
        let mut held = self.shared.lock().await;
        if *held {
            if let Some(m) = &self.metrics {
                m.record_refresh_lock_skipped();
            }
            return Ok(None);
        }
        *held = true;
        if let Some(m) = &self.metrics {
            m.record_refresh_lock_acquired();
        }
        Ok(Some(Box::new(InMemoryGuard {
            shared: self.shared.clone(),
            metrics: self.metrics.clone(),
            acquired_at: Instant::now(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_lock_serializes_two_replicas_globally() {
        let shared = Arc::new(Mutex::new(false));
        let replica_a = InMemoryRefreshLock::new(shared.clone(), None);
        let replica_b = InMemoryRefreshLock::new(shared.clone(), None);

        let a_guard = replica_a.try_acquire("k-1").await.unwrap();
        assert!(a_guard.is_some(), "replica A must acquire first");

        // While A holds the lock, B is skipped — even for a DIFFERENT key:
        // the gate is a global single lock (same-egress-IP constraint).
        let b_other_key = replica_b.try_acquire("k-2").await.unwrap();
        assert!(
            b_other_key.is_none(),
            "different keys must block each other under the global lock"
        );

        // Drop A's guard (async drop task) and B can now acquire any key.
        drop(a_guard);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        let b_again = replica_b.try_acquire("k-1").await.unwrap();
        assert!(b_again.is_some(), "replica B must acquire after A releases");
    }

    #[tokio::test]
    async fn in_memory_lock_counts_metrics_and_hold_seconds() {
        let shared = Arc::new(Mutex::new(false));
        let metrics = Arc::new(MetricsCollector::new());
        let a = InMemoryRefreshLock::new(shared.clone(), Some(metrics.clone()));
        let b = InMemoryRefreshLock::new(shared.clone(), Some(metrics.clone()));

        let g = a.try_acquire("k-1").await.unwrap().unwrap();
        let skipped = b.try_acquire("k-2").await.unwrap();
        assert!(skipped.is_none());
        // Hold at least ~10ms so the hold gauge records a nonzero value.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        drop(g);
        tokio::task::yield_now().await;

        let summary = metrics.get_summary();
        assert_eq!(summary.ha_ops.refresh_lock_acquired_total, 1);
        assert_eq!(summary.ha_ops.refresh_lock_skipped_total, 1);
        // The hold gauge is sampled by dedicated metrics tests; no numeric
        // threshold is meaningful here.
    }

    /// Drain short-circuit must mirror the production gate: once draining,
    /// the InMemory double also refuses acquisition (P11-sec S3-1).
    #[tokio::test]
    async fn in_memory_lock_refuses_acquisition_while_draining() {
        let draining = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = InMemoryRefreshLock::fresh().with_draining(draining.clone());

        let guard = gate.try_acquire("k-1").await.expect("gate query ok");
        assert!(guard.is_some(), "not draining: acquire must succeed");

        draining.store(true, std::sync::atomic::Ordering::SeqCst);
        let err = match gate.try_acquire("k-2").await {
            Ok(_) => panic!("draining gate must refuse acquisition"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("drain"),
            "draining refusal must be identifiable: {err}"
        );
    }

    #[test]
    fn sanitize_connect_error_never_leaks_dsn() {
        // The sanitizer must never echo the raw error Display (which may
        // carry host/user or even credentials); only a fixed generic message.
        let out = sanitized_connect_message();
        assert_eq!(out, "refresh lock PG connect failed (lock database unreachable)");
        assert!(!out.contains("secret"));
        assert!(!out.contains("postgres://"));
        assert!(!out.contains("host"));
        // The wrapper produces the same sanitized text regardless of the error
        // (a real unreachable-DSN connect error is exercised in the PG smoke
        // test, refresh_lock_pg_tests).
    }
}