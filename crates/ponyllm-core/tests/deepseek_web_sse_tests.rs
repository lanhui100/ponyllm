//! Wave-9 deepseek-web — SSE event parsing coverage of the frozen H-09
//! (`parse_completion_event`, contract §5 DW-T-* input side; REA §5).
//!
//! The `dw_t_*` judgment tests live in the gateway acceptance file (HTTP
//! full-chain, task-4 types). These tests pin the event-model contract at the
//! core level: every test calls the frozen stub `parse_completion_event`
//! (`todo!()`) and is RED at this phase.

use ponyllm_core::deepseek_web::{parse_completion_event, DeltaOp, SseError, SseEvent};
use serde_json::json;

// ---------------------------------------------------------------------------
// DW-T-01 input: ready event fields (REA §5)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_ready_event() {
    let got = parse_completion_event(
        "ready",
        r#"{"response_message_id":"rsp_1","request_message_id":"req_1","model_type":"deepseek-chat"}"#,
    )
    .expect("stub panics -> red")
    .expect("ready must produce an event");
    assert_eq!(
        got,
        SseEvent::Ready {
            response_message_id: "rsp_1".into(),
            request_message_id: "req_1".into(),
            model_type: "deepseek-chat".into(),
        },
        "DW-T-01 input: ready -> Ready{{response_message_id, request_message_id, model_type}}"
    );
}

// ---------------------------------------------------------------------------
// DW-T-02 input: SET op {o,p,v} (REA §5)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_delta_set_op() {
    let got = parse_completion_event(
        "delta",
        r#"{"o":"SET","p":"/message/content/0/content/0/text","v":"Hel"}"#,
    )
    .expect("stub panics -> red")
    .expect("delta must produce an event");
    assert_eq!(
        got,
        SseEvent::Delta(DeltaOp::Set {
            path: "/message/content/0/content/0/text".into(),
            value: json!("Hel"),
        }),
        "DW-T-02 input: SET -> Delta(Set{{path, value}})"
    );
}

// ---------------------------------------------------------------------------
// DW-T-03 input: BATCH op, recursive in-order expansion (REA §5)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_delta_batch_recursive() {
    let data = r#"{"o":"BATCH","p":"/message/content/0/content","v":[{"o":"SET","p":"/message/content/0/content/0/type","v":"text"},{"o":"BATCH","p":"/message/content/0/content","v":[{"o":"SET","p":"/message/content/0/content/0/text","v":"world"}]}]}"#;
    let got = parse_completion_event("delta", data)
        .expect("stub panics -> red")
        .expect("delta must produce an event");
    assert_eq!(
        got,
        SseEvent::Delta(DeltaOp::Batch(vec![
            DeltaOp::Set {
                path: "/message/content/0/content/0/type".into(),
                value: json!("text"),
            },
            DeltaOp::Batch(vec![DeltaOp::Set {
                path: "/message/content/0/content/0/text".into(),
                value: json!("world"),
            }]),
        ])),
        "DW-T-03 input: BATCH -> Delta(Batch([...])) with nested BATCH preserved recursively"
    );
}

