//! F2 (VULN-01): client-IP-scoped authentication failure rate limiting.
//!
//! Sliding-window budget per `(resolved client IP, key-prefix)` pair. The
//! budget is consumed ONLY by failed authentications; successful calls never
//! touch it. Exceeding the budget locks the pair with tiered backoff
//! (base → ×4 → ×16), answered as HTTP 429 (`auth::rate_limited`).
//!
//! Multi-replica note: each pod keeps its own in-memory budgets (capacity
//! scales ×replicas); a shared PG-backed counter is deferred (VULN-01 B项).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sliding-window state for one (IP, prefix) budget.
#[derive(Debug)]
struct Budget {
    /// Failure timestamps inside the current window (oldest first).
    failures: VecDeque<Instant>,
    /// Lockout active until this instant (tiered backoff).
    locked_until: Option<Instant>,
    /// Consecutive lockouts this budget has served (tier escalation).
    lockout_count: u32,
}

impl Budget {
    fn fresh() -> Self {
        Self {
            failures: VecDeque::new(),
            locked_until: None,
            lockout_count: 0,
        }
    }
}

/// Anti-DoS bound on simultaneous (ip, prefix) budgets: an attacker spraying
/// distinct forged addresses must not grow the map without limit. When the
/// cap is hit, expired entries are swept first; if still at the cap, one
/// existing budget is evicted (worst case: that pair's failures are forgiven).
const MAX_BUDGETS: usize = 4096;

