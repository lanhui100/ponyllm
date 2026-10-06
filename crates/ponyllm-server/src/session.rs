//! Phase-3 (task-6 / VULN-05): per-pod in-memory admin session store.
//!
//! A browser exchanges its pasted gateway credential ONCE for an opaque
//! HttpOnly cookie `ponyllm_session=<sid>`; the raw key never lives in JS
//! memory afterwards. The store is deliberately per-pod (same semantic as the
//! auth rate limiter — replicas do not share sessions; a shared backend is a
//! deferred Phase-? item). TTL slides on every validated use (8h default);
//! the table is bounded at [`MAX_SESSIONS`] with LRU eviction so a sid
//! spray cannot grow memory without limit.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ponyllm_config::KeyScope;

/// Hard cap on concurrent live sessions (evicted past this).
pub const MAX_SESSIONS: usize = 4096;

/// Phase-3b (R-S3): per-originating-key session cap — one credential may hold
/// at most this many live sessions (low-privilege key cannot flood the table).
pub const PER_KEY_SESSION_CAP: usize = 64;

/// Default TTL in seconds (contract: Max-Age=28800, 8h sliding).
pub const DEFAULT_SESSION_TTL_SECS: u64 = 28800;

/// One live session: the scope captured at exchange time + timestamps.
#[derive(Debug, Clone)]
struct SessionEntry {
    scope: KeyScope,
    /// Originating credential identity (`None` for the bare `create(scope)`
    /// path — that path enforces no per-key cap, see R-S3 contract).
    creator: Option<String>,
    last_seen: Instant,
}

#[derive(Debug)]
struct Inner {
    ttl: Duration,
    sessions: HashMap<String, SessionEntry>,
}

/// Lock-protected session table.
#[derive(Debug)]
pub struct SessionStore {
    inner: Mutex<Inner>,
}

