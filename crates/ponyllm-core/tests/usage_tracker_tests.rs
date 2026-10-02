use ponyllm_core::pool::usage::{
    aligned_period_observations, KeyUsageStateSnapshot, KeyUsageTracker,
    PoolCycleBenchmark, CYCLE_KIND_5H, CYCLE_KIND_WEEKLY, FIVE_HOURS_MS, SEVEN_DAYS_MS,
};
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use std::collections::BTreeMap;

#[test]
fn test_key_usage_tracker_sliding_windows() {
    let tracker = KeyUsageTracker::new();
    let base_ms = 1_700_000_000_000u64;

    // Record usage 6 hours ago (outside 5h, inside 7d)
    tracker.record_tokens(base_ms - 6 * 3600 * 1000, 10_000, 2_000, 1_000);

    // Record usage 2 hours ago (inside 5h, inside 7d)
    tracker.record_tokens(base_ms - 2 * 3600 * 1000, 20_000, 5_000, 2_000);

    // Record usage 10 minutes ago
    tracker.record_tokens(base_ms - 10 * 60 * 1000, 5_000, 1_000, 500);

    // Query 5h window
    let u_5h = tracker.query_window(base_ms, FIVE_HOURS_MS);
    assert_eq!(u_5h.prompt_tokens, 25_000);
    assert_eq!(u_5h.completion_tokens, 6_000);
    assert_eq!(u_5h.total_tokens, 31_000);
    assert_eq!(u_5h.requests, 2);

    // Query weekly window
    let u_7d = tracker.query_window(base_ms, SEVEN_DAYS_MS);
    assert_eq!(u_7d.prompt_tokens, 35_000);
    assert_eq!(u_7d.completion_tokens, 8_000);
    assert_eq!(u_7d.total_tokens, 43_000);
    assert_eq!(u_7d.requests, 3);
}

#[test]
fn test_capacity_estimation_and_tier_inference() {
    let tracker = KeyUsageTracker::new();
    let t0 = 1_700_000_000_000u64;

    // Probe 1: 100% remaining
    tracker.observe_upstream_probe(t0, 1.0);

    // Initial tier should be unknown
    let est0 = tracker.estimate_capacity(t0, Some(1.0));
    assert_eq!(est0.account_tier, "unknown");

    // Burn 100k tokens over 1 hour
    let t1 = t0 + 3600 * 1000;
    tracker.record_tokens(t1 - 1800 * 1000, 80_000, 20_000, 5_000);

    // Probe 2: 80% remaining (delta fraction = 0.20, delta tokens = 100,000)
    tracker.observe_upstream_probe(t1, 0.80);

    // Inferred capacity should be within reasonable bounds
    let est1 = tracker.estimate_capacity(t1, Some(0.80));
    assert!(est1.estimated_capacity_5h.is_some());
    let cap = est1.estimated_capacity_5h.unwrap();
    assert!(cap >= 400_000 && cap <= 700_000, "inferred cap {} out of expected range", cap);
    assert!(est1.estimated_tokens_remaining_5h.is_some());
    assert_eq!(est1.account_tier, "pro"); // >= 350k is pro tier
    assert!(est1.confidence >= 0.8);

    // Upstream reset detection test: jump from 0.80 back to 1.0
    // Record another request to advance tokens before reset, or reset happens after traffic
    let t2 = t1 + 3600 * 1000;
    tracker.record_tokens(t2 - 1000, 10_000, 5_000, 1_000);
    tracker.observe_upstream_probe(t2, 1.0);
    // Capacity should remain intact and calibration status should be benchmarked
    let est2 = tracker.estimate_capacity(t2, Some(1.0));
    assert_eq!(est2.calibration_status, "benchmarked");
    assert!(est2.completed_5h_stats.is_some());
    let stats = est2.completed_5h_stats.unwrap();
    assert_eq!(stats.count, 1);
    assert_eq!(stats.prompt_tokens, 10_000);
    assert_eq!(stats.completion_tokens, 5_000);
    assert_eq!(stats.cached_tokens, 1_000);
    assert_eq!(stats.total_tokens, 15_000);
    assert_eq!(stats.requests, 1);
}

#[test]
fn test_pool_record_tokens_integration() {
    let pool = KeyPool::new("antigravity", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("acc-1", "dummy-secret", 1, 10));

    let now_ms = 1_700_000_000_000u64;
    pool.record_tokens("acc-1", now_ms, 5_000, 1_000, 200);

    let keys = pool.snapshot_keys();
    let entry = keys.iter().find(|k| k.id == "acc-1").unwrap();
    let usage = entry.usage_tracker.query_window(now_ms, FIVE_HOURS_MS);

    assert_eq!(usage.total_tokens, 6_000);
    assert_eq!(usage.requests, 1);
}