// ---------------------------------------------------------------------------
// DW-T-04 input: finish usage present/absent (REA §5; contract §10.1)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_finish_usage_absent_and_present() {
    let no_usage = parse_completion_event(
        "finish",
        r#"{"response_message_id":"rsp_1","model_type":"deepseek-chat"}"#,
    )
    .expect("stub panics -> red")
    .expect("finish must produce an event");
    assert_eq!(no_usage, SseEvent::Finish { usage: None });

    let with_usage = parse_completion_event(
        "finish",
        r#"{"response_message_id":"rsp_1","usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}"#,
    )
    .expect("stub panics -> red")
    .expect("finish must produce an event");
    match with_usage {
        SseEvent::Finish { usage } => {
            let u = usage.expect("DW-T-04 input: usage present must be surfaced");
            assert_eq!(u["prompt_tokens"], 11);
            assert_eq!(u["completion_tokens"], 3);
            assert_eq!(u["total_tokens"], 14);
        }
        other => panic!("DW-T-04 input: expected Finish, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DW-T-05 input: close reason mapping (REA §5; contract §10.2)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_close_reasons() {
    for (reason, expected) in [
        ("success", Some("success")),
        ("invalid_query", Some("invalid_query")),
        ("timeout", Some("timeout")),
        ("error", Some("error")),
        // missing close_reason -> tolerated (contract §10.2 treats as success)
        ("{}", None),
    ] {
        let data = if reason == "{}" {
            "{}".to_string()
        } else {
            format!(r#"{{"close_reason":"{reason}","click_behavior":{{"force_mobile":false}}}}"#)
        };
        let got = parse_completion_event("close", &data)
            .expect("stub panics -> red")
            .expect("close must produce an event");
        assert_eq!(
            got,
            SseEvent::Close { close_reason: expected.map(String::from) },
            "DW-T-05 input: close {reason:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-T-07 input: non-fatal hint/toast/title/updateSession events
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_non_fatal_events() {
    let cases: [(&str, &str, SseEvent); 4] = [
        ("hint", r#"{"type":"notice"}"#, SseEvent::Hint(json!({"type":"notice"}))),
        ("toast", r#"{"type":"business_hint","msg":"x"}"#, SseEvent::Toast(json!({"type":"business_hint","msg":"x"}))),
        ("title", r#"{"title":"New title"}"#, SseEvent::Title(json!({"title":"New title"}))),
        ("updateSession", r#"{"session_id":"s1"}"#, SseEvent::UpdateSession(json!({"session_id":"s1"}))),
    ];
    for (name, data, expected) in cases {
        let got = parse_completion_event(name, data)
            .expect("stub panics -> red")
            .expect("non-fatal events must produce typed events");
        assert_eq!(got, expected, "DW-T-07 input: {name} -> {expected:?}");
    }
}

// ---------------------------------------------------------------------------
// DW-T-08 input: non-event frames are ignored (Ok(None)) — keep-alive, [DONE]
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_non_event_frames_ok_none() {
    let non_frames: [(&str, &str); 3] = [
        ("", "[DONE]"),          // [DONE]-style sentinel (no event name)
        ("", ""),                // comment-only / keep-alive frame: no name, no data
        ("", ": keep-alive"),    // comment line payload has no data: semantics
    ];
    for (name, data) in non_frames {
        let got = parse_completion_event(name, data)
            .expect("stub panics -> red");
        assert_eq!(got, None, "DW-T-08 input: {name:?}/{data:?} must be ignored (Ok(None))");
    }
}

// ---------------------------------------------------------------------------
// Malformed payloads / unknown ops -> typed SseError (contract H-09)
// ---------------------------------------------------------------------------

#[test]
fn sse_parse_malformed_json_errors() {
    let err = parse_completion_event("delta", "{not json").expect_err("stub panics -> red");
    assert!(
        matches!(err, SseError::Malformed(_)),
        "malformed JSON -> SseError::Malformed; got {err:?}"
    );
}

#[test]
fn sse_parse_unknown_op_errors() {
    let err = parse_completion_event("delta", r#"{"o":"REMOVE","p":"/x","v":1}"#)
        .expect_err("stub panics -> red");
    assert!(
        matches!(err, SseError::UnknownOp(_)),
        "unknown o -> SseError::UnknownOp; got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Amendment v4 delta frames: o=APPEND and missing-o (implicit APPEND) must
// parse to a value-carrying DeltaOp (REA §5 v4; real-chain smoke 2026-10-22:
// {"p":"response/fragments/-1/content","o":"APPEND","v":"..."},
// {"v":","}, {"v":" the"}).
// ---------------------------------------------------------------------------

/// Extract the text value carried by a DeltaOp (Set arm; Batch carries none).
/// Compile-safe against future DeltaOp variants (wildcard arm).
fn delta_value(op: &DeltaOp) -> Option<&serde_json::Value> {
    match op {
        DeltaOp::Set { value, .. } => Some(value),
        // Amendment v4: APPEND carries the appended text (real frame
        // {"p":"response/fragments/-1/content","o":"APPEND","v":"..."}).
        DeltaOp::Append { value, .. } => Some(value),
        _ => None,
    }
}

/// Flatten a DeltaOp tree into the ordered list of text values (BATCH recursed).
fn delta_values_in_order(op: &DeltaOp, out: &mut Vec<serde_json::Value>) {
    match op {
        DeltaOp::Set { value, .. } => out.push(value.clone()),
        DeltaOp::Append { value, .. } => out.push(value.clone()),
        DeltaOp::Batch(ops) => {
            for sub in ops {
                delta_values_in_order(sub, out);
            }
        }
        _ => {}
    }
}

#[test]
fn sse_parse_delta_append_op() {
    let got = parse_completion_event(
        "delta",
        r#"{"p":"response/fragments/-1/content","o":"APPEND","v":"Hel"}"#,
    )
    .expect("stub panics -> red")
    .expect("APPEND frame must produce an event (Amendment v4)");
    match got {
        SseEvent::Delta(op) => assert_eq!(
            delta_value(&op),
            Some(&json!("Hel")),
            "DW-T-02: APPEND carries the appended text as its value"
        ),
        other => panic!("APPEND frame -> Delta; got {other:?}"),
    }
}

#[test]
fn sse_parse_delta_missing_op_implicit_append() {
    for (label, data, expected) in [
        ("implicit append 1", r#"{"v":","}"#, json!(",")),
        ("implicit append 2", r#"{"v":" the"}"#, json!(" the")),
    ] {
        let got = parse_completion_event("delta", data)
            .expect("stub panics -> red")
            .expect("missing-o frame must produce an event (Amendment v4)");
        match got {
            SseEvent::Delta(op) => assert_eq!(
                delta_value(&op),
                Some(&expected),
                "{label}: missing o -> implicit APPEND with value"
            ),
            other => panic!("{label}: missing-o -> Delta; got {other:?}"),
        }
    }
}

#[test]
fn sse_parse_delta_batch_mixed_append_set_missing() {
    let data = r#"{"o":"BATCH","p":"/message/content/0/content","v":[{"o":"APPEND","p":"response/fragments/-1/content","v":"A"},{"v":"B"},{"o":"SET","p":"/message/content/0/content/0/text","v":"C"}]}"#;
    let got = parse_completion_event("delta", data)
        .expect("stub panics -> red")
        .expect("BATCH with mixed ops must produce an event (Amendment v4)");
    match got {
        SseEvent::Delta(op) => {
            let mut values = Vec::new();
            delta_values_in_order(&op, &mut values);
            assert_eq!(
                values,
                vec![json!("A"), json!("B"), json!("C")],
                "DW-T-03: BATCH recursion preserves APPEND/SET/missing-o order"
            );
        }
        other => panic!("mixed BATCH -> Delta; got {other:?}"),
    }
}


// ---------------------------------------------------------------------------
// Full-stream framing pipeline: raw SSE text -> sequence of typed events,
// mirroring the gateway-side consumer (event name + joined data -> H-09)
// ---------------------------------------------------------------------------

/// Split a raw SSE body into (event_name, joined_data) frames.
fn sse_frames(raw: &str) -> Vec<(String, String)> {
    let mut frames = Vec::new();
    for block in raw.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut name = String::new();
        let mut data_lines: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data_lines.push(v.trim_start());
            }
            // comment/keep-alive lines ("...") carry no data -> ignored
        }
        if !data_lines.is_empty() || !name.is_empty() {
            frames.push((name, data_lines.join("\n")));
        }
    }
    frames
}

#[test]
fn sse_full_stream_framing_pipeline() {
    let stream = "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n\
: keep-alive comment\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"Hel\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"lo\"}\n\n\
event: delta\ndata: {\"o\":\"BATCH\",\"p\":\"/message/content/0/content\",\"v\":[{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/type\",\"v\":\"text\"},{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/text\",\"v\":\" world\"}]}\n\n\
data: [DONE]\n\n\
event: finish\ndata: {\"response_message_id\":\"r1\",\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}}\n\n\
event: close\ndata: {\"close_reason\":\"success\"}\n\n";

    let events: Vec<SseEvent> = sse_frames(stream)
        .into_iter()
        .map(|(name, data)| parse_completion_event(&name, &data).expect("stub panics -> red"))
        .filter_map(|e| e)
        .collect();

    assert_eq!(
        events,
        vec![
            SseEvent::Ready { response_message_id: "r1".into(), request_message_id: "q1".into(), model_type: "m".into() },
            SseEvent::Delta(DeltaOp::Set { path: "/message/content/0/content/0/text".into(), value: json!("Hel") }),
            SseEvent::Delta(DeltaOp::Set { path: "/message/content/0/content/0/text".into(), value: json!("lo") }),
            SseEvent::Delta(DeltaOp::Batch(vec![
                DeltaOp::Set { path: "/message/content/0/content/1/type".into(), value: json!("text") },
                DeltaOp::Set { path: "/message/content/0/content/1/text".into(), value: json!(" world") },
            ])),
            SseEvent::Finish { usage: Some(json!({"prompt_tokens":5,"completion_tokens":3,"total_tokens":8})) },
            SseEvent::Close { close_reason: Some("success".into()) },
        ],
        "full pipeline: keep-alive and [DONE] frames skipped, ops in order"
    );
}