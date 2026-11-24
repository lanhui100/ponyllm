//! Integration acceptance for proactive performance alerting (backlog
//! `B1795490123835`, handover `TASK-2026-09-29-alerting-handover.md` §5).
//!
//! These drive the **real** `EventBus` so the assertion is about the wiring
//! (projection registered → retries counted → thresholds evaluated → log +
//! webhook emitted), not about the pure evaluator (unit-tested in `alerts.rs`).
//!
//! The webhook receiver is a local `wiremock` server, so "webhook received and
//! its shape matches §4.2" is checked end-to-end without leaving the process.

use ponyllm_core::telemetry::{
    EventBus, EventCtx, GatewayEvent, StreamFlowSample,
};
use ponyllm_server::alerts::{
    AlertConfig, AlertProjection, AlertThresholds, AlertTransport,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sample(ttft_ms: f64, max_gap_ms: f64) -> StreamFlowSample {
    StreamFlowSample {
        ttft_ms: Some(ttft_ms),
        downstream_ttft_ms: Some(ttft_ms),
        ttlb_ms: 42_000.0,
        chunks: 120,
        bytes: 900,
        max_gap_ms: Some(max_gap_ms),
        stall_count: 1,
        tps: Some(3.0),
        tpot_p50_ms: Some(20.0),
        tpot_p95_ms: Some(80.0),
        tpot_mean_ms: Some(30.0),
        prompt_tokens: 10,
        completion_tokens: 200,
        cached_tokens: 0,
    }
}

fn ctx(request_id: &str) -> EventCtx {
    EventCtx {
        request_id: request_id.to_string(),
        session_id: None,
        model: Some("gemini-3.8-flash-high".to_string()),
        endpoint: "/v1/chat/completions".to_string(),
        start: Instant::now(),
    }
}

fn emit_attempt_failures(bus: &EventBus, request_id: &str, n: u32) {
    for attempt in 0..n {
        bus.append(
            &ctx(request_id),
            Some("antigravity".to_string()),
            GatewayEvent::UpstreamAttemptFailed {
                key_id: "key-1".to_string(),
                attempt,
                status_code: Some(502),
                kind: "empty_stop".to_string(),
                failover: true,
                summary: "empty STOP".to_string(),
                detail: None,
                latency_ms: 120.0,
                request_snippet: None,
            },
        );
    }
}

fn complete_stream(bus: &EventBus, request_id: &str, ttft: f64, gap: f64) {
    bus.append(
        &ctx(request_id),
        Some("antigravity".to_string()),
        GatewayEvent::StreamCompleted {
            flow: sample(ttft, gap),
            stages: Default::default(),
            request_snippet: None,
            response_snippet: None,
        },
    );
}

/// Threshold-only projection with no transport: proves "below threshold → zero
/// events" through the real bus.
#[tokio::test]
async fn healthy_request_emits_no_alert() {
    let bus = EventBus::new(64);
    let proj = Arc::new(AlertProjection::new(
        AlertThresholds {
            cooldown: Duration::from_secs(60),
            ..AlertConfig::default().thresholds()
        },
        None,
    ));
    bus.add_projection(proj.clone());
    complete_stream(&bus, "req-healthy", 1_500.0, 200.0);
    assert_eq!(proj.emitted(), 0, "healthy request must produce no alert");
}

/// TTFT > 20s through the real bus → exactly one `ttft_slow`.
#[tokio::test]
async fn slow_ttft_emits_single_alert() {
    let bus = EventBus::new(64);
    let proj = Arc::new(AlertProjection::new(
        AlertThresholds {
            cooldown: Duration::from_secs(60),
            ..AlertConfig::default().thresholds()
        },
        None,
    ));
    bus.add_projection(proj.clone());
    complete_stream(&bus, "req-slow", 32_700.0, 100.0);
    assert_eq!(proj.emitted(), 1);
}

/// Cooldown: two slow requests inside the window produce ONE alert.
#[tokio::test]
async fn cooldown_dedups_repeated_alerts() {
    let bus = EventBus::new(64);
    let proj = Arc::new(AlertProjection::new(
        AlertThresholds {
            cooldown: Duration::from_secs(60),
            ..AlertConfig::default().thresholds()
        },
        None,
    ));
    bus.add_projection(proj.clone());
    complete_stream(&bus, "req-a", 48_400.0, 100.0);
    complete_stream(&bus, "req-b", 48_400.0, 100.0);
    complete_stream(&bus, "req-c", 48_400.0, 100.0);
    assert_eq!(
        proj.emitted(),
        1,
        "three in-window ttft_slow must merge into one push"
    );
}

/// Retry storm: 11 transparent retries crosses the critical threshold (handover
/// measured a max of 11) and the count survives the bus round-trip.
#[tokio::test]
async fn retry_burst_counts_attempts_through_bus() {
    struct Capture(std::sync::Mutex<Vec<ponyllm_server::alerts::AlertEvent>>);
    impl AlertTransport for Capture {
        fn deliver(&self, event: &ponyllm_server::alerts::AlertEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    let bus = EventBus::new(64);
    let capture = Arc::new(Capture(std::sync::Mutex::new(Vec::new())));
    let proj = Arc::new(AlertProjection::new(
        AlertThresholds {
            cooldown: Duration::from_secs(60),
            ..AlertConfig::default().thresholds()
        },
        Some(capture.clone() as Arc<dyn AlertTransport>),
    ));
    bus.add_projection(proj.clone());

    emit_attempt_failures(&bus, "req-storm", 11);
    complete_stream(&bus, "req-storm", 1_000.0, 100.0);

    let seen = capture.0.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].event, "retry_burst");
    assert_eq!(seen[0].level, ponyllm_server::alerts::AlertLevel::Critical);
    assert_eq!(seen[0].retries, 11);
}

/// Webhook end-to-end: the delivered body matches the §4.2/§4.3 contract.
#[tokio::test]
async fn webhook_receives_spec_shaped_event() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let bus = EventBus::new(64);
    let cfg = AlertConfig {
        enabled: true,
        webhook_url: Some(format!("{}/alerts", server.uri())),
        // 0 cooldown so the single event is never merged away.
        cooldown_secs: 0,
        ..Default::default()
    };
    let proj = Arc::new(
        ponyllm_server::alerts::build_projection(&cfg).expect("enabled config yields a projection"),
    );
    bus.add_projection(proj.clone());
    complete_stream(&bus, "req-hook", 33_000.0, 100.0);

    // Delivery is detached by design; poll instead of sleeping a fixed amount.
    for _ in 0..100 {
        if !server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "webhook must receive exactly one push");

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json body");
    for key in ["event", "level", "message", "ts", "fields"] {
        assert!(body.get(key).is_some(), "webhook body missing `{key}`: {body}");
    }
    assert_eq!(body["event"], "ttft_slow");
    assert_eq!(body["level"], "warning");
    assert_eq!(body["fields"]["request_id"], "req-hook");
    assert_eq!(body["fields"]["model"], "gemini-3.8-flash-high");
    assert_eq!(body["fields"]["provider"], "antigravity");
}

/// Non-goal guard (handover §7): alerting must not alter the flow samples the
/// rest of the pipeline sees. The projection only reads them.
#[tokio::test]
async fn alerting_does_not_mutate_flow_sample() {
    let bus = EventBus::new(64);
    let proj = Arc::new(AlertProjection::new(
        AlertThresholds::default(),
        None,
    ));
    bus.add_projection(proj);
    let expected = sample(33_000.0, 12_000.0);
    bus.append(
        &ctx("req-immutable"),
        Some("antigravity".to_string()),
        GatewayEvent::StreamCompleted {
            flow: expected.clone(),
            stages: Default::default(),
            request_snippet: None,
            response_snippet: None,
        },
    );
    // The bus ring holds the envelope verbatim: the projection is read-only.
    let ring = bus.recent(1);
    let last = ring.last().expect("event recorded");
    match &last.event {
        GatewayEvent::StreamCompleted { flow, .. } => {
            assert_eq!(flow.ttft_ms, expected.ttft_ms);
            assert_eq!(flow.max_gap_ms, expected.max_gap_ms);
            assert_eq!(flow.chunks, expected.chunks);
        }
        other => panic!("unexpected event {other:?}"),
    }
}