#[test]
fn test_weekly_cycle_recorded_via_dual_probe_and_surfaces_in_estimate() {
    let tracker = KeyUsageTracker::new();
    let t0 = 1_700_000_000_000u64;

    // Establish both baselines.
    tracker.observe_upstream_probe_dual(t0, Some(1.0), Some(1.0));

    // Consume tokens, then the weekly bucket resets (fraction jumps 0.5 -> 1.0).
    let t1 = t0 + 3600 * 1000;
    tracker.record_tokens(t1 - 1000, 20_000, 10_000, 5_000);
    tracker.observe_upstream_probe_dual(t1, Some(0.98), Some(0.5));
    let t2 = t1 + 3600 * 1000;
    tracker.record_tokens(t2 - 1000, 4_000, 1_000, 0);
    tracker.observe_upstream_probe_dual(t2, Some(0.96), Some(1.0));

    let est = tracker.estimate_capacity_dual(t2, Some(0.96), Some(1.0));
    let weekly = est
        .completed_weekly_stats
        .expect("weekly cycle must be archived");
    assert_eq!(weekly.count, 1, "one completed weekly cycle");
    // Cycle = tokens consumed between the last weekly probe (0.5) and the
    // weekly reset probe (1.0): 4k prompt + 1k completion = 5k.
    assert_eq!(weekly.total_tokens, 5_000);
    assert_eq!(weekly.requests, 1);

    // The 5h probe only dropped (no 5h reset) → no 5h cycle recorded.
    assert!(est.completed_5h_stats.is_none());
}

#[test]
fn test_completed_cycles_carry_monotonic_seq_across_snapshot_roundtrip() {
    let tracker = KeyUsageTracker::new();
    let t0 = 1_700_000_000_000u64;

    // Both baselines established at full.
    tracker.observe_upstream_probe_dual(t0, Some(1.0), Some(1.0));

    // Leg 1: 5h bucket drops; no reset yet.
    let t1 = t0 + 3600 * 1000;
    tracker.record_tokens(t1 - 1000, 10_000, 5_000, 1_000);
    tracker.observe_upstream_probe_dual(t1, Some(0.8), Some(0.95));

    // Leg 2: 5h bucket resets (0.8 -> 1.0) → completed 5h cycle, seq=1.
    let t2 = t1 + 3600 * 1000;
    tracker.record_tokens(t2 - 1000, 2_000, 1_000, 0);
    tracker.observe_upstream_probe_dual(t2, Some(1.0), Some(0.9));

    // Leg 3: weekly bucket drains further; no reset.
    let t3 = t2 + 3600 * 1000;
    tracker.observe_upstream_probe_dual(t3, Some(1.0), Some(0.9));
    let t4 = t3 + 3600 * 1000;
    tracker.record_tokens(t4 - 1000, 4_000, 1_000, 0);
    tracker.observe_upstream_probe_dual(t4, Some(0.9), Some(0.4));

    // Leg 4: weekly bucket resets (0.4 -> 1.0) → completed weekly cycle, seq=2.
    let t5 = t4 + 3600 * 1000;
    tracker.record_tokens(t5 - 1000, 1_000, 500, 0);
    tracker.observe_upstream_probe_dual(t5, Some(0.88), Some(1.0));

    let snap = tracker.export_snapshot();
    assert_eq!(snap.completed_5h_records.len(), 1);
    assert_eq!(snap.completed_weekly_records.len(), 1);
    assert_eq!(snap.completed_5h_records[0].kind, CYCLE_KIND_5H);
    assert_eq!(snap.completed_5h_records[0].seq, 1);
    assert_eq!(snap.completed_weekly_records[0].kind, CYCLE_KIND_WEEKLY);
    assert_eq!(snap.completed_weekly_records[0].seq, 2);
    assert!(snap.next_cycle_seq >= 2, "counter advanced: {}", snap.next_cycle_seq);

    // Roundtrip: restore into a fresh tracker preserves records + seq counter.
    let fresh = KeyUsageTracker::new();
    fresh.import_snapshot(snap);
    let est = fresh.estimate_capacity_dual(t5, Some(0.88), Some(1.0));
    // Cycle = tokens consumed between the last probe-baseline and the reset.
    assert_eq!(
        est.completed_5h_stats.unwrap().total_tokens,
        3_000,
        "5h cycle restored (leg 2 delta)"
    );
    assert_eq!(
        est.completed_weekly_stats.unwrap().total_tokens,
        1_500,
        "weekly cycle restored (leg 4 delta: 1k + 0.5k)"
    );
    let re_export = fresh.export_snapshot();
    assert_eq!(re_export.completed_5h_records[0].seq, 1);
    assert_eq!(re_export.completed_weekly_records[0].seq, 2);
}

