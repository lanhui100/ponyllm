use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const FIVE_HOURS_MS: u64 = 5 * 3600 * 1000;
pub const SEVEN_DAYS_MS: u64 = 7 * 24 * 3600 * 1000;

/// 5-minute slice interval for sliding windows (60 buckets for 5h, 2016 for 7d).
pub const SLICE_INTERVAL_MS: u64 = 5 * 60 * 1000;
pub const THIRTY_DAYS_MS: u64 = 30 * 24 * 3600 * 1000;
pub const MAX_USAGE_SLICES: usize = 8640; // 30 days * 24 * 12 slices

pub const MIN_REASONABLE_CAPACITY: u64 = 20_000;
pub const MAX_REASONABLE_CAPACITY: u64 = 5_000_000;

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
    pub total_tokens: u64,
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
    /// Inferred 5-hour total capacity in tokens, if fitted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_capacity_5h: Option<u64>,
    /// Estimated remaining tokens in 5h window based on current remaining fraction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_remaining_5h: Option<u64>,
    /// Inferred account tier: "pro", "standard", "free", "calibrating", or "unknown"
    pub account_tier: String,
    /// Fitting confidence (0.0 ~ 1.0)
    pub confidence: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyUsageStateSnapshot {
    pub slices: Vec<UsageSlice>,
    pub cached_capacity: Option<u64>,
    pub last_probe: Option<(u64, f64, u64)>, // (timestamp_ms, fraction, lifetime_tokens)
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
    /// Completed 5h cycles history: (cycle_end_ms, tokens, requests)
    completed_5h_cycles: RwLock<Vec<(u64, u64, u64)>>,
    /// Completed weekly cycles history: (cycle_end_ms, tokens, requests)
    completed_weekly_cycles: RwLock<Vec<(u64, u64, u64)>>,
    /// Last observed remaining fraction from quota refresh: (timestamp_ms, remaining_fraction, lifetime_tokens_at_probe)
    last_probe_snapshot: RwLock<Option<(u64, f64, u64)>>,
    cached_capacity: RwLock<Option<u64>>,
}

impl KeyUsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_tokens(&self, wall_ms: u64, prompt: u64, completion: u64, cached: u64) {
        let total = prompt.saturating_add(completion);
        self.lifetime_tokens.fetch_add(total, Ordering::Relaxed);

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
            usage_30d.completion_tokens = usage_30d.completion_tokens.saturating_add(slice.completion_tokens);
            usage_30d.cached_tokens = usage_30d.cached_tokens.saturating_add(slice.cached_tokens);
            usage_30d.requests = usage_30d.requests.saturating_add(slice.requests);

            if time >= cutoff_7d {
                usage_7d.prompt_tokens = usage_7d.prompt_tokens.saturating_add(slice.prompt_tokens);
                usage_7d.completion_tokens = usage_7d.completion_tokens.saturating_add(slice.completion_tokens);
                usage_7d.cached_tokens = usage_7d.cached_tokens.saturating_add(slice.cached_tokens);
                usage_7d.requests = usage_7d.requests.saturating_add(slice.requests);
            }

