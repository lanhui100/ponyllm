use std::sync::Arc;
use std::time::Instant;
use ponyllm_core::telemetry::*;

// Fixed script exercising every projection-relevant variant.
fn script() -> Vec<(Option<String>, GatewayEvent)> {
    vec![
        (
            Some("prov-a".to_string()),
            GatewayEvent::RouteResolved {
                provider: "prov-a".to_string(),
                translated: false,
                routing_ms: 2.0,
            },
        ),
        (
            Some("prov-a".to_string()),
            GatewayEvent::KeySelected {
                key_id: "k1".to_string(),
                select_ms: 0.5,
            },
        ),
        (
            Some("prov-a".to_string()),
            GatewayEvent::UpstreamHeaders {
                key_id: "k1".to_string(),
                attempt: 0,
                ttfb_ms: 120.0,
            },
        ),
        (
            Some("prov-a".to_string()),
            GatewayEvent::StreamStarted { request_snippet: None },
        ),
        (
            Some("prov-a".to_string()),
            GatewayEvent::StreamCompleted {
                flow: StreamFlowSample {
                    ttft_ms: Some(200.0),
                    ttlb_ms: 1000.0,
                    chunks: 50,
                    bytes: 2048,
                    max_gap_ms: Some(60.0),
                    stall_count: 0,
                    tps: Some(50.0),
                    tpot_p50_ms: Some(18.0),
                    tpot_p95_ms: Some(40.0),
                    tpot_mean_ms: Some(20.0),
                },
                stages: StageTimings {
                    routing_ms: Some(2.0),
                    upstream_ttfb_ms: Some(120.0),
                    upstream_ttft_ms: None,
                    downstream_ttft_ms: Some(200.0),
                },
                request_snippet: None,
            },
        ),
        (
            Some("prov-b".to_string()),
            GatewayEvent::UpstreamAttemptFailed {
                key_id: "k9".to_string(),
                attempt: 0,
                status_code: Some(429),
                kind: "rate_limit_exceeded".to_string(),
                failover: true,
                summary: "HTTP 429".to_string(),
                detail: None,
                latency_ms: 90.0,
                request_snippet: None,
            },
        ),
        (
            Some("prov-b".to_string()),
            GatewayEvent::UpstreamAttemptFailed {
                key_id: "k9".to_string(),
                attempt: 1,
                status_code: Some(400),
                kind: "client_bad_request".to_string(),
                failover: false,
                summary: "bad".to_string(),
                detail: None,
                latency_ms: 10.0,
                request_snippet: None,
            },
        ),
        (
            Some("prov-b".to_string()),
            GatewayEvent::RequestCompleted {
                status_code: 200,
                latency_ms: 300.0,
                prompt_tokens: 100,
                completion_tokens: 20,
                tps: Some(66.0),
                request_snippet: None,
                response_snippet: None,
            },
        ),
        (
            None,
            GatewayEvent::RequestFailed {
                status_code: 502,
                latency_ms: 50.0,
                error: "down".to_string(),
                request_snippet: None,
            },
        ),
    ]
}

fn fold_all() -> MetricsSummary {
    let metrics = Arc::new(MetricsCollector::new());
    let mproj = MetricsProjection::new(metrics.clone());
    let sproj = StreamProjection::default();
    let bus = EventBus::new(64);
    bus.add_projection(Arc::new(mproj));
    bus.add_projection(Arc::new(sproj));
    let ctx = EventCtx::new("req-replay", "/v1/chat/completions", Instant::now());
    for (i, (provider, ev)) in script().into_iter().enumerate() {
        bus.append_at(&ctx, provider, ev, 1_700_000_000_000 + i as u64);
    }
    metrics.get_summary()
}

#[test]
fn test_replay_is_deterministic() {
    let a = fold_all();
    let b = fold_all();
    assert_eq!(a.total_requests, b.total_requests);
    assert_eq!(a.total_requests, 3); // stream-complete + json-complete + failed
    assert_eq!(a.total_failover, b.total_failover);
    assert_eq!(a.total_failover, 1); // only the 429 counts, not the 400
    assert_eq!(a.stream.stream_count, 1);
    assert_eq!(a.stream.total_chunks, 50);
    assert_eq!(a.prompt_tokens, 100);
    assert_eq!(a.completion_tokens, 70); // 50 stream chunks + 20 json
    assert_eq!(a.failed_requests, 1);
    assert_eq!(a.successful_requests, 2);
}

#[test]
fn test_stream_projection_per_provider() {
    let sproj = StreamProjection::default();
    let ctx = EventCtx::new("req-p", "/v1/chat/completions", Instant::now());
    for (provider, ev) in script() {
        let env = EventEnvelope {
            seq: 0,
            request_id: ctx.request_id.clone(),
            session_id: None,
            provider,
            endpoint: ctx.endpoint.clone(),
            wall_ms: 1,
            elapsed_ms: 1.0,
            event: ev,
        };
        Projection::apply(&sproj, &env);
    }
    let snap = sproj.snapshot_all();
    assert_eq!(snap["prov-a"].stream_count, 1);
    assert_eq!(snap["prov-a"].total_stalls, 0);
    assert!(snap["prov-a"].avg_gap_ms.is_some());
    // prov-b never streamed
    assert!(snap.get("prov-b").map(|s| s.stream_count).unwrap_or(0) == 0);
}

#[test]
fn test_overflow_marker_on_full_segment_channel() {
    let bus = EventBus::new(16);
    // Rendezvous channel with no receiver parked: try_send always fails.
    let (tx, _rx) = std::sync::mpsc::sync_channel::<EventEnvelope>(0);
    bus.attach_segment_sink(tx);
    let ctx = EventCtx::new("req-o", "/e", Instant::now());
    bus.append(&ctx, None, GatewayEvent::StreamProgress { chunks: 1, bytes: 1 });
    assert_eq!(bus.dropped_count(), 1);
    let recent = bus.recent(4);
    assert!(recent.iter().any(|e| matches!(
        e.event,
        GatewayEvent::TelemetryOverflow { dropped: 1 }
    )));
}

#[test]
fn test_error_kind_names_stable() {
    use ponyllm_core::error::GatewayErrorKind;
    assert_eq!(
        GatewayErrorKind::RateLimitExceeded { retry_after: None }.kind_name(),
        "rate_limit_exceeded"
    );
    assert_eq!(
        GatewayErrorKind::ClientBadRequest.kind_name(),
        "client_bad_request"
    );
    assert!(GatewayErrorKind::UpstreamUnavailable.triggers_failover());
    assert!(!GatewayErrorKind::ClientBadRequest.triggers_failover());
}
