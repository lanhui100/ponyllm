use super::entry::{ApiKeyEntry, KeyState, PoolErrorType};
use super::strategy::RoutingStrategy;
use crate::error::{CoreError, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Short-window account rate limits (config-driven budget, ADR
/// `2026-09-30-unified-quota-metering-governance-kernel`).
///
/// Every field is optional so legacy configs without the section stay
/// unlimited. Numeric budgets (`rpm`/`tpm`/`concurrency`) treat `None` and
/// `0` identically: no limit. The resolved sliding-window budget is applied
/// per key through the key's [`ShortWindowMeter`](super::meter::ShortWindowMeter)
/// at selection time — a key whose short-window usage is at its limit is
/// treated exactly like a cooling key (skipped, and surfaced through
/// [`KeyPool::exhausted_by_window`] / [`KeyPool::window_refill_in`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RateLimits {
    /// Max requests admitted per sliding window (`None`/`0` = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    /// Max tokens admitted per sliding window (`None`/`0` = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u64>,
    /// Sliding window size in seconds (`None` = [`RateLimits::DEFAULT_WINDOW_SECS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<u64>,
    /// Max in-flight concurrent requests (`None`/`0` = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    /// Whether cache-hit tokens count toward the TPM budget
    /// (`None` = `true`, per upstream accounting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_cached: Option<bool>,
}

impl RateLimits {
    /// Sliding window used when `window_secs` is unset: 60s.
    pub const DEFAULT_WINDOW_SECS: u64 = 60;

    /// Effective sliding window size.
    pub fn window_secs_effective(&self) -> u64 {
        self.window_secs.unwrap_or(Self::DEFAULT_WINDOW_SECS)
    }

    /// Effective cached-token accounting (default: cached tokens count).
    pub fn count_cached_effective(&self) -> bool {
        self.count_cached.unwrap_or(true)
    }

    /// `None`/`0` both mean unlimited, folded to `None` here.
    pub fn rpm_effective(&self) -> Option<u32> {
        self.rpm.filter(|v| *v > 0)
    }

    /// `None`/`0` both mean unlimited, folded to `None` here.
    pub fn tpm_effective(&self) -> Option<u64> {
        self.tpm.filter(|v| *v > 0)
    }

    /// `None`/`0` both mean unlimited, folded to `None` here.
    pub fn concurrency_effective(&self) -> Option<u32> {
        self.concurrency.filter(|v| *v > 0)
    }

    /// True when no budget dimension constrains scheduling.
    pub fn is_unlimited(&self) -> bool {
        self.rpm_effective().is_none()
            && self.tpm_effective().is_none()
            && self.concurrency_effective().is_none()
    }

    /// Resolve effective limits: the model-level override wins field by field
    /// over the provider-level default; both `None` resolve to `None`
    /// (unlimited). Window/cached accounting defaults are applied later by
    /// [`RateLimits::window_secs_effective`] / [`RateLimits::count_cached_effective`].
    pub fn resolve(
        provider_default: Option<&RateLimits>,
        model_override: Option<&RateLimits>,
    ) -> Option<RateLimits> {
        match (provider_default, model_override) {
            (None, None) => None,
            (Some(p), None) => Some(*p),
            (None, Some(m)) => Some(*m),
            (Some(p), Some(m)) => Some(RateLimits {
                rpm: m.rpm.or(p.rpm),
                tpm: m.tpm.or(p.tpm),
                window_secs: m.window_secs.or(p.window_secs),
                concurrency: m.concurrency.or(p.concurrency),
                count_cached: m.count_cached.or(p.count_cached),
            }),
        }
    }

    /// Fail-fast validation for config-edited limits.
    ///
    /// The short-window meter ring is 12 × 5s = 60s, so `window_secs` must
    /// live in `1..=60`: `0` degenerates the sliding window (every attempt
    /// instantly out of window), and `> 60` would silently run as 60s,
    /// loosening the budget by roughly a factor of `window_secs/60`. Longer
    /// horizons belong to the `CycleStats` periodic buckets, not this meter.
    /// The numeric budgets accept `0` as "unlimited" by contract.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(w) = self.window_secs {
            if w == 0 || w > 60 {
                return Err(format!(
                    "rate_limits.window_secs 必须在 1..=60（收到 {}），长窗限额请走 CycleStats 周期桶",
                    w
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct KeyPool {
    pub provider: String,
    pub strategy: RoutingStrategy,
    keys: RwLock<Vec<Arc<ApiKeyEntry>>>,
    rr_counter: AtomicUsize,
}

impl KeyPool {
    pub fn new(provider: impl Into<String>, strategy: RoutingStrategy) -> Self {
        Self {
            provider: provider.into(),
            strategy,
            keys: RwLock::new(Vec::new()),
            rr_counter: AtomicUsize::new(0),
        }
    }

    pub fn add_key(&self, entry: ApiKeyEntry) {
        let mut keys = self.keys.write();
        if let Some(existing_idx) = keys.iter().position(|k| k.id == entry.id) {
            keys[existing_idx] = Arc::new(entry);
        } else {
            keys.push(Arc::new(entry));
        }
        // Sort keys primarily by priority (ascending: 1, 2, 3...)
        keys.sort_by_key(|k| k.priority);
    }

    /// Hot-reload survival: transplant the usage-tracker (measurement state:
    /// slices, completed cycles, capacity EWMA) of the donor entry with the
    /// same key id into this pool's fresh entries. Keeps per-key cycle history
    /// across config rebuilds — data must not reset on account/config churn.
    pub fn import_matched_usage_trackers(
        &self,
        donors: &HashMap<String, Arc<ApiKeyEntry>>,
    ) -> usize {
        let mut keys = self.keys.write();
        let mut transplanted = 0usize;
        for entry in keys.iter_mut() {
            let Some(donor) = donors.get(&entry.id) else {
                continue;
            };
            let tracker = donor.usage_tracker.clone();
            let mut replaced = ApiKeyEntry::new(
                entry.id.clone(),
                entry.api_key.clone(),
                entry.priority,
                entry.weight,
            );
            replaced.account_id = entry.account_id.clone();
            replaced.auth = entry.auth.clone();
            replaced.usage_tracker = tracker;
            *entry = Arc::new(replaced);
            transplanted += 1;
        }
        transplanted
    }

    pub fn get_key_status(&self, key_id: &str) -> Option<KeyState> {
        let keys = self.keys.read();
        keys.iter()
            .find(|k| k.id == key_id)
            .map(|k| k.current_state())
    }

    /// Snapshot of all keys for admin observability (WEB-03): id/priority/weight
    /// plus effective state. Read-only; never exposes the raw key material.
    pub fn list_keys(&self) -> Vec<(String, u32, u32, KeyState)> {
        let keys = self.keys.read();
        keys.iter()
            .map(|k| (k.id.clone(), k.priority, k.weight, k.current_state()))
            .collect()
    }

    /// Read-only snapshot of key entries (e.g. for project-id peeking).
    /// Clones only the `Arc`s; counters and state stay live.
    pub fn snapshot_keys(&self) -> Vec<Arc<ApiKeyEntry>> {
        self.keys.read().clone()
    }

    /// Select the next active, healthy key according to configured routing strategy
    pub fn select_key(&self) -> Result<Arc<ApiKeyEntry>> {
        self.select_key_excluding(&[])
    }

    /// Select the next active, healthy key excluding already attempted keys in current request
    pub fn select_key_excluding(&self, excluded_key_ids: &[String]) -> Result<Arc<ApiKeyEntry>> {
        self.select_key_excluding_with_limits(excluded_key_ids, None)
    }

    /// Select key with KV-cache / session affinity, gracefully falling back to other accounts on congestion.
    pub fn select_key_with_affinity(
        &self,
        affinity_seed: Option<u64>,
        excluded_key_ids: &[String],
        limits: Option<&RateLimits>,
    ) -> Result<Arc<ApiKeyEntry>> {
        self.select_key_with_affinity_for_family(affinity_seed, excluded_key_ids, limits, None)
    }

    /// Select key with KV-cache / session affinity.
    ///
    /// The `family` hint is retained for API compatibility but **not** used to
    /// pre-reject keys whose family ledger (from a real upstream 429 or a
    /// probe bucket snapshot) currently has an unexpired verdict. Selection-time
    /// family gating is removed (ADR: `2026-10-04-antigravity-group-quota-aware-scheduling`):
    /// the family ledger only records live 429 outcomes for post-hoc 429
    /// semantics and honest unlock hints; it must never decide which key is
    /// schedulable. `family = None` keeps legacy key-level semantics, so
    /// non-Antigravity providers and unclassified models are never filtered.
    pub fn select_key_with_affinity_for_family(
        &self,
        affinity_seed: Option<u64>,
        excluded_key_ids: &[String],
        limits: Option<&RateLimits>,
        _family: Option<crate::pool::entry::QuotaFamily>,
    ) -> Result<Arc<ApiKeyEntry>> {
        let keys = self.keys.read();
        let active_keys: Vec<Arc<ApiKeyEntry>> = keys
            .iter()
            .filter(|k| {
                k.current_state() == KeyState::Active
                    && !excluded_key_ids.iter().any(|ex| ex == &k.id)
                    && Self::budget_ok(k, limits)
            })
            .cloned()
            .collect();

        if active_keys.is_empty() {
            return Err(CoreError::NoAvailableKey(self.provider.clone()));
        }

        // If affinity seed is provided, group candidate keys by effective account boundary
        if let Some(seed) = affinity_seed {
            let mut accounts: Vec<String> = active_keys
                .iter()
                .map(|k| k.effective_account_id().to_string())
                .collect();
            accounts.sort();
            accounts.dedup();

            if !accounts.is_empty() {
                // Consistent hash: select preferred account
                let chosen_idx = (seed as usize) % accounts.len();
                let chosen_account = &accounts[chosen_idx];

                // Keys inside this tenant account
                let account_keys: Vec<Arc<ApiKeyEntry>> = active_keys
                    .iter()
                    .filter(|k| k.effective_account_id() == chosen_account)
                    .cloned()
                    .collect();

                if !account_keys.is_empty() {
                    // Internal balance inside the chosen account to preserve concurrency & RPM
                    return Ok(Self::select_from_active(
                        account_keys,
                        &self.strategy,
                        &self.rr_counter,
                    ));
                }
            }
        }

        // Fallback / standard selection
        Ok(Self::select_from_active(
            active_keys,
            &self.strategy,
            &self.rr_counter,
        ))
    }

    /// Select the next active, healthy, budget-available key.
    ///
    /// Same semantics as [`KeyPool::select_key_excluding`], plus the
    /// short-window budget filter: a key whose meter is at its RPM/TPM limit
    /// (or at its concurrency cap) is treated exactly like a cooling key —
    /// skipped and surfaced through [`KeyPool::exhausted_by_window_with_limits`]
    /// / [`KeyPool::window_refill_in_with_limits`]. `limits = None` disables
    /// the budget dimension (legacy behavior). `rpm`/`tpm`/`concurrency` of
    /// `None` or `0` mean "no limit" on that axis (see [`RateLimits`]).
    pub fn select_key_excluding_with_limits(
        &self,
        excluded_key_ids: &[String],
        limits: Option<&RateLimits>,
    ) -> Result<Arc<ApiKeyEntry>> {
        self.select_key_with_affinity(None, excluded_key_ids, limits)
    }

    /// True when the key may receive a request right now under the given
    /// budget: concurrency cap not reached and both the request and token
    /// windows still have >= 1 unit of headroom.
    pub fn budget_ok(entry: &ApiKeyEntry, limits: Option<&RateLimits>) -> bool {
        let Some(limits) = limits else {
            return true;
        };
        if let Some(cap) = limits.concurrency_effective() {
            if entry.meter().in_flight() >= cap {
                return false;
            }
        }
        let (requests_left, tokens_left) = entry.meter().remaining(
            limits.rpm_effective(),
            limits.tpm_effective(),
            limits.window_secs_effective(),
            limits.count_cached_effective(),
        );
        if requests_left.is_some_and(|r| r < 1) {
            return false;
        }
        if tokens_left.is_some_and(|t| t < 1) {
            return false;
        }
        true
    }

    /// Strategy pick from an already-filtered, non-empty candidate list.
    fn select_from_active(
        active_keys: Vec<Arc<ApiKeyEntry>>,
        strategy: &RoutingStrategy,
        rr_counter: &AtomicUsize,
    ) -> Arc<ApiKeyEntry> {
        match strategy {
            RoutingStrategy::Priority => {
                // Return the lowest priority number (highest priority) available
                let mut sorted = active_keys;
                sorted.sort_by_key(|k| k.priority);
                sorted[0].clone()
            }
            RoutingStrategy::RoundRobin => {
                let idx = rr_counter.fetch_add(1, Ordering::Relaxed) % active_keys.len();
                active_keys[idx].clone()
            }
            RoutingStrategy::WeightedRoundRobin => {
                // Weighted selection based on weight field
                let total_weight: u32 = active_keys.iter().map(|k| k.weight.max(1)).sum();
                if total_weight == 0 {
                    let idx = rr_counter.fetch_add(1, Ordering::Relaxed) % active_keys.len();
                    return active_keys[idx].clone();
                }
                let count = rr_counter.fetch_add(1, Ordering::Relaxed) as u32 % total_weight;
                let mut acc = 0;
                for k in &active_keys {
                    acc += k.weight.max(1);
                    if count < acc {
                        return k.clone();
                    }
                }
                active_keys[0].clone()
            }
            RoutingStrategy::ConsistentHashAffinity => {
                // If strategy is explicitly ConsistentHashAffinity but selected via active fallback:
                // default to priority or first available
                active_keys[0].clone()
            }
        }
    }

    /// True when no key is schedulable and every non-disabled key is blocked
    /// by budget exhaustion or cooldown — the `exhausted-by-window` state
    /// (distinct from a permanent auth/disabled pool, which never refills).
    ///
    /// Budget dimension disabled (equivalent to cooldown-only) without limits.
    pub fn exhausted_by_window(&self) -> bool {
        self.exhausted_by_window_with_limits(None)
    }

    /// [`KeyPool::exhausted_by_window`] with an explicit budget.
    pub fn exhausted_by_window_with_limits(&self, limits: Option<&RateLimits>) -> bool {
        let keys = self.keys.read();
        let mut any_window_blocked = false;
        for k in keys.iter() {
            match k.current_state() {
                // Permanent states never refill via a window: they neither
                // count as blocked-by-window nor as schedulable.
                KeyState::Disabled => continue,
                KeyState::CoolingDown => any_window_blocked = true,
                KeyState::Active => {
                    if Self::budget_ok(k, limits) {
                        // A schedulable key exists → not exhausted by window.
                        return false;
                    }
                    any_window_blocked = true;
                }
            }
        }
        any_window_blocked
    }

    /// Shortest wait until at least one key of the pool is schedulable again
    /// (min across keys of cooldown end / budget refill; `0` = a key is
    /// available right now). `None` when no key can ever refill (all disabled)
    /// or the pool is empty.
    pub fn window_refill_in(&self) -> Option<Duration> {
        self.window_refill_in_with_limits(None)
    }

    /// [`KeyPool::window_refill_in`] with an explicit budget.
    pub fn window_refill_in_with_limits(&self, limits: Option<&RateLimits>) -> Option<Duration> {
        let keys = self.keys.read();
        let mut min: Option<Duration> = None;
        for k in keys.iter() {
            if k.current_state() == KeyState::Disabled {
                continue;
            }
            let wait = Self::time_until_schedulable(k, limits);
            if let Some(w) = wait {
                min = Some(min.map_or(w, |m: Duration| m.min(w)));
            }
        }
        min
    }

    /// Longest wait until the WHOLE pool is schedulable again (max across
    /// keys; `0` = fully available now). `None` when no key can ever refill
    /// (all disabled) or the pool is empty. Feeds an honest `Retry-After`.
    pub fn longest_window_refill_in(&self) -> Option<Duration> {
        self.longest_window_refill_in_with_limits(None)
    }

    /// [`KeyPool::longest_window_refill_in`] with an explicit budget.
    pub fn longest_window_refill_in_with_limits(
        &self,
        limits: Option<&RateLimits>,
    ) -> Option<Duration> {
        let keys = self.keys.read();
        let mut max: Option<Duration> = None;
        for k in keys.iter() {
            if k.current_state() == KeyState::Disabled {
                continue;
            }
            let wait = Self::time_until_schedulable(k, limits);
            if let Some(w) = wait {
                max = Some(max.map_or(w, |m: Duration| m.max(w)));
            }
        }
        max
    }

    /// Wall-clock milliseconds since the UNIX epoch — the same clock source
    /// the per-key [`ShortWindowMeter`](super::meter::ShortWindowMeter) uses,
    /// so expiry timestamps from [`earliest_expiry`](super::meter::ShortWindowMeter::earliest_expiry)
    /// convert to wait durations consistently.
    fn wall_now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Per-key wait until schedulable: cooldown end and budget refill must
    /// BOTH have passed (max of the two); a key already schedulable waits `0`.
    ///
    /// The budget refill term is precise, not a full-window upper bound: the
    /// blocking usage ages out of the meter's 60s ring when its earliest
    /// (oldest) in-window slot expires, so the wait is
    /// `min(earliest_expiry - now, window_secs)` — clamped to the configured
    /// window so it never exceeds what the window can guarantee (M3 caps the
    /// hold at `pool_wait_max` anyway).
    fn time_until_schedulable(
        entry: &ApiKeyEntry,
        limits: Option<&RateLimits>,
    ) -> Option<Duration> {
        let cooldown = entry.cooldown_remaining();
        let budget = match limits {
            Some(l) if !Self::budget_ok(entry, Some(l)) => {
                let window = Duration::from_secs(l.window_secs_effective());
                entry.meter().earliest_expiry().map(|expiry| {
                    Duration::from_millis(expiry.saturating_sub(Self::wall_now_ms())).min(window)
                })
            }
            _ => None,
        };
        match (cooldown, budget) {
            (None, None) => Some(Duration::ZERO),
            (Some(c), None) => Some(c),
            (None, Some(b)) => Some(b),
            (Some(c), Some(b)) => Some(c.max(b)),
        }
    }

    /// Record a successful request on a key
    pub fn record_success(&self, key_id: &str) {
        let keys = self.keys.read();
        if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
            entry.record_success();
        }
    }

    /// Record token usage on a key
    pub fn record_tokens(
        &self,
        key_id: &str,
        wall_ms: u64,
        prompt: u64,
        completion: u64,
        cached: u64,
    ) {
        let keys = self.keys.read();
        if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
            entry.record_tokens(wall_ms, prompt, completion, cached);
        }
    }

    /// Record an error on a key
    pub fn record_error(&self, key_id: &str, error: PoolErrorType) {
        let keys = self.keys.read();
        // Singleton passthrough (B2): a lone key stays Active through
        // up to 2 transient 429 retries. Sustained failures (>=2 consecutive)
        // MUST cool down to prevent hammering the upstream without backoff.
        if keys.len() == 1 {
            if let PoolErrorType::RateLimit { retry_after } = &error {
                if retry_after
                    .map(|d| d <= std::time::Duration::from_secs(60))
                    .unwrap_or(true)
                {
                    if let Some(entry) = keys.iter().find(|k| k.id == key_id) {
                        if entry.stats.consecutive_failures.load(Ordering::Relaxed) < 2 {
                            entry.record_transient_failure();
                            return;
                        }
                    }
                }
            }
        }
        let Some(entry) = keys.iter().find(|k| k.id == key_id).cloned() else {
            return;
        };

        let is_policy_violation = matches!(
            error,
            PoolErrorType::PolicyViolation | PoolErrorType::AccountValidationRequired
        );
        let violations = if is_policy_violation {
            entry
                .stats
                .policy_violations
                .fetch_add(1, Ordering::Relaxed)
                + 1
        } else {
            0
        };

        // Mass-disable circuit breaker: permanently isolating a key
        // while it would leave <=50% of the pool alive is downgraded to a
        // 5-minute cooling.
        // HOWEVER, a key that triggers PolicyViolation for a second time (violations >= 2)
        // is confirmed dead and must be permanently disabled to avoid infinite oscillation loops.
        let permanent = is_policy_violation
            || (matches!(error, PoolErrorType::AuthInvalid { .. }) && entry.is_antigravity());

        if permanent && violations < 2 && self.would_break_floor_locked(&keys, key_id) {
            tracing::warn!(
                provider = %self.provider,
                key_id = %key_id,
                error = ?error,
                violations = violations,
                "mass-disable breaker tripped: downgrading permanent isolate to 5m cooling"
            );
            entry.record_failure(PoolErrorType::RateLimit {
                retry_after: Some(std::time::Duration::from_secs(300)),
            });
            return;
        }
        entry.record_failure(error);
    }

    /// True when permanently isolating `key_id` would leave at most half of
    /// the pool alive. Single-key pools are exempt (no floor to protect).
    fn would_break_floor_locked(&self, keys: &[Arc<ApiKeyEntry>], key_id: &str) -> bool {
        if keys.len() < 2 {
            return false;
        }
        let alive = keys
            .iter()
            .filter(|k| k.id == key_id || k.current_state() != KeyState::Disabled)
            .count();
        alive.saturating_sub(1) * 2 <= keys.len()
    }

    /// Count active healthy keys
    pub fn active_key_count(&self) -> usize {
        let keys = self.keys.read();
        keys.iter()
            .filter(|k| k.current_state() == KeyState::Active)
            .count()
    }

    /// True when no key can currently serve (none Active: all cooling down or
    /// disabled). Feeds the quota boundary guard (bugfix 2026-10-02).
    pub fn no_schedulable_keys(&self) -> bool {
        let keys = self.keys.read();
        keys.iter().all(|k| k.current_state() != KeyState::Active)
    }

    /// True when at least one cooling key is cooling because its account/model
    /// quota was exhausted. Combined with [`Self::no_schedulable_keys`] this
    /// lets the routing layer reclassify a `NoAvailableKey` failure as a quota
    /// boundary instead of draining a second provider's quota.
    pub fn any_key_quota_cooldown(&self) -> bool {
        let keys = self.keys.read();
        keys.iter().any(|k| {
            k.current_state() == KeyState::CoolingDown
                && k.cooldown_reason() == Some(crate::pool::entry::CooldownReason::Quota)
        })
    }

    /// True when any *non-disabled* key carries an unexpired quota-group
    /// exhaustion for some family (real upstream 429 writeback only). Feeds the
    /// H1 quota-boundary reclassification (extractors::pool_quota_exhausted):
    /// `NoAvailableKey` with family-exhausted keys present is a quota boundary,
    /// not a transient no-key error — the routing guard must stop before
    /// draining a second provider (ADR
    /// `2026-10-04-antigravity-group-quota-aware-scheduling`).
    pub fn any_key_family_exhausted_any(&self) -> bool {
        let keys = self.keys.read();
        let now = chrono::Utc::now();
        keys.iter().any(|k| {
            k.current_state() != KeyState::Disabled
                && k.quota_group_exhaustions()
                    .values()
                    .any(|reset| *reset > now)
        })
    }

    /// Earliest unexpired quota-group reset across keys (any family), for an
    /// honest `Retry-After` when the pool is family-quota-bound.
    pub fn earliest_family_reset_any(&self) -> Option<std::time::Duration> {
        let keys = self.keys.read();
        let now = chrono::Utc::now();
        let mut min: Option<chrono::DateTime<chrono::Utc>> = None;
        for k in keys.iter() {
            for reset in k.quota_group_exhaustions().values() {
                if *reset <= now {
                    continue;
                }
                min = Some(match min {
                    Some(cur) => cur.min(*reset),
                    None => *reset,
                });
            }
        }
        min.map(|reset| {
            let secs = (reset - now).num_seconds().max(0) as u64;
            std::time::Duration::from_secs(secs)
        })
    }

    /// Copy the runtime state of matching key ids from a donor pool onto this
    /// (freshly rebuilt) pool: permanent `disabled_reason`, the active cooldown
    /// (remaining deadline + wall-clock reset), its `cooldown_reason` and the
    /// hard-error message. Every config reload / admin rebuild path must call
    /// this, otherwise a hot reload inside a 3-day eligibility freeze revives
    /// the account and re-triggers the hammering the freeze exists to stop.
    pub fn inherit_runtime_state(&self, donor: &KeyPool) {
        let donors: HashMap<String, Arc<ApiKeyEntry>> = donor
            .snapshot_keys()
            .into_iter()
            .map(|k| (k.id.clone(), k))
            .collect();
        for new_entry in self.snapshot_keys() {
            let Some(old_entry) = donors.get(&new_entry.id) else {
                continue;
            };
            if let Some(reason) = old_entry.disabled_reason() {
                *new_entry.stats.disabled_reason.write() = Some(reason);
            }
            if let (Some(remaining), Some(reset_at)) = (
                old_entry.cooldown_remaining(),
                old_entry.cooldown_reset_at(),
            ) {
                new_entry.set_cooldown(remaining);
                *new_entry.stats.cooldown_reset_at.write() = Some(reset_at);
                if let Some(cd_reason) = old_entry.cooldown_reason() {
                    *new_entry.stats.cooldown_reason.write() = Some(cd_reason);
                }
                if let Some(err_reason) = old_entry.raw_error_reason() {
                    *new_entry.stats.error_reason.write() = Some(err_reason);
                }
            }
            // Family-scoped quota-group verdicts survive rebuilds too: a hot
            // reload inside a Gemini weekly exhaustion must not revive the key
            // for Gemini traffic (ADR
            // `2026-10-04-antigravity-group-quota-aware-scheduling`).
            new_entry.restore_quota_group_exhaustions(old_entry.quota_group_exhaustions());
        }
    }

    /// Earliest unlock across cooling keys, for honest Retry-After.
    pub fn earliest_unlock(&self) -> Option<std::time::Duration> {
        let keys = self.keys.read();
        keys.iter().filter_map(|k| k.cooldown_remaining()).min()
    }

    /// Cooldown snapshot for admin/observability surfaces: remaining time plus
    /// the wall-clock reset instant. Both are `None` unless the key is still
    /// cooling; the key id is not exposed raw, only looked up.
    pub fn key_cooldown(&self, key_id: &str) -> (Option<Duration>, Option<SystemTime>) {
        let keys = self.keys.read();
        match keys.iter().find(|k| k.id == key_id) {
            Some(k) => (k.cooldown_remaining(), k.cooldown_reset_at()),
            None => (None, None),
        }
    }

    /// Retrieve the disabled reason for a key, if present.
    pub fn key_disabled_reason(&self, key_id: &str) -> Option<String> {
        let keys = self.keys.read();
        keys.iter()
            .find(|k| k.id == key_id)
            .and_then(|k| k.disabled_reason())
    }

    /// Why a key is currently cooling down, when known (admin surface).
    pub fn key_cooldown_reason(&self, key_id: &str) -> Option<crate::pool::entry::CooldownReason> {
        let keys = self.keys.read();
        keys.iter()
            .find(|k| k.id == key_id)
            .and_then(|k| k.cooldown_reason())
    }

    /// Human-readable reason for a hard non-active state (eligibility freeze /
    /// permanent disable), surfaced so the web pool matrix can render the
    /// exact upstream message in red.
    pub fn key_error_reason(&self, key_id: &str) -> Option<String> {
        let keys = self.keys.read();
        keys.iter()
            .find(|k| k.id == key_id)
            .and_then(|k| k.error_reason())
    }

    /// Total keys in pool
    pub fn total_key_count(&self) -> usize {
        self.keys.read().len()
    }

    /// Clear cooldown for a specific key, immediately restoring it to Active
    pub fn clear_key_cooldown(&self, key_id: &str) -> bool {
        let keys = self.keys.read();
        if let Some(k) = keys.iter().find(|k| k.id == key_id) {
            k.clear_cooldown();
            true
        } else {
            false
        }
    }

    /// Set cooldown for a specific key
    pub fn set_key_cooldown(&self, key_id: &str, duration: Duration) -> bool {
        let keys = self.keys.read();
        if let Some(k) = keys.iter().find(|k| k.id == key_id) {
            k.set_cooldown(duration);
            true
        } else {
            false
        }
    }

    /// Clear disabled state for a specific key, restoring it to active.
    pub fn clear_key_disabled(&self, key_id: &str) -> bool {
        let keys = self.keys.read();
        if let Some(k) = keys.iter().find(|k| k.id == key_id) {
            k.clear_disabled();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limits_validate_rejects_zero_and_over_60_window() {
        // Bounds: 1..=60 are accepted (None defaults to 60s).
        assert!(RateLimits::default().validate().is_ok());
        assert!((RateLimits {
            window_secs: Some(1),
            ..Default::default()
        })
        .validate()
        .is_ok());
        assert!((RateLimits {
            window_secs: Some(60),
            ..Default::default()
        })
        .validate()
        .is_ok());

        // 0 degenerates the sliding window; > 60 would silently run as 60s
        // (meter ring is 12 × 5s) and loosen the budget ~window_secs/60×.
        let err0 = (RateLimits {
            window_secs: Some(0),
            ..Default::default()
        })
        .validate()
        .unwrap_err();
        assert!(err0.contains("1..=60"), "got: {err0}");
        let err61 = (RateLimits {
            window_secs: Some(61),
            ..Default::default()
        })
        .validate()
        .unwrap_err();
        assert!(err61.contains("1..=60"), "got: {err61}");
        assert!(
            err61.contains("CycleStats"),
            "long-window must point to CycleStats: {err61}"
        );
    }

    #[test]
    fn test_add_key_deduplication_upsert() {
        let pool = KeyPool::new("test-provider", RoutingStrategy::RoundRobin);
        pool.add_key(ApiKeyEntry::new("k1", "token-v1", 1, 10));
        assert_eq!(pool.total_key_count(), 1);

        // Add key with same ID but different token and priority
        pool.add_key(ApiKeyEntry::new("k1", "token-v2", 2, 20));
        assert_eq!(pool.total_key_count(), 1);

        let keys = pool.snapshot_keys();
        assert_eq!(keys[0].id, "k1");
        assert_eq!(keys[0].api_key, "token-v2");
        assert_eq!(keys[0].priority, 2);
        assert_eq!(keys[0].weight, 20);
    }

    #[test]
    fn test_select_budget_rpm_blocks_exhausted_key() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        let limits = RateLimits {
            rpm: Some(1),
            ..Default::default()
        };

        // Fresh key: the single rpm=1 budget slot is available.
        let k = pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .unwrap();
        assert_eq!(k.id, "k1");

        // One attempt recorded (conservative accounting): the budget slot is
        // spent, so the same key is no longer schedulable under limits.
        pool.snapshot_keys()[0].meter().record_attempt(0);
        let err = pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .unwrap_err();
        assert!(matches!(err, CoreError::NoAvailableKey(_)));

        // Legacy path without limits still selects the key (budget disabled).
        let k = pool.select_key_excluding(&[]).unwrap();
        assert_eq!(k.id, "k1");
    }

    #[test]
    fn test_select_budget_prefers_key_with_headroom() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("spent", "t1", 1, 10));
        pool.add_key(ApiKeyEntry::new("fresh", "t2", 2, 10));
        let limits = RateLimits {
            rpm: Some(1),
            ..Default::default()
        };

        // Spend the only slot of the priority key.
        pool.snapshot_keys()
            .iter()
            .find(|k| k.id == "spent")
            .unwrap()
            .meter()
            .record_attempt(0);

        // The spent high-priority key is skipped; the fresh fallback wins.
        let k = pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .unwrap();
        assert_eq!(k.id, "fresh");
        assert!(!pool.exhausted_by_window_with_limits(Some(&limits)));
    }

    #[test]
    fn test_select_budget_concurrency_cap() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        let limits = RateLimits {
            concurrency: Some(2),
            ..Default::default()
        };
        assert!(pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .is_ok());
        pool.snapshot_keys()[0].meter().in_flight_inc();
        pool.snapshot_keys()[0].meter().in_flight_inc();
        // At the cap: no schedulable key.
        assert!(pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .is_err());
        assert!(pool.exhausted_by_window_with_limits(Some(&limits)));
        pool.snapshot_keys()[0].meter().in_flight_dec();
        assert!(pool
            .select_key_excluding_with_limits(&[], Some(&limits))
            .is_ok());
    }

    #[test]
    fn test_exhausted_by_window_and_refill() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        let limits = RateLimits {
            rpm: Some(1),
            ..Default::default()
        };

        // Fresh: schedulable now.
        assert!(!pool.exhausted_by_window_with_limits(Some(&limits)));
        assert_eq!(
            pool.window_refill_in_with_limits(Some(&limits)),
            Some(Duration::ZERO)
        );
        assert_eq!(
            pool.longest_window_refill_in_with_limits(Some(&limits)),
            Some(Duration::ZERO)
        );

        // Budget spent: exhausted by window. The refill is the meter's
        // earliest in-window slot expiry minus now — within one 5s slot
        // granularity of the full window (precise, not a full-window bound).
        pool.snapshot_keys()[0].meter().record_attempt(10);
        assert!(pool.exhausted_by_window_with_limits(Some(&limits)));
        let refill = pool.window_refill_in_with_limits(Some(&limits)).unwrap();
        assert!(
            refill > Duration::from_secs(55) && refill <= Duration::from_secs(RateLimits::DEFAULT_WINDOW_SECS),
            "precise refill must sit within the 5s slot granularity of the 60s window, got {refill:?}"
        );
        let longest = pool
            .longest_window_refill_in_with_limits(Some(&limits))
            .unwrap();
        assert_eq!(
            longest, refill,
            "single-key pool: min and max refill coincide"
        );

        // No-arg variants ignore the budget dimension (cooldown-only): the
        // active key stays schedulable, so the pool is not window-exhausted.
        assert!(!pool.exhausted_by_window());
        assert_eq!(pool.window_refill_in(), Some(Duration::ZERO));
    }

    #[test]
    fn test_exhausted_by_window_distinguishes_disabled_pool() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("k1", "t", 1, 10));
        pool.record_error("k1", PoolErrorType::AuthInvalid { reason: None });
        // A permanently disabled pool is NOT window-exhausted: it never
        // refills, so M3 must not hold-and-wait on it.
        assert!(!pool.exhausted_by_window());
        assert_eq!(pool.window_refill_in(), None);
        assert_eq!(pool.longest_window_refill_in(), None);
    }

    #[test]
    fn test_eligibility_freeze_skips_key_and_keeps_pool_alive() {
        // 上游资格类 403：坏账号长冷冻（数日），同请求 failover 与后续请求
        // 都必须路由到池内其它 Active key，agent 运行不中断。
        let pool = KeyPool::new("antigravity", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("bad", "t1", 1, 10));
        pool.add_key(ApiKeyEntry::new("good", "t2", 2, 10));
        pool.record_error(
            "bad",
            PoolErrorType::AccountEligibility {
                reason: Some(
                    "Your current account is not eligible for Gemini Code Assist".to_string(),
                ),
            },
        );
        // 坏账号：长冷冻 + 原因可查（管理面红显的输入）。
        let remaining = pool.key_cooldown("bad").0.expect("bad key must be cooling");
        assert!(
            remaining >= Duration::from_secs(3 * 24 * 60 * 60) - Duration::from_secs(60),
            "freeze must be ~3 days, got {remaining:?}"
        );
        assert_eq!(
            pool.key_cooldown_reason("bad"),
            Some(crate::pool::entry::CooldownReason::Eligibility)
        );
        assert!(
            pool.key_error_reason("bad")
                .unwrap_or_default()
                .contains("not eligible"),
            "error_reason must carry the upstream message"
        );
        // 计划行为：调度跳过冷冻账号，next 请求落到合格账号。
        let picked = pool
            .select_key()
            .expect("pool must still have a schedulable key");
        assert_eq!(picked.id, "good", "routing must skip the frozen account");
    }

    #[test]
    fn test_inherit_runtime_state_preserves_eligibility_freeze_across_rebuild() {
        // 热重建继承：config 重载 / PUT key / PUT provider / DELETE key 四条
        // 重建路径都会重建 KeyPool，必须把进行中的资格冻结及其原因原样搬给
        // 新池，否则冻结窗口内的任意管理操作都会复活账号、重演整池锤打
        // （2026-10-04 事故路径；对抗审核 P1-3）。
        let donor = KeyPool::new("antigravity", RoutingStrategy::Priority);
        donor.add_key(ApiKeyEntry::new("ag1", "t1", 1, 10));
        donor.record_error(
            "ag1",
            PoolErrorType::AccountEligibility {
                reason: Some(
                    "Your current account is not eligible for Gemini Code Assist".to_string(),
                ),
            },
        );
        donor.add_key(ApiKeyEntry::new("ag2", "t2", 2, 10));
        donor.set_key_cooldown("ag2", Duration::from_secs(120));

        let rebuilt = KeyPool::new("antigravity", RoutingStrategy::Priority);
        rebuilt.add_key(ApiKeyEntry::new("ag1", "t1", 1, 10));
        rebuilt.add_key(ApiKeyEntry::new("ag2", "t2", 2, 10));
        rebuilt.add_key(ApiKeyEntry::new("ag3", "t3", 3, 10));
        rebuilt.inherit_runtime_state(&donor);

        // 资格冻结及其原因保留；软冷却保留。
        assert_eq!(
            rebuilt.key_cooldown_reason("ag1"),
            Some(crate::pool::entry::CooldownReason::Eligibility)
        );
        assert!(
            rebuilt
                .key_error_reason("ag1")
                .unwrap_or_default()
                .contains("not eligible"),
            "rebuild must not lose the eligibility reason"
        );
        let ag2 = rebuilt
            .key_cooldown("ag2")
            .0
            .expect("ag2 cooldown must survive");
        assert!(
            ag2 >= Duration::from_secs(119),
            "soft cooldown must survive rebuild, got {ag2:?}"
        );
        // 冻结仍然生效：ag1/ag2 都被冻结/冷却，调度只落到重建期间新加入的 ag3。
        let picked = rebuilt.select_key().expect("ag3 must be schedulable");
        assert_eq!(
            picked.id, "ag3",
            "rebuild must not revive the frozen account"
        );
    }

    #[test]
    fn test_refill_min_includes_cooldown_dimension() {
        let pool = KeyPool::new("p", RoutingStrategy::Priority);
        pool.add_key(ApiKeyEntry::new("cool1", "t1", 1, 10));
        pool.add_key(ApiKeyEntry::new("cool2", "t2", 2, 10));
        let limits = RateLimits {
            rpm: Some(1),
            window_secs: Some(30),
            ..Default::default()
        };
        // Both keys cooling: earliest unlock is the min cooldown (10s here).
        pool.set_key_cooldown("cool1", Duration::from_secs(10));
        pool.set_key_cooldown("cool2", Duration::from_secs(60));
        assert!(pool.exhausted_by_window_with_limits(Some(&limits)));
        let min = pool.window_refill_in_with_limits(Some(&limits)).unwrap();
        assert!(
            min <= Duration::from_secs(10),
            "min must be <= 10s, got {min:?}"
        );
        assert!(min > Duration::ZERO);
        let max = pool
            .longest_window_refill_in_with_limits(Some(&limits))
            .unwrap();
        // Monotonic-clock drift shaves microseconds off `set_cooldown`'s 60s.
        assert!(
            max >= Duration::from_secs(59),
            "max must be ~60s, got {max:?}"
        );
    }

    #[test]
    fn test_select_key_with_affinity_and_soft_spillover() {
        let pool = KeyPool::new("deepseek", RoutingStrategy::RoundRobin);
        // Account A has two keys (k1, k2), Account B has one key (k3)
        let k1 = ApiKeyEntry::new("k1", "sk-1", 1, 10).with_account_id(Some("acct_a".into()));
        let k2 = ApiKeyEntry::new("k2", "sk-2", 1, 10).with_account_id(Some("acct_a".into()));
        let k3 = ApiKeyEntry::new("k3", "sk-3", 1, 10).with_account_id(Some("acct_b".into()));

        pool.add_key(k1);
        pool.add_key(k2);
        pool.add_key(k3);

        // Session 1 consistent hash maps to one account consistently
        let seed_1 = 42u64;
        let selected_1 = pool
            .select_key_with_affinity(Some(seed_1), &[], None)
            .unwrap();
        let selected_2 = pool
            .select_key_with_affinity(Some(seed_1), &[], None)
            .unwrap();
        // Both selections stay within the same account!
        assert_eq!(
            selected_1.effective_account_id(),
            selected_2.effective_account_id()
        );

        // Soft spillover test: if keys in that chosen account are excluded or cooling down,
        // it gracefully spills over to the other account instead of failing.
        let chosen_account = selected_1.effective_account_id();
        let excluded: Vec<String> = if chosen_account == "acct_a" {
            vec!["k1".into(), "k2".into()]
        } else {
            vec!["k3".into()]
        };

        let spillover = pool
            .select_key_with_affinity(Some(seed_1), &excluded, None)
            .unwrap();
        assert_ne!(spillover.effective_account_id(), chosen_account);
    }
}
