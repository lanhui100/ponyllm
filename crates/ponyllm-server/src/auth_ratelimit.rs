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

/// Threshold/backoff configuration + per-budget state.
#[derive(Debug)]
pub struct AuthRateLimiter {
    window: Duration,
    limit: u32,
    lockout: Duration,
    budgets: Mutex<HashMap<(IpAddr, &'static str), Budget>>,
}

impl AuthRateLimiter {
    pub fn new(window_secs: u64, limit: u32, lockout_secs: u64) -> Self {
        Self {
            window: Duration::from_secs(window_secs.max(1)),
            limit: limit.max(1),
            lockout: Duration::from_secs(lockout_secs.max(1)),
            budgets: Mutex::new(HashMap::new()),
        }
    }

    /// Pre-authentication gate: `Err(())` means the (ip, prefix) pair is
    /// locked out or already over budget → caller answers 429. Must be called
    /// BEFORE `authenticate` so an attacker cannot burn SHA-256 CPU first.
    pub fn check(&self, ip: IpAddr, prefix: &'static str) -> Result<(), ()> {
        let now = Instant::now();
        let mut map = match self.budgets.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::prune(&mut map, &self.window, now);
        if let Some(b) = map.get(&(ip, prefix)) {
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
        let mut map = match self.budgets.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::prune(&mut map, &self.window, now);
        let key = (ip, prefix);
        let b = map.entry(key).or_insert_with(Budget::fresh);
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

    /// Drop expired state: prune old failure timestamps, clear expired
    /// lockouts, drop budgets that no longer carry any signal.
    fn prune(
        map: &mut HashMap<(IpAddr, &'static str), Budget>,
        window: &Duration,
        now: Instant,
    ) {
        let cutoff = now - *window;
        map.retain(|_, b| {
            if let Some(until) = b.locked_until {
                if now < until {
                    return true;
                }
                b.locked_until = None;
            }
            b.failures.retain(|t| *t >= cutoff);
            !b.failures.is_empty() || b.lockout_count > 0
        });
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