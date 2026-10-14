//! RED-PHASE L2-T conformance tests — **non-stream** wire surfaces, both protocols.
//!
//! Contract: `.dev-team/contracts/wave-7-protocol-conformance.md` (§1.1 `R-NS`, §1.3 `A-NS`,
//! §2 assertion table, §7 UNVERIFIED register). Every test cites the assertion id(s) it covers in
//! its name and in a leading comment.
//!
//! These tests are written to **fail on current main** and to pass only once the frozen contract is
//! implemented. Assertions are exact JSON-path equality against the serialized wire body — never
//! "contains a key" and never "did not return an error".
//!
//! Test granularity note: a test is scoped to one *coherent contract unit* (the success envelope,
//! the assistant message item, the reasoning item, the usage object, …) and asserts every MUST
//! path of that unit. Where a MUST path already holds on current main it is asserted inside the
//! unit test rather than split into a test that could not go red — a green test is not a valid
//! red-phase test, and a unit test only reports success when the whole unit conforms.
//!
//! UNVERIFIED-register discipline (§7): `UV-06` says Responses reasoning-item ordering is not
//! normative, so no test here asserts `output[0]`; items are located by their `type` and the exact
//! path is then asserted on that index. `UV-01`..`UV-05` are not asserted beyond what an assertion
//! id explicitly requires.

use ponyllm_protocol::anthropic::messages::{AnthropicStopReason, AnthropicUsage, MessageResponse};
use ponyllm_protocol::openai::chat::{
    AssistantResponseChoiceMessage, ChatChoice, ChatCompletionResponse, CompletionTokensDetails,
    FinishReason, PromptTokensDetails, Usage,
};
use ponyllm_protocol::openai::responses::{
    ResponseContentPart, ResponseObject, ResponseOutputItem, ResponseUsage,
};
use ponyllm_protocol::translator::{
    anthropic_to_responses_response, chat_to_anthropic_response, chat_to_responses_response,
    responses_to_anthropic_response,
};
use serde_json::Value;

const ROUTED_MODEL: &str = "workbuddy/deepseek-v4.1-flash";
const ANSWER: &str = "ping";
const REASONING: &str = "The user wants a single word: ping.";

// ---------------------------------------------------------------------------
// assertion helpers — exact JSON-path equality, loud on failure
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_path_eq(body: &Value, ptr: &str, expected: Value, id: &str) {
    let actual = body.pointer(ptr);
    assert_eq!(
        actual,
        Some(&expected),
        "{id}: wire body path `{ptr}` must equal {expected}; full body: {body}"
    );
}

#[track_caller]
fn assert_int_ge(body: &Value, ptr: &str, min: i64, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_i64);
    assert!(
        actual.is_some_and(|v| v >= min),
        "{id}: wire body path `{ptr}` must be an integer >= {min}; got {:?}; full body: {body}",
        body.pointer(ptr)
    );
}

#[track_caller]
fn assert_string_matching(body: &Value, ptr: &str, prefix: &str, min_tail: usize, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_str);
    let ok = actual.is_some_and(|s| s.starts_with(prefix) && s.len() >= prefix.len() + min_tail);
    assert!(
        ok,
        "{id}: wire body path `{ptr}` must be a string starting with `{prefix}` and >= {} chars total; got {actual:?}; full body: {body}",
        prefix.len() + min_tail
    );
}

/// Index of the first `output[]` item whose `type` equals `ty`.
///
/// Deliberately does not assume ordering (contract §7 UV-06).
#[track_caller]
fn output_index_of(body: &Value, ty: &str) -> usize {
    body.pointer("/output")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .position(|it| it.get("type").and_then(Value::as_str) == Some(ty))
        })
        .unwrap_or_else(|| panic!("no `output[]` item with type `{ty}` in body: {body}"))
}

/// Every `output[message].content[].type` value present in the body.
fn message_content_part_types(body: &Value) -> Vec<String> {
    body.pointer("/output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|it| it.get("type").and_then(Value::as_str) == Some("message"))
                .filter_map(|it| it.get("content").and_then(Value::as_array))
                .flatten()
                .filter_map(|part| part.get("type").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn usage(prompt: u32, completion: u32, cached: u32, reasoning_tokens: u32) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: Some(cached),
            audio_tokens: None,
        }),
        completion_tokens_details: Some(CompletionTokensDetails {
            reasoning_tokens: Some(reasoning_tokens),
            audio_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
        }),
    }
}

