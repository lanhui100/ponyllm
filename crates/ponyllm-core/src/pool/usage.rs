use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const FIVE_HOURS_MS: u64 = 5 * 3600 * 1000;
pub const SEVEN_DAYS_MS: u64 = 7 * 24 * 3600 * 1000;

/// 5-minute slice interval for sliding windows (60 buckets for 5h, 2016 for 7d).
pub const SLICE_INTERVAL_MS: u64 = 5 * 60 * 1000;
pub const THIRTY_DAYS_MS: u64 = 30 * 24 * 3600 * 1000;
pub const MAX_USAGE_SLICES: usize = 8640; // 30 days * 24 * 12 slices

pub const MIN_REASONABLE_CAPACITY: u64 = 20_000;
pub const MAX_REASONABLE_CAPACITY: u64 = 5_000_000;

/// Completed-cycle kind: the upstream quota window that reset.
pub const CYCLE_KIND_5H: &str = "5h";
pub const CYCLE_KIND_WEEKLY: &str = "weekly";
pub const CYCLE_KIND_MONTHLY: &str = "monthly";
/// Per-kind aligned wall-clock period lengths (used by the persisted pool
/// benchmark archive to aggregate closed periods from usage slices).
pub fn cycle_kind_period_ms(kind: &str) -> u64 {
    match kind {
        CYCLE_KIND_WEEKLY => SEVEN_DAYS_MS,
        CYCLE_KIND_MONTHLY => THIRTY_DAYS_MS,
        _ => FIVE_HOURS_MS,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UsageSlice {
    pub timestamp_ms: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WindowUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CycleStats {
    pub count: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    pub total_tokens: u64,
    #[serde(default)]
    pub requests: u64,
    pub avg_tokens: u64,
}

/// Dynamic capacity estimation and account tier inference
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KeyCapacityEstimate {
    /// 5-hour rolling usage metrics
    pub window_5h: WindowUsage,
    /// 7-day (weekly) rolling usage metrics
    pub window_weekly: WindowUsage,
    /// 30-day (monthly) rolling usage metrics
    pub window_monthly: WindowUsage,
    /// Completed 5h cycle weighted average stats (objective factual historical cycles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_5h_stats: Option<CycleStats>,
    /// Completed weekly cycle weighted average stats (objective factual historical cycles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_weekly_stats: Option<CycleStats>,
    /// Inferred 5-hour total capacity in tokens, if fitted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_capacity_5h: Option<u64>,
    /// Estimated remaining tokens in 5h window based on current remaining fraction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_remaining_5h: Option<u64>,
    /// Inferred weekly total capacity in tokens (via weekly quota bucket fraction)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_capacity_weekly: Option<u64>,
    /// Inferred account tier: "pro", "standard", "free", "calibrating", or "unknown"
    pub account_tier: String,
    /// Fitting confidence (0.0 ~ 1.0)
    pub confidence: f64,
    /// Calibration status: "benchmarked" (completed reset cycle), "estimated" (inferred slope), or "calibrating"
    #[serde(default = "default_calibration_status")]
    pub calibration_status: String,
}

fn default_calibration_status() -> String {
    "calibrating".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletedCycleRecord {
    pub cycle_end_ms: u64,
    /// Window kind: [`CYCLE_KIND_5H`] (default, legacy) or [`CYCLE_KIND_WEEKLY`].
    #[serde(default = "default_cycle_kind")]
    pub kind: String,
    /// Monotonic per-tracker sequence for idempotent pool-archive merging:
    /// a record with `seq <= last archived seq` is never merged twice even
    /// across restarts / re-saves.
    #[serde(default)]
    pub seq: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub requests: u64,
}

fn default_cycle_kind() -> String {
    CYCLE_KIND_5H.to_string()
}

/// One (account × closed aligned period) consumption observation, derived
/// from usage slices by [`aligned_period_observations`]. Feeds the persisted
/// pool benchmark archive (跨账号跨周期累计平均的数据单元).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeriodObservation {
    /// Aligned period end (exclusive) in wall-clock ms; only closed periods
    /// (`<= now_ms`) are produced, so a partial current window is never
    /// mistaken for a full period.
    pub period_end_ms: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub requests: u64,
}

/// Aggregates usage slices into closed, aligned wall-clock periods of
/// `period_ms`. A period is closed when its end is `<= now_ms`. Slices inside
/// the partial current period are ignored.
///
/// To prevent a freshly created account (or one with only a few days of data)
/// from closing a multi-day window (e.g. 7d or 30d) prematurely when its earliest
/// slice happens to land near an epoch-aligned boundary, a period is only considered
/// valid if the slices span a substantial portion of the period (at least 60% of period_ms),
/// or for short periods (5h).
pub fn aligned_period_observations(
    slices: &[UsageSlice],
    period_ms: u64,
    now_ms: u64,
) -> Vec<PeriodObservation> {
    if slices.is_empty() {
        return Vec::new();
    }
    let mut map: BTreeMap<u64, (PeriodObservation, u64, u64)> = BTreeMap::new(); // (entry, min_ts, max_ts)
    for s in slices {
        // Align the slice's start to its containing period; the period is
        // closed once its end boundary has passed.
        let period_start = (s.timestamp_ms / period_ms) * period_ms;
        let period_end = period_start.saturating_add(period_ms);
        if period_end > now_ms {
            continue; // partial/current period — not a full observation yet
        }
        let entry = map.entry(period_end).or_insert_with(|| {
            (
                PeriodObservation {
                    period_end_ms: period_end,
                    ..Default::default()
                },
                s.timestamp_ms,
                s.timestamp_ms,
            )
        });
        entry.0.prompt_tokens = entry.0.prompt_tokens.saturating_add(s.prompt_tokens);
        entry.0.completion_tokens = entry
            .0
            .completion_tokens
            .saturating_add(s.completion_tokens);
        entry.0.cached_tokens = entry.0.cached_tokens.saturating_add(s.cached_tokens);
        entry.0.total_tokens = entry
            .0
            .total_tokens
            .saturating_add(s.prompt_tokens)
            .saturating_add(s.completion_tokens);
        entry.0.requests = entry.0.requests.saturating_add(s.requests);
        entry.1 = entry.1.min(s.timestamp_ms);
        entry.2 = entry.2.max(s.timestamp_ms);
    }

    map.into_values()
        .filter_map(|(obs, min_t, max_t)| {
            if obs.total_tokens == 0 {
                return None;
            }
            // For multi-day periods (7d, 30d), ensure the slices span at least 60% of the period,
            // preventing 1-2 days of traffic from falsely closing a 30-day epoch period.
            if period_ms >= SEVEN_DAYS_MS {
                let span = max_t.saturating_sub(min_t);
                if span < (period_ms * 6 / 10) {
                    return None;
                }
            }
            Some(obs)
        })
        .collect()
}

/// Cumulative totals for one window kind across ALL tracked accounts and ALL
/// closed periods / completed cycles. This is the persisted "多账号多周期累计
/// 不断求平均" benchmark backing the dashboard headline numbers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CycleBenchmarkTotals {
    /// Number of (account × closed period) observations merged.
    pub observations: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub requests: u64,
    /// Completed (hard reset) cycles observed via upstream fraction jumps.
    pub completed_cycles: u64,
    pub completed_prompt_tokens: u64,
    pub completed_completion_tokens: u64,
    pub completed_cached_tokens: u64,
    pub completed_total_tokens: u64,
    pub completed_requests: u64,
    pub first_observation_ms: u64,
    pub last_observation_ms: u64,
}

impl CycleBenchmarkTotals {
    /// Average tokens per (account × period) observation; 0 when no observations.
    pub fn avg_tokens(&self) -> u64 {
        if self.observations > 0 {
            self.total_tokens / self.observations
        } else {
            0
        }
    }

    /// Average tokens per completed (hard reset) cycle; 0 when none.
    pub fn avg_completed_tokens(&self) -> u64 {
        if self.completed_cycles > 0 {
            self.completed_total_tokens / self.completed_cycles
        } else {
            0
        }
    }

    fn absorb(&mut self, o: &PeriodObservation) {
        self.observations = self.observations.saturating_add(1);
        self.prompt_tokens = self.prompt_tokens.saturating_add(o.prompt_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(o.completion_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(o.cached_tokens);
        self.total_tokens = self.total_tokens.saturating_add(o.total_tokens);
        self.requests = self.requests.saturating_add(o.requests);
        if self.first_observation_ms == 0 || o.period_end_ms < self.first_observation_ms {
            self.first_observation_ms = o.period_end_ms;
        }
        if o.period_end_ms > self.last_observation_ms {
            self.last_observation_ms = o.period_end_ms;
        }
    }

    fn absorb_completed(&mut self, r: &CompletedCycleRecord) {
        self.completed_cycles = self.completed_cycles.saturating_add(1);
        self.completed_prompt_tokens = self.completed_prompt_tokens.saturating_add(r.prompt_tokens);
        self.completed_completion_tokens = self
            .completed_completion_tokens
            .saturating_add(r.completion_tokens);
        self.completed_cached_tokens = self.completed_cached_tokens.saturating_add(r.cached_tokens);
        self.completed_total_tokens = self.completed_total_tokens.saturating_add(r.total_tokens);
        self.completed_requests = self.completed_requests.saturating_add(r.requests);
    }
}

/// Pool-wide persisted cycle benchmark archive with per-(kind, key) watermark
/// dedup so re-saves / restarts never double-count observations or cycles.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolCycleBenchmark {
    pub kind_5h: CycleBenchmarkTotals,
    pub kind_weekly: CycleBenchmarkTotals,
    pub kind_monthly: CycleBenchmarkTotals,
    /// Watermarks: kind -> key_id -> highest merged closed `period_end_ms`.
    #[serde(default)]
    pub last_merged_period_end: BTreeMap<String, BTreeMap<String, u64>>,
    /// Watermarks: kind -> key_id -> highest archived completed-cycle `seq`.
    #[serde(default)]
    pub last_archived_seq: BTreeMap<String, BTreeMap<String, u64>>,
}

impl PoolCycleBenchmark {
    /// Idempotently merge per-key usage state into the archive.
    ///
    /// - Closed aligned periods (`aligned_period_observations`) are merged when
    ///   `period_end_ms` is newer than the key's watermark for that kind.
    /// - Completed cycles (5h/weekly) are merged when `seq` is newer than the
    ///   key's archived seq for that kind.
    ///
    /// Both watermarks persist with the archive, so calling this on the same
    /// data twice (crash-replay, restart re-import, repeated saves) never
    /// changes the totals.
    pub fn merge_usages(&mut self, usages: &BTreeMap<String, KeyUsageStateSnapshot>, now_ms: u64) {
        for (key_id, snap) in usages {
            for kind in [CYCLE_KIND_5H, CYCLE_KIND_WEEKLY, CYCLE_KIND_MONTHLY] {
                let totals = match kind {
                    CYCLE_KIND_WEEKLY => &mut self.kind_weekly,
                    CYCLE_KIND_MONTHLY => &mut self.kind_monthly,
                    _ => &mut self.kind_5h,
                };
                let period_ms = cycle_kind_period_ms(kind);
                let wm = self
                    .last_merged_period_end
                    .entry(kind.to_string())
                    .or_default()
                    .entry(key_id.clone())
                    .or_insert(0);
                let mut wm_value = *wm;
                for obs in aligned_period_observations(&snap.slices, period_ms, now_ms) {
                    if obs.period_end_ms > wm_value {
                        totals.absorb(&obs);
                        wm_value = obs.period_end_ms;
                    }
                }
                *wm = wm_value;
            }
            // Completed cycles (both tracked kinds) deduped by seq.
            for rec in snap
                .completed_5h_records
                .iter()
                .chain(snap.completed_weekly_records.iter())
            {
                if rec.total_tokens == 0 {
                    continue;
                }
                let kind = if rec.kind == CYCLE_KIND_WEEKLY {
                    CYCLE_KIND_WEEKLY
                } else {
                    CYCLE_KIND_5H
                };
                let totals = match kind {
                    CYCLE_KIND_WEEKLY => &mut self.kind_weekly,
                    _ => &mut self.kind_5h,
                };
                let seq_wm = self
                    .last_archived_seq
                    .entry(kind.to_string())
                    .or_default()
                    .entry(key_id.clone())
                    .or_insert(0);
                if rec.seq > *seq_wm {
                    totals.absorb_completed(rec);
                    *seq_wm = rec.seq;
                }
            }
        }
    }

    /// Total merged observations across all kinds (for display labels).
    pub fn total_observations(&self) -> u64 {
        self.kind_5h
            .observations
            .saturating_add(self.kind_weekly.observations)
            .saturating_add(self.kind_monthly.observations)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyUsageStateSnapshot {
    pub slices: Vec<UsageSlice>,
    pub cached_capacity: Option<u64>,
    pub last_probe: Option<(u64, f64, u64)>, // (timestamp_ms, fraction, lifetime_tokens)
    #[serde(default)]
    pub completed_5h_records: Vec<CompletedCycleRecord>,
    #[serde(default)]
    pub completed_weekly_records: Vec<CompletedCycleRecord>,
    /// Next monotonic completed-cycle sequence id (idempotent archiving).
    #[serde(default)]
    pub next_cycle_seq: u64,
    #[serde(default)]
    pub completed_5h: Vec<(u64, u64, u64)>,
    #[serde(default)]
    pub completed_weekly: Vec<(u64, u64, u64)>,
}

#[derive(Debug, Default)]
pub struct KeyUsageTracker {
    slices: RwLock<BTreeMap<u64, UsageSlice>>,
    /// Monotonically increasing lifetime total tokens to prevent rolling-window underflow
    lifetime_tokens: AtomicU64,
    /// Monotonically increasing lifetime prompt tokens
    lifetime_prompt: AtomicU64,
    /// Monotonically increasing lifetime completion tokens
    lifetime_completion: AtomicU64,
    /// Monotonically increasing lifetime cached tokens
    lifetime_cached: AtomicU64,
    /// Monotonically increasing lifetime requests
    lifetime_requests: AtomicU64,
    /// Completed 5h cycles history with full 4-factor breakdown
    completed_5h_cycles: RwLock<Vec<CompletedCycleRecord>>,
    /// Completed weekly cycles history (full 4-factor breakdown)
    completed_weekly_cycles: RwLock<Vec<CompletedCycleRecord>>,
    /// Monotonic sequence counter for completed cycles (idempotent archiving).
    cycle_seq: AtomicU64,
    /// Last observed remaining fraction from quota refresh: (timestamp_ms, remaining_fraction, lifetime_tokens_at_probe, prompt, comp, cached, reqs)
    last_probe_snapshot: RwLock<Option<(u64, f64, u64, u64, u64, u64, u64)>>,
    /// Weekly-bucket probe baseline, same shape as `last_probe_snapshot`.
    last_probe_weekly: RwLock<Option<(u64, f64, u64, u64, u64, u64, u64)>>,
    cached_capacity: RwLock<Option<u64>>,
}

impl KeyUsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_tokens(&self, wall_ms: u64, prompt: u64, completion: u64, cached: u64) {
        let total = prompt.saturating_add(completion);
        self.lifetime_tokens.fetch_add(total, Ordering::Relaxed);
        self.lifetime_prompt.fetch_add(prompt, Ordering::Relaxed);
        self.lifetime_completion
            .fetch_add(completion, Ordering::Relaxed);
        self.lifetime_cached.fetch_add(cached, Ordering::Relaxed);
        self.lifetime_requests.fetch_add(1, Ordering::Relaxed);

        let slice_key = (wall_ms / SLICE_INTERVAL_MS) * SLICE_INTERVAL_MS;
        let mut slices = self.slices.write();

        let slice = slices.entry(slice_key).or_insert_with(|| UsageSlice {
            timestamp_ms: slice_key,
            ..Default::default()
        });

        slice.prompt_tokens = slice.prompt_tokens.saturating_add(prompt);
        slice.completion_tokens = slice.completion_tokens.saturating_add(completion);
        slice.cached_tokens = slice.cached_tokens.saturating_add(cached);
        slice.requests = slice.requests.saturating_add(1);

        // Fast O(log N) pruning via split_off (30 days retention)
        let cutoff = wall_ms.saturating_sub(THIRTY_DAYS_MS);
        if let Some(&oldest) = slices.keys().next() {
            if oldest < cutoff {
                *slices = slices.split_off(&cutoff);
            }
        }
    }

    /// Single-pass query for 5h, 7d, and 30d rolling windows to eliminate duplicate lock contention
    pub fn query_windows(&self, now_ms: u64) -> (WindowUsage, WindowUsage, WindowUsage) {
        let cutoff_5h = now_ms.saturating_sub(FIVE_HOURS_MS);
        let cutoff_7d = now_ms.saturating_sub(SEVEN_DAYS_MS);
        let cutoff_30d = now_ms.saturating_sub(THIRTY_DAYS_MS);
        let slices = self.slices.read();

        let mut usage_5h = WindowUsage::default();
        let mut usage_7d = WindowUsage::default();
        let mut usage_30d = WindowUsage::default();

        for (&time, slice) in slices.range(cutoff_30d..=now_ms) {
            usage_30d.prompt_tokens = usage_30d.prompt_tokens.saturating_add(slice.prompt_tokens);
            usage_30d.completion_tokens = usage_30d
                .completion_tokens
                .saturating_add(slice.completion_tokens);
            usage_30d.cached_tokens = usage_30d.cached_tokens.saturating_add(slice.cached_tokens);
            usage_30d.requests = usage_30d.requests.saturating_add(slice.requests);

            if time >= cutoff_7d {
                usage_7d.prompt_tokens = usage_7d.prompt_tokens.saturating_add(slice.prompt_tokens);
                usage_7d.completion_tokens = usage_7d
                    .completion_tokens
                    .saturating_add(slice.completion_tokens);
                usage_7d.cached_tokens = usage_7d.cached_tokens.saturating_add(slice.cached_tokens);
                usage_7d.requests = usage_7d.requests.saturating_add(slice.requests);
            }

            if time >= cutoff_5h {
                usage_5h.prompt_tokens = usage_5h.prompt_tokens.saturating_add(slice.prompt_tokens);
                usage_5h.completion_tokens = usage_5h
                    .completion_tokens
                    .saturating_add(slice.completion_tokens);
                usage_5h.cached_tokens = usage_5h.cached_tokens.saturating_add(slice.cached_tokens);
                usage_5h.requests = usage_5h.requests.saturating_add(slice.requests);
            }
        }

        usage_5h.total_tokens = usage_5h
            .prompt_tokens
            .saturating_add(usage_5h.completion_tokens);
        usage_7d.total_tokens = usage_7d
            .prompt_tokens
            .saturating_add(usage_7d.completion_tokens);
        usage_30d.total_tokens = usage_30d
            .prompt_tokens
            .saturating_add(usage_30d.completion_tokens);
        (usage_5h, usage_7d, usage_30d)
    }

    pub fn query_window(&self, now_ms: u64, window_ms: u64) -> WindowUsage {
        let cutoff = now_ms.saturating_sub(window_ms);
        let slices = self.slices.read();
        let mut usage = WindowUsage::default();

        for (&_time, slice) in slices.range(cutoff..=now_ms) {
            usage.prompt_tokens = usage.prompt_tokens.saturating_add(slice.prompt_tokens);
            usage.completion_tokens = usage
                .completion_tokens
                .saturating_add(slice.completion_tokens);
            usage.cached_tokens = usage.cached_tokens.saturating_add(slice.cached_tokens);
            usage.requests = usage.requests.saturating_add(slice.requests);
        }

        usage.total_tokens = usage.prompt_tokens.saturating_add(usage.completion_tokens);
        usage
    }

    /// Observe upstream probe fraction with reset detection, monotonic token deltas, and sanity bounds
    pub fn observe_upstream_probe(&self, now_ms: u64, remaining_fraction: f64) {
        self.observe_upstream_probe_dual(now_ms, Some(remaining_fraction), None);
    }

    /// Dual-track probe observation: 5h bucket + weekly bucket reset detection.
    ///
    /// - A 5h fraction jump (>5%) archives a completed 5h cycle (existing
    ///   capacity inference is unchanged).
    /// - A weekly fraction jump (>5%) archives a completed weekly cycle, so
    ///   long-term measurement covers more than the single 5h window.
    pub fn observe_upstream_probe_dual(
        &self,
        now_ms: u64,
        h5_fraction: Option<f64>,
        weekly_fraction: Option<f64>,
    ) {
        if let Some(frac) = h5_fraction {
            self.observe_h5_probe(now_ms, frac);
        }
        if let Some(frac) = weekly_fraction {
            self.observe_weekly_probe(now_ms, frac);
        }
    }

    fn observe_h5_probe(&self, now_ms: u64, remaining_fraction: f64) {
        let current_lifetime = self.lifetime_tokens.load(Ordering::Relaxed);
        let current_prompt = self.lifetime_prompt.load(Ordering::Relaxed);
        let current_comp = self.lifetime_completion.load(Ordering::Relaxed);
        let current_cached = self.lifetime_cached.load(Ordering::Relaxed);
        let current_reqs = self.lifetime_requests.load(Ordering::Relaxed);

        let mut probe = self.last_probe_snapshot.write();

        if let Some((
            prev_time,
            prev_frac,
            prev_lifetime,
            prev_prompt,
            prev_comp,
            prev_cached,
            prev_reqs,
        )) = *probe
        {
            // Check for upstream quota reset (fraction jumped upwards by > 5%)
            if remaining_fraction > prev_frac + 0.05 {
                // An actual completed cycle occurred before this reset!
                // Guard: only record cycle if previous baseline was initialized (prev_lifetime > 0)
                if prev_lifetime > 0 {
                    self.record_completed_cycle(
                        CYCLE_KIND_5H,
                        now_ms,
                        current_lifetime,
                        current_prompt,
                        current_comp,
                        current_cached,
                        current_reqs,
                        prev_lifetime,
                        prev_prompt,
                        prev_comp,
                        prev_cached,
                        prev_reqs,
                    );
                }

                // Upstream window reset occurred: reset baseline without fitting
                *probe = Some((
                    now_ms,
                    remaining_fraction,
                    current_lifetime,
                    current_prompt,
                    current_comp,
                    current_cached,
                    current_reqs,
                ));
                return;
            }

            // Normal within-window drop (monotonic delta)
            if now_ms >= prev_time && (now_ms - prev_time) <= FIVE_HOURS_MS {
                let frac_delta = prev_frac - remaining_fraction;
                let token_delta = current_lifetime.saturating_sub(prev_lifetime);
                let prompt_delta = current_prompt.saturating_sub(prev_prompt);
                let comp_delta = current_comp.saturating_sub(prev_comp);
                let cached_delta = current_cached.saturating_sub(prev_cached);

                // Significant consumption jump (tolerance for IEEE 754 precision)
                if frac_delta >= 0.0095 && token_delta >= 1000 {
                    // Equivalent benchmark tokens: output weighted 3x, cache weighted 0.25x
                    let eq_tokens = (prompt_delta as f64)
                        + (comp_delta as f64 * 3.0)
                        + (cached_delta as f64 * 0.25);
                    // Blended capacity: 60% total tokens baseline + 40% equivalent weighted tokens
                    let raw_capacity = (token_delta as f64 / frac_delta).round() as u64;
                    let eq_capacity = (eq_tokens / frac_delta).round() as u64;
                    let inferred_capacity = if comp_delta > 0 || cached_delta > 0 {
                        ((raw_capacity as f64 * 0.6 + eq_capacity as f64 * 0.4).round() as u64)
                            .clamp(MIN_REASONABLE_CAPACITY, MAX_REASONABLE_CAPACITY)
                    } else {
                        raw_capacity.clamp(MIN_REASONABLE_CAPACITY, MAX_REASONABLE_CAPACITY)
                    };

                    // Clamping guard: ignore unreasonable outliers
                    if (MIN_REASONABLE_CAPACITY..=MAX_REASONABLE_CAPACITY)
                        .contains(&inferred_capacity)
                    {
                        let mut cap = self.cached_capacity.write();
                        if let Some(existing) = *cap {
                            // EWMA smooth capacity (70% historical, 30% new observation)
                            let smoothed = (existing as f64 * 0.7 + inferred_capacity as f64 * 0.3)
                                .round() as u64;
                            *cap = Some(smoothed);
                        } else {
                            *cap = Some(inferred_capacity);
                        }
                    }
                }
            }
        }

        *probe = Some((
            now_ms,
            remaining_fraction,
            current_lifetime,
            current_prompt,
            current_comp,
            current_cached,
            current_reqs,
        ));
    }

    fn observe_weekly_probe(&self, now_ms: u64, weekly_fraction: f64) {
        let current_lifetime = self.lifetime_tokens.load(Ordering::Relaxed);
        let current_prompt = self.lifetime_prompt.load(Ordering::Relaxed);
        let current_comp = self.lifetime_completion.load(Ordering::Relaxed);
        let current_cached = self.lifetime_cached.load(Ordering::Relaxed);
        let current_reqs = self.lifetime_requests.load(Ordering::Relaxed);

        let mut probe = self.last_probe_weekly.write();
        if let Some((
            _prev_time,
            prev_frac,
            prev_lifetime,
            prev_prompt,
            prev_comp,
            prev_cached,
            prev_reqs,
        )) = *probe
        {
            // Weekly quota bucket reset: fraction jumped upwards by > 5%.
            if weekly_fraction > prev_frac + 0.05 {
                if prev_lifetime > 0 {
                    self.record_completed_cycle(
                        CYCLE_KIND_WEEKLY,
                        now_ms,
                        current_lifetime,
                        current_prompt,
                        current_comp,
                        current_cached,
                        current_reqs,
                        prev_lifetime,
                        prev_prompt,
                        prev_comp,
                        prev_cached,
                        prev_reqs,
                    );
                }
                *probe = Some((
                    now_ms,
                    weekly_fraction,
                    current_lifetime,
                    current_prompt,
                    current_comp,
                    current_cached,
                    current_reqs,
                ));
                return;
            }
        }
        *probe = Some((
            now_ms,
            weekly_fraction,
            current_lifetime,
            current_prompt,
            current_comp,
            current_cached,
            current_reqs,
        ));
    }

    /// Archive one completed cycle with a monotonic sequence id (idempotent
    /// pool-archive merging); keeps a bounded per-key history (100 records).
    fn record_completed_cycle(
        &self,
        kind: &str,
        now_ms: u64,
        current_lifetime: u64,
        current_prompt: u64,
        current_comp: u64,
        current_cached: u64,
        current_reqs: u64,
        prev_lifetime: u64,
        prev_prompt: u64,
        prev_comp: u64,
        prev_cached: u64,
        prev_reqs: u64,
    ) {
        let cycle_tokens = current_lifetime.saturating_sub(prev_lifetime);
        if cycle_tokens == 0 {
            return;
        }
        let seq = self
            .cycle_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let record = CompletedCycleRecord {
            cycle_end_ms: now_ms,
            kind: kind.to_string(),
            seq,
            prompt_tokens: current_prompt.saturating_sub(prev_prompt),
            completion_tokens: current_comp.saturating_sub(prev_comp),
            cached_tokens: current_cached.saturating_sub(prev_cached),
            total_tokens: cycle_tokens,
            requests: current_reqs.saturating_sub(prev_reqs).max(1),
        };
        if kind == CYCLE_KIND_WEEKLY {
            let mut weekly = self.completed_weekly_cycles.write();
            weekly.push(record);
            if weekly.len() > 100 {
                weekly.remove(0);
            }
        } else {
            let mut cycles = self.completed_5h_cycles.write();
            cycles.push(record);
            if cycles.len() > 100 {
                cycles.remove(0);
            }
        }
    }

    /// Compute full estimate snapshot with optional weekly fraction for dual-track estimation
    pub fn estimate_capacity(
        &self,
        now_ms: u64,
        current_remaining_fraction: Option<f64>,
    ) -> KeyCapacityEstimate {
        self.estimate_capacity_dual(now_ms, current_remaining_fraction, None)
    }

    /// Compute full estimate snapshot with dual-track 5h and weekly capacity inference
    pub fn estimate_capacity_dual(
        &self,
        now_ms: u64,
        current_remaining_fraction: Option<f64>,
        weekly_remaining_fraction: Option<f64>,
    ) -> KeyCapacityEstimate {
        let (window_5h, window_weekly, window_monthly) = self.query_windows(now_ms);

        let cap_5h = *self.cached_capacity.read();
        let completed_cycles = self.completed_5h_cycles.read();
        let has_benchmarked_cycles = !completed_cycles.is_empty();

        let (calibration_status, account_tier, confidence) = if has_benchmarked_cycles {
            let avg_benchmarked = completed_cycles.iter().map(|c| c.total_tokens).sum::<u64>()
                / completed_cycles.len() as u64;
            let tier = if avg_benchmarked >= 350_000 {
                "pro".to_string()
            } else if avg_benchmarked >= 100_000 {
                "standard".to_string()
            } else {
                "free".to_string()
            };
            ("benchmarked".to_string(), tier, 0.98)
        } else if let Some(cap) = cap_5h {
            let tier = if cap >= 350_000 {
                "pro".to_string()
            } else if cap >= 100_000 {
                "standard".to_string()
            } else {
                "free".to_string()
            };
            ("estimated".to_string(), tier, 0.85)
        } else {
            let (tier, conf) =
                if window_5h.total_tokens > 200_000 || window_weekly.total_tokens > 500_000 {
                    ("pro".to_string(), 0.50)
                } else if window_5h.requests > 0 {
                    ("calibrating".to_string(), 0.30)
                } else {
                    ("unknown".to_string(), 0.0)
                };
            ("calibrating".to_string(), tier, conf)
        };

        let estimated_tokens_remaining_5h = match (cap_5h, current_remaining_fraction) {
            (Some(cap), Some(frac)) => Some((cap as f64 * frac).round() as u64),
            _ => None,
        };

        // Weekly capacity estimation based on remaining fraction and weekly tokens spent
        let estimated_capacity_weekly = weekly_remaining_fraction.and_then(|w_frac| {
            if !w_frac.is_finite() || !(0.0..=1.0).contains(&w_frac) {
                return None;
            }
            let spent_frac = 1.0 - w_frac;
            // Guard: only infer when spent fraction is significant and within sane bounds
            if spent_frac >= 0.03 && spent_frac <= 0.95 && window_weekly.total_tokens >= 5_000 {
                let inferred = (window_weekly.total_tokens as f64 / spent_frac).round() as u64;
                Some(inferred.clamp(100_000, 20_000_000))
            } else if let Some(c5) = cap_5h {
                // Heuristic baseline for weekly capacity: ~5x of 5h burst capacity
                Some(c5.saturating_mul(5))
            } else {
                None
            }
        });

        let completed_5h_stats = cycle_stats_from_records(&completed_cycles);

        let weekly_cycles = self.completed_weekly_cycles.read();
        let completed_weekly_stats = cycle_stats_from_records(&weekly_cycles);
        drop(weekly_cycles);

        KeyCapacityEstimate {
            window_5h,
            window_weekly,
            window_monthly,
            completed_5h_stats,
            completed_weekly_stats,
            estimated_capacity_5h: cap_5h,
            estimated_tokens_remaining_5h,
            estimated_capacity_weekly,
            account_tier,
            confidence,
            calibration_status,
        }
    }

    pub fn export_snapshot(&self) -> KeyUsageStateSnapshot {
        let last_probe = self
            .last_probe_snapshot
            .read()
            .map(|(t, f, lt, ..)| (t, f, lt));
        KeyUsageStateSnapshot {
            slices: self.slices.read().values().cloned().collect(),
            cached_capacity: *self.cached_capacity.read(),
            last_probe,
            completed_5h_records: self.completed_5h_cycles.read().clone(),
            completed_weekly_records: self.completed_weekly_cycles.read().clone(),
            next_cycle_seq: self.cycle_seq.load(Ordering::Relaxed),
            completed_5h: self
                .completed_5h_cycles
                .read()
                .iter()
                .map(|r| (r.cycle_end_ms, r.total_tokens, r.requests))
                .collect(),
            completed_weekly: self
                .completed_weekly_cycles
                .read()
                .iter()
                .map(|r| (r.cycle_end_ms, r.total_tokens, r.requests))
                .collect(),
        }
    }

    pub fn import_snapshot(&self, snap: KeyUsageStateSnapshot) {
        if let Some(cap) = snap.cached_capacity {
            *self.cached_capacity.write() = Some(cap);
        }
        if let Some((t, f, lt)) = snap.last_probe {
            *self.last_probe_snapshot.write() = Some((t, f, lt, 0, 0, 0, 0));
            self.lifetime_tokens.store(lt, Ordering::Relaxed);
        }
        // 5h records: prefer the structured archive, fall back to the legacy
        // (end, tokens, requests) list. Assign monotonic seq ids so legacy
        // records still archive exactly once in the pool benchmark.
        let mut seq_start = snap.next_cycle_seq;
        if !snap.completed_5h_records.is_empty() {
            let mut records =
                prepare_imported_records(snap.completed_5h_records, CYCLE_KIND_5H, &mut seq_start);
            if records.len() > 100 {
                records = records.split_off(records.len() - 100);
            }
            *self.completed_5h_cycles.write() = records;
        } else if !snap.completed_5h.is_empty() {
            let mut legacy = snap.completed_5h;
            if legacy.len() > 100 {
                legacy = legacy.split_off(legacy.len() - 100);
            }
            *self.completed_5h_cycles.write() = legacy
                .into_iter()
                .map(|(end, tot, req)| CompletedCycleRecord {
                    cycle_end_ms: end,
                    kind: CYCLE_KIND_5H.to_string(),
                    seq: next_seq(&mut seq_start),
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cached_tokens: 0,
                    total_tokens: tot,
                    requests: req,
                })
                .collect();
        }
        // Weekly records (structured + legacy tuple list).
        if !snap.completed_weekly_records.is_empty() {
            let mut records = prepare_imported_records(
                snap.completed_weekly_records,
                CYCLE_KIND_WEEKLY,
                &mut seq_start,
            );
            if records.len() > 100 {
                records = records.split_off(records.len() - 100);
            }
            *self.completed_weekly_cycles.write() = records;
        } else if !snap.completed_weekly.is_empty() {
            let mut legacy = snap.completed_weekly;
            if legacy.len() > 100 {
                legacy = legacy.split_off(legacy.len() - 100);
            }
            *self.completed_weekly_cycles.write() = legacy
                .into_iter()
                .map(|(end, tot, req)| CompletedCycleRecord {
                    cycle_end_ms: end,
                    kind: CYCLE_KIND_WEEKLY.to_string(),
                    seq: next_seq(&mut seq_start),
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cached_tokens: 0,
                    total_tokens: tot,
                    requests: req,
                })
                .collect();
        }
        if seq_start > 0 {
            self.cycle_seq.store(seq_start, Ordering::Relaxed);
        }
        let mut slices = self.slices.write();
        for item in snap.slices {
            slices.insert(item.timestamp_ms, item);
        }
    }

    pub fn completed_cycle_stats(&self) -> (u64, u64) {
        let cycles = self.completed_5h_cycles.read();
        let count = cycles.len() as u64;
        let total: u64 = cycles.iter().map(|c| c.total_tokens).sum();
        (count, total)
    }

    pub fn snapshot_slices(&self) -> Vec<UsageSlice> {
        self.slices.read().values().cloned().collect()
    }

    pub fn restore_slices(&self, loaded: Vec<UsageSlice>) {
        let mut slices = self.slices.write();
        for item in loaded {
            slices.insert(item.timestamp_ms, item);
        }
    }
}

/// Condense completed-cycle records into a [`CycleStats`] (weighted average
/// across all archived cycles); `None` when there are no records.
fn cycle_stats_from_records(records: &[CompletedCycleRecord]) -> Option<CycleStats> {
    if records.is_empty() {
        return None;
    }
    let count = records.len() as u64;
    let prompt_tokens: u64 = records.iter().map(|c| c.prompt_tokens).sum();
    let completion_tokens: u64 = records.iter().map(|c| c.completion_tokens).sum();
    let cached_tokens: u64 = records.iter().map(|c| c.cached_tokens).sum();
    let total_tokens: u64 = records.iter().map(|c| c.total_tokens).sum();
    let requests: u64 = records.iter().map(|c| c.requests).sum();
    Some(CycleStats {
        count,
        prompt_tokens,
        completion_tokens,
        cached_tokens,
        total_tokens,
        requests,
        avg_tokens: total_tokens / count,
    })
}

/// Assign monotonic sequence ids to imported records, keeping `seq_start`
/// (the persisted per-key counter) ahead of every archived record so the pool
/// benchmark archive merges each legacy/structured record exactly once.
fn prepare_imported_records(
    mut records: Vec<CompletedCycleRecord>,
    kind: &str,
    seq_start: &mut u64,
) -> Vec<CompletedCycleRecord> {
    for r in records.iter_mut() {
        if r.kind.is_empty() {
            r.kind = kind.to_string();
        }
        if r.seq == 0 {
            r.seq = next_seq(seq_start);
        }
    }
    records
}

fn next_seq(seq_start: &mut u64) -> u64 {
    *seq_start = seq_start.saturating_add(1);
    *seq_start
}
