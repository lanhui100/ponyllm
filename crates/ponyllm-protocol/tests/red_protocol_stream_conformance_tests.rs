//! RED-PHASE L2-T conformance tests — **streaming** wire surfaces, both protocols.
//!
//! Contract: `.dev-team/contracts/wave-7-protocol-conformance.md` (§1.2 `R-SS`, §1.4 `A-SS`,
//! §2 assertion table, §7 UNVERIFIED register). Every test cites the assertion id(s) it covers in
//! its name and in a leading comment.
//!
//! Level: protocol FSMs (`ChatToResponsesFsm`, `ChatStreamToAnthropicFsm`) driven with a fixed
//! chat-chunk sequence. The SSE `event:` framing and `event name == json type` rules (R-SS-17,
//! R-SS-20, A-SS-13, A-SS-14) are asserted at the HTTP boundary in
//! `red_protocol_http_conformance_tests.rs`.
//!
//! UNVERIFIED-register discipline (§7): `UV-01` forbids asserting a specific `sequence_number`
//! origin, so the assertion is *presence + strict monotonic increase* (as R-SS-04 words it). No
//! test asserts the Responses reasoning-item index (UV-06); items are located by `type`.

use ponyllm_protocol::openai::chat::{
    ChatChunkChoice, ChatChunkDelta, ChatCompletionChunk, FinishReason, PromptTokensDetails, Usage,
};
use ponyllm_protocol::translator::{ChatStreamToAnthropicFsm, ChatToResponsesFsm};
use serde_json::Value;

const ROUTED_MODEL: &str = "workbuddy/deepseek-v4.1-flash";
const ANSWER: &str = "ping";
const REASONING: &str = "The user wants a single word: ping.";

// ---------------------------------------------------------------------------
// assertion helpers
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_path_eq(body: &Value, ptr: &str, expected: Value, id: &str) {
    let actual = body.pointer(ptr);
    assert_eq!(
        actual,
        Some(&expected),
        "{id}: wire path `{ptr}` must equal {expected}; full event: {body}"
    );
}

#[track_caller]
fn assert_path_absent(body: &Value, ptr: &str, id: &str) {
    assert!(
        body.pointer(ptr).is_none(),
        "{id}: wire path `{ptr}` MUST be absent; full event: {body}"
    );
}

#[track_caller]
fn assert_int_ge(body: &Value, ptr: &str, min: i64, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_i64);
    assert!(
        actual.is_some_and(|v| v >= min),
        "{id}: wire path `{ptr}` must be an integer >= {min}; got {:?}; full event: {body}",
        body.pointer(ptr)
    );
}

fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            e.pointer("/type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every stream event must carry a string `type`: {e}"))
                .to_string()
        })
        .collect()
}

#[track_caller]
fn assert_event_sequence(events: &[Value], expected: &[&str], id: &str) {
    let actual = event_types(events);
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(
        actual, expected,
        "{id}: stream event sequence mismatch; full events: {events:#?}"
    );
}

/// Index of the first event whose `type` equals `ty`.
#[track_caller]
fn index_of(events: &[Value], ty: &str) -> usize {
    events
        .iter()
        .position(|e| e.pointer("/type").and_then(Value::as_str) == Some(ty))
        .unwrap_or_else(|| panic!("no event with type `{ty}` in stream: {events:#?}"))
}

// ---------------------------------------------------------------------------
// fixtures — a fixed chat-chunk sequence: role, reasoning, answer, terminal.
// ---------------------------------------------------------------------------

fn usage(prompt: u32, completion: u32) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: Some(0),
            audio_tokens: None,
        }),
        completion_tokens_details: None,
    }
}

fn chunk(
    delta: ChatChunkDelta,
    finish: Option<FinishReason>,
    usage: Option<Usage>,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chatcmpl-9f961259c79311f1".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 1_760_417_629,
        model: ROUTED_MODEL.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta,
            finish_reason: finish,
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    }
}