/// Lock-protected state: budgets plus the last global-sweep timestamp so the
/// full-map scan is amortized (at most once per window, or when oversized).
#[derive(Debug)]
struct Inner {
    budgets: HashMap<(IpAddr, &'static str), Budget>,
    last_sweep: Instant,
}

/// Threshold/backoff configuration + per-budget state.
#[derive(Debug)]
pub struct AuthRateLimiter {
    window: Duration,
    limit: u32,
    lockout: Duration,
    inner: Mutex<Inner>,
}

impl AuthRateLimiter {
    pub fn new(window_secs: u64, limit: u32, lockout_secs: u64) -> Self {
        Self {
            window: Duration::from_secs(window_secs.max(1)),
            limit: limit.max(1),
            lockout: Duration::from_secs(lockout_secs.max(1)),
            inner: Mutex::new(Inner {
                budgets: HashMap::new(),
                last_sweep: Instant::now(),
            }),
        }
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Per-entry cleanup (O(1), runs on every access to this budget): drop
    /// failures older than the window and clear an expired lockout.
    fn prune_budget(b: &mut Budget, window: &Duration, now: Instant) {
        if let Some(until) = b.locked_until {
            if now >= until {
                b.locked_until = None;
            }
        }
        let cutoff = now - *window;
        b.failures.retain(|t| *t >= cutoff);
    }

    /// Full-map sweep (O(n)): drop expired lockouts and budgets that no
    /// longer carry any signal, then record the sweep time. R2 (Phase-2b):
    /// `lockout_count` never keeps an entry alive — after lockout expiry with
    /// zero in-window failures the budget is reclaimed (memory bounded).
    fn sweep(inner: &mut Inner, window: &Duration, now: Instant) {
        let cutoff = now - *window;
        inner.budgets.retain(|_, b| {
            let locked = b.locked_until.map(|u| now < u).unwrap_or(false);
            if !locked && b.locked_until.is_some() {
                b.locked_until = None;
            }
            b.failures.retain(|t| *t >= cutoff);
            !b.failures.is_empty() || locked
        });
        inner.last_sweep = now;
    }

    /// Amortized sweep gate: run the full scan at most once per window, or
    /// immediately when the map approaches its cap — per-request cost stays
    /// O(1) amortized instead of O(n) every request (R2 DoS hardening).
    fn maybe_sweep(inner: &mut Inner, window: &Duration, now: Instant) {
        let due = now.duration_since(inner.last_sweep) >= *window;
        if due || inner.budgets.len() >= MAX_BUDGETS {
            Self::sweep(inner, window, now);
        }
    }

    /// Pre-authentication gate: `Err(())` means the (ip, prefix) pair is
    /// locked out or already over budget → caller answers 429. Must be called
    /// BEFORE `authenticate` so an attacker cannot burn SHA-256 CPU first.
    pub fn check(&self, ip: IpAddr, prefix: &'static str) -> Result<(), ()> {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        Self::maybe_sweep(&mut inner, &self.window, now);
        if let Some(b) = inner.budgets.get_mut(&(ip, prefix)) {
            Self::prune_budget(b, &self.window, now);
            if let Some(until) = b.locked_until {
                if now < until {
                    return Err(());
                }
            }
            if b.failures.len() as u32 >= self.limit {
                return Err(());
            }
        }
        Ok(())
    }

    /// Record one failed authentication (call after `authenticate` returned
    /// `Invalid`/`LegacyDisabled`). Never called on success.
    pub fn record_failure(&self, ip: IpAddr, prefix: &'static str) {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        Self::maybe_sweep(&mut inner, &self.window, now);
        let key = (ip, prefix);
        // Hard bound: if the map is still at the cap after a sweep and this
        // is a brand-new budget, evict one existing entry (attacker-spray DoS).
        if !inner.budgets.contains_key(&key) && inner.budgets.len() >= MAX_BUDGETS {
            if let Some(k) = inner.budgets.keys().next().cloned() {
                inner.budgets.remove(&k);
            }
        }
        let b = inner.budgets.entry(key).or_insert_with(Budget::fresh);
        Self::prune_budget(b, &self.window, now);
        b.failures.push_back(now);
        let cutoff = now - self.window;
        while b.failures.front().map(|t| *t < cutoff).unwrap_or(false) {
            b.failures.pop_front();
        }
        if b.failures.len() as u32 >= self.limit {
            // Tiered backoff: base × 4^tier, tier capped at 2 (900→3600→14400).
            let tier = b.lockout_count.min(2);
            let d = self.lockout.saturating_mul(4u32.pow(tier));
            b.locked_until = Some(now + d);
            b.lockout_count += 1;
            b.failures.clear();
        }
    }

    /// Force a full sweep now (R2 contract): drop expired lockouts and empty
    /// budgets. `live_budget_count()` then reflects live state only. The
    /// amortized request paths call `sweep` internally; tests call this
    /// explicitly after sleeping past the window.
    pub fn prune_expired(&self) {
        let now = Instant::now();
        let mut inner = self.lock_inner();
        Self::sweep(&mut inner, &self.window, now);
    }

    /// Number of live (ip, prefix) budgets (R2 observability: memory-bounded
    /// assertion).
    pub fn live_budget_count(&self) -> usize {
        let inner = self.lock_inner();
        inner.budgets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(o: [u8; 4]) -> IpAddr {
        IpAddr::V4(o.into())
    }

    #[test]
    fn budget_allows_until_limit_then_429() {
        let rl = AuthRateLimiter::new(60, 3, 900);
        let ip = v4([203, 0, 113, 50]);
        assert_eq!(rl.check(ip, "admin"), Ok(()));
        rl.record_failure(ip, "admin");
        rl.record_failure(ip, "admin");
        assert_eq!(rl.check(ip, "admin"), Ok(()));
        rl.record_failure(ip, "admin"); // hits limit → lockout starts
        assert_eq!(rl.check(ip, "admin"), Err(()));
    }

    #[test]
    fn per_prefix_budgets_are_independent() {
        let rl = AuthRateLimiter::new(60, 3, 900);
        let ip = v4([203, 0, 113, 60]);
        for _ in 0..4 {
            rl.record_failure(ip, "admin");
        }
        assert_eq!(rl.check(ip, "admin"), Err(()));
        assert_eq!(rl.check(ip, "infer"), Ok(()));
    }

    #[test]
    fn correct_key_not_affected_without_failures() {
        let rl = AuthRateLimiter::new(60, 3, 900);
        let ip = v4([198, 51, 100, 77]);
        assert_eq!(rl.check(ip, "legacy"), Ok(()));
    }
}