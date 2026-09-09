use std::sync::Arc;
use std::time::Instant;
use ponyllm_core::telemetry::{
    ConnectivitySampler, ConnectivityStatus, EventBus, EventCtx, GatewayEvent,
    TimeseriesProjection, GATEWAY_SLOT_COUNT, GATEWAY_STEP_MS,
};

#[test]
fn test_connectivity_sampler_thresholds_and_bar_count() {
    let sampler = ConnectivitySampler::new(40, 1500); // 40 slots, 1500ms step
    let now = 1_700_000_000_000u64;

    // 1. Record fast success (<300ms) -> Ok
    sampler.record("deepseek", now, Some(120.0), true);
    // 2. Record moderate success (300ms..1000ms) -> Degraded
    sampler.record("openai", now, Some(450.0), true);
    // 3. Record slow success (>=1000ms) -> Down
    sampler.record("anthropic", now, Some(1500.0), true);
    // 4. Record failure -> Down
    sampler.record("fallback", now, Some(80.0), false);

    let ds_bars = sampler.get_series("deepseek", now);
    assert_eq!(ds_bars.slots.len(), 40, "Must return exactly 40 slots");
    assert_eq!(ds_bars.latest_latency_ms, Some(120.0));
    let last_ds = ds_bars.slots.last().unwrap();
    assert_eq!(last_ds.status, ConnectivityStatus::Ok);

    let oa_bars = sampler.get_series("openai", now);
    assert_eq!(oa_bars.latest_latency_ms, Some(450.0));
    let last_oa = oa_bars.slots.last().unwrap();
    assert_eq!(last_oa.status, ConnectivityStatus::Degraded);

    let an_bars = sampler.get_series("anthropic", now);
    assert_eq!(an_bars.latest_latency_ms, Some(1500.0));
    let last_an = an_bars.slots.last().unwrap();
    assert_eq!(last_an.status, ConnectivityStatus::Down);

    let fb_bars = sampler.get_series("fallback", now);
    let last_fb = fb_bars.slots.last().unwrap();
    assert_eq!(last_fb.status, ConnectivityStatus::Down);
}

#[test]
fn test_timeseries_projection_aggregation_and_model_breakdown() {
    let bus = Arc::new(EventBus::new(100));
    let timeseries_proj = Arc::new(TimeseriesProjection::default());
    bus.add_projection(timeseries_proj.clone());

    let now = 1_700_000_000_000u64;
    let mut ctx = EventCtx::new("req-1", "/v1/chat/completions", Instant::now());
    ctx.model = Some("deepseek-chat".to_string());

    // Send RequestCompleted event for deepseek
    bus.append_at(
        &ctx,
        Some("deepseek".to_string()),
        GatewayEvent::RequestCompleted {
            status_code: 200,
            latency_ms: 250.0,
            prompt_tokens: 100,
            completion_tokens: 50,
            tps: Some(40.0),
            request_snippet: None,
            response_snippet: None,
        },
        now,
    );

    // Send another request for openai
    let mut ctx2 = EventCtx::new("req-2", "/v1/chat/completions", Instant::now());
    ctx2.model = Some("gpt-4o".to_string());
    bus.append_at(
        &ctx2,
        Some("openai".to_string()),
        GatewayEvent::RequestCompleted {
            status_code: 200,
            latency_ms: 400.0,
            prompt_tokens: 200,
            completion_tokens: 100,
            tps: Some(30.0),
            request_snippet: None,
            response_snippet: None,
        },
        now + 1000,
    );

    // Query 24h history
    let resp = timeseries_proj.query_history("24h", now + 2000);
    assert_eq!(resp.range, "24h");
    assert_eq!(resp.total_requests, 2);
    assert_eq!(resp.total_tokens, 450); // (100+50) + (200+100) = 450
    assert_eq!(resp.provider_tokens.get("deepseek"), Some(&150));
    assert_eq!(resp.provider_tokens.get("openai"), Some(&300));
    assert_eq!(resp.model_tokens.get("deepseek-chat"), Some(&150));
    assert_eq!(resp.model_tokens.get("gpt-4o"), Some(&300));

    // Verify points
    assert!(!resp.points.is_empty(), "Must have bucket points");
    let active_point = resp.points.iter().find(|p| p.total_requests > 0).unwrap();
    assert_eq!(active_point.total_tokens, 450);
    assert_eq!(active_point.tokens_by_provider.get("deepseek"), Some(&150));
    assert_eq!(active_point.tokens_by_model.get("deepseek-chat"), Some(&150));
}