#[test]
fn test_aligned_period_observations_only_closed_periods() {
    use ponyllm_core::pool::usage::UsageSlice;
    let period = FIVE_HOURS_MS;
    let base = 1_700_000_000_000u64; // aligned to 5h boundary if divisible
    let aligned_now = (base / period) * period + period; // exact end of a period
    let mut slices = vec![
        UsageSlice {
            timestamp_ms: aligned_now - period + 1000,
            prompt_tokens: 10_000,
            completion_tokens: 5_000,
            cached_tokens: 0,
            requests: 2,
            ..Default::default()
        },
        // Slice in the current (still open) period — must be excluded. The
        // next boundary is `aligned_now + period`, so anything between
        // `aligned_now` and that boundary is in the open period.
        UsageSlice {
            timestamp_ms: aligned_now + 60_000,
            prompt_tokens: 99_000,
            completion_tokens: 1_000,
            cached_tokens: 0,
            requests: 9,
            ..Default::default()
        },
        // Zero-usage slice — skipped entirely.
        UsageSlice {
            timestamp_ms: aligned_now - period + 3000,
            ..Default::default()
        },
    ];
    let obs = aligned_period_observations(&slices, period, aligned_now);
    assert_eq!(obs.len(), 1, "only the single closed period survives");
    assert_eq!(obs[0].period_end_ms, aligned_now);
    assert_eq!(obs[0].total_tokens, 15_000);
    assert_eq!(obs[0].requests, 2);

    // Once the same period closes later, additional closed periods appear.
    let later = aligned_now + period;
    let obs2 = aligned_period_observations(&slices, period, later);
    assert_eq!(obs2.len(), 2, "both closed periods now observable");
    assert_eq!(obs2[1].total_tokens, 100_000);

    slices.clear();
}

#[test]
fn test_pool_benchmark_merge_is_idempotent_across_restarts() {
    let period = FIVE_HOURS_MS;
    let aligned_now = (1_700_000_000_000u64 / period) * period + period;
    let slices = vec![ponyllm_core::pool::usage::UsageSlice {
        timestamp_ms: aligned_now - period + 1000,
        prompt_tokens: 8_000,
        completion_tokens: 2_000,
        cached_tokens: 500,
        requests: 4,
        ..Default::default()
    }];
    let usages: BTreeMap<String, KeyUsageStateSnapshot> = [(
        "acc-1".to_string(),
        KeyUsageStateSnapshot {
            slices: slices.clone(),
            ..Default::default()
        },
    )]
    .into_iter()
    .collect();

    let mut bench = PoolCycleBenchmark::default();
    let merges = |b: &mut PoolCycleBenchmark| b.merge_usages(&usages, aligned_now);
    merges(&mut bench);
    let after_first = bench.kind_5h.clone();
    assert_eq!(after_first.observations, 1);
    assert_eq!(after_first.total_tokens, 10_000);
    assert_eq!(after_first.avg_tokens(), 10_000);

    // Same data merged again (re-save / restart replay) must NOT double count.
    merges(&mut bench);
    assert_eq!(
        bench.kind_5h.observations, after_first.observations,
        "watermark prevents duplicate merging"
    );
    assert_eq!(bench.kind_5h.total_tokens, after_first.total_tokens);

    // A new closed period from the same account adds exactly one observation.
    let usages2: BTreeMap<String, KeyUsageStateSnapshot> = [(
        "acc-1".to_string(),
        KeyUsageStateSnapshot {
            slices: vec![
                slices[0].clone(),
                ponyllm_core::pool::usage::UsageSlice {
                    timestamp_ms: aligned_now + 1000,
                    prompt_tokens: 3_000,
                    completion_tokens: 1_000,
                    cached_tokens: 0,
                    requests: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    )]
    .into_iter()
    .collect();
    let later = aligned_now + period;
    bench.merge_usages(&usages2, later);
    assert_eq!(bench.kind_5h.observations, 2);
    assert_eq!(bench.kind_5h.total_tokens, 14_000);
    assert_eq!(bench.kind_monthly.observations, 0);
    assert_eq!(bench.kind_weekly.observations, 0);
}

#[test]
fn test_pool_benchmark_merge_completed_cycles_dedup_by_seq() {
    let rec = |seq: u64| ponyllm_core::pool::usage::CompletedCycleRecord {
        cycle_end_ms: 1_700_000_000_000,
        kind: CYCLE_KIND_5H.to_string(),
        seq,
        prompt_tokens: 100,
        completion_tokens: 50,
        cached_tokens: 0,
        total_tokens: 150,
        requests: 2,
    };
    let snap = KeyUsageStateSnapshot {
        completed_5h_records: vec![rec(1), rec(2)],
        completed_weekly_records: vec![],
        ..Default::default()
    };
    let usages: BTreeMap<String, KeyUsageStateSnapshot> =
        [("acc-1".to_string(), snap)].into_iter().collect();

    let mut bench = PoolCycleBenchmark::default();
    bench.merge_usages(&usages, 1_700_000_000_000);
    assert_eq!(bench.kind_5h.completed_cycles, 2);
    assert_eq!(bench.kind_5h.completed_total_tokens, 300);

    // Restart replay of the same snapshot (same seq) must not re-merge.
    bench.merge_usages(&usages, 1_700_000_000_000);
    assert_eq!(bench.kind_5h.completed_cycles, 2);

    // A later cycle with a higher seq merges once.
    let usages2: BTreeMap<String, KeyUsageStateSnapshot> = [(
        "acc-1".to_string(),
        KeyUsageStateSnapshot {
            completed_5h_records: vec![rec(3)],
            ..Default::default()
        },
    )]
    .into_iter()
    .collect();
    bench.merge_usages(&usages2, 1_700_000_000_000);
    assert_eq!(bench.kind_5h.completed_cycles, 3);
    assert_eq!(bench.kind_5h.completed_total_tokens, 450);
}