impl SessionStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                ttl: ttl.max(Duration::from_secs(1)),
                sessions: HashMap::new(),
            }),
        }
    }

    /// The effective TTL in whole seconds (used for the cookie Max-Age).
    pub fn ttl_secs(&self) -> i64 {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).ttl.as_secs() as i64
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Drop expired sessions (R-S3: `create` must reclaim expired entries
    /// BEFORE deciding eviction, so stale rows never force a live eviction).
    fn sweep_expired(inner: &mut Inner, now: Instant) {
        inner
            .sessions
            .retain(|_, e| now.duration_since(e.last_seen) < inner.ttl);
    }

    /// Pick an eviction candidate when the table is full: prefer a session
    /// from the SAME scope as the incoming one (a low-privilege flood evicts
    /// its own kind first, established admin sessions survive), falling back
    /// to the global least-recently-used entry.
    fn eviction_candidate(inner: &Inner, scope: KeyScope) -> Option<String> {
        inner
            .sessions
            .iter()
            .filter(|(_, e)| e.scope == scope)
            .min_by_key(|(_, e)| e.last_seen)
            .map(|(k, _)| k.clone())
            .or_else(|| {
                inner
                    .sessions
                    .iter()
                    .min_by_key(|(_, e)| e.last_seen)
                    .map(|(k, _)| k.clone())
            })
    }

    /// Issue a new session for `scope`; returns the opaque sid.
    ///
    /// R-S3 contract (frozen test): after sweeping expired entries, when the
    /// table is at [`MAX_SESSIONS`] the eviction prefers a same-scope LRU so a
    /// low-privilege spray cannot evict an active admin session; `live_count`
    /// stays bounded. No per-key cap here (the bare signature has no creator).
    pub fn create(&self, scope: KeyScope) -> String {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        Self::sweep_expired(&mut inner, now);
        if inner.sessions.len() >= MAX_SESSIONS {
            if let Some(evict) = Self::eviction_candidate(&inner, scope) {
                inner.sessions.remove(&evict);
            }
        }
        let sid = uuid::Uuid::new_v4().simple().to_string();
        inner.sessions.insert(
            sid.clone(),
            SessionEntry {
                scope,
                creator: None,
                last_seen: now,
            },
        );
        sid
    }

    /// Issue a session for `scope` bound to the originating credential
    /// `creator` (R-S2 handler path). Enforces the per-key cap
    /// [`PER_KEY_SESSION_CAP`]: `None` = the creator already holds the cap
    /// (handler answers 429 `session_limit_reached`).
    pub fn create_with_creator(&self, scope: KeyScope, creator: &str) -> Option<String> {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        Self::sweep_expired(&mut inner, now);
        let creator_sessions = inner
            .sessions
            .values()
            .filter(|e| e.creator.as_deref() == Some(creator))
            .count();
        if creator_sessions >= PER_KEY_SESSION_CAP {
            return None;
        }
        if inner.sessions.len() >= MAX_SESSIONS {
            if let Some(evict) = Self::eviction_candidate(&inner, scope) {
                inner.sessions.remove(&evict);
            }
        }
        let sid = uuid::Uuid::new_v4().simple().to_string();
        inner.sessions.insert(
            sid.clone(),
            SessionEntry {
                scope,
                creator: Some(creator.to_string()),
                last_seen: now,
            },
        );
        Some(sid)
    }

    /// Validate `sid` and SLIDE its TTL. `None` when unknown or expired (the
    /// expired entry is removed so it cannot linger).
    ///
    /// R-S6b: the sid match is CONSTANT-TIME — sid values are opaque
    /// credentials; a timing channel must not reveal whether a guessed sid
    /// matches any live entry. Table is bounded (≤4096), so the linear
    /// scan is acceptable on the low-QPS admin path.
    pub fn validate(&self, sid: &str) -> Option<KeyScope> {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        let ttl = inner.ttl;
        let matched: Option<String> = inner
            .sessions
            .keys()
            .find(|k| crate::auth::sids_equal(k, sid))
            .cloned();
        let key = matched?;
        let entry = inner.sessions.get_mut(&key)?;
        if now.duration_since(entry.last_seen) >= ttl {
            inner.sessions.remove(&key);
            return None;
        }
        entry.last_seen = now;
        Some(entry.scope)
    }

    /// Revoke a session (immediate; the cookie dies with it). Constant-time
    /// sid match, same rationale as [`SessionStore::validate`].
    pub fn revoke(&self, sid: &str) {
        let mut inner = self.lock_inner();
        if let Some(key) = inner
            .sessions
            .keys()
            .find(|k| crate::auth::sids_equal(k, sid))
            .cloned()
        {
            inner.sessions.remove(&key);
        }
    }

    /// Number of live sessions (observability / tests).
    pub fn live_count(&self) -> usize {
        self.lock_inner().sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn create_validate_slide_and_revoke() {
        let store = SessionStore::new(Duration::from_secs(60));
        let sid = store.create(KeyScope::Admin);
        assert_eq!(store.live_count(), 1);
        assert_eq!(store.validate(&sid), Some(KeyScope::Admin));
        store.revoke(&sid);
        assert_eq!(store.validate(&sid), None);
        assert_eq!(store.live_count(), 0);
    }

    #[test]
    fn expired_session_is_invalid_and_removed() {
        let store = SessionStore::new(Duration::from_secs(1));
        let sid = store.create(KeyScope::Inference);
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(store.validate(&sid), None);
        assert_eq!(store.live_count(), 0, "过期条目必须被清理");
    }

    #[test]
    fn sliding_refresh_keeps_session_alive() {
        let store = SessionStore::new(Duration::from_secs(2));
        let sid = store.create(KeyScope::Readonly);
        // 每 1s 使用一次 → last_seen 持续滑动 → 永不因整体年龄过期
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(900));
            assert_eq!(store.validate(&sid), Some(KeyScope::Readonly));
        }
    }

    #[test]
    fn lru_eviction_at_cap() {
        let store = SessionStore::new(Duration::from_secs(60));
        let mut sids = Vec::new();
        for _ in 0..MAX_SESSIONS {
            sids.push(store.create(KeyScope::Admin));
        }
        assert_eq!(store.live_count(), MAX_SESSIONS);
        // 触碰第一个 sid（成为最新）→ 下一次 create 应淘汰"最旧"（第二个）
        assert!(store.validate(&sids[0]).is_some());
        let new_sid = store.create(KeyScope::Admin);
        assert_eq!(store.live_count(), MAX_SESSIONS, "容量必须封顶");
        assert!(store.validate(&sids[1]).is_none(), "最旧条目应被 LRU 淘汰");
        assert!(store.validate(&sids[0]).is_some());
        assert!(store.validate(&new_sid).is_some());
    }
}