            if time >= cutoff_5h {
                usage_5h.prompt_tokens = usage_5h.prompt_tokens.saturating_add(slice.prompt_tokens);
                usage_5h.completion_tokens = usage_5h.completion_tokens.saturating_add(slice.completion_tokens);
                usage_5h.cached_tokens = usage_5h.cached_tokens.saturating_add(slice.cached_tokens);
                usage_5h.requests = usage_5h.requests.saturating_add(slice.requests);
            }
        }

        usage_5h.total_tokens = usage_5h.prompt_tokens.saturating_add(usage_5h.completion_tokens);
        usage_7d.total_tokens = usage_7d.prompt_tokens.saturating_add(usage_7d.completion_tokens);
        usage_30d.total_tokens = usage_30d.prompt_tokens.saturating_add(usage_30d.completion_tokens);
        (usage_5h, usage_7d, usage_30d)
    }

    pub fn query_window(&self, now_ms: u64, window_ms: u64) -> WindowUsage {
        let cutoff = now_ms.saturating_sub(window_ms);
        let slices = self.slices.read();
        let mut usage = WindowUsage::default();

        for (&_time, slice) in slices.range(cutoff..=now_ms) {
            usage.prompt_tokens = usage.prompt_tokens.saturating_add(slice.prompt_tokens);
            usage.completion_tokens = usage.completion_tokens.saturating_add(slice.completion_tokens);
            usage.cached_tokens = usage.cached_tokens.saturating_add(slice.cached_tokens);
            usage.requests = usage.requests.saturating_add(slice.requests);
        }

        usage.total_tokens = usage.prompt_tokens.saturating_add(usage.completion_tokens);
        usage
    }

    /// Observe upstream probe fraction with reset detection, monotonic token deltas, and sanity bounds
    pub fn observe_upstream_probe(&self, now_ms: u64, remaining_fraction: f64) {
        let current_lifetime = self.lifetime_tokens.load(Ordering::Relaxed);
        let mut probe = self.last_probe_snapshot.write();

        if let Some((prev_time, prev_frac, prev_lifetime)) = *probe {
            // Check for upstream quota reset (fraction jumped upwards by > 5%)
            if remaining_fraction > prev_frac + 0.05 {
                // An actual completed cycle occurred before this reset!
                let cycle_tokens = current_lifetime.saturating_sub(prev_lifetime);
                if cycle_tokens > 0 {
                    let mut completed_5h = self.completed_5h_cycles.write();
                    completed_5h.push((now_ms, cycle_tokens, 1));
                    if completed_5h.len() > 100 {
                        completed_5h.remove(0);
                    }
                }

                // Upstream window reset occurred: reset baseline without fitting
                *probe = Some((now_ms, remaining_fraction, current_lifetime));
                return;
            }

            // Normal within-window drop (monotonic delta)
            if now_ms >= prev_time && (now_ms - prev_time) <= FIVE_HOURS_MS {
                let frac_delta = prev_frac - remaining_fraction;
                let token_delta = current_lifetime.saturating_sub(prev_lifetime);

                // Significant consumption jump (tolerance for IEEE 754 precision)
                if frac_delta >= 0.0095 && token_delta >= 1000 {
                    let inferred_capacity = (token_delta as f64 / frac_delta).round() as u64;

                    // Clamping guard: ignore unreasonable outliers
                    if (MIN_REASONABLE_CAPACITY..=MAX_REASONABLE_CAPACITY).contains(&inferred_capacity) {
                        let mut cap = self.cached_capacity.write();
                        if let Some(existing) = *cap {
                            // EWMA smooth capacity (70% historical, 30% new observation)
                            let smoothed = (existing as f64 * 0.7 + inferred_capacity as f64 * 0.3).round() as u64;
                            *cap = Some(smoothed);
                        } else {
                            *cap = Some(inferred_capacity);
                        }
                    }
                }
            }
        }

        *probe = Some((now_ms, remaining_fraction, current_lifetime));
    }

    /// Compute full estimate snapshot
    pub fn estimate_capacity(&self, now_ms: u64, current_remaining_fraction: Option<f64>) -> KeyCapacityEstimate {
        let (window_5h, window_weekly, window_monthly) = self.query_windows(now_ms);

        let cap_5h = *self.cached_capacity.read();
        let (account_tier, confidence) = match cap_5h {
            Some(cap) if cap >= 350_000 => ("pro".to_string(), 0.95),
            Some(cap) if cap >= 100_000 => ("standard".to_string(), 0.85),
            Some(_) => ("free".to_string(), 0.75),
            None => {
                // Heuristic based on consumption without full drop calibration yet
                if window_5h.total_tokens > 200_000 || window_weekly.total_tokens > 500_000 {
                    ("pro".to_string(), 0.50)
                } else if window_5h.requests > 0 {
                    ("calibrating".to_string(), 0.30)
                } else {
                    ("unknown".to_string(), 0.0)
                }
            }
        };

        let estimated_tokens_remaining_5h = match (cap_5h, current_remaining_fraction) {
            (Some(cap), Some(frac)) => Some((cap as f64 * frac).round() as u64),
            _ => None,
        };

        let completed_5h_stats = {
            let cycles = self.completed_5h_cycles.read();
            if !cycles.is_empty() {
                let count = cycles.len() as u64;
                let total_tokens: u64 = cycles.iter().map(|(_, t, _)| *t).sum();
                Some(CycleStats {
                    count,
                    total_tokens,
                    avg_tokens: total_tokens / count,
                })
            } else {
                None
            }
        };

        KeyCapacityEstimate {
            window_5h,
            window_weekly,
            window_monthly,
            completed_5h_stats,
            estimated_capacity_5h: cap_5h,
            estimated_tokens_remaining_5h,
            account_tier,
            confidence,
        }
    }

    pub fn export_snapshot(&self) -> KeyUsageStateSnapshot {
        KeyUsageStateSnapshot {
            slices: self.slices.read().values().cloned().collect(),
            cached_capacity: *self.cached_capacity.read(),
            last_probe: *self.last_probe_snapshot.read(),
            completed_5h: self.completed_5h_cycles.read().clone(),
            completed_weekly: self.completed_weekly_cycles.read().clone(),
        }
    }

    pub fn import_snapshot(&self, snap: KeyUsageStateSnapshot) {
        if let Some(cap) = snap.cached_capacity {
            *self.cached_capacity.write() = Some(cap);
        }
        if let Some(probe) = snap.last_probe {
            *self.last_probe_snapshot.write() = Some(probe);
            self.lifetime_tokens.store(probe.2, Ordering::Relaxed);
        }
        *self.completed_5h_cycles.write() = snap.completed_5h;
        *self.completed_weekly_cycles.write() = snap.completed_weekly;
        let mut slices = self.slices.write();
        for item in snap.slices {
            slices.insert(item.timestamp_ms, item);
        }
    }

    pub fn completed_cycle_stats(&self) -> (u64, u64) {
        let cycles = self.completed_5h_cycles.read();
        let count = cycles.len() as u64;
        let total: u64 = cycles.iter().map(|(_, t, _)| *t).sum();
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
