//! FROZEN RED-PHASE ACCEPTANCE — cross-protocol `extra` sanitization (P0).
//!
//! Contract (frozen by Lead, v4):
//! - C1: `ponyllm_protocol::sanitize::sanitize_extra(extra, target)` drops every
//!   key reserved by the target wire format (case-insensitive), keeps at most
//!   `MAX_EXTRA_ENTRIES` entries and drops any single value whose serialized form
//!   exceeds `MAX_EXTRA_VALUE_BYTES`; every other entry passes through unchanged.
//! - C2: after translation, every reserved field on the upstream body must equal
//!   the gateway-computed value — a client-supplied `extra` key can never
//!   override `system` / `thinking` / `max_output_tokens` / `metadata` / `store`.
//! - v4 reserved semantics: `reserved_keys(target)` = the target's own explicit
//!   request fields ∪ GLOBAL_CONTROL_KEYS = ["system", "instructions", "thinking",
//!   "metadata", "store", "max_output_tokens", "output_config", "stop_sequences",
//!   "reasoning", "reasoning_effort", "previous_response_id", "conversation",
//!   "prompt", "safety_identifier", "stream_options"], compared case-insensitively.
//!
//! Role discipline: this file is the L2-T / L2-AT acceptance net — the Executor
//! may not edit it. Every test states a real wire-level assertion (exact value
//! equality, exact key-occurrence counts, exact map size); there is no
//! "does not panic" smoke test without a matching semantic assertion.
//!
//! Red-phase expectation (contract skeleton only, `unimplemented!()` stubs):
//! - T4 / AT-* call the stubs directly → panic;
//! - T1 / T2 / T3 / T5 go through the real translators and fail on the
//!   `#[serde(flatten)]` override that the fix must close.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use ponyllm_protocol::anthropic::messages::{
    AnthropicContent, AnthropicMessage, AnthropicOutputConfig, AnthropicRole, AnthropicSystem,
    AnthropicTool, AnthropicToolChoice, MessageRequest, ThinkingConfig,
};
use ponyllm_protocol::common::{ReasoningEffort, StopCondition};
use ponyllm_protocol::openai::chat::{
    ChatCompletionRequest, ChatMessage, FunctionDefinition, MessageContent, ResponseFormat,
    StreamOptions, ToolChoice, ToolDefinition, UserMessage,
};
use ponyllm_protocol::openai::responses::{
    CreateResponseRequest, ResponseInput, ResponseReasoningConfig, ResponseToolDefinition,
};
use ponyllm_protocol::sanitize::{
    reserved_keys, sanitize_extra, TargetProtocol, MAX_EXTRA_ENTRIES, MAX_EXTRA_VALUE_BYTES,
    RESERVED_ANTHROPIC, RESERVED_CHAT, RESERVED_RESPONSES,
};
use ponyllm_protocol::translator::{chat_to_anthropic_request, chat_to_responses_request};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the flattened `extra` map exactly as `#[serde(flatten)]` would (case
/// variants are distinct JSON keys, which is the whole point of T5).
fn extra_map(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// Number of times `needle` occurs in `haystack` — used to prove that the
/// serialized wire body carries exactly ONE `"<key>":` pair, i.e. the flatten
/// map no longer appends a second, client-controlled copy of a reserved field.
fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// Top-level keys of a serialized request structure.
fn top_level_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .expect("serialized request must be a JSON object")
        .keys()
        .cloned()
        .collect()
}

fn chat_request(raw: &str) -> ChatCompletionRequest {
    serde_json::from_str(raw).expect("client chat body must deserialize")
}

/// Assert every key in `keys` is covered by `reserved` (case-insensitive).
fn assert_covers(label: &str, reserved: &[&str], keys: &[String]) {
    let missing: Vec<&String> = keys
        .iter()
        .filter(|k| !reserved.iter().any(|r| r.eq_ignore_ascii_case(k)))
        .collect();
    assert!(
        missing.is_empty(),
        "{label}: reserved set must cover every serialized request field; missing {missing:?}, reserved = {reserved:?}"
    );
}

// ---------------------------------------------------------------------------
// T1 — chat → anthropic: client `system` / `thinking` can never override the
//      gateway-computed values (contract C2).
// ---------------------------------------------------------------------------

#[test]
fn t1_chat_to_anthropic_client_system_and_thinking_cannot_override_gateway() {
    // --- T1.a: client-injected top-level `system` must lose to the gateway ---
    // Arrange: the gateway derives `system` from the role=system message; the
    // client also ships a top-level `system` key (no such field on the Chat
    // request struct → it lands in `extra`).
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": [
                {"role": "system", "content": "GATEWAY_SYSTEM_V1"},
                {"role": "user", "content": "hi"}
            ],
            "system": "CLIENT_INJECTED_SYSTEM"
        }"#,
    );

    // Act
    let translated = chat_to_anthropic_request(&req).expect("chat→anthropic translation");
    let raw = serde_json::to_string(&translated).expect("serialize upstream body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    // Assert: exactly one `system` pair, and it is the gateway value.
    assert_eq!(
        occurrences(&raw, "\"system\":"),
        1,
        "T1: upstream body must carry exactly one `system` field (flatten extra must not append a second); body = {raw}"
    );
    assert_eq!(
        body.get("system"),
        Some(&Value::String("GATEWAY_SYSTEM_V1".to_string())),
        "T1: reserved field `system` must equal the gateway-computed value, got {:?}",
        body.get("system")
    );
    assert!(
        !raw.contains("CLIENT_INJECTED_SYSTEM"),
        "T1: client-injected system value must never reach the upstream body; body = {raw}"
    );

    // --- T1.b: client-injected `thinking` must lose to the gateway value ---
    // Arrange: reasoning_effort=high makes the gateway emit its own thinking
    // block; the client also injects a contradictory one.
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "reasoning_effort": "high",
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "disabled", "budget_tokens": 1}
        }"#,
    );

    let translated = chat_to_anthropic_request(&req).expect("chat→anthropic translation");
    let raw = serde_json::to_string(&translated).expect("serialize upstream body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    let thinking = body
        .get("thinking")
        .filter(|_| occurrences(&raw, "\"thinking\":") == 1)
        .unwrap_or_else(|| {
            panic!(
                "T1: upstream body must carry exactly one gateway-computed `thinking` field, got {} occurrence(s); body = {raw}",
                occurrences(&raw, "\"thinking\":")
            )
        });
    assert_eq!(
        thinking.get("type").and_then(Value::as_str),
        Some("enabled"),
        "T1: `thinking.type` must be the gateway-computed value (enabled), not the client's (disabled); thinking = {thinking}"
    );
    assert!(
        !raw.contains("\"disabled\""),
        "T1: client-injected thinking shape must never survive translation; body = {raw}"
    );
}

