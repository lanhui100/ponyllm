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

/// Hard cap on concurrent live sessions (LRU evicted past this).
pub const MAX_SESSIONS: usize = 4096;

/// Default TTL in seconds (contract: Max-Age=28800, 8h sliding).
pub const DEFAULT_SESSION_TTL_SECS: u64 = 28800;

/// One live session: the scope captured at exchange time + timestamps.
#[derive(Debug, Clone)]
struct SessionEntry {
    scope: KeyScope,
    created_at: Instant,
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

    /// Issue a new session for `scope`; returns the opaque sid.
    /// Bounded: past [`MAX_SESSIONS`] the least-recently-used entry is
    /// evicted (attacker sid-spray cannot grow the table unboundedly).
    pub fn create(&self, scope: KeyScope) -> String {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        if inner.sessions.len() >= MAX_SESSIONS {
            if let Some(evict) = inner
                .sessions
                .iter()
                .min_by_key(|(_, e)| e.last_seen)
                .map(|(k, _)| k.clone())
            {
                inner.sessions.remove(&evict);
            }
        }
        let sid = uuid::Uuid::new_v4().simple().to_string();
        inner.sessions.insert(
            sid.clone(),
            SessionEntry {
                scope,
                created_at: now,
                last_seen: now,
            },
        );
        sid
    }

    /// Validate `sid` and SLIDE its TTL. `None` when unknown or expired (the
    /// expired entry is removed so it cannot linger).
    pub fn validate(&self, sid: &str) -> Option<KeyScope> {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        let ttl = inner.ttl;
        let entry = inner.sessions.get_mut(sid)?;
        if now.duration_since(entry.last_seen) >= ttl {
            inner.sessions.remove(sid);
            return None;
        }
        entry.last_seen = now;
        Some(entry.scope)
    }

    /// Revoke a session (immediate; the cookie dies with it).
    pub fn revoke(&self, sid: &str) {
        self.lock_inner().sessions.remove(sid);
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
