//! SSE event parsing for `POST /api/v0/chat/completion` (wave-9 contract
//! DW-T-*, H-09). Evidence: REA §5. Translation into OpenAI chunks is a
//! task-4 server-side concern (contract §5); this module yields typed events
//! from raw SSE frames.
//!
//! Event names (REA §5): `ready`, `delta`, `finish`, `close`, plus non-fatal
//! `hint`/`toast`/`title`/`update_session` (real wire) / `updateSession`.
//! Content frames on the real wire carry **no `event:` line at all** — an
//! empty event name with delta-shaped JSON parses to `SseEvent::Delta`, never
//! dropped (downstream call 2026-10-22). `delta` is a `{p, o, v}` frame with
//! `o ∈ {APPEND, SET, BATCH}`; a **missing `o` is an implicit APPEND**
//! (real-wire frames `{"v":","}`, `{"v":" the"}` — Amendment v4 / REA §5),
//! and an object `v` under a missing `o` is a full-state snapshot
//! (`{"v":{"response":{…fragments:[{type:"THINK"|"TEXT",content:…}]}}}`) kept
//! as `Append { value }` for the translator's snapshot extraction; a BATCH's
//! `v` is an op array applied recursively, in order (DW-T-03).

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
/// * Non-event frames — empty data, `[DONE]` sentinels, comment-only lines —
///   yield `Ok(None)` (DW-T-08).
/// * **Empty event name** (`name.is_empty()`, the `parse_sse_frames` path) and
///   **`"message"`** (the SSE-spec default event name — the server-side frame
///   reader assigns it to frames without an `event:` line, streaming.rs:222):
///   the real wire (downstream call 2026-10-22) sends ALL content frames this
///   way. Such frames are delta-shaped (`{"p":…,"o":…,"v":…}`, implicit-append
///   `{"v":"…"}`, full snapshot `{"v":{"response":{…fragments…}}}`) and MUST
///   NOT be dropped — they parse to `SseEvent::Delta`. Only non-JSON or
///   non-delta-shaped data is a non-event frame (`Ok(None)`).
/// * Unknown event names yield `Ok(None)` so forward-compatible servers do
///   not break the stream (DW-T-09; the translator logs the raw frame at
///   trace level).
///
/// Pure function, zero I/O.
pub fn parse_completion_event(name: &str, data: &str) -> Result<Option<SseEvent>, SseError> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    if name.is_empty() || name == "message" {
        return parse_delta_frame_data(trimmed);
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| SseError::Malformed(format!("{name}: {e}")))?;
    match name {
        "ready" => {
            // Real wire sends NUMERIC ids (`{"request_message_id":1,
            // "response_message_id":2,"model_type":"default"}` — real-link
            // smoke 2026-10-22), sometimes strings; both normalize to String.
            // Missing key / non-string-non-number still errors (never
            // silently fabricate an id).
            let need = |key: &str| {
                value
                    .get(key)
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| value.get(key).and_then(|v| v.as_i64().map(|n| n.to_string())))
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
        // Real wire uses `event: update_session` (downstream call
        // 2026-10-22); the contract name `updateSession` stays accepted.
        "updateSession" | "update_session" => Ok(Some(SseEvent::UpdateSession(value))),
        _ => Ok(None),
    }
}