// ---------------------------------------------------------------------------
// T2 — chat → responses: client top-level `max_output_tokens` cannot bypass the
//      gateway clamp (contract C2).
// ---------------------------------------------------------------------------

#[test]
fn t2_chat_to_responses_client_max_output_tokens_cannot_bypass_clamp() {
    // Arrange: 512 is the gateway-clamped budget for this model; the client
    // additionally ships the Responses-only field `max_output_tokens: 999999`
    // on a Chat body, where it can only land in `extra`.
    let req = chat_request(
        r#"{
            "model": "gpt-4o-mini",
            "max_tokens": 512,
            "messages": [{"role": "user", "content": "hi"}],
            "max_output_tokens": 999999
        }"#,
    );

    // Act
    let translated = chat_to_responses_request(&req).expect("chat→responses translation");
    let raw = serde_json::to_string(&translated).expect("serialize upstream body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    // Assert: the clamped value wins, exactly one pair, hostile value absent.
    assert_eq!(
        occurrences(&raw, "\"max_output_tokens\":"),
        1,
        "T2: upstream body must carry exactly one `max_output_tokens` pair; body = {raw}"
    );
    assert_eq!(
        body.get("max_output_tokens").and_then(Value::as_u64),
        Some(512),
        "T2: `max_output_tokens` must equal the gateway clamp (512), got {:?}",
        body.get("max_output_tokens")
    );
    assert!(
        !raw.contains("999999"),
        "T2: client-injected 999999 must never reach the upstream body; body = {raw}"
    );

    // Direct contract check: same-protocol Chat target also reserves it.
    let out = sanitize_extra(
        extra_map(&[("max_output_tokens", json!(999999))]),
        TargetProtocol::OpenAiChat,
    );
    assert!(
        !out.keys().any(|k| k.eq_ignore_ascii_case("max_output_tokens")),
        "T2: `max_output_tokens` is a reserved control key for every target (v4 GLOBAL_CONTROL_KEYS); got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// T3 — `metadata` / `store` never appear on an upstream body, in every
//      translation direction (contract C1/C2).
// ---------------------------------------------------------------------------

#[test]
fn t3_metadata_and_store_are_stripped_from_upstream_body() {
    // Arrange: hostile metadata + store on a Chat body.
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "hi"}],
            "metadata": {"user_id": "ATTACKER_META"},
            "store": true
        }"#,
    );

    // --- anthropic direction ---
    let anth = serde_json::to_string(&chat_to_anthropic_request(&req).unwrap())
        .expect("serialize anthropic body");
    let anth_body: Value = serde_json::from_str(&anth).expect("anthropic body parses");
    assert!(
        anth_body.get("metadata").is_none(),
        "T3: `metadata` must be stripped from the anthropic upstream body; body = {anth}"
    );
    assert!(
        anth_body.get("store").is_none(),
        "T3: `store` must be stripped from the anthropic upstream body; body = {anth}"
    );
    assert!(
        !anth.contains("ATTACKER_META"),
        "T3: hostile metadata payload must never reach the anthropic body; body = {anth}"
    );

    // --- responses direction ---
    let resp = serde_json::to_string(&chat_to_responses_request(&req).unwrap())
        .expect("serialize responses body");
    let resp_body: Value = serde_json::from_str(&resp).expect("responses body parses");
    assert!(
        resp_body.get("metadata").is_none(),
        "T3: `metadata` must be stripped from the responses upstream body; body = {resp}"
    );
    assert!(
        resp_body.get("store").is_none(),
        "T3: `store` must be stripped from the responses upstream body; body = {resp}"
    );
    assert!(
        !resp.contains("ATTACKER_META"),
        "T3: hostile metadata payload must never reach the responses body; body = {resp}"
    );

    // --- direct contract check across all three targets ---
    for target in [
        TargetProtocol::OpenAiChat,
        TargetProtocol::OpenAiResponses,
        TargetProtocol::Anthropic,
    ] {
        let out = sanitize_extra(
            extra_map(&[
                ("metadata", json!({"user_id": "ATTACKER_META"})),
                ("store", json!(true)),
            ]),
            target,
        );
        assert!(
            out.is_empty(),
            "T3: `metadata`/`store` must be reserved for {target:?} (v4 GLOBAL_CONTROL_KEYS); left {out:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// T4 — legal extension keys survive (over-tightening regression guard).
// ---------------------------------------------------------------------------

#[test]
fn t4_legal_extension_keys_are_preserved() {
    // `x_provider_ext_flag` is not an explicit field of ANY of the three request
    // structures and is not a v4 GLOBAL_CONTROL_KEY → it must pass through.
    for target in [
        TargetProtocol::OpenAiChat,
        TargetProtocol::OpenAiResponses,
        TargetProtocol::Anthropic,
    ] {
        let out = sanitize_extra(
            extra_map(&[
                ("x_provider_ext_flag", json!(true)),
                ("parallel_tool_calls_hint", json!("keep-me")),
            ]),
            target,
        );
        assert_eq!(
            out.get("x_provider_ext_flag"),
            Some(&json!(true)),
            "T4: legal extension key must be preserved verbatim for {target:?}; out = {out:?}"
        );
        assert_eq!(
            out.get("parallel_tool_calls_hint"),
            Some(&json!("keep-me")),
            "T4: legal extension key must be preserved verbatim for {target:?}; out = {out:?}"
        );
        assert_eq!(
            out.len(),
            2,
            "T4: sanitization must add or drop nothing beyond reserved names (over-tightening guard); out = {out:?}"
        );
    }

    // `top_k` is an explicit field of the Anthropic request only: it is legal
    // passthrough for the Chat/Responses targets and reserved for Anthropic.
    let chat_target = sanitize_extra(
        extra_map(&[("top_k", json!(40))]),
        TargetProtocol::OpenAiChat,
    );
    assert_eq!(
        chat_target.get("top_k"),
        Some(&json!(40)),
        "T4: `top_k` is NOT an OpenAI Chat field → must survive the Chat target; out = {chat_target:?}"
    );
    let responses_target = sanitize_extra(
        extra_map(&[("top_k", json!(40))]),
        TargetProtocol::OpenAiResponses,
    );
    assert_eq!(
        responses_target.get("top_k"),
        Some(&json!(40)),
        "T4: `top_k` is NOT an OpenAI Responses field → must survive the Responses target; out = {responses_target:?}"
    );
    let anthropic_target = sanitize_extra(
        extra_map(&[("top_k", json!(40))]),
        TargetProtocol::Anthropic,
    );
    assert!(
        anthropic_target.get("top_k").is_none(),
        "T4: `top_k` IS an explicit Anthropic field → must be reserved for the Anthropic target; out = {anthropic_target:?}"
    );

    // End-to-end: the legal extension key still reaches the upstream body.
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "hi"}],
            "x_provider_ext_flag": true
        }"#,
    );
    let raw = serde_json::to_string(&chat_to_anthropic_request(&req).unwrap())
        .expect("serialize anthropic body");
    let body: Value = serde_json::from_str(&raw).expect("anthropic body parses");
    assert_eq!(
        body.get("x_provider_ext_flag"),
        Some(&json!(true)),
        "T4: a legal extension key must survive translation to the upstream body; body = {raw}"
    );
}