fn chat_response(
    text: Option<&str>,
    reasoning: Option<&str>,
    finish: FinishReason,
    usage: Option<Usage>,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "chatcmpl-9f961259c79311f1".to_string(),
        object: "chat.completion".to_string(),
        created: 1_760_417_629,
        model: ROUTED_MODEL.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: AssistantResponseChoiceMessage {
                role: "assistant".to_string(),
                content: text.map(str::to_string),
                reasoning_content: reasoning.map(str::to_string),
                refusal: None,
                tool_calls: None,
            },
            finish_reason: Some(finish),
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    }
}

/// A chat turn with both reasoning and an answer, plus full usage detail.
fn chat_turn_with_reasoning() -> ChatCompletionResponse {
    chat_response(
        Some(ANSWER),
        Some(REASONING),
        FinishReason::Stop,
        Some(usage(42, 34, 0, 20)),
    )
}

/// A chat turn with an answer only (no reasoning) — used for unambiguous `output[0]` paths.
fn chat_turn_text_only() -> ChatCompletionResponse {
    chat_response(Some(ANSWER), None, FinishReason::Stop, Some(usage(42, 34, 0, 0)))
}

fn anthropic_turn_with_thinking() -> MessageResponse {
    MessageResponse {
        id: "msg_a1937b91c79311f1af88da1fc94e0a20".to_string(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![
            ponyllm_protocol::anthropic::messages::AnthropicContentBlock::Thinking {
                thinking: REASONING.to_string(),
                signature: Some("sig-upstream".to_string()),
            },
            ponyllm_protocol::anthropic::messages::AnthropicContentBlock::Text {
                text: ANSWER.to_string(),
                cache_control: None,
            },
        ],
        model: ROUTED_MODEL.to_string(),
        stop_reason: Some(AnthropicStopReason::EndTurn),
        stop_sequence: None,
        usage: AnthropicUsage {
            input_tokens: 42,
            output_tokens: 36,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
        },
    }
}

/// Fixture for the cross-translation (Anthropic upstream -> Responses) path.
///
/// `..Default::default()` on `ResponseObject` / `ResponseUsage` is deliberate (Lead ruling: real
/// field values win over compile convenience): the wave-7 executor will ADD the contract-required
/// fields (`created_at`, `completed_at`, the nullable set, `input_tokens_details`,
/// `output_tokens_details`). Those additions must not break this frozen fixture, and every field
/// listed explicitly here stays explicit.
fn responses_object_text_only() -> ResponseObject {
    ResponseObject {
        id: "resp_9f961259c79311f18ee2d21a2ae33f18".to_string(),
        object: "response".to_string(),
        status: "completed".to_string(),
        model: ROUTED_MODEL.to_string(),
        output: vec![ResponseOutputItem::Message {
            id: "msg_9f961259c79311f18ee2d21a2ae33f18".to_string(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![ResponseContentPart::Text {
                text: ANSWER.to_string(),
            }],
        }],
        usage: Some(ResponseUsage {
            total_tokens: 76,
            input_tokens: 42,
            output_tokens: 34,
            ..Default::default()
        }),
        error: None,
        ..Default::default()
    }
}

fn responses_body(turn: &ChatCompletionResponse) -> Value {
    serde_json::to_value(chat_to_responses_response(turn).unwrap())
        .expect("ResponseObject must serialize")
}

fn anthropic_body(turn: &ChatCompletionResponse) -> Value {
    serde_json::to_value(chat_to_anthropic_response(turn).unwrap())
        .expect("MessageResponse must serialize")
}

// ===========================================================================
// R-NS — POST /v1/responses, stream absent/false  (contract §1.1)
// ===========================================================================

/// R-NS-01 / R-NS-02 / R-NS-03 — response identity: `resp_` id, `object`, `created_at`.
#[test]
fn r_ns_01_02_03_response_identity_and_created_at() {
    let body = responses_body(&chat_turn_text_only());

    assert_string_matching(&body, "/id", "resp_", 8, "R-NS-01");
    assert_path_eq(&body, "/object", Value::String("response".into()), "R-NS-02");
    assert_int_ge(&body, "/created_at", 1, "R-NS-03");
}

/// R-NS-04 / R-NS-05 / R-NS-06 / R-NS-07 / R-NS-08 / R-NS-09 / R-NS-10 / R-NS-11 —
/// the success envelope plus the assistant message item, whose content must be `output_text`.
#[test]
fn r_ns_04_to_11_status_model_and_assistant_message_item() {
    let body = responses_body(&chat_turn_text_only());

    assert_path_eq(&body, "/status", Value::String("completed".into()), "R-NS-04");
    assert_path_eq(&body, "/model", Value::String(ROUTED_MODEL.into()), "R-NS-05");
    assert!(
        body.pointer("/output").is_some_and(Value::is_array),
        "R-NS-06: `/output` must be an array; body: {body}"
    );

    let idx = output_index_of(&body, "message");
    assert_path_eq(
        &body,
        &format!("/output/{idx}/type"),
        Value::String("message".into()),
        "R-NS-07",
    );
    assert_string_matching(&body, &format!("/output/{idx}/id"), "", 1, "R-NS-08");
    assert_path_eq(
        &body,
        &format!("/output/{idx}/status"),
        Value::String("completed".into()),
        "R-NS-09",
    );
    assert_path_eq(
        &body,
        &format!("/output/{idx}/role"),
        Value::String("assistant".into()),
        "R-NS-10",
    );
    // A message item is only conformant if its content is a conformant output_text part.
    assert_path_eq(
        &body,
        &format!("/output/{idx}/content/0/type"),
        Value::String("output_text".into()),
        "R-NS-11",
    );
}

/// R-NS-11 — assistant text parts serialize as `output_text`, never `input_text`.
#[test]
fn r_ns_11_output_text_part_type_is_never_input_text() {
    let body = responses_body(&chat_turn_text_only());

    // No reasoning in this turn, so the message item is the only output item and index 0 is
    // unambiguous: assert the literal contract path.
    assert_path_eq(
        &body,
        "/output/0/content/0/type",
        Value::String("output_text".into()),
        "R-NS-11",
    );
    assert_path_eq(
        &body,
        "/output/0/content/0/text",
        Value::String(ANSWER.into()),
        "R-NS-11",
    );
}

/// R-NS-12 — `output_text` carries a required `annotations` array (`[]` when no citations).
#[test]
fn r_ns_12_output_text_carries_annotations() {
    let body = responses_body(&chat_turn_text_only());

    assert_path_eq(
        &body,
        "/output/0/content/0/annotations",
        Value::Array(vec![]),
        "R-NS-12",
    );
}

/// R-NS-13 — reasoning is a separate top-level `output[]` item of `type == "reasoning"`.
#[test]
fn r_ns_13_reasoning_is_a_top_level_output_item() {
    let body = responses_body(&chat_turn_with_reasoning());

    let idx = output_index_of(&body, "reasoning");
    assert_path_eq(
        &body,
        &format!("/output/{idx}/type"),
        Value::String("reasoning".into()),
        "R-NS-13",
    );
}

/// R-NS-14 — the reasoning item's `summary` is `[{"type":"summary_text","text":…}]`.
#[test]
fn r_ns_14_reasoning_summary_is_summary_text_parts() {
    let body = responses_body(&chat_turn_with_reasoning());

    let idx = output_index_of(&body, "reasoning");
    assert_path_eq(
        &body,
        &format!("/output/{idx}/summary/0/type"),
        Value::String("summary_text".into()),
        "R-NS-14",
    );
    assert_path_eq(
        &body,
        &format!("/output/{idx}/summary/0/text"),
        Value::String(REASONING.into()),
        "R-NS-14",
    );
}

/// R-NS-16 — the gateway-local `{"type":"reasoning","reasoning":…}` part MUST NOT appear inside a
/// message item, and `input_text` MUST NOT appear anywhere in a response body.
#[test]
fn r_ns_16_no_reasoning_or_input_text_content_part_inside_message() {
    let body = responses_body(&chat_turn_with_reasoning());

    let types = message_content_part_types(&body);
    assert!(
        !types.is_empty(),
        "R-NS-06/R-NS-16: the assistant message must carry at least one content part (guards against a vacuous pass); body: {body}"
    );
    assert!(
        !types.iter().any(|t| t == "reasoning"),
        "R-NS-16: `output[message].content[].type` must never be `reasoning`; got {types:?}; body: {body}"
    );
    assert!(
        !types.iter().any(|t| t == "input_text"),
        "R-NS-11/R-NS-16: `output[message].content[].type` must never be `input_text`; got {types:?}; body: {body}"
    );
    assert!(
        types.iter().all(|t| t == "output_text" || t == "refusal"),
        "R-NS-16: message content parts must be `output_text`/`refusal` only; got {types:?}; body: {body}"
    );
}

/// R-NS-17 / R-NS-18 / R-NS-19 — the usage object: both `*_tokens_details` sub-objects plus the
/// real counts and the `total == input + output` invariant.
///
/// The fixture pins real detail values (`cached_tokens: 0`, `reasoning_tokens: 20`), so these are
/// exact-equality assertions rather than presence-only checks.
#[test]
fn r_ns_17_18_19_usage_object_is_conformant() {
    let body = responses_body(&chat_turn_with_reasoning());

    assert_path_eq(
        &body,
        "/usage/input_tokens_details/cached_tokens",
        Value::from(0u64),
        "R-NS-17",
    );
    assert_path_eq(
        &body,
        "/usage/output_tokens_details/reasoning_tokens",
        Value::from(20u64),
        "R-NS-18",
    );
    assert_path_eq(&body, "/usage/input_tokens", Value::from(42u64), "R-NS-19");
    assert_path_eq(&body, "/usage/output_tokens", Value::from(34u64), "R-NS-19");
    assert_path_eq(&body, "/usage/total_tokens", Value::from(76u64), "R-NS-19");
}

/// R-NS-20 / R-NS-21 — `error` and `incomplete_details` are explicit `null` on success.
#[test]
fn r_ns_20_21_error_and_incomplete_details_are_null() {
    let body = responses_body(&chat_turn_text_only());

    assert_path_eq(&body, "/error", Value::Null, "R-NS-20");
    assert_path_eq(&body, "/incomplete_details", Value::Null, "R-NS-21");
}

/// R-NS-24 — cross-translation parity: `/v1/responses` served from an Anthropic-native upstream
/// must produce the same conformant Responses body as the chat-pivot path.
#[test]
fn r_ns_24_cross_translation_from_anthropic_upstream_conforms() {
    let body = serde_json::to_value(
        anthropic_to_responses_response(&anthropic_turn_with_thinking()).unwrap(),
    )
    .expect("ResponseObject must serialize");

    assert_string_matching(&body, "/id", "resp_", 8, "R-NS-24/R-NS-01");
    assert_int_ge(&body, "/created_at", 1, "R-NS-24/R-NS-03");

    let msg_idx = output_index_of(&body, "message");
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/content/0/type"),
        Value::String("output_text".into()),
        "R-NS-24/R-NS-11",
    );

    let reasoning_idx = output_index_of(&body, "reasoning");
    assert_path_eq(
        &body,
        &format!("/output/{reasoning_idx}/summary/0/type"),
        Value::String("summary_text".into()),
        "R-NS-24/R-NS-14",
    );

    // The Anthropic fixture carries no cache reads/writes, so the cached count is exactly 0.
    assert_path_eq(
        &body,
        "/usage/input_tokens_details/cached_tokens",
        Value::from(0u64),
        "R-NS-24/R-NS-17",
    );
}

// ===========================================================================
// A-NS — POST /v1/messages, stream absent/false  (contract §1.3)
// ===========================================================================

/// A-NS-01 / A-NS-02 / A-NS-03 — the message envelope: `msg_` id, `type`, `role`.
#[test]
fn a_ns_01_02_03_message_envelope_id_type_role() {
    let body = anthropic_body(&chat_turn_text_only());

    assert_string_matching(&body, "/id", "msg_", 1, "A-NS-01");
    assert_path_eq(&body, "/type", Value::String("message".into()), "A-NS-02");
    assert_path_eq(&body, "/role", Value::String("assistant".into()), "A-NS-03");
}

/// A-NS-05 — a thinking block always carries a `signature` key (`""` when the gateway holds no
/// upstream signature).
#[test]
fn a_ns_05_thinking_block_has_signature_key() {
    let body = anthropic_body(&chat_turn_with_reasoning());

    assert_path_eq(
        &body,
        "/content/0/signature",
        Value::String(String::new()),
        "A-NS-05",
    );
}

/// A-NS-06 / A-NS-07 — `stop_reason` is a non-null documented value and `stop_sequence` is an
/// always-present key, `null` when no custom stop sequence matched.
#[test]
fn a_ns_06_07_stop_reason_non_null_and_stop_sequence_present_null() {
    let body = anthropic_body(&chat_turn_text_only());

    let stop_reason = body
        .pointer("/stop_reason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("A-NS-06: `/stop_reason` must be a non-null string; body: {body}")
        });
    const ALLOWED: [&str; 7] = [
        "end_turn",
        "max_tokens",
        "stop_sequence",
        "tool_use",
        "pause_turn",
        "refusal",
        "model_context_window_exceeded",
    ];
    assert!(
        ALLOWED.contains(&stop_reason),
        "A-NS-06: `/stop_reason` must be one of {ALLOWED:?}; got `{stop_reason}`; body: {body}"
    );
    assert_path_eq(&body, "/stop_sequence", Value::Null, "A-NS-07");
}

