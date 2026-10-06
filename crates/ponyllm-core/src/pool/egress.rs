//! Egress pool: per-provider exit-IP rotation with independent per-entry
//! cooldowns (contract `2026-10-07-egress-pool-contract`).
//!
//! Scheduling dimension for upstreams that meter quota by the **client exit
//! IP** rather than by API key (openCode zen free tier: Redis
//! `ratelimit:ip:<ip>:<YYYYMMDD>`, `public` keys normalized to anonymous).
//! Each entry is one exit shape:
//! - `url == None` — direct dial on the gateway node's own exit IP;
//! - `url == Some("http(s)://host:port" | "socks5://...")` — dial through
//!   that forward proxy (one distinct exit IP per proxy).
//!
//! Rotation is round-robin (internal counter, skipping cooling entries) or
//! priority (first available, insertion order = priority). Every entry keeps
//! its own cooldown: an upstream 429 `FreeUsageLimitError` (classified
//! `QuotaExhausted` by the executor) cools only the offending exit, so one
//! exhausted IP never drags the others down. When every entry is cooling the
//! pool surfaces `CoreError::NoAvailableKey(provider)` — the discriminator
//! the executor maps to `quota_exhausted` semantics (mirroring the key pool's
//! `any_key_quota_cooldown` gate).

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;

use crate::error::CoreError;

/// Ceiling for any single egress cooldown: mirrors the key pool's
/// [`crate::pool::entry::MAX_COOLDOWN`] (30 days) so a garbled or hostile
/// upstream reset hint cannot pin an exit for a pathological span.
pub const MAX_EGRESS_COOLDOWN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Default egress cooldown when the upstream 429 carried no reset hint:
/// 900s (15 minutes) — the conservative free-window recovery assumption
/// (contract scheduling semantics: body `Resets in` > `Retry-After` header >
/// this default).
pub const DEFAULT_EGRESS_COOLDOWN: Duration = Duration::from_secs(900);

/// Egress rotation strategy (contract C2). `egress_strategy` stays a plain
/// string in the TOML model; this is the canonical parse entry
/// (`EgressStrategy::from_str`), pinned by the core acceptance tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EgressStrategy {
    /// Rotate by an internal counter, skipping cooling entries.
    #[default]
    RoundRobin,
    /// Always take the first non-cooling entry (insertion order = priority).
    Priority,
}

impl FromStr for EgressStrategy {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "round_robin" => Ok(Self::RoundRobin),
            "priority" => Ok(Self::Priority),
            other => Err(format!(
                "invalid egress strategy '{}': must be round_robin or priority",
                other
            )),
        }
    }
}

impl fmt::Display for EgressStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RoundRobin => "round_robin",
            Self::Priority => "priority",
        })
    }
}

/// One exit in the pool.
///
/// `id` is a caller-chosen internal identifier (e.g. `"direct"` / `"vps"`,
/// unique per pool). `url == None` means a direct dial on the gateway node's
/// own exit IP; `url == Some(proxy)` dials through that forward proxy.
///
/// Cooldown state lives on the entry (interior mutability, shared through the
/// `Arc` handed out by [`EgressPool::select_egress`]) and mirrors the key
/// entry conventions: monotonic deadline + wall-clock reset mirror written in
/// lockstep, `MAX_EGRESS_COOLDOWN` clamp, keep-max on re-cooldown.
#[derive(Debug)]
pub struct EgressEntry {
    pub id: String,
    pub url: Option<String>,
    cooldown_until: RwLock<Option<Instant>>,
    cooldown_reset_at: RwLock<Option<SystemTime>>,
    transient_failures: AtomicU64,
}