// ---------------------------------------------------------------------------
// T5 — case variants of reserved names are stripped just the same.
// ---------------------------------------------------------------------------

#[test]
fn t5_case_variants_of_reserved_keys_are_stripped() {
    // Arrange: three distinct spellings of the same reserved name.
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": [
                {"role": "system", "content": "GATEWAY_SYSTEM"},
                {"role": "user", "content": "hi"}
            ],
            "System": "CLIENT_CASE_TITLE",
            "SYSTEM": "CLIENT_CASE_UPPER",
            "Metadata": {"user_id": "CLIENT_CASE_META"}
        }"#,
    );

    // Act
    let raw = serde_json::to_string(&chat_to_anthropic_request(&req).unwrap())
        .expect("serialize anthropic body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    // Assert: only the gateway field remains, none of the case variants leak.
    assert_eq!(
        occurrences(&raw, "\"system\":"),
        1,
        "T5: case variants (`System`, `SYSTEM`) must not append extra `system` pairs; body = {raw}"
    );
    assert_eq!(
        body.get("system"),
        Some(&Value::String("GATEWAY_SYSTEM".to_string())),
        "T5: the single `system` field must be the gateway value; body = {raw}"
    );
    for leaked in ["CLIENT_CASE_TITLE", "CLIENT_CASE_UPPER", "CLIENT_CASE_META"] {
        assert!(
            !raw.contains(leaked),
            "T5: case-variant reserved value {leaked} must never reach the upstream body; body = {raw}"
        );
    }
    assert!(
        body.get("Metadata").is_none(),
        "T5: `Metadata` must be stripped case-insensitively; body = {raw}"
    );

    // Direct contract check: every spelling is reserved.
    let out = sanitize_extra(
        extra_map(&[
            ("System", json!("a")),
            ("SYSTEM", json!("b")),
            ("sYsTeM", json!("c")),
        ]),
        TargetProtocol::OpenAiChat,
    );
    assert!(
        out.is_empty(),
        "T5: reserved matching must be case-insensitive for every target; left {out:?}"
    );
}