/// role -> reasoning -> answer -> finish(stop, usage). No zero-length delta.
fn reasoning_then_text_turn() -> Vec<ChatCompletionChunk> {
    vec![
        chunk(
            ChatChunkDelta {
                role: Some("assistant".to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                reasoning_content: Some(REASONING.to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(ANSWER.to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta::default(),
            Some(FinishReason::Stop),
            Some(usage(42, 34)),
        ),
    ]
}

/// Same turn, but the upstream interleaves a **zero-length** content delta — the exact shape the
/// live gateway produced on 2026-10-14 and the trigger for the observed per-chunk block churn.
fn turn_with_zero_length_delta() -> Vec<ChatCompletionChunk> {
    vec![
        chunk(
            ChatChunkDelta {
                role: Some("assistant".to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                reasoning_content: Some(REASONING.to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(String::new()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(ANSWER.to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta::default(),
            Some(FinishReason::Stop),
            Some(usage(42, 34)),
        ),
    ]
}

/// The live 2026-10-14 production shape: the upstream alternates reasoning deltas with
/// **zero-length** content deltas across many chunks. This is what turned a two-word answer into
/// 245 `content_block_start` frames.
fn turn_with_interleaved_reasoning_and_empty_content() -> Vec<ChatCompletionChunk> {
    vec![
        chunk(
            ChatChunkDelta {
                role: Some("assistant".to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                reasoning_content: Some("The user".to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(String::new()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                reasoning_content: Some(" wants a single word.".to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(String::new()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta {
                content: Some(ANSWER.to_string()),
                ..Default::default()
            },
            None,
            None,
        ),
        chunk(
            ChatChunkDelta::default(),
            Some(FinishReason::Stop),
            Some(usage(42, 34)),
        ),
    ]
}

/// The OpenAI `stream_options.include_usage` shape: the terminal usage arrives in a **trailing
/// usage-only chunk** (empty `choices`) after the chunk that carried `finish_reason`.
fn turn_with_trailing_usage_chunk() -> Vec<ChatCompletionChunk> {
    let mut chunks = reasoning_then_text_turn();
    let terminal = chunks.pop().expect("fixture has a terminal chunk");
    let mut finish_without_usage = terminal.clone();
    finish_without_usage.usage = None;
    chunks.push(finish_without_usage);
    chunks.push(ChatCompletionChunk {
        choices: vec![],
        usage: Some(usage(42, 34)),
        ..terminal
    });
    chunks
}

fn drive_responses(chunks: &[ChatCompletionChunk]) -> Vec<Value> {
    let mut fsm = ChatToResponsesFsm::new(ROUTED_MODEL);
    let mut out = Vec::new();
    for c in chunks {
        for e in fsm.process_chunk(c.clone()).unwrap() {
            out.push(serde_json::to_value(&e).expect("ResponseStreamEvent must serialize"));
        }
    }
    if let Some(e) = fsm.finish_if_open() {
        out.push(serde_json::to_value(&e).expect("ResponseStreamEvent must serialize"));
    }
    out
}

fn drive_anthropic(chunks: &[ChatCompletionChunk]) -> Vec<Value> {
    let mut fsm = ChatStreamToAnthropicFsm::new(ROUTED_MODEL);
    let mut out = Vec::new();
    for c in chunks {
        for e in fsm.process_chunk(c.clone()).unwrap() {
            out.push(serde_json::to_value(&e).expect("MessageStreamEvent must serialize"));
        }
    }
    if let Some(es) = fsm.finish_if_open() {
        for e in es {
            out.push(serde_json::to_value(&e).expect("MessageStreamEvent must serialize"));
        }
    }
    out
}

// ===========================================================================
// R-SS — POST /v1/responses, "stream":true  (contract §1.2)
// ===========================================================================

/// R-SS-01 / R-SS-02 / R-SS-03 / R-SS-12 / R-SS-13 — full normative event sequence for a turn with
/// reasoning, ending at `response.completed` with an `output_text` terminal part.
#[test]
fn r_ss_01_to_13_full_normative_event_sequence() {
    let events = drive_responses(&reasoning_then_text_turn());

    assert_event_sequence(
        &events,
        &[
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.completed",
        ],
        "R-SS-01..R-SS-13",
    );

    assert_path_eq(
        &events[0],
        "/response/status",
        Value::String("in_progress".into()),
        "R-SS-02",
    );
}

/// R-SS-01 / R-SS-02 / R-SS-03 — the opening events specifically: `response.created` first with
/// `status == "in_progress"`, then `response.in_progress`.
#[test]
fn r_ss_01_02_03_opening_events() {
    let events = drive_responses(&reasoning_then_text_turn());

    assert_path_eq(
        &events[0],
        "/type",
        Value::String("response.created".into()),
        "R-SS-01",
    );
    assert_path_eq(
        &events[0],
        "/response/status",
        Value::String("in_progress".into()),
        "R-SS-02",
    );
    assert_path_eq(
        &events[1],
        "/type",
        Value::String("response.in_progress".into()),
        "R-SS-03",
    );
}

/// R-SS-04 — `sequence_number` present on every event and strictly increasing.
#[test]
fn r_ss_04_sequence_number_present_and_strictly_increasing() {
    let events = drive_responses(&reasoning_then_text_turn());

    let numbers: Vec<i64> = events
        .iter()
        .map(|e| {
            e.pointer("/sequence_number")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("R-SS-04: every event must carry an integer `sequence_number`: {e}")
                })
        })
        .collect();
    assert!(
        numbers.windows(2).all(|w| w[1] > w[0]),
        "R-SS-04: `sequence_number` must strictly increase; got {numbers:?}; events: {events:#?}"
    );
}

/// R-SS-05 / R-SS-06 / R-SS-07 / R-SS-08 / R-SS-09 / R-SS-10 — the assistant message item's
/// content-part lifecycle, in order, with the required fields.
#[test]
fn r_ss_05_to_10_message_item_and_content_part_lifecycle() {
    let events = drive_responses(&reasoning_then_text_turn());
    let types = event_types(&events);

    let item_added = index_of(&events, "response.output_item.added");
    let part_added = index_of(&events, "response.content_part.added");
    let text_done = index_of(&events, "response.output_text.done");
    let part_done = index_of(&events, "response.content_part.done");
    let item_done_last = types
        .iter()
        .rposition(|t| t == "response.output_item.done")
        .unwrap_or_else(|| panic!("R-SS-10: no `response.output_item.done` event: {events:#?}"));

    assert!(
        item_added < part_added && part_added < text_done && text_done < part_done && part_done < item_done_last,
        "R-SS-05..R-SS-10: lifecycle order must be item.added < content_part.added < output_text.done < content_part.done < item.done; got {types:?}"
    );

    assert_path_eq(
        &events[part_added],
        "/part/type",
        Value::String("output_text".into()),
        "R-SS-06",
    );
    assert_int_ge(&events[item_added], "/output_index", 0, "R-SS-05");
    assert_int_ge(&events[part_added], "/content_index", 0, "R-SS-06");
    assert_path_eq(
        &events[text_done],
        "/text",
        Value::String(ANSWER.into()),
        "R-SS-08",
    );
    assert_path_eq(
        &events[part_done],
        "/part/type",
        Value::String("output_text".into()),
        "R-SS-09",
    );
    assert_path_eq(
        &events[item_done_last],
        "/item/type",
        Value::String("message".into()),
        "R-SS-10",
    );
}

/// R-SS-07 — `response.output_text.delta` carries `item_id`/`output_index`/`content_index`/`delta`/
/// `logprobs` and MUST NOT carry a `response_id` field.
#[test]
fn r_ss_07_output_text_delta_fields() {
    let events = drive_responses(&reasoning_then_text_turn());
    let idx = index_of(&events, "response.output_text.delta");
    let delta = &events[idx];

    assert_path_eq(
        delta,
        "/delta",
        Value::String(ANSWER.into()),
        "R-SS-07",
    );
    assert_int_ge(delta, "/output_index", 0, "R-SS-07");
    assert_int_ge(delta, "/content_index", 0, "R-SS-07");
    assert_path_eq(delta, "/logprobs", Value::Array(vec![]), "R-SS-07");
    assert_path_absent(delta, "/response_id", "R-SS-07");
    assert!(
        delta
            .pointer("/item_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "R-SS-07: `/item_id` must be a non-empty string; got {:?}; event: {delta}",
        delta.pointer("/item_id")
    );
}

/// R-SS-11 — the reasoning item lifecycle: `output_item.added(item.type=="reasoning")`, then
/// summary part/text delta/done, then `output_item.done`.
#[test]
fn r_ss_11_reasoning_summary_lifecycle() {
    let events = drive_responses(&reasoning_then_text_turn());
    let types = event_types(&events);

    let reasoning_events: Vec<String> = types
        .iter()
        .filter(|t| {
            t.contains("reasoning")
                || t.as_str() == "response.output_item.added"
                || t.as_str() == "response.output_item.done"
        })
        .cloned()
        .collect();

    let item_added = index_of(&events, "response.output_item.added");
    assert_path_eq(
        &events[item_added],
        "/item/type",
        Value::String("reasoning".into()),
        "R-SS-11",
    );

    assert_event_sequence(
        &events,
        &[
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.completed",
        ],
        "R-SS-11",
    );

    let delta_idx = index_of(&events, "response.reasoning_summary_text.delta");
    assert_path_eq(
        &events[delta_idx],
        "/delta",
        Value::String(REASONING.into()),
        "R-SS-11",
    );
    assert!(
        reasoning_events
            .iter()
            .any(|t| t.as_str() == "response.reasoning_summary_text.delta"),
        "R-SS-11: reasoning summary delta missing; got {reasoning_events:?}"
    );
}

/// R-SS-12 / R-SS-13 — terminal event is `response.completed`, nothing after it, and its output
/// content part is `output_text`.
#[test]
fn r_ss_12_13_terminal_completed_carries_output_text() {
    let events = drive_responses(&reasoning_then_text_turn());
    let last = events.last().expect("stream must not be empty");

    assert_path_eq(
        last,
        "/type",
        Value::String("response.completed".into()),
        "R-SS-12",
    );
    assert_path_eq(
        last,
        "/response/status",
        Value::String("completed".into()),
        "R-SS-12",
    );

    let output = last
        .pointer("/response/output")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("R-SS-12: terminal `/response/output` must be an array: {last}"));
    let message = output
        .iter()
        .find(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        .unwrap_or_else(|| panic!("R-SS-12: terminal output must contain a message item: {last}"));
    assert_path_eq(
        message,
        "/content/0/type",
        Value::String("output_text".into()),
        "R-SS-13",
    );
    assert_path_eq(
        message,
        "/content/0/text",
        Value::String(ANSWER.into()),
        "R-SS-13",
    );
}

/// R-SS-14 / R-SS-15 — the terminal event's `usage` carries real counts and both `*_tokens_details`
/// sub-objects.
#[test]
fn r_ss_14_15_terminal_usage_is_real_and_detailed() {
    let events = drive_responses(&reasoning_then_text_turn());
    let last = events.last().expect("stream must not be empty");

    assert_path_eq(
        last,
        "/response/usage/input_tokens",
        Value::from(42u64),
        "R-SS-14",
    );
    assert_path_eq(
        last,
        "/response/usage/output_tokens",
        Value::from(34u64),
        "R-SS-14",
    );
    assert_path_eq(
        last,
        "/response/usage/total_tokens",
        Value::from(76u64),
        "R-SS-14",
    );
    assert_int_ge(
        last,
        "/response/usage/input_tokens_details/cached_tokens",
        0,
        "R-SS-15",
    );
    assert_int_ge(
        last,
        "/response/usage/output_tokens_details/reasoning_tokens",
        0,
        "R-SS-15",
    );
}

/// R-SS-18 / R-SS-19 — `response.completed.response.id == response.created.response.id`, the id is
/// `resp_`-prefixed, and item ids are stable from `output_item.added` through the terminal output.
#[test]
fn r_ss_18_19_ids_are_stable_and_resp_prefixed() {
    let events = drive_responses(&reasoning_then_text_turn());

    let created_id = events[0]
        .pointer("/response/id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("R-SS-18: `/response/id` must be a string: {}", events[0]));
    assert!(
        created_id.starts_with("resp_"),
        "R-SS-19: response id must be `resp_`-prefixed; got `{created_id}`"
    );

    let last = events.last().expect("stream must not be empty");
    assert_path_eq(
        last,
        "/response/id",
        Value::String(created_id.to_string()),
        "R-SS-18",
    );

    // The item id announced in output_item.added for the message must survive into the terminal
    // output[] entry of the same type.
    let added_item_id = events
        .iter()
        .filter(|e| {
            e.pointer("/type").and_then(Value::as_str) == Some("response.output_item.added")
                && e.pointer("/item/type").and_then(Value::as_str) == Some("message")
        })
        .filter_map(|e| e.pointer("/item/id").and_then(Value::as_str))
        .next()
        .unwrap_or_else(|| {
            panic!("R-SS-19: no `response.output_item.added` for the message item: {events:#?}")
        });
    let terminal_item_id = last
        .pointer("/response/output")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        })
        .and_then(|it| it.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("R-SS-19: terminal message item has no id: {last}"));
    assert_eq!(
        terminal_item_id, added_item_id,
        "R-SS-19: message item id must be stable from output_item.added to the terminal output"
    );
}

// ===========================================================================
// A-SS — POST /v1/messages, "stream":true  (contract §1.4)
// ===========================================================================

/// A-SS-01 / A-SS-02 / A-SS-03 / A-SS-04 / A-SS-05 / A-SS-06 — the `message_start` envelope.
#[test]
fn a_ss_01_to_06_message_start_envelope() {
    let events = drive_anthropic(&reasoning_then_text_turn());

    assert_path_eq(
        &events[0],
        "/type",
        Value::String("message_start".into()),
        "A-SS-01",
    );
    // A-SS-02 (Lead ruling, contract v2): `message_start` is emitted on the FIRST upstream chunk,
    // before any upstream usage exists — a proxy cannot know the real prompt count at that point.
    // This path therefore only requires the key to be PRESENT and an integer >= 0. The real
    // cumulative prompt count is asserted on `message_delta` instead (A-SS-17, below).
    assert_int_ge(&events[0], "/message/usage/input_tokens", 0, "A-SS-02");
    assert_int_ge(&events[0], "/message/usage/output_tokens", 1, "A-SS-03");
    assert_path_eq(&events[0], "/message/stop_reason", Value::Null, "A-SS-04");
    assert_path_eq(&events[0], "/message/stop_sequence", Value::Null, "A-SS-05");

    let id = events[0]
        .pointer("/message/id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("A-SS-06: `/message/id` must be a string: {}", events[0]));
    assert!(
        id.starts_with("msg_") && id.len() > "msg_".len(),
        "A-SS-06: `message_start.message.id` must match ^msg_; got `{id}`"
    );
}

/// A-SS-01 / A-SS-07 / A-SS-09 / A-SS-12 — full normative Anthropic event sequence: one
/// `content_block_start`/`content_block_stop` per block, a single `signature_delta` before the
/// thinking block closes, and `message_stop` last.
#[test]
fn a_ss_01_07_09_12_full_normative_event_sequence() {
    let events = drive_anthropic(&reasoning_then_text_turn());

    assert_event_sequence(
        &events,
        &[
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ],
        "A-SS-01/A-SS-07/A-SS-09/A-SS-12",
    );

    let last = events.last().expect("stream must not be empty");
    assert_path_eq(
        last,
        "/type",
        Value::String("message_stop".into()),
        "A-SS-12",
    );
}

/// A-SS-07 — exactly one `content_block_start`/`content_block_stop` per content block, with `index`
/// equal to the block's position in the final `content` array. Driven with the interleaved
/// reasoning/zero-length-delta shape, so a per-token block churn cannot pass.
#[test]
fn a_ss_07_one_start_and_stop_per_content_block() {
    let events = drive_anthropic(&turn_with_interleaved_reasoning_and_empty_content());

    let starts: Vec<i64> = events
        .iter()
        .filter(|e| e.pointer("/type").and_then(Value::as_str) == Some("content_block_start"))
        .filter_map(|e| e.pointer("/index").and_then(Value::as_i64))
        .collect();
    let stops: Vec<i64> = events
        .iter()
        .filter(|e| e.pointer("/type").and_then(Value::as_str) == Some("content_block_stop"))
        .filter_map(|e| e.pointer("/index").and_then(Value::as_i64))
        .collect();

    assert_eq!(
        starts,
        vec![0, 1],
        "A-SS-07: a thinking block + a text block must yield exactly two content_block_start events \
         with indices [0, 1]; got {starts:?}; events: {events:#?}"
    );
    assert_eq!(
        stops,
        vec![0, 1],
        "A-SS-07: exactly one content_block_stop per block with matching indices; got {stops:?}; \
         events: {events:#?}"
    );

    // Every delta must belong to one of those two blocks.
    let delta_indices: Vec<i64> = events
        .iter()
        .filter(|e| e.pointer("/type").and_then(Value::as_str) == Some("content_block_delta"))
        .filter_map(|e| e.pointer("/index").and_then(Value::as_i64))
        .collect();
    assert!(
        delta_indices.iter().all(|i| *i == 0 || *i == 1),
        "A-SS-07: every delta must carry the index of its block (0 or 1); got {delta_indices:?}"
    );
}

/// A-SS-08 — `content_block_start.content_block` for a thinking block is exactly
/// `{"type":"thinking","thinking":"","signature":""}`.
#[test]
fn a_ss_08_thinking_block_start_shape() {
    let events = drive_anthropic(&reasoning_then_text_turn());

    let idx = events
        .iter()
        .position(|e| {
            e.pointer("/type").and_then(Value::as_str) == Some("content_block_start")
                && e.pointer("/content_block/type").and_then(Value::as_str) == Some("thinking")
        })
        .unwrap_or_else(|| panic!("A-SS-08: no thinking content_block_start: {events:#?}"));

    assert_path_eq(
        &events[idx],
        "/content_block",
        serde_json::json!({"type": "thinking", "thinking": "", "signature": ""}),
        "A-SS-08",
    );
}

/// A-SS-09 — a `signature_delta` is emitted exactly once, immediately before the thinking block's
/// `content_block_stop`.
#[test]
fn a_ss_09_signature_delta_once_before_thinking_block_stop() {
    let events = drive_anthropic(&reasoning_then_text_turn());

    let signature_indices: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.pointer("/type").and_then(Value::as_str) == Some("content_block_delta")
                && e.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
        })
        .map(|(i, _)| i)
        .collect();

    assert_eq!(
        signature_indices.len(),
        1,
        "A-SS-09: exactly one signature_delta must be emitted; got {signature_indices:?}; events: {events:#?}"
    );
    let sig = signature_indices[0];
    assert_eq!(
        events[sig].pointer("/index").and_then(Value::as_i64),
        Some(0),
        "A-SS-09: signature_delta must belong to the thinking block (index 0); event: {}",
        events[sig]
    );
    let next = &events[sig + 1];
    assert_eq!(
        next.pointer("/type").and_then(Value::as_str),
        Some("content_block_stop"),
        "A-SS-09: signature_delta must be immediately followed by the thinking block's content_block_stop; got {next}"
    );
    assert_eq!(
        next.pointer("/index").and_then(Value::as_i64),
        Some(0),
        "A-SS-09: the closing content_block_stop must be the thinking block; got {next}"
    );
}

/// A-SS-10 — `message_delta.delta.stop_sequence` key is present and `null`.
#[test]
fn a_ss_10_message_delta_stop_sequence_present_null() {
    let events = drive_anthropic(&reasoning_then_text_turn());
    let idx = index_of(&events, "message_delta");

    assert_path_eq(
        &events[idx],
        "/delta/stop_sequence",
        Value::Null,
        "A-SS-10",
    );
    assert_path_eq(
        &events[idx],
        "/delta/stop_reason",
        Value::String("end_turn".into()),
        "A-SS-10",
    );
}

/// A-SS-11 / A-SS-15 / A-SS-17 — `message_delta.usage` carries the **real cumulative** counts: the
/// prompt count (`input_tokens`, A-SS-17) and the output count (`output_tokens`, A-SS-11). Driven
/// with the `include_usage` shape (usage in a trailing usage-only chunk) because that is the only
/// way to distinguish real counts from hardcoded zeros. Absorbing that trailing chunk must also not
/// surface it as content (A-SS-15 / A-SS-17).
///
/// The point-7 assertions (no extra content block, no empty/duplicate text delta for the trailing
/// usage-only chunk) live in this test rather than a standalone one: on current main the FSM already
/// early-returns on the trailing chunk, so a standalone test of that property alone could not go
/// red and would not be a valid red-phase test.
#[test]
fn a_ss_11_17_message_delta_usage_is_real_cumulative() {
    let events = drive_anthropic(&turn_with_trailing_usage_chunk());
    let idx = index_of(&events, "message_delta");

    // Fixture supplies a known prompt count (42), so exact equality is asserted.
    assert_path_eq(
        &events[idx],
        "/usage/input_tokens",
        Value::from(42u64),
        "A-SS-17",
    );
    assert_int_ge(&events[idx], "/usage/input_tokens", 1, "A-SS-17");
    assert_path_eq(
        &events[idx],
        "/usage/output_tokens",
        Value::from(34u64),
        "A-SS-11",
    );
    assert_int_ge(&events[idx], "/usage/output_tokens", 1, "A-SS-11");

    let starts: Vec<i64> = events
        .iter()
        .filter(|e| e.pointer("/type").and_then(Value::as_str) == Some("content_block_start"))
        .filter_map(|e| e.pointer("/index").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        starts,
        vec![0, 1],
        "A-SS-17: the trailing usage-only chunk must not open an extra content block; expected the \
         thinking + text blocks [0, 1]; got {starts:?}; events: {events:#?}"
    );

    let text_deltas: Vec<&str> = events
        .iter()
        .filter(|e| {
            e.pointer("/type").and_then(Value::as_str) == Some("content_block_delta")
                && e.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
        })
        .filter_map(|e| e.pointer("/delta/text").and_then(Value::as_str))
        .collect();
    assert_eq!(
        text_deltas,
        vec![ANSWER],
        "A-SS-15/A-SS-17: the trailing usage-only chunk must not add an empty or duplicate text \
         delta; got {text_deltas:?}; events: {events:#?}"
    );
}

/// A-SS-15 — a `text_delta` is never emitted with empty text.
#[test]
fn a_ss_15_no_empty_text_deltas() {
    let events = drive_anthropic(&turn_with_zero_length_delta());

    let empty: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e.pointer("/type").and_then(Value::as_str) == Some("content_block_delta")
                && e.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && e.pointer("/delta/text").and_then(Value::as_str) == Some("")
        })
        .collect();

    assert!(
        empty.is_empty(),
        "A-SS-15: zero-length text_delta frames MUST NOT be emitted; got {} of them; events: {events:#?}",
        empty.len()
    );
}
