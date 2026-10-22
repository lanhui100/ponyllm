//! SSE event parsing for `POST /api/v0/chat/completion` (wave-9 contract
//! DW-T-*, H-09). Evidence: REA §5. Translation into OpenAI chunks is a
//! task-4 server-side concern (contract §5); this module yields typed events
//! from raw SSE frames.
//!
//! Event names (REA §5): `ready`, `delta`, `finish`, `close`, plus non-fatal
//! `hint`/`toast`/`title`/`updateSession`. `delta` is a `{p, o, v}` frame with
//! `o ∈ {APPEND, SET, BATCH}`; a **missing `o` is an implicit APPEND**
//! (real-wire frames `{"v":","}`, `{"v":" the"}` — Amendment v4 / REA §5);
//! a BATCH's `v` is an op array applied recursively, in order (DW-T-03).

use serde_json::Value;

/// One completion-stream event (REA §5): `ready`, `delta`, `finish`, `close`,
/// plus non-fatal `hint`/`toast`/`title`/`updateSession` (DW-T-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// `ready`: starts the assistant turn; carries response/request message ids
    /// and model type (DW-T-01).
    Ready {
        response_message_id: String,
        request_message_id: String,
        model_type: String,
    },
    /// `delta`: one JSON-patch op (DW-T-02/03).
    Delta(DeltaOp),
    /// `finish`: end-of-message metadata; usage may be absent (DW-T-04, §10.1).
    Finish { usage: Option<Value> },
    /// `close`: `close_reason` = success|invalid_query|timeout|error (DW-T-05).
    Close { close_reason: Option<String> },
    /// Non-fatal business events (DW-T-07).
    Hint(Value),
    Toast(Value),
    Title(Value),
    UpdateSession(Value),
}

/// One delta op `{p, o, v}` (REA §5, Amendment v4): `o ∈ {APPEND, SET, BATCH}`;
/// a missing `o` is an implicit `APPEND` (real wire `{"v":","}`). `v` of a
/// BATCH is an op array applied recursively, in order (DW-T-03); sub-ops may
/// be APPEND / SET / implicit-APPEND / nested BATCH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaOp {
    /// `APPEND`: append `value` as incremental text (real path
    /// `response/fragments/-1/content`); `path` is `None` on implicit-APPEND
    /// frames that only carry `v` (DW-T-02, Amendment v4).
    Append { path: Option<String>, value: Value },
    /// `SET`: set path `p` to value `v`.
    Set { path: String, value: Value },
    /// `BATCH`: `v` is an op array, applied recursively in order.
    Batch(Vec<DeltaOp>),
}