// ---------------------------------------------------------------------------
// T6 — reserved-set coverage: no drift between struct fields and the constants.
// ---------------------------------------------------------------------------

#[test]
fn t6_reserved_constants_cover_every_serialized_request_field() {
    // Fully populated instances: every explicit field is `Some`, so the
    // serialized key set is the complete field-name universe of each struct.
    let chat = ChatCompletionRequest {
        model: "m".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: MessageContent::Text("hi".to_string()),
            name: Some("n".to_string()),
        })],
        temperature: Some(0.1),
        top_p: Some(0.9),
        n: Some(1),
        stream: Some(false),
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
        stop: Some(StopCondition::Single("STOP".to_string())),
        max_tokens: Some(1024),
        max_completion_tokens: Some(2048),
        presence_penalty: Some(0.0),
        frequency_penalty: Some(0.0),
        logit_bias: Some(HashMap::from([("1".to_string(), 0.5f32)])),
        user: Some("u".to_string()),
        response_format: Some(ResponseFormat::Text),
        seed: Some(42),
        tools: Some(vec![ToolDefinition {
            r#type: "function".to_string(),
            function: FunctionDefinition {
                name: "t".to_string(),
                description: None,
                parameters: None,
                strict: None,
            },
        }]),
        tool_choice: Some(ToolChoice::Mode("auto".to_string())),
        parallel_tool_calls: Some(true),
        reasoning_effort: Some(ReasoningEffort::High),
        extra: HashMap::new(),
    };

    let responses = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Text("hi".to_string()),
        instructions: Some("be brief".to_string()),
        modalities: Some(vec!["text".to_string()]),
        tools: Some(vec![ResponseToolDefinition::Function {
            name: "t".to_string(),
            description: None,
            parameters: None,
            strict: None,
        }]),
        tool_choice: Some(json!("auto")),
        temperature: Some(0.1),
        top_p: Some(0.9),
        max_output_tokens: Some(2048),
        stream: Some(false),
        metadata: Some(HashMap::from([("k".to_string(), "v".to_string())])),
        reasoning_effort: Some(ReasoningEffort::High),
        reasoning: Some(ResponseReasoningConfig {
            effort: Some(ReasoningEffort::High),
        }),
        extra: HashMap::new(),
    };

    let anthropic = MessageRequest {
        model: "m".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: AnthropicContent::Text("hi".to_string()),
        }],
        max_tokens: 1024,
        system: Some(AnthropicSystem::Text("be brief".to_string())),
        metadata: Some(json!({"user_id": "u"})),
        stop_sequences: Some(vec!["STOP".to_string()]),
        stream: Some(false),
        temperature: Some(0.1),
        top_p: Some(0.9),
        top_k: Some(40),
        tools: Some(vec![AnthropicTool {
            name: "t".to_string(),
            description: None,
            input_schema: json!({"type": "object"}),
            cache_control: None,
        }]),
        tool_choice: Some(AnthropicToolChoice::Auto {
            disable_parallel_tool_use: None,
        }),
        thinking: Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: Some(1024),
            effort: Some(ReasoningEffort::High),
        }),
        output_config: Some(AnthropicOutputConfig {
            effort: Some(ReasoningEffort::High),
        }),
        reasoning_effort: Some(ReasoningEffort::High),
        extra: HashMap::new(),
    };

    // (a) each target's `reserved_keys()` covers that target's serialized fields
    for (target, label, value) in [
        (
            TargetProtocol::OpenAiChat,
            "OpenAiChat",
            serde_json::to_value(&chat).unwrap(),
        ),
        (
            TargetProtocol::OpenAiResponses,
            "OpenAiResponses",
            serde_json::to_value(&responses).unwrap(),
        ),
        (
            TargetProtocol::Anthropic,
            "Anthropic",
            serde_json::to_value(&anthropic).unwrap(),
        ),
    ] {
        let keys = top_level_keys(&value);
        assert!(
            !keys.is_empty(),
            "T6: {label} serialized request must expose at least one field (test fixture broken)"
        );
        assert!(
            !reserved_keys(target).is_empty(),
            "T6: reserved_keys({label}) must not be empty — every wire field would be client-overridable"
        );
        assert_covers(
            &format!("T6/{label} (reserved_keys)"),
            reserved_keys(target),
            &keys,
        );
    }

    // (b) the RESERVED_* constants themselves cover their protocol's fields —
    //     the declared reserved set is the protocol's explicit field universe
    //     (C1), not just whatever happens to also sit in the global list.
    assert_covers(
        "T6/RESERVED_CHAT",
        RESERVED_CHAT,
        &top_level_keys(&serde_json::to_value(&chat).unwrap()),
    );
    assert_covers(
        "T6/RESERVED_RESPONSES",
        RESERVED_RESPONSES,
        &top_level_keys(&serde_json::to_value(&responses).unwrap()),
    );
    assert_covers(
        "T6/RESERVED_ANTHROPIC",
        RESERVED_ANTHROPIC,
        &top_level_keys(&serde_json::to_value(&anthropic).unwrap()),
    );

    // (c) default-constructed structs (the literal T6 wording) are covered too.
    let default_chat = serde_json::to_value(ChatCompletionRequest::default()).unwrap();
    assert!(
        default_chat.as_object().is_some_and(|m| !m.is_empty()),
        "T6: default chat request must serialize at least model/messages"
    );
    assert_covers(
        "T6/default chat",
        reserved_keys(TargetProtocol::OpenAiChat),
        &top_level_keys(&default_chat),
    );
    let default_anthropic = serde_json::to_value(MessageRequest::default()).unwrap();
    assert!(
        default_anthropic.as_object().is_some_and(|m| !m.is_empty()),
        "T6: default anthropic request must serialize at least model/messages/max_tokens"
    );
    assert_covers(
        "T6/default anthropic",
        reserved_keys(TargetProtocol::Anthropic),
        &top_level_keys(&default_anthropic),
    );
}