/// Interpret a frame payload as a real-wire delta content frame.
///
/// Shared by the empty-name and `"message"` (SSE default event name) paths —
/// both denote frames that arrived without an `event:` line. Returns:
/// * `Ok(Some(SseEvent::Delta(..)))` for JSON data carrying any of `o`/`p`/`v`
///   (implicit-append `{"v":"…"}`, `APPEND`, `SET`, `BATCH`, full snapshot
///   `{"v":{"response":{…fragments…}}}` — all routed through `parse_delta`);
/// * `Ok(None)` for empty/`[DONE]` data, non-JSON data, or JSON without
///   `o`/`p`/`v`.
fn parse_delta_frame_data(data: &str) -> Result<Option<SseEvent>, SseError> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Ok(None);
    };
    if value.get("o").is_some() || value.get("p").is_some() || value.get("v").is_some() {
        return Ok(Some(SseEvent::Delta(parse_delta(&value)?)));
    }
    Ok(None)
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
    fn ready_numeric_ids_normalized_to_string() {
        // real-link smoke frame (2026-10-22): ids are numbers
        let ev = parse_completion_event(
            "ready",
            r#"{"request_message_id":1,"response_message_id":2,"model_type":"default"}"#,
        )
        .unwrap()
        .expect("numeric-id ready parses");
        assert_eq!(
            ev,
            SseEvent::Ready {
                response_message_id: "2".to_string(),
                request_message_id: "1".to_string(),
                model_type: "default".to_string(),
            }
        );
        // mixed string/number normalized uniformly
        let ev = parse_completion_event(
            "ready",
            r#"{"request_message_id":"rq-9","response_message_id":7,"model_type":"deepseek-chat"}"#,
        )
        .unwrap()
        .expect("mixed ready parses");
        assert_eq!(
            ev,
            SseEvent::Ready {
                response_message_id: "7".to_string(),
                request_message_id: "rq-9".to_string(),
                model_type: "deepseek-chat".to_string(),
            }
        );
    }

    #[test]
    fn ready_missing_key_or_unacceptable_type_errors() {
        assert!(matches!(
            parse_completion_event("ready", r#"{"response_message_id":2,"model_type":"m"}"#),
            Err(SseError::Other(ref m)) if m == "ready event missing request_message_id"
        ));
        // booleans/null are not acceptable id forms
        assert!(matches!(
            parse_completion_event(
                "ready",
                r#"{"request_message_id":true,"response_message_id":2,"model_type":"m"}"#
            ),
            Err(SseError::Other(_))
        ));
    }

    #[test]
    fn close_reason_is_string() {
        // real wire: close_reason stays a string (no normalization needed)
        let c = parse_completion_event("close", r#"{"close_reason":"success"}"#)
            .unwrap()
            .expect("close parses");
        assert_eq!(c, SseEvent::Close { close_reason: Some("success".to_string()) });
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
    fn empty_event_name_delta_frames_parse() {
        // Real wire (downstream call 2026-10-22): content frames carry NO
        // `event:` line. All four real shapes must parse to SseEvent::Delta.
        let ev = parse_completion_event(
            "",
            r#"{"p":"response/fragments/-1/content","o":"APPEND","v":" need"}"#,
        )
        .unwrap()
        .expect("unnamed APPEND parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: Some("response/fragments/-1/content".to_string()),
                value: json!(" need"),
            })
        );

        let ev = parse_completion_event("", r#"{"v":" answer"}"#)
            .unwrap()
            .expect("unnamed implicit append parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append { path: None, value: json!(" answer") })
        );

        let ev = parse_completion_event("", r#"{"p":"response/status","o":"SET","v":"FINISHED"}"#)
            .unwrap()
            .expect("unnamed SET parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Set {
                path: "response/status".to_string(),
                value: json!("FINISHED"),
            })
        );
    }

    #[test]
    fn empty_event_name_snapshot_frame_parses_as_append_object() {
        // Full-state snapshot `{"v":{"response":{...fragments...}}}` — missing
        // `o`, object `v`. Parsed as `Append { value: <object> }` so the
        // translator's snapshot extraction (fragments[].type/content,
        // THINK→reasoning / TEXT→content) can act on it.
        let ev = parse_completion_event(
            "",
            r#"{"v":{"response":{"fragments":[{"type":"THINK","content":"hmm"},{"type":"RESPONSE","content":"Hi"}]}}}"#,
        )
        .unwrap()
        .expect("unnamed snapshot parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: None,
                value: json!({"response": {"fragments": [
                    {"type":"THINK","content":"hmm"},
                    {"type":"RESPONSE","content":"Hi"}
                ]}}),
            })
        );
    }

    #[test]
    fn message_default_event_name_parses_delta_frames() {
        // SSE-spec default event name `"message"`: the server-side frame
        // reader (streaming.rs:222) assigns it to frames with no `event:`
        // line, so content frames arrive as `("message", data)`. These MUST
        // parse as deltas — this is the final content gate.
        let ev = parse_completion_event(
            "message",
            r#"{"p":"response/fragments/-1/content","o":"APPEND","v":" need"}"#,
        )
        .unwrap()
        .expect("message APPEND parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: Some("response/fragments/-1/content".to_string()),
                value: json!(" need"),
            })
        );

        let ev = parse_completion_event(
            "message",
            r#"{"v":{"response":{"fragments":[{"type":"THINK","content":"hmm"},{"type":"RESPONSE","content":"Hi"}]}}}"#,
        )
        .unwrap()
        .expect("message snapshot parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append {
                path: None,
                value: json!({"response": {"fragments": [
                    {"type":"THINK","content":"hmm"},
                    {"type":"RESPONSE","content":"Hi"}
                ]}}),
            })
        );

        // implicit append inside message-default frames
        let ev = parse_completion_event("message", r#"{"v":" answer"}"#)
            .unwrap()
            .expect("message implicit append parses");
        assert_eq!(
            ev,
            SseEvent::Delta(DeltaOp::Append { path: None, value: json!(" answer") })
        );

        // non-JSON / non-delta data under "message" is not an event
        assert_eq!(parse_completion_event("message", "not-json").unwrap(), None);
        assert_eq!(parse_completion_event("message", r#"{"foo":"bar"}"#).unwrap(), None);
        assert_eq!(parse_completion_event("message", "[DONE]").unwrap(), None);
    }

    #[test]
    fn empty_event_name_rejects_non_delta_frames() {
        // non-JSON data under an empty name is NOT a completion event
        assert_eq!(parse_completion_event("", "not-json").unwrap(), None);
        // JSON without o/p/v is not a delta frame either
        assert_eq!(parse_completion_event("", r#"{"foo":"bar"}"#).unwrap(), None);
        assert_eq!(parse_completion_event("", r#"{}"#).unwrap(), None);
    }

    #[test]
    fn update_session_accepts_snake_and_camel_names() {
        // real wire: `event: update_session` (downstream call 2026-10-22)
        let ev = parse_completion_event("update_session", r#"{"updated_at":"2026-10-22T00:00:00Z"}"#)
            .unwrap()
            .expect("snake update_session parses");
        assert!(matches!(ev, SseEvent::UpdateSession(_)));
        let ev = parse_completion_event("updateSession", r#"{"updated_at":"x"}"#)
            .unwrap()
            .expect("camel updateSession parses");
        assert!(matches!(ev, SseEvent::UpdateSession(_)));
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