impl EgressEntry {
    /// Direct exit (gateway node's own IP).
    pub fn direct(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: None,
            cooldown_until: RwLock::new(None),
            cooldown_reset_at: RwLock::new(None),
            transient_failures: AtomicU64::new(0),
        }
    }

    /// Exit through a forward proxy URL (one distinct exit IP per proxy).
    pub fn proxy(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: Some(url.into()),
            cooldown_until: RwLock::new(None),
            cooldown_reset_at: RwLock::new(None),
            transient_failures: AtomicU64::new(0),
        }
    }

    /// Remaining cooldown, if still cooling.
    pub fn cooldown_remaining(&self) -> Option<Duration> {
        let until = (*self.cooldown_until.read())?;
        let now = Instant::now();
        if now < until {
            Some(until - now)
        } else {
            None
        }
    }

    /// Wall-clock instant the exit is expected to recover, while still cooling.
    pub fn cooldown_reset_at(&self) -> Option<SystemTime> {
        let until = (*self.cooldown_until.read())?;
        if Instant::now() >= until {
            return None;
        }
        *self.cooldown_reset_at.read()
    }

    /// Apply a cooldown, keeping the monotonic deadline and its wall-clock
    /// mirror in lockstep (mirror of the key entry's `set_cooldown`).
    ///
    /// A later deadline always wins: an in-flight transient failure must never
    /// shorten a cooldown already advertised by a quota reset, or the exit
    /// starts receiving traffic again mid-window. Durations are clamped to
    /// [`MAX_EGRESS_COOLDOWN`]; if either clock addition fails the whole
    /// update is skipped, so a garbled/hostile reset can neither panic nor
    /// desync.
    pub fn set_cooldown(&self, duration: Duration) {
        let duration = duration.min(MAX_EGRESS_COOLDOWN);
        let Some(deadline) = Instant::now().checked_add(duration) else {
            return;
        };
        let Some(reset_at) = SystemTime::now().checked_add(duration) else {
            return;
        };
        let mut cd = self.cooldown_until.write();
        if let Some(existing) = *cd {
            if existing >= deadline {
                return;
            }
        }
        *self.cooldown_reset_at.write() = Some(reset_at);
        *cd = Some(deadline);
    }

    /// Clear any active cooldown, immediately returning the exit to Active.
    pub fn clear_cooldown(&self) {
        *self.cooldown_until.write() = None;
        *self.cooldown_reset_at.write() = None;
    }

    /// Number of transient failures recorded (never cooling, see
    /// [`EgressPool::record_transient_failure`]).
    pub fn transient_failures(&self) -> u64 {
        self.transient_failures.load(Ordering::Relaxed)
    }

    fn is_cooling(&self) -> bool {
        self.cooldown_until
            .read()
            .is_some_and(|until| Instant::now() < until)
    }
}

/// Wire state of one egress entry for the admin quota view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressState {
    Active,
    Cooling,
}

impl EgressState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Cooling => "cooling",
        }
    }
}

/// Snapshot row of one egress entry for the admin surface (per-entry
/// `state` / `cooldown_reset_at`, contract C7).
#[derive(Debug, Clone)]
pub struct EgressStatus {
    /// Position in the pool (insertion order).
    pub index: usize,
    /// `"direct"` or the raw proxy URL.
    pub entry: String,
    pub state: EgressState,
    pub cooldown_reset_at: Option<SystemTime>,
}

/// Per-provider egress pool: entries in insertion order + rotation counter.
/// Selection never mutates cooldowns; only explicit quota-exhausted signals
/// cool an entry, and transient failures only count toward the failure stats.
#[derive(Debug)]
pub struct EgressPool {
    provider: String,
    strategy: EgressStrategy,
    counter: AtomicUsize,
    entries: RwLock<Vec<Arc<EgressEntry>>>,
}