// ---------------------------------------------------------------------------
// L2-AT adversarial strain
// ---------------------------------------------------------------------------

#[test]
fn at01_entry_cap_is_enforced_without_panic() {
    // Arrange: 200 legal, non-reserved entries — far past the 64-entry cap.
    let pairs: Vec<(String, Value)> = (0..200)
        .map(|i| (format!("x_ext_{i:03}"), json!(i)))
        .collect();
    let extra: HashMap<String, Value> = pairs.into_iter().collect();

    // Act
    let out = sanitize_extra(extra, TargetProtocol::OpenAiChat);

    // Assert: bounded by the cap, every surviving key is an input key.
    assert!(
        out.len() <= MAX_EXTRA_ENTRIES,
        "AT-01: sanitized extra must never exceed MAX_EXTRA_ENTRIES ({MAX_EXTRA_ENTRIES}), got {}",
        out.len()
    );
    for key in out.keys() {
        assert!(
            key.starts_with("x_ext_"),
            "AT-01: sanitization invented a key '{key}'"
        );
    }
    // Nothing legal is silently mangled: values are the original numbers.
    for (key, value) in &out {
        let idx: usize = key.trim_start_matches("x_ext_").parse().expect("key shape");
        assert_eq!(
            value,
            &json!(idx),
            "AT-01: retained value for '{key}' must be the original input value"
        );
    }
}

