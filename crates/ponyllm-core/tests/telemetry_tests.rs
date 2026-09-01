use std::time::Duration;
use ponyllm_core::telemetry::*;

#[test]
fn test_flight_recorder_record_and_sanitize() {
    let recorder = FlightRecorder::new(10);

    // Record a failed request with sensitive key
    recorder.record(FlightFrame {
        request_id: "req-123".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        key_id: "primary-key".to_string(),
        raw_key: Some("sk-proj-1234567890abcdef".to_string()),
        status_code: Some(429),
        latency: Duration::from_millis(150),
        error: Some("Rate limit exceeded".to_string()),
        request_snippet: Some("{\"model\":\"gpt-4o\"}".to_string()),
        response_snippet: Some("{\"error\":\"rate_limit\"}".to_string()),
    });

    let frames = recorder.get_recent_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].request_id, "req-123");
    assert_eq!(frames[0].status_code, Some(429));
    
    // Key must be sanitized in output!
    assert_eq!(frames[0].sanitized_key, "sk-***cdef");
}

#[test]
fn test_flight_recorder_ring_buffer_capacity() {
    let recorder = FlightRecorder::new(3);

    for i in 0..5 {
        recorder.record(FlightFrame {
            request_id: format!("req-{}", i),
            endpoint: "/v1/chat/completions".to_string(),
            key_id: format!("key-{}", i),
            raw_key: None,
            status_code: Some(200),
            latency: Duration::from_millis(50),
            error: None,
            request_snippet: None,
            response_snippet: None,
        });
    }

    let frames = recorder.get_recent_frames();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].request_id, "req-2");
    assert_eq!(frames[1].request_id, "req-3");
    assert_eq!(frames[2].request_id, "req-4");
}

#[test]
fn test_metrics_collector() {
    let metrics = MetricsCollector::new();

    metrics.record_request("/v1/chat/completions", Duration::from_millis(100), 50, 20, true);
    metrics.record_request("/v1/chat/completions", Duration::from_millis(200), 30, 10, false);

    let summary = metrics.get_summary();
    assert_eq!(summary.total_requests, 2);
    assert_eq!(summary.failed_requests, 1);
    assert_eq!(summary.prompt_tokens, 80);
    assert_eq!(summary.completion_tokens, 30);
    assert_eq!(summary.total_tokens, 110);
}