/// A-NS-08 / A-NS-09 / A-NS-10 / A-NS-11 — the usage object: real token counts plus the cache
/// counters.
#[test]
fn a_ns_08_09_10_11_usage_real_tokens_and_cache_counts() {
    let body = anthropic_body(&chat_turn_with_reasoning());

    assert_int_ge(&body, "/usage/input_tokens", 1, "A-NS-08");
    assert_int_ge(&body, "/usage/output_tokens", 1, "A-NS-09");
    assert_int_ge(
        &body,
        "/usage/cache_creation_input_tokens",
        0,
        "A-NS-10",
    );
    assert_int_ge(&body, "/usage/cache_read_input_tokens", 0, "A-NS-11");
}

/// A-NS-04 / A-NS-05 / A-NS-13 — block discipline: `/content` is an array of typed blocks; the
/// thinking block is complete (including `signature`) and carries the reasoning text, and the text
/// block carries the answer text — never crossed.
#[test]
fn a_ns_04_13_thinking_and_text_blocks_are_not_crossed() {
    let body = anthropic_body(&chat_turn_with_reasoning());

    assert!(
        body.pointer("/content").is_some_and(Value::is_array),
        "A-NS-04: `/content` must be an array; body: {body}"
    );
    assert_path_eq(
        &body,
        "/content/0/type",
        Value::String("thinking".into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/0/thinking",
        Value::String(REASONING.into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/0/signature",
        Value::String(String::new()),
        "A-NS-05",
    );
    assert_path_eq(
        &body,
        "/content/1/type",
        Value::String("text".into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/1/text",
        Value::String(ANSWER.into()),
        "A-NS-13",
    );
}

/// A-NS-14 — cross-translation parity: `/v1/messages` served from a Responses-native upstream must
/// produce the same conformant Anthropic body as the chat-pivot path.
#[test]
fn a_ns_14_cross_translation_from_responses_upstream_conforms() {
    let body =
        serde_json::to_value(responses_to_anthropic_response(&responses_object_text_only()).unwrap())
            .expect("MessageResponse must serialize");

    assert_string_matching(&body, "/id", "msg_", 1, "A-NS-14/A-NS-01");
    assert_path_eq(&body, "/stop_sequence", Value::Null, "A-NS-14/A-NS-07");
    assert_path_eq(
        &body,
        "/content/0/type",
        Value::String("text".into()),
        "A-NS-14/A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/0/text",
        Value::String(ANSWER.into()),
        "A-NS-14/A-NS-13",
    );
    assert_int_ge(
        &body,
        "/usage/cache_read_input_tokens",
        0,
        "A-NS-14/A-NS-11",
    );
}