#[test]
fn at02_oversized_single_value_is_dropped_small_one_kept() {
    // Arrange: one value far past the 16 KiB serialized cap, one comfortably
    // inside it (both under legal, non-reserved keys).
    let big = "A".repeat(MAX_EXTRA_VALUE_BYTES * 2);
    let small = "B".repeat(4096);
    assert!(big.len() > MAX_EXTRA_VALUE_BYTES);
    assert!(small.len() < MAX_EXTRA_VALUE_BYTES);

    // Act
    let out = sanitize_extra(
        extra_map(&[("x_big", json!(big)), ("x_small", json!(small))]),
        TargetProtocol::OpenAiChat,
    );

    // Assert: exactly the oversized entry is dropped.
    assert!(
        out.get("x_big").is_none(),
        "AT-02: a value whose serialized form exceeds MAX_EXTRA_VALUE_BYTES ({MAX_EXTRA_VALUE_BYTES}) must be dropped, got len {:?}",
        out.get("x_big").map(|v| v.as_str().map(str::len))
    );
    assert_eq!(
        out.get("x_small").and_then(Value::as_str).map(str::len),
        Some(4096),
        "AT-02: an in-budget value must be preserved byte-for-byte; out = {out:?}"
    );
    assert_eq!(
        out.len(),
        1,
        "AT-02: exactly one entry survives; out = {out:?}"
    );
}

#[test]
fn at03_type_confused_reserved_values_are_still_stripped() {
    // Arrange: reserved names carrying non-string / non-object hostile shapes.
    // Stripping must be name-driven, never type-driven (a typed client can
    // otherwise slip an object where the gateway expects a scalar).
    let out = sanitize_extra(
        extra_map(&[
            ("max_tokens", json!({"evil": [1, 2, 3]})),
            ("system", json!([{"role": "system", "content": "evil"}])),
            ("messages", Value::Null),
            ("thinking", json!(true)),
            ("stream_options", json!("notanobject")),
            ("systemx", json!("near-miss-legal")),
        ]),
        TargetProtocol::OpenAiChat,
    );

    // Assert: only the near-miss survives.
    assert_eq!(
        out.len(),
        1,
        "AT-03: every reserved name must be stripped regardless of value type; out = {out:?}"
    );
    assert_eq!(
        out.get("systemx"),
        Some(&json!("near-miss-legal")),
        "AT-03: prefix-adjacent names are not reserved and must survive; out = {out:?}"
    );
}

