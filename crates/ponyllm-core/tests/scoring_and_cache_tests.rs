use std::time::Duration;
use ponyllm_core::pool::{
    BillingMode, EconomyScorer, HotCacheTracker, NodeLatencyMetrics, PricingConfig,
    SpeedScorer, parse_retry_after_header,
};

#[test]
fn test_hot_cache_tracker_threshold_and_probe() {
    let tracker = HotCacheTracker::new();

    // Short prompt (< 1024 chars) should be ignored to prevent cache churn
    let short_prompt = "Hello, write a quick python function";
    tracker.record_dispatch(short_prompt, "openai");
    assert_eq!(tracker.probe_cached_provider(short_prompt), None);

    // Long prompt (>= 1024 chars) should be tracked
    let long_prompt = "A".repeat(1500);
    tracker.record_dispatch(&long_prompt, "deepseek");
    assert_eq!(tracker.probe_cached_provider(&long_prompt), Some("deepseek".to_string()));

    // Chinese multi-byte UTF-8 prompt
    let chinese_prompt = "深度求索人工智能模型推理系统测试".repeat(100);
    tracker.record_dispatch(&chinese_prompt, "deepseek-chinese");
    assert_eq!(tracker.probe_cached_provider(&chinese_prompt), Some("deepseek-chinese".to_string()));

    // Different long prompt should not hit
    let another_long_prompt = "B".repeat(1500);
    assert_eq!(tracker.probe_cached_provider(&another_long_prompt), None);
}

#[test]
fn test_economy_scorer_hierarchy() {
    // 1. Free provider (0元免费节点)
    let free_pricing = PricingConfig { input_price: 0.0, cached_price: 0.0, output_price: 0.0 };
    let free_score = EconomyScorer::score_candidate(&free_pricing, BillingMode::Metered, false, 100_000, 1000);

    // 2. Plan node with quota
    let plan_pricing = PricingConfig { input_price: 1.0, cached_price: 0.5, output_price: 2.0 };
    let plan_score = EconomyScorer::score_candidate(&plan_pricing, BillingMode::Plan, false, 100_000, 1000);

    // 3. Cache hit metered node
    let metered_pricing = PricingConfig { input_price: 0.14, cached_price: 0.014, output_price: 0.28 };
    let cache_hit_score = EconomyScorer::score_candidate(&metered_pricing, BillingMode::Metered, true, 100_000, 1000);

    // 4. Normal cache miss metered node
    let cache_miss_score = EconomyScorer::score_candidate(&metered_pricing, BillingMode::Metered, false, 100_000, 1000);

    // Hierarchy rule: Free (Tier 0) < Plan (Tier 1) < Cache Hit (Tier 2) < Cache Miss (Tier 3) (Lower score = higher priority)
    assert!(free_score < plan_score);
    assert!(plan_score < cache_hit_score);
    assert!(cache_hit_score < cache_miss_score);
}

#[test]
fn test_speed_scorer_physical_latency_formula() {
    // Node A: Fast TTFT (200ms) but slow TPS (10 tokens/sec)
    // For 500 tokens: 200 + (500 / 10 * 1000) = 200 + 50000 = 50200ms
    let node_a = NodeLatencyMetrics::new_with_values(200.0, 10.0);
    let score_a = SpeedScorer::estimate_total_latency_ms(&node_a, 500);

    // Node B: Normal TTFT (800ms) but fast TPS (100 tokens/sec)
    // For 500 tokens: 800 + (500 / 100 * 1000) = 800 + 5000 = 5800ms
    let node_b = NodeLatencyMetrics::new_with_values(800.0, 100.0);
    let score_b = SpeedScorer::estimate_total_latency_ms(&node_b, 500);

    // Node B must win for 500 token generation even though TTFT is higher! (Do NOT only look at TTFT)
    assert!(score_b < score_a);
    assert_eq!(score_b as u64, 5800);
    assert_eq!(score_a as u64, 50200);

    // Cold start node has sensible prior (TTFT=800ms, TPS=40.0)
    let cold_node = NodeLatencyMetrics::default();
    let cold_score = SpeedScorer::estimate_total_latency_ms(&cold_node, 500);
    assert_eq!(cold_score as u64, 800 + 12500); // 800 + (500/40 * 1000) = 13300ms
}

#[test]
fn test_retry_after_parsing_and_safe_clamping() {
    // 1. Seconds format "30"
    let dur1 = parse_retry_after_header("30");
    assert_eq!(dur1, Duration::from_secs(30));

    // 2. Out of bounds high seconds "999999" clamped to 300s (5 minutes safe cap)
    let dur2 = parse_retry_after_header("999999");
    assert_eq!(dur2, Duration::from_secs(300));

    // 3. 0 seconds clamped to 1s minimum
    let dur3 = parse_retry_after_header("0");
    assert_eq!(dur3, Duration::from_secs(1));

    // 4. Invalid header returns default fallback 60s
    let dur4 = parse_retry_after_header("invalid-data");
    assert_eq!(dur4, Duration::from_secs(60));
}