/// Parse one SSE `data:` frame into a typed event (H-09, DW-T-01/02/08).
///
/// * Non-event frames — empty data, `[DONE]` sentinels, comment-only lines,
///   frames without an event name (REA §5, DW-T-08) — yield `Ok(None)`.
/// * Unknown event names yield `Ok(None)` so forward-compatible servers do
///   not break the stream (DW-T-09; the translator logs the raw frame at
///   trace level).
///
/// Pure function, zero I/O.
pub fn parse_completion_event(name: &str, data: &str) -> Result<Option<SseEvent>, SseError> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" || name.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| SseError::Malformed(format!("{name}: {e}")))?;
    match name {
        "ready" => {
            let need = |key: &str| {
                value
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| SseError::Other(format!("ready event missing {key}")))
            };
            Ok(Some(SseEvent::Ready {
                response_message_id: need("response_message_id")?,
                request_message_id: need("request_message_id")?,
                model_type: need("model_type")?,
            }))
        }
        "delta" => Ok(Some(SseEvent::Delta(parse_delta(&value)?))),
        "finish" => Ok(Some(SseEvent::Finish {
            usage: value.get("usage").cloned(),
        })),
        "close" => Ok(Some(SseEvent::Close {
            close_reason: value
                .get("close_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })),
        "hint" => Ok(Some(SseEvent::Hint(value))),
        "toast" => Ok(Some(SseEvent::Toast(value))),
        "title" => Ok(Some(SseEvent::Title(value))),
        "updateSession" => Ok(Some(SseEvent::UpdateSession(value))),
        _ => Ok(None),
    }
}

/// Recursively parse one delta payload `{p, o, v}` (DW-T-02/03, Amendment v4).
///
/// * `o = "APPEND"` → [`DeltaOp::Append`] with the frame's optional `p`;
/// * **missing `o`** → implicit [`DeltaOp::Append`] (real wire
///   `{"v":","}`, `{"v":" the"}`);
/// * `o = "SET"` → [`DeltaOp::Set`];
/// * `o = "BATCH"` → recursive expansion of the `v` op array.
pub fn parse_delta(value: &Value) -> Result<DeltaOp, SseError> {
    let op = value.get("o").and_then(|v| v.as_str());
    match op {
        Some("SET") => {
            let path = value
                .get("p")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SseError::Malformed("SET delta missing 'p'".to_string()))?
                .to_string();
            let v = value
                .get("v")
                .ok_or_else(|| SseError::Malformed("SET delta missing 'v'".to_string()))?
                .clone();
            Ok(DeltaOp::Set { path, value: v })
        }
        Some("APPEND") | None => {
            let path = value
                .get("p")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let v = value
                .get("v")
                .ok_or_else(|| SseError::Malformed("delta missing 'v'".to_string()))?
                .clone();
            Ok(DeltaOp::Append { path, value: v })
        }
        Some("BATCH") => {
            let items = value
                .get("v")
                .and_then(|v| v.as_array())
                .ok_or_else(|| SseError::Malformed("BATCH delta 'v' must be an array".to_string()))?;
            let mut ops = Vec::with_capacity(items.len());
            for item in items {
                ops.push(parse_delta(item)?);
            }
            Ok(DeltaOp::Batch(ops))
        }
        Some(other) => Err(SseError::UnknownOp(other.to_string())),
    }
}

/// Simple, spec-faithful line-based SSE frame reader: splits a raw SSE body
/// into `event:`/`data:` frames (multi-line `data:` joined with `\n`, per the
/// SSE spec) and maps each frame through `parse_completion_event`.
///
/// This is the typed-event-stream convenience for task-2 scope; the task-4
/// integration may equally drive `parse_completion_event` from its existing
/// streaming layer. Zero I/O.
pub fn parse_sse_frames(body: &str) -> Vec<Result<Option<SseEvent>, SseError>> {
    let mut out = Vec::new();
    let mut event_name = String::new();
    let mut data_lines: Vec<&str> = Vec::new();
    for raw_line in body.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.is_empty() {
            // frame delimiter
            if !data_lines.is_empty() {
                let data = data_lines.join("\n");
                out.push(parse_completion_event(&event_name, &data));
                event_name.clear();
                data_lines.clear();
            }
        }
        // comment lines (":" ...), other fields, and trailing whitespace
        // are ignored per the SSE spec.
    }
    // flush a final frame without a trailing blank line
    if !data_lines.is_empty() {
        let data = data_lines.join("\n");
        out.push(parse_completion_event(&event_name, &data));
    }
    out
}