impl EgressPool {
    /// Empty pool for `provider`; entries are added via [`Self::add_egress`].
    pub fn new(provider: &str, strategy: EgressStrategy) -> Self {
        Self {
            provider: provider.to_string(),
            strategy,
            counter: AtomicUsize::new(0),
            entries: RwLock::new(Vec::new()),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn strategy(&self) -> EgressStrategy {
        self.strategy
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// Append one exit. The id must be unique per pool (callers own the
    /// identifier space, e.g. `"direct"` / `"vps"`).
    pub fn add_egress(&self, entry: EgressEntry) {
        self.entries.write().push(Arc::new(entry));
    }

    /// Select the next exit for one upstream attempt, skipping cooling
    /// entries. `Err(CoreError::NoAvailableKey(provider))` when the pool is
    /// empty or every entry is cooling (contract C6).
    pub fn select_egress(&self) -> Result<Arc<EgressEntry>, CoreError> {
        let entries = self.entries.read();
        let n = entries.len();
        if n == 0 {
            return Err(CoreError::NoAvailableKey(self.provider.clone()));
        }
        match self.strategy {
            EgressStrategy::RoundRobin => {
                // The counter advances on every selection, even when the
                // landed slot is cooling, so rotation resumes cleanly after a
                // cooldown expires instead of pinning the first active slot.
                let start = self.counter.fetch_add(1, Ordering::Relaxed) % n;
                for offset in 0..n {
                    let candidate = &entries[(start + offset) % n];
                    if !candidate.is_cooling() {
                        return Ok(candidate.clone());
                    }
                }
            }
            EgressStrategy::Priority => {
                for candidate in entries.iter() {
                    if !candidate.is_cooling() {
                        return Ok(candidate.clone());
                    }
                }
            }
        }
        Err(CoreError::NoAvailableKey(self.provider.clone()))
    }

    /// Cool `egress_id` after an upstream quota-exhaustion signal (429
    /// `FreeUsageLimitError` / quota wording / 402). `retry_after` is the
    /// reset the caller resolved (body `Resets in` > `Retry-After` header);
    /// `None` falls back to [`DEFAULT_EGRESS_COOLDOWN`] (900s). The wall-clock
    /// `cooldown_reset_at` is derived internally from the duration.
    pub fn record_quota_exhausted(&self, egress_id: &str, retry_after: Option<Duration>) {
        if let Some(entry) = self.find(egress_id) {
            entry.set_cooldown(retry_after.unwrap_or(DEFAULT_EGRESS_COOLDOWN));
        }
    }

    /// Record a transient failure (network / 5xx / TTFB) on `egress_id`:
    /// count only, never cool — the next attempt fails over to another exit
    /// (contract scheduling semantics).
    pub fn record_transient_failure(&self, egress_id: &str) {
        if let Some(entry) = self.find(egress_id) {
            entry.transient_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `(remaining, wall-clock reset)` for `egress_id`, while it is cooling.
    pub fn egress_cooldown(&self, id: &str) -> (Option<Duration>, Option<SystemTime>) {
        match self.find(id) {
            Some(entry) => (entry.cooldown_remaining(), entry.cooldown_reset_at()),
            None => (None, None),
        }
    }

    /// Whether every entry is currently cooling (vacuously `true` for an
    /// empty pool — no exit is selectable either way).
    pub fn all_cooling(&self) -> bool {
        let entries = self.entries.read();
        entries.iter().all(|e| e.is_cooling())
    }

    /// Operator/test recovery: clear the cooldown of `egress_id`.
    pub fn clear_egress_cooldown(&self, egress_id: &str) {
        if let Some(entry) = self.find(egress_id) {
            entry.clear_cooldown();
        }
    }

    /// Snapshot rows for the admin quota view: per-entry index / entry string
    /// / state / cooldown_reset_at.
    pub fn status(&self) -> Vec<EgressStatus> {
        let entries = self.entries.read();
        entries
            .iter()
            .enumerate()
            .map(|(index, e)| EgressStatus {
                index,
                entry: e.url.clone().unwrap_or_else(|| "direct".to_string()),
                state: if e.is_cooling() {
                    EgressState::Cooling
                } else {
                    EgressState::Active
                },
                cooldown_reset_at: e.cooldown_reset_at(),
            })
            .collect()
    }

    /// Unique proxy URLs across the pool, in insertion order (direct entries
    /// are skipped). Used by the server to build the per-exit upstream
    /// clients.
    pub fn proxy_urls(&self) -> Vec<String> {
        let entries = self.entries.read();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in entries.iter() {
            if let Some(u) = &e.url {
                if seen.insert(u.clone()) {
                    out.push(u.clone());
                }
            }
        }
        out
    }

    /// Stable shape identity (strategy + ordered entries), independent of
    /// live cooldown state. Lets hot reloads keep the SAME `Arc<EgressPool>`
    /// (with its per-exit cooldowns) when nothing about the pool changed.
    pub fn shape(&self) -> (EgressStrategy, Vec<Option<String>>) {
        let entries = self.entries.read();
        (
            self.strategy,
            entries.iter().map(|e| e.url.clone()).collect(),
        )
    }

    fn find(&self, id: &str) -> Option<Arc<EgressEntry>> {
        self.entries.read().iter().find(|e| e.id == id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_from_str_and_default() {
        assert_eq!(
            "round_robin".parse::<EgressStrategy>().unwrap(),
            EgressStrategy::RoundRobin
        );
        assert_eq!(
            "priority".parse::<EgressStrategy>().unwrap(),
            EgressStrategy::Priority
        );
        assert_eq!(
            " Round_Robin ".parse::<EgressStrategy>().unwrap(),
            EgressStrategy::RoundRobin
        );
        assert!("weighted".parse::<EgressStrategy>().is_err());
        assert_eq!(EgressStrategy::default(), EgressStrategy::RoundRobin);
        assert_eq!(EgressStrategy::Priority.to_string(), "priority");
    }

    #[test]
    fn round_robin_rotates_and_skips_cooling() {
        let pool = EgressPool::new("p", EgressStrategy::RoundRobin);
        pool.add_egress(EgressEntry::direct("direct"));
        pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        let ids: Vec<String> = (0..4)
            .map(|_| pool.select_egress().unwrap().id.clone())
            .collect();
        assert_eq!(ids, vec!["direct", "vps", "direct", "vps"]);
        pool.record_quota_exhausted("direct", Some(Duration::from_secs(3600)));
        for _ in 0..3 {
            assert_eq!(pool.select_egress().unwrap().id, "vps");
        }
        pool.clear_egress_cooldown("direct");
        let ids: Vec<String> = (0..4)
            .map(|_| pool.select_egress().unwrap().id.clone())
            .collect();
        assert_eq!(ids, vec!["vps", "direct", "vps", "direct"]);
    }

    #[test]
    fn priority_pins_first_available() {
        let pool = EgressPool::new("p", EgressStrategy::Priority);
        pool.add_egress(EgressEntry::direct("direct"));
        pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        for _ in 0..3 {
            assert_eq!(pool.select_egress().unwrap().id, "direct");
        }
        pool.record_quota_exhausted("direct", None);
        for _ in 0..3 {
            assert_eq!(pool.select_egress().unwrap().id, "vps");
        }
    }

    #[test]
    fn quota_exhausted_defaults_to_900s_and_other_entry_stays_active() {
        let pool = EgressPool::new("p", EgressStrategy::RoundRobin);
        pool.add_egress(EgressEntry::direct("direct"));
        pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        pool.record_quota_exhausted("vps", None);
        let (rem, reset) = pool.egress_cooldown("vps");
        assert!((850..=950).contains(&rem.unwrap().as_secs()));
        assert!(reset.is_some());
        assert_eq!(pool.egress_cooldown("direct").0, None);
        assert_eq!(pool.select_egress().unwrap().id, "direct");
    }

    #[test]
    fn transient_failure_never_cools() {
        let pool = EgressPool::new("p", EgressStrategy::RoundRobin);
        pool.add_egress(EgressEntry::direct("direct"));
        pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        pool.record_transient_failure("vps");
        assert_eq!(pool.egress_cooldown("vps").0, None);
        assert_eq!(pool.select_egress().unwrap().id, "direct");
    }

    #[test]
    fn all_cooling_yields_no_available_key() {
        let pool = EgressPool::new("opencode-zen", EgressStrategy::RoundRobin);
        pool.add_egress(EgressEntry::direct("direct"));
        pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
        pool.record_quota_exhausted("direct", Some(Duration::from_secs(3600)));
        pool.record_quota_exhausted("vps", Some(Duration::from_secs(3600)));
        assert!(pool.all_cooling());
        match pool.select_egress() {
            Err(CoreError::NoAvailableKey(p)) => assert_eq!(p, "opencode-zen"),
            other => panic!("expected NoAvailableKey, got {other:?}"),
        }
    }
}
