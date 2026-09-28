//! Cross-replica serialization gate for a single upstream OAuth refresh round
//! (multi-node HA, 2026-09-28).
//!
//! Every Antigravity refresh — whether driven by the keepalive worker or by a
//! request hitting a 401 — must pass through a [`RefreshGate`] before
//! touching the OAuth endpoint. The gate is injected into each
//! `AntigravityTokenManager` by the server process; the concrete
//! implementation (PostgreSQL advisory lock, in-memory test double) lives in
//! ponyllm-server. Core only defines the seam so both refresh entry points
//! serialize through the same lock without core depending on any backend.

use std::sync::Arc;

use async_trait::async_trait;

/// Handle returned by a successful [`RefreshGate::try_acquire`]. Dropping it
/// releases the underlying lock. Callers must keep it alive across the whole
/// "refresh + persist" critical section.
pub trait RefreshGateGuard: Send + Sync {}

/// No-op guard used when no gate is configured (local single-instance runs).
#[derive(Debug, Default)]
pub struct NoopRefreshGateGuard;
impl RefreshGateGuard for NoopRefreshGateGuard {}

/// Lock backend failure. Callers MUST fail closed (skip the refresh round) —
/// never fall back to an unlocked refresh — because bypassing the gate would
/// re-enable the same-egress-IP concurrent-refresh hazard the gate exists to
/// prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshGateError {
    /// Lock backend unreachable or the lock query itself failed.
    Unavailable(String),
}

impl std::fmt::Display for RefreshGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshGateError::Unavailable(msg) => write!(f, "refresh lock unavailable: {}", msg),
        }
    }
}

/// Outcome of a gate probe, expressed as the guard itself:
/// `Ok(Some(guard))` — this replica owns the lock and must run the full
/// refresh + persist cycle before dropping it;
/// `Ok(None)` — another replica holds the lock, skip this round;
/// `Err` — backend unavailable, skip this round and count an error.
#[async_trait]
pub trait RefreshGate: Send + Sync + std::fmt::Debug {
    async fn try_acquire(
        &self,
        key_id: &str,
    ) -> Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError>;
}

/// Convenience alias so callers can store `Arc<dyn RefreshGate>`.
pub type DynRefreshGate = Arc<dyn RefreshGate>;