#[test]
fn test_connectivity_sampler_constant_ring_and_provider_isolation() {
    let sampler = ConnectivitySampler::new(40, 1500);
    let base_time = 1_700_000_000_000u64;

    // Record thousands of samples for provider-1 and provider-2
    for i in 0..5000 {
        let ts = base_time + (i % 60) * 1000;
        sampler.record("provider-1", ts, Some(150.0), true);
        sampler.record("provider-2", ts, Some(500.0), true);
    }

    let p1_bars = sampler.get_series("provider-1", base_time + 59_000);
    assert_eq!(p1_bars.slots.len(), 40);
    assert_eq!(p1_bars.latest_latency_ms, Some(150.0));

    let p2_bars = sampler.get_series("provider-2", base_time + 59_000);
    assert_eq!(p2_bars.slots.len(), 40);
    assert_eq!(p2_bars.latest_latency_ms, Some(500.0));

    // Non-existent provider returns 40 empty slots
    let empty_bars = sampler.get_series("non-existent", base_time + 59_000);
    assert_eq!(empty_bars.slots.len(), 40);
    assert_eq!(empty_bars.latest_latency_ms, None);
    assert!(empty_bars.slots.iter().all(|s| s.status == ConnectivityStatus::Empty));
}

#[test]
fn test_timeseries_alignment_7d_and_30d() {
    let timeseries_proj = TimeseriesProjection::default();
    let now = 1_700_000_000_000u64; // arbitrary reference time

    // Test 7d query: 28 buckets of 6h = 168h
    let resp_7d = timeseries_proj.query_history("7d", now);
    assert_eq!(resp_7d.range, "7d");
    assert_eq!(resp_7d.points.len(), 28);
    let six_hours_ms = 6 * 3600 * 1000;
    for (i, p) in resp_7d.points.iter().enumerate() {
        assert_eq!(p.timestamp_ms % six_hours_ms, 0, "Bucket {} not aligned to 6h", i);
    }

    // Test 30d query: 30 buckets of 24h = 720h
    let resp_30d = timeseries_proj.query_history("30d", now);
    assert_eq!(resp_30d.range, "30d");
    assert_eq!(resp_30d.points.len(), 30);
    let day_ms = 24 * 3600 * 1000;
    for (i, p) in resp_30d.points.iter().enumerate() {
        assert_eq!(p.timestamp_ms % day_ms, 0, "Bucket {} not aligned to 24h", i);
    }
}

#[test]
fn test_timeseries_float_safety_and_serialization() {
    let bus = Arc::new(EventBus::new(100));
    let timeseries_proj = Arc::new(TimeseriesProjection::default());
    bus.add_projection(timeseries_proj.clone());

    let now = 1_700_000_000_000u64;
    let ctx = EventCtx::new("req-nan", "/v1/chat/completions", Instant::now());

    // Send events with NaN and Infinity
    bus.append_at(
        &ctx,
        Some("prov".to_string()),
        GatewayEvent::RequestCompleted {
            status_code: 200,
            latency_ms: f64::NAN,
            prompt_tokens: 10,
            completion_tokens: 10,
            tps: Some(f64::INFINITY),
            request_snippet: None,
            response_snippet: None,
        },
        now,
    );

    let resp = timeseries_proj.query_history("24h", now + 1000);
    // Must serialize to JSON without error (NaN/Inf causes serde_json error if not handled)
    let json_str = serde_json::to_string(&resp);
    assert!(json_str.is_ok(), "Serialization failed due to non-finite float");
}

#[test]
fn test_timeseries_clock_skew_resilience() {
    let timeseries_proj = TimeseriesProjection::default();
    let base_now = 1_700_000_000_000u64;

    // Record legitimate sample
    timeseries_proj.record_metric(base_now, Some("prov"), Some("model"), 100, 50, 150.0, true);

    // Try to record extreme future sample (e.g. +10 years)
    let future_time = base_now + 10 * 365 * 24 * 3600 * 1000;
    timeseries_proj.record_metric(future_time, Some("prov"), Some("model"), 500, 500, 150.0, true);

    // Past legitimate bucket must NOT be evicted
    let resp = timeseries_proj.query_history("24h", base_now + 1000);
    assert_eq!(resp.total_requests, 1, "Legitimate sample should still be present");
    assert_eq!(resp.total_tokens, 150);
}

