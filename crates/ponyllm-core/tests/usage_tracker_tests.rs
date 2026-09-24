use ponyllm_core::pool::usage::{KeyUsageTracker, FIVE_HOURS_MS, SEVEN_DAYS_MS};
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};

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

    // Inferred capacity should be 100_000 / 0.20 = 500_000 tokens
    let est1 = tracker.estimate_capacity(t1, Some(0.80));
    assert_eq!(est1.estimated_capacity_5h, Some(500_000));
    assert_eq!(est1.estimated_tokens_remaining_5h, Some(400_000)); // 500k * 0.8
    assert_eq!(est1.account_tier, "pro"); // >= 350k is pro tier
    assert!(est1.confidence >= 0.9);

    // Upstream reset detection test: jump from 0.80 back to 1.0
    let t2 = t1 + 3600 * 1000;
    tracker.observe_upstream_probe(t2, 1.0);
    // Capacity should remain intact
    let est2 = tracker.estimate_capacity(t2, Some(1.0));
    assert_eq!(est2.estimated_capacity_5h, Some(500_000));
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
