use std::time::Duration;
use ponyllm_core::telemetry::*;

#[test]
fn test_flight_recorder_record_and_sanitize() {
    let recorder = FlightRecorder::new(10);

    // Record a failed request with sensitive key
    recorder.record(FlightFrame {
        request_id: "req-123".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        provider: None,
        key_id: "primary-key".to_string(),
        raw_key: Some("sk-proj-1234567890abcdef".to_string()),
        attempt: None,
        status_code: Some(429),
        latency: Duration::from_millis(150),
        error: Some("Rate limit exceeded".to_string()),
        request_snippet: Some("{\"model\":\"gpt-4o\"}".to_string()),
        response_snippet: Some("{\"error\":\"rate_limit\"}".to_string()),
        stream_flow: None,
    });

    let frames = recorder.get_recent_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].request_id, "req-123");
    assert_eq!(frames[0].status_code, Some(429));
    
    // Key must be sanitized in output!
    assert_eq!(frames[0].sanitized_key, "sk-***cdef");
}

#[test]
fn test_unicode_and_emoji_key_sanitization_safety() {
    // 1. Emoji prefix key (must not panic!)
    let emoji_key = "🔑sk-12345678abcdef";
    let sanitized_emoji = FlightRecorder::sanitize_key(emoji_key);
    assert!(sanitized_emoji.contains("***"));

    // 2. Multibyte Chinese key
    let cn_key = "自定义密钥-abcdef123456";
    let sanitized_cn = FlightRecorder::sanitize_key(cn_key);
    assert!(sanitized_cn.contains("***"));

    // 3. Short keys (<= 8 chars)
    assert_eq!(FlightRecorder::sanitize_key("123"), "****");
    assert_eq!(FlightRecorder::sanitize_key("sk-12345"), "****");

    // 4. Standard sk- key
    let std_key = "sk-1234567890abcdef";
    assert_eq!(FlightRecorder::sanitize_key(std_key), "sk-***cdef");
}

#[test]
fn test_flight_recorder_snippet_truncation() {
    let recorder = FlightRecorder::new(5);
    let giant_snippet = "x".repeat(2000);

    recorder.record(FlightFrame {
        request_id: "req-giant".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        provider: None,
        key_id: "k1".to_string(),
        raw_key: None,
        attempt: None,
        status_code: Some(200),
        latency: Duration::from_millis(50),
        error: None,
        request_snippet: Some(giant_snippet),
        response_snippet: None,
        stream_flow: None,
    });

    let frames = recorder.get_recent_frames();
    assert_eq!(frames.len(), 1);
    let snip = frames[0].request_snippet.as_ref().unwrap();
    assert!(snip.len() < 1000);
    assert!(snip.ends_with("...[TRUNCATED]"));
}

#[test]
fn test_flight_recorder_ring_buffer_capacity() {
    let recorder = FlightRecorder::new(3);

    for i in 0..5 {
        recorder.record(FlightFrame {
            request_id: format!("req-{}", i),
            endpoint: "/v1/chat/completions".to_string(),
            provider: None,
            key_id: format!("key-{}", i),
            raw_key: None,
            attempt: None,
            status_code: Some(200),
            latency: Duration::from_millis(50),
            error: None,
            request_snippet: None,
            response_snippet: None,
            stream_flow: None,
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

#[test]
fn test_stream_flow_aggregate_reusable() {
    let metrics = MetricsCollector::new();
    metrics.record_stream(&StreamFlowSample {
        ttft_ms: Some(800.0),
        ttlb_ms: 5000.0,
        chunks: 100,
        bytes: 4096,
        max_gap_ms: Some(1200.0),
        stall_count: 1,
        tps: Some(20.0),
        tpot_p50_ms: Some(40.0),
        tpot_p95_ms: Some(300.0),
        tpot_mean_ms: Some(45.0),
    });
    metrics.record_stream(&StreamFlowSample {
        ttft_ms: Some(1000.0),
        ttlb_ms: 6000.0,
        chunks: 200,
        bytes: 8192,
        max_gap_ms: Some(800.0),
        stall_count: 0,
        tps: Some(30.0),
        tpot_p50_ms: Some(30.0),
        tpot_p95_ms: Some(200.0),
        tpot_mean_ms: Some(35.0),
    });
    let summary = metrics.get_summary();
    assert_eq!(summary.stream.stream_count, 2);
    assert_eq!(summary.stream.total_stalls, 1);
    assert_eq!(summary.stream.max_gap_ms, Some(1200.0));
    assert_eq!(summary.stream.total_chunks, 300);
    assert_eq!(summary.stream.total_bytes, 12288);
    let avg_ttft = summary.stream.avg_ttft_ms.unwrap();
    assert!((avg_ttft - 900.0).abs() < 1.0, "avg_ttft={}", avg_ttft);
}

#[test]
fn test_gap_percentiles_empty_and_bursty() {
    let (p50, p95, max) = gap_percentiles(vec![]);
    assert_eq!((p50, p95, max), (None, None, None));
    let (p50, p95, max) = gap_percentiles(vec![10.0, 20.0, 30.0, 40.0, 1200.0]);
    assert_eq!(max, Some(1200.0));
    assert!(p50.unwrap() <= p95.unwrap());
    assert_eq!(p95, Some(1200.0));
}

#[test]
fn test_stream_flow_detail_survives_recorder() {
    let recorder = FlightRecorder::new(10);
    recorder.record(FlightFrame {
        request_id: "req-stream".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        provider: Some("opencode-zen".to_string()),
        key_id: "zen".to_string(),
        raw_key: None,
        attempt: None,
        status_code: Some(200),
        latency: Duration::from_millis(5200),
        error: None,
        request_snippet: None,
        response_snippet: Some("[STREAM_COMPLETED chunks=100]".to_string()),
        stream_flow: Some(StreamFlowDetail {
            ttft_ms: Some(900.0),
            ttlb_ms: Some(5200.0),
            chunks: Some(100),
            bytes: Some(4096),
            max_gap_ms: Some(1100.0),
            stall_count: Some(1),
            tps: Some(20.0),
            tpot_p50_ms: Some(40.0),
            tpot_p95_ms: Some(350.0),
        }),
    });
    let frames = recorder.get_recent_frames();
    let flow = frames[0].stream_flow.as_ref().expect("stream_flow kept");
    assert_eq!(flow.stall_count, Some(1));
    assert_eq!(flow.chunks, Some(100));
}