/// SSE parsing errors (frozen).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SseError {
    /// Malformed event payload that cannot be decoded as JSON.
    #[error("malformed sse event: {0}")]
    Malformed(String),
    /// Unknown delta `o` value (neither `SET` nor `BATCH`).
    #[error("unknown delta op: {0}")]
    UnknownOp(String),
    /// Any other parsing failure.
    #[error("sse error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ready_event_parses() {
        let ev = parse_completion_event(
            "ready",
            r#"{"response_message_id":"rm1","request_message_id":"rq1","model_type":"deepseek-chat"}"#,
        )
        .unwrap()
        .expect("ready parses");
        assert_eq!(
            ev,
            SseEvent::Ready {
                response_message_id: "rm1".to_string(),
                request_message_id: "rq1".to_string(),
                model_type: "deepseek-chat".to_string(),
            }
        );
    }

    #[test]
    fn delta_set_parses() {
        let ev = parse_completion_event(
            "delta",
            r#"{"o":"SET","p":"/message/content/0","v":"Hello"}"#,
        )
        .unwrap()
        .expect("set parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Set {
                path: "/message/content/0".to_string(),
                value: json!("Hello"),
            })
        );
    }

    #[test]
    fn delta_batch_recursive_in_order() {
        let ev = parse_completion_event(
            "delta",
            r#"{"o":"BATCH","v":[{"o":"SET","p":"/message/content/1","v":"A"},{"o":"BATCH","v":[{"o":"SET","p":"/message/content/2","v":"B"}]},{"o":"SET","p":"/message/content/3","v":"C"}]}"#,
        )
        .unwrap()
        .expect("batch parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Batch(vec![
                DeltaOp::Set { path: "/message/content/1".to_string(), value: json!("A") },
                DeltaOp::Batch(vec![DeltaOp::Set {
                    path: "/message/content/2".to_string(),
                    value: json!("B")
                }]),
                DeltaOp::Set { path: "/message/content/3".to_string(), value: json!("C") },
            ]))
        );
    }

    #[test]
    fn delta_append_with_path_parses() {
        let ev = parse_completion_event(
            "delta",
            r#"{"p":"response/fragments/-1/content","o":"APPEND","v":"Hello"}"#,
        )
        .unwrap()
        .expect("append parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: Some("response/fragments/-1/content".to_string()),
                value: json!("Hello"),
            })
        );
    }

    #[test]
    fn delta_implicit_append_when_o_missing() {
        // real-wire frames (Amendment v4 / REA §5): {"v":","}, {"v":" the"}
        for (frame, want) in [
            (r#"{"v":","}"#, ","),
            (r#"{"v":" the"}"#, " the"),
        ] {
            let ev = parse_completion_event("delta", frame).unwrap().expect("implicit append parses");
            assert_eq!(
                ev,
                SseEvent::Delta(DeltaOp::Append { path: None, value: json!(want) }),
                "frame {frame}"
            );
        }
        // a missing `o` frame that still carries a path keeps it
        let ev = parse_completion_event(
            "delta",
            r#"{"p":"response/fragments/-1/content","v":"x"}"#,
        )
        .unwrap()
        .expect("implicit append with path parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: Some("response/fragments/-1/content".to_string()),
                value: json!("x"),
            })
        );
    }

    #[test]
    fn delta_batch_mixed_append_set_implicit() {
        // Amendment v4: BATCH sub-ops may be APPEND / SET / implicit APPEND,
        // nested BATCH recurses — all in order.
        let ev = parse_completion_event(
            "delta",
            r#"{"o":"BATCH","v":[
                {"o":"APPEND","p":"response/fragments/-1/content","v":"A"},
                {"v":","},
                {"o":"SET","p":"/message/content/0/type","v":"text"},
                {"o":"BATCH","v":[{"o":"APPEND","p":"response/fragments/-1/content","v":"B"}]},
                {"v":" the"}
            ]}"#,
        )
        .unwrap()
        .expect("mixed batch parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Batch(vec![
                DeltaOp::Append {
                    path: Some("response/fragments/-1/content".to_string()),
                    value: json!("A")
                },
                DeltaOp::Append { path: None, value: json!(",") },
                DeltaOp::Set { path: "/message/content/0/type".to_string(), value: json!("text") },
                DeltaOp::Batch(vec![DeltaOp::Append {
                    path: Some("response/fragments/-1/content".to_string()),
                    value: json!("B")
                }]),
                DeltaOp::Append { path: None, value: json!(" the") },
            ]))
        );
    }

    #[test]
    fn delta_missing_o_and_v_errors() {
        assert!(matches!(
            parse_completion_event("delta", r#"{}"#),
            Err(SseError::Malformed(_))
        ));
    }

    #[test]
    fn finish_with_and_without_usage() {
        let with = parse_completion_event("finish", r#"{"usage":{"prompt_tokens":1}}"#)
            .unwrap()
            .expect("finish parses");
        assert_eq!(with, SseEvent::Finish { usage: Some(json!({"prompt_tokens": 1})) });
        let without = parse_completion_event("finish", r#"{"msg":"done"}"#)
            .unwrap()
            .expect("finish without usage parses");
        assert_eq!(without, SseEvent::Finish { usage: None });
    }

    #[test]
    fn close_with_and_without_reason() {
        let c = parse_completion_event("close", r#"{"close_reason":"success"}"#)
            .unwrap()
            .expect("close parses");
        assert_eq!(c, SseEvent::Close { close_reason: Some("success".to_string()) });
        let c2 = parse_completion_event("close", r#"{"click_behavior":{}}"#)
            .unwrap()
            .expect("close without reason parses");
        assert_eq!(c2, SseEvent::Close { close_reason: None });
    }

    #[test]
    fn non_fatal_events_parse() {
        assert!(matches!(
            parse_completion_event("hint", r#"{"type":"hi"}"#).unwrap(),
            Some(SseEvent::Hint(_))
        ));
        assert!(matches!(
            parse_completion_event("toast", r#"{"type":"warn"}"#).unwrap(),
            Some(SseEvent::Toast(_))
        ));
        assert!(matches!(
            parse_completion_event("title", r#"{"title":"t"}"#).unwrap(),
            Some(SseEvent::Title(_))
        ));
        assert!(matches!(
            parse_completion_event("updateSession", r#"{}"#).unwrap(),
            Some(SseEvent::UpdateSession(_))
        ));
    }

    #[test]
    fn non_event_frames_are_none() {
        assert_eq!(parse_completion_event("", "").unwrap(), None);
        assert_eq!(parse_completion_event("", " ").unwrap(), None);
        assert_eq!(parse_completion_event("delta", "[DONE]").unwrap(), None);
        assert_eq!(parse_completion_event("ready", "").unwrap(), None);
    }

    #[test]
    fn unknown_event_name_is_none_not_error() {
        assert_eq!(parse_completion_event("future-event", r#"{"x":1}"#).unwrap(), None);
    }

    #[test]
    fn malformed_json_errors() {
        assert!(matches!(
            parse_completion_event("ready", "not-json"),
            Err(SseError::Malformed(_))
        ));
    }

    #[test]
    fn unknown_delta_op_errors() {
        assert!(matches!(
            parse_completion_event("delta", r#"{"o":"DELETE","p":"/x"}"#).unwrap_err(),
            SseError::UnknownOp(ref o) if o == "DELETE"
        ));
    }

    #[test]
    fn missing_required_delta_fields_error() {
        assert!(matches!(
            parse_completion_event("delta", r#"{"o":"SET","p":"/x"}"#),
            Err(SseError::Malformed(_))
        ));
        assert!(matches!(
            parse_completion_event("delta", r#"{"o":"BATCH","v":{}}"#),
            Err(SseError::Malformed(_))
        ));
    }

    #[test]
    fn sse_body_frames_multiline_data_joined() {
        let body = "event: ready\ndata: {\"response_message_id\":\"rm\",\"request_message_id\":\"rq\",\"model_type\":\"m\"}\n\n\
                    event: delta\ndata: {\"o\":\"SET\",\"p\":\"/a\",\"v\":\"x\"}\n\n";
        let events: Vec<_> = parse_sse_frames(body).into_iter().collect::<Result<_, _>>().unwrap();
        let non_null: Vec<_> = events.iter().filter_map(|e| e.as_ref()).collect();
        assert_eq!(non_null.len(), 2);
        assert!(matches!(non_null[0], SseEvent::Ready { .. }));
        assert!(matches!(non_null[1], SseEvent::Delta(_)));
    }

    #[test]
    fn sse_body_ignore_keepalive_and_done() {
        let body = "event: delta\ndata: {\"o\":\"SET\",\"p\":\"/a\",\"v\":\"x\"}\n\n\
                    : keep-alive comment\n\n\
                    data: [DONE]\n\n";
        let events = parse_sse_frames(body);
        // the [DONE] frame parses to Ok(None); the comment line is not a
        // frame at all; exactly one real event survives.
        assert_eq!(events.len(), 2);
        assert!(events[0].as_ref().unwrap().is_some());
        assert!(events[1].as_ref().unwrap().is_none());
    }
}