#[test]
fn test_gateway_default_is_24_slots_5s_covering_2min() {
    assert_eq!(GATEWAY_SLOT_COUNT, 24);
    assert_eq!(GATEWAY_STEP_MS, 5000);
    assert_eq!(GATEWAY_SLOT_COUNT as u64 * GATEWAY_STEP_MS, 120_000);

    let sampler = ConnectivitySampler::default();
    assert_eq!(sampler.gateway_slot_count(), 24);
    assert_eq!(sampler.gateway_step_ms(), 5000);
    let now = 1_700_000_000_000u64;
    sampler.record("gateway", now, Some(20.0), true);
    let series = sampler.get_series("gateway", now);
    assert_eq!(series.slots.len(), 24);
    assert_eq!(series.slots.last().unwrap().status, ConnectivityStatus::Ok);
    // 间隔应为5s
    let n = series.slots.len();
    assert_eq!(
        series.slots[n - 1].timestamp_ms - series.slots[n - 2].timestamp_ms,
        5000
    );
}

#[test]
fn test_provider_bars_are_continuous_per_call_no_time_gaps() {
    let sampler = ConnectivitySampler::default();
    let base = 1_700_000_000_000u64;
    // 稀疏调用：间隔远大于5s，时间桶方案会在中间产生Empty
    sampler.record("prov-a", base, Some(100.0), true);
    sampler.record("prov-a", base + 3600_000, Some(500.0), true);
    sampler.record("prov-a", base + 7200_000, Some(50.0), false);

    let series = sampler.get_series("prov-a", base + 7200_000 + 1000);
    assert_eq!(series.slots.len(), 40);
    // 尾部3柱必须连续为本次3次调用，无Empty空洞
    let tail = &series.slots[37..];
    assert_eq!(tail[0].status, ConnectivityStatus::Ok);
    assert_eq!(tail[1].status, ConnectivityStatus::Degraded);
    assert_eq!(tail[2].status, ConnectivityStatus::Down);
    assert!(tail.iter().all(|s| s.status != ConnectivityStatus::Empty));
}

#[test]
fn test_connectivity_snapshot_restore_preserves_calls() {
    let sampler = ConnectivitySampler::default();
    let base = 1_700_000_000_000u64;
    sampler.record("prov-b", base, Some(120.0), true);
    sampler.record("prov-b", base + 1000, Some(1500.0), true);
    sampler.record("gateway", base, Some(10.0), true);

    let snap = sampler.snapshot_state();
    let restored = ConnectivitySampler::default();
    restored.restore_state(snap);

    let p = restored.get_series("prov-b", base + 2000);
    assert_eq!(p.slots.len(), 40);
    let tail = &p.slots[38..];
    assert_eq!(tail[0].status, ConnectivityStatus::Ok);
    assert_eq!(tail[1].status, ConnectivityStatus::Down);

    let g = restored.get_series("gateway", base + 2000);
    assert_eq!(g.slots.len(), 24);
}

#[test]
fn test_timeseries_and_metrics_snapshot_restore() {
    use ponyllm_core::telemetry::MetricsCollector;
    let ts = TimeseriesProjection::default();
    let now = 1_700_000_000_000u64;
    ts.record_metric(now, Some("p"), Some("m"), 100, 50, 150.0, true);
    let buckets = ts.snapshot_buckets();
    let ts2 = TimeseriesProjection::default();
    ts2.restore_buckets(buckets);
    let resp = ts2.query_history("24h", now + 1000);
    assert_eq!(resp.total_requests, 1);
    assert_eq!(resp.total_tokens, 150);

    let m = MetricsCollector::new();
    m.record_request("/v1/chat", std::time::Duration::from_millis(100), 10, 20, true);
    let snap = m.snapshot_counters();
    let m2 = MetricsCollector::new();
    m2.restore_counters(&snap);
    let summary = m2.get_summary();
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.total_tokens, 30);
}