#[test]
fn at04_deeply_nested_value_does_not_panic_and_reserved_nesting_is_stripped() {
    // Arrange: depth-100 nesting (well under any size cap, but deep enough to
    // break naive recursive walkers / stack assumptions).
    let mut deep = json!("leaf");
    for _ in 0..100 {
        deep = json!([deep]);
    }

    // Act on a legal key (must survive intact) and on a reserved key (must die).
    let out = sanitize_extra(
        extra_map(&[
            ("x_deep", deep.clone()),
            ("system", deep.clone()),
            (
                "thinking",
                json!({"type": "enabled", "budget_tokens": deep}),
            ),
        ]),
        TargetProtocol::Anthropic,
    );

    // Assert: no panic; the legal deep value round-trips, reserved ones are gone.
    assert_eq!(
        out.get("x_deep"),
        Some(&deep),
        "AT-04: a legal value nested 100 levels deep must survive sanitization unchanged"
    );
    assert!(
        out.get("system").is_none(),
        "AT-04: a reserved key must be stripped even when its value is 100 levels deep; out = {out:?}"
    );
    assert!(
        out.get("thinking").is_none(),
        "AT-04: a reserved key must be stripped even when it contains 100-level nesting; out = {out:?}"
    );
    assert_eq!(
        out.len(),
        1,
        "AT-04: only the legal key survives; out = {out:?}"
    );
    // The result must still be serializable (no dangling / cyclic structure).
    let serialized = serde_json::to_string(&out).expect("sanitized map must serialize");
    assert!(
        serialized.contains("x_deep"),
        "AT-04: sanitized map must serialize with its legal key; got {serialized}"
    );
}

#[test]
fn at05_duplicate_reserved_keys_in_raw_json_are_neutralized() {
    // Arrange: a hostile raw body that repeats the same reserved key twice —
    // serde keeps the last occurrence in `extra`, which must still lose to the
    // gateway value after translation.
    let req = chat_request(
        r#"{
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": [
                {"role": "system", "content": "GATEWAY_SYSTEM_DUP"},
                {"role": "user", "content": "hi"}
            ],
            "system": "FIRST_INJECT",
            "system": "LAST_INJECT"
        }"#,
    );

    // Act
    let raw = serde_json::to_string(&chat_to_anthropic_request(&req).unwrap())
        .expect("serialize anthropic body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    // Assert: the duplicate-client-key trick changes nothing.
    assert_eq!(
        occurrences(&raw, "\"system\":"),
        1,
        "AT-05: duplicated client `system` keys must not append pairs upstream; body = {raw}"
    );
    assert_eq!(
        body.get("system"),
        Some(&Value::String("GATEWAY_SYSTEM_DUP".to_string())),
        "AT-05: gateway-computed `system` must win over duplicate client keys; body = {raw}"
    );
    for leaked in ["FIRST_INJECT", "LAST_INJECT"] {
        assert!(
            !raw.contains(leaked),
            "AT-05: duplicate client value {leaked} must never reach the upstream body; body = {raw}"
        );
    }
}

#[test]
fn at06_empty_and_all_reserved_maps_are_safe() {
    // Empty map: nothing to do, nothing invented.
    let out = sanitize_extra(HashMap::new(), TargetProtocol::OpenAiResponses);
    assert!(
        out.is_empty(),
        "AT-06: empty extra must stay empty; got {out:?}"
    );

    // Entirely reserved map: the result must be empty, not partially forwarded.
    // v5 note: `previous_response_id` / `conversation` / `prompt` were REMOVED
    // from GLOBAL_CONTROL_KEYS (CreateResponseRequest has no such explicit
    // field; stripping them broke /v1/responses chained conversations), so they
    // are asserted as PRESERVED in
    // tests/security_extra_sanitize_v5_acceptance.rs.
    let out = sanitize_extra(
        extra_map(&[
            ("instructions", json!("evil")),
            ("store", json!(true)),
            ("metadata", json!({})),
            ("output_config", json!({"effort": "max"})),
            ("safety_identifier", json!("sid")),
        ]),
        TargetProtocol::OpenAiResponses,
    );
    assert!(
        out.is_empty(),
        "AT-06: every v4 GLOBAL_CONTROL_KEY must be stripped for the Responses target; left {out:?}"
    );
}

// Keep the unused-import lint honest if a helper type is only used in one test.
#[allow(dead_code)]
fn _type_check(map: &Map<String, Value>) -> usize {
    map.len()
}
