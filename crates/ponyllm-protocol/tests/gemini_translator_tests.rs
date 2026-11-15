//! Acceptance tests for the native Gemini (`generativelanguage.googleapis.com`)
//! translator: request body shape, non-stream response projection, SSE chunk
//! projection and the terminal chunk.
//!
//! Every assertion pins a concrete wire value (shapes and numbers observed
//! against the live API), so a regression fails here instead of at the upstream
//! 400. Run: `cargo test -p ponyllm-protocol --test gemini_translator_tests`.

use serde_json::{json, Value};

use ponyllm_protocol::common::{ReasoningEffort, StopCondition};
use ponyllm_protocol::openai::chat::*;
use ponyllm_protocol::translator::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn user_text(text: &str) -> ChatMessage {
    ChatMessage::User(UserMessage {
        content: MessageContent::Text(text.to_string()),
        name: None,
    })
}

fn assistant_text(text: &str) -> ChatMessage {
    ChatMessage::Assistant(AssistantMessage {
        content: Some(MessageContent::Text(text.to_string())),
        ..Default::default()
    })
}

fn tool_call(id: &str, name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    }
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".to_string(),
        function: FunctionDefinition {
            name: "get.weather".to_string(),
            description: Some("Get the weather".to_string()),
            parameters: Some(json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "city": { "type": "string", "description": "city name" },
                    "unit": { "const": "celsius" },
                    "tags": { "type": "array" }
                },
                "required": ["city"]
            })),
            strict: Some(true),
        },
    }
}

fn assert_close(actual: &Value, expected: f64) {
    let got = actual.as_f64().expect("expected a JSON number");
    assert!(
        (got - expected).abs() < 1e-6,
        "expected {expected}, got {got} in {actual}"
    );
}

// ---------------------------------------------------------------------------
// Request: system / roles / merging / envelope
// ---------------------------------------------------------------------------

#[test]
fn request_maps_system_roles_merges_turns_and_has_no_envelope() {
    let req = ChatCompletionRequest {
        model: "gemini-3.6-flash".to_string(),
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: MessageContent::Text("sys rules".to_string()),
                name: None,
            }),
            ChatMessage::Developer(DeveloperMessage {
                content: MessageContent::Text("dev rules".to_string()),
                name: None,
            }),
            user_text("hi"),
            user_text("again"),
            assistant_text("hello!"),
        ],
        ..Default::default()
    };

    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None)
        .expect("chat_to_gemini_request must succeed");

    assert_eq!(
        body["systemInstruction"]["parts"],
        json!([{ "text": "sys rules" }, { "text": "dev rules" }]),
        "system + developer must both land in systemInstruction.parts: {body}"
    );

    let contents = body["contents"]
        .as_array()
        .expect("contents must be an array");
    assert_eq!(
        contents.len(),
        2,
        "consecutive same-role turns must merge, got {contents:?}"
    );
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(
        contents[0]["parts"],
        json!([{ "text": "hi" }, { "text": "again" }])
    );
    assert_eq!(contents[1]["role"], "model", "assistant must map to model");
    assert_eq!(contents[1]["parts"], json!([{ "text": "hello!" }]));

    // Native body only: the Antigravity envelope and any request-scoped
    // metadata must never leak (Gemini rejects unknown body fields).
    for forbidden in [
        "model",
        "project",
        "requestId",
        "sessionId",
        "labels",
        "safetySettings",
        "request",
    ] {
        assert!(
            body.get(forbidden).is_none(),
            "native Gemini body must not carry `{forbidden}`: {body}"
        );
    }
}

#[test]
fn request_empty_system_is_omitted() {
    let req = ChatCompletionRequest {
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: MessageContent::Text("   ".to_string()),
                name: None,
            }),
            user_text("hi"),
        ],
        ..Default::default()
    };
    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    assert!(
        body.get("systemInstruction").is_none(),
        "blank system prompt must not emit systemInstruction: {body}"
    );
}

// ---------------------------------------------------------------------------
// Request: tools + tool_choice
// ---------------------------------------------------------------------------

#[test]
fn request_sanitizes_tool_schema_and_maps_tool_choice() {
    let tool = weather_tool();

    // 1) tool_choice absent → AUTO.
    let req = ChatCompletionRequest {
        messages: vec![user_text("weather?")],
        tools: Some(vec![tool.clone()]),
        ..Default::default()
    };
    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    let decl = &body["tools"][0]["functionDeclarations"][0];
    assert_eq!(decl["name"], "get.weather");
    assert_eq!(decl["description"], "Get the weather");
    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");

    let params = &decl["parameters"];
    assert_eq!(params["type"], "object");
    assert_eq!(params["required"], json!(["city"]));
    assert_eq!(params["properties"]["city"]["type"], "string");
    // const → enum, and unsupported keywords dropped (Gemini hard-rejects them).
    assert_eq!(params["properties"]["unit"]["enum"], json!(["celsius"]));
    assert!(params["properties"]["unit"].get("const").is_none());
    assert!(
        params.get("$schema").is_none(),
        "`$schema` must be stripped: {params}"
    );
    assert!(
        params.get("additionalProperties").is_none(),
        "`additionalProperties` must be stripped: {params}"
    );
    // array without items gets a permissive backfill.
    assert_eq!(
        params["properties"]["tags"]["items"],
        json!({ "type": "string" })
    );

    // 2) tool_choice = "none" → NONE.
    let req = ChatCompletionRequest {
        messages: vec![user_text("weather?")],
        tools: Some(vec![tool.clone()]),
        tool_choice: Some(ToolChoice::Mode("none".to_string())),
        ..Default::default()
    };
    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "NONE");

    // 3) required / any → ANY.
    for mode in ["required", "any"] {
        let req = ChatCompletionRequest {
            messages: vec![user_text("weather?")],
            tools: Some(vec![tool.clone()]),
            tool_choice: Some(ToolChoice::Mode(mode.to_string())),
            ..Default::default()
        };
        let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
        assert_eq!(
            body["toolConfig"]["functionCallingConfig"]["mode"], "ANY",
            "tool_choice={mode}"
        );
    }

    // 4) pinned function → ANY + allowedFunctionNames.
    let req = ChatCompletionRequest {
        messages: vec![user_text("weather?")],
        tools: Some(vec![tool.clone()]),
        tool_choice: Some(ToolChoice::Specific {
            r#type: "function".to_string(),
            function: SpecificFunctionName {
                name: "get.weather".to_string(),
            },
        }),
        ..Default::default()
    };
    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    let fcc = &body["toolConfig"]["functionCallingConfig"];
    assert_eq!(fcc["mode"], "ANY");
    assert_eq!(fcc["allowedFunctionNames"], json!(["get.weather"]));
}

// ---------------------------------------------------------------------------
// Request: thinking config
// ---------------------------------------------------------------------------

#[test]
fn thinking_config_uses_budget_for_25_and_level_for_newer() {
    // gemini-2.5 → thinkingBudget ladder.
    assert_eq!(
        gemini_thinking_config("gemini-2.5-pro", Some(ReasoningEffort::Low)),
        Some(json!({ "includeThoughts": true, "thinkingBudget": 1024 }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-2.5-flash", Some(ReasoningEffort::Medium)),
        Some(json!({ "includeThoughts": true, "thinkingBudget": 4096 }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-2.5-pro", Some(ReasoningEffort::High)),
        Some(json!({ "includeThoughts": true, "thinkingBudget": 16384 }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-2.5-pro", Some(ReasoningEffort::Max)),
        Some(json!({ "includeThoughts": true, "thinkingBudget": 24576 }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-2.5-pro", Some(ReasoningEffort::Off)),
        Some(json!({ "includeThoughts": false, "thinkingBudget": 0 }))
    );

    // gemini-3.x → thinkingLevel.
    assert_eq!(
        gemini_thinking_config("gemini-3.6-flash", Some(ReasoningEffort::Low)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "low" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-3-pro", Some(ReasoningEffort::Medium)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "medium" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-3.6-flash", Some(ReasoningEffort::High)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "high" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-3.6-flash", Some(ReasoningEffort::Max)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "high" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-3.6-flash", Some(ReasoningEffort::Off)),
        Some(json!({ "includeThoughts": false, "thinkingLevel": "low" }))
    );

    // Gemini 4 Hydrogen adds `xhigh` (gateway Max) and keeps `high` as the
    // top of the 3.x ladder for every other Gemini 4 sibling.
    assert_eq!(
        gemini_thinking_config("gemini-4-hydrogen", Some(ReasoningEffort::Max)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "xhigh" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-4-hydrogen", Some(ReasoningEffort::High)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "high" }))
    );
    assert_eq!(
        gemini_thinking_config("gemini-4-carbon", Some(ReasoningEffort::Max)),
        Some(json!({ "includeThoughts": true, "thinkingLevel": "high" }))
    );

    // No explicit effort → upstream default, no thinkingConfig at all.
    assert_eq!(gemini_thinking_config("gemini-3.6-flash", None), None);
}

#[test]
fn request_embeds_thinking_config_only_when_requested() {
    let req = ChatCompletionRequest {
        messages: vec![user_text("hi")],
        ..Default::default()
    };

    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    assert!(
        body.get("generationConfig").is_none(),
        "no explicit effort → no generationConfig: {body}"
    );

    let body =
        chat_to_gemini_request(&req, "gemini-3.6-flash", Some(ReasoningEffort::High)).unwrap();
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({ "includeThoughts": true, "thinkingLevel": "high" })
    );

    let body =
        chat_to_gemini_request(&req, "gemini-2.5-flash", Some(ReasoningEffort::Low)).unwrap();
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({ "includeThoughts": true, "thinkingBudget": 1024 })
    );
}

// ---------------------------------------------------------------------------
// Request: multimodal
// ---------------------------------------------------------------------------

#[test]
fn request_maps_multimodal_parts_to_inline_data() {
    let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let req = ChatCompletionRequest {
        messages: vec![ChatMessage::User(UserMessage {
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "describe".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrlObject {
                        url: format!("data:image/png;base64,{png_b64}"),
                        detail: Some("high".to_string()),
                    },
                },
                ContentPart::InputAudio {
                    input_audio: InputAudioObject {
                        data: "UklGRg==".to_string(),
                        format: "wav".to_string(),
                    },
                },
                ContentPart::File {
                    file: InputFileObject {
                        file_id: None,
                        file_url: Some("data:application/pdf;base64,JVBERi0xLjQK".to_string()),
                        filename: Some("doc.pdf".to_string()),
                    },
                },
                ContentPart::File {
                    file: InputFileObject {
                        file_id: None,
                        file_url: Some("data:text/plain;base64,SGVsbG8=".to_string()),
                        filename: Some("note.txt".to_string()),
                    },
                },
            ]),
            name: None,
        })],
        ..Default::default()
    };

    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    let parts = body["contents"][0]["parts"]
        .as_array()
        .expect("parts must be an array");
    assert_eq!(parts.len(), 5, "got {parts:?}");

    assert_eq!(parts[0], json!({ "text": "describe" }));
    assert_eq!(
        parts[1],
        json!({
            "inlineData": { "mimeType": "image/png", "data": png_b64 }
        })
    );
    assert_eq!(
        parts[2],
        json!({
            "inlineData": { "mimeType": "audio/wav", "data": "UklGRg==" }
        })
    );
    assert_eq!(
        parts[3],
        json!({
            "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0xLjQK" }
        })
    );
    // Text documents are decoded instead of being sent as inlineData
    // (Gemini rejects text/* MIME types on the inline path).
    assert_eq!(parts[4], json!({ "text": "Hello" }));
}

#[test]
fn request_replays_tool_calls_and_results() {
    let req = ChatCompletionRequest {
        messages: vec![
            user_text("weather in Paris and Rome?"),
            ChatMessage::Assistant(AssistantMessage {
                content: Some(MessageContent::Text("Checking.".to_string())),
                tool_calls: Some(vec![
                    tool_call("call_1", "get_weather", r#"{"city":"Paris"}"#),
                    tool_call("call_2", "get_weather", r#"{"city":"Rome"}"#),
                ]),
                ..Default::default()
            }),
            ChatMessage::Tool(ToolMessage {
                content: MessageContent::Text(r#"{"temp":21}"#.to_string()),
                tool_call_id: "call_1".to_string(),
                name: None,
            }),
            ChatMessage::Tool(ToolMessage {
                content: MessageContent::Text(r#"{"temp":25}"#.to_string()),
                tool_call_id: "call_2".to_string(),
                name: Some("get_weather".to_string()),
            }),
        ],
        ..Default::default()
    };

    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "got {contents:?}");

    // model turn: text + two functionCall parts, every functionCall carrying
    // the mandatory replay sentinel (native API 400s without it).
    let model_parts = contents[1]["parts"].as_array().unwrap();
    assert_eq!(model_parts.len(), 3);
    assert_eq!(model_parts[0], json!({ "text": "Checking." }));
    assert_eq!(
        model_parts[1]["functionCall"],
        json!({ "name": "get_weather", "args": { "city": "Paris" } })
    );
    assert_eq!(
        model_parts[1]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    assert_eq!(
        model_parts[2]["functionCall"],
        json!({ "name": "get_weather", "args": { "city": "Rome" } })
    );
    assert_eq!(
        model_parts[2]["thoughtSignature"],
        "skip_thought_signature_validator"
    );

    // tool results merge into one user turn; names resolve through tool_call ids.
    assert_eq!(contents[2]["role"], "user");
    let result_parts = contents[2]["parts"].as_array().unwrap();
    assert_eq!(result_parts.len(), 2);
    assert_eq!(
        result_parts[0]["functionResponse"],
        json!({ "name": "get_weather", "response": { "temp": 21 } })
    );
    assert_eq!(
        result_parts[1]["functionResponse"],
        json!({ "name": "get_weather", "response": { "temp": 25 } })
    );
}

// ---------------------------------------------------------------------------
// Request: generationConfig + no unknown fields
// ---------------------------------------------------------------------------

#[test]
fn request_builds_generation_config_and_drops_unknown_fields() {
    let mut extra = std::collections::HashMap::new();
    extra.insert("top_k".to_string(), json!(40));
    extra.insert("foo".to_string(), json!("bar"));
    extra.insert("reasoning_effort".to_string(), json!("high"));

    let req = ChatCompletionRequest {
        messages: vec![user_text("json please")],
        temperature: Some(0.7),
        top_p: Some(0.9),
        max_tokens: Some(256),
        stop: Some(StopCondition::Single("\n\n".to_string())),
        seed: Some(42),
        n: Some(2),
        response_format: Some(ResponseFormat::JsonSchema {
            json_schema: JsonSchemaObject {
                name: "answer".to_string(),
                description: None,
                schema: Some(json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "additionalProperties": false,
                    "properties": { "answer": { "type": "string" } },
                    "required": ["answer"]
                })),
                strict: Some(true),
            },
        }),
        extra,
        ..Default::default()
    };

    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    let gen = &body["generationConfig"];
    assert_close(&gen["temperature"], 0.7);
    assert_close(&gen["topP"], 0.9);
    assert_eq!(gen["topK"], 40);
    assert_eq!(gen["maxOutputTokens"], 256);
    assert_eq!(gen["stopSequences"], json!(["\n\n"]));
    assert_eq!(gen["seed"], 42);
    assert_eq!(gen["responseMimeType"], "application/json");
    assert_eq!(gen["responseSchema"]["type"], "object");
    assert_eq!(
        gen["responseSchema"]["properties"]["answer"]["type"],
        "string"
    );
    assert!(gen["responseSchema"].get("$schema").is_none());
    assert!(gen["responseSchema"].get("additionalProperties").is_none());
    assert!(
        gen.get("candidateCount").is_none(),
        "n>1 is unsupported; candidateCount must not be sent: {gen}"
    );

    let serialized = body.to_string();
    for unknown in ["\"foo\"", "reasoning_effort", "top_k"] {
        assert!(
            !serialized.contains(unknown),
            "unknown/passthrough field `{unknown}` must not reach the wire: {serialized}"
        );
    }

    // max_completion_tokens is the fallback cap when max_tokens is absent.
    let req = ChatCompletionRequest {
        messages: vec![user_text("hi")],
        max_completion_tokens: Some(512),
        ..Default::default()
    };
    let body = chat_to_gemini_request(&req, "gemini-3.6-flash", None).unwrap();
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 512);
}

// ---------------------------------------------------------------------------
// Non-stream response
// ---------------------------------------------------------------------------

/// Shape captured from the live `generateContent` endpoint.
fn live_response() -> Value {
    json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "The answer " },
                    { "text": "is 42", "thought": true },
                    {
                        "functionCall": {
                            "name": "get_weather",
                            "args": { "city": "Paris" },
                            "id": "call_abc"
                        }
                    }
                ],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 8,
            "candidatesTokenCount": 5,
            "totalTokenCount": 393,
            "thoughtsTokenCount": 380,
            "cachedContentTokenCount": 0
        },
        "modelVersion": "gemini-3.6-flash-001",
        "responseId": "resp-123"
    })
}

#[test]
fn response_maps_text_thought_tool_and_usage() {
    let chat = gemini_to_chat_response(&live_response(), "gemini-3.6-flash");

    assert_eq!(chat["id"], "chatcmpl-resp-123");
    assert_eq!(chat["object"], "chat.completion");
    assert_eq!(chat["model"], "gemini-3.6-flash");
    assert!(chat["created"].as_u64().unwrap() > 0);
    assert_eq!(chat["choices"][0]["index"], 0);

    let message = &chat["choices"][0]["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["content"], "The answer ");
    assert_eq!(message["reasoning_content"], "is 42");
    assert_eq!(message["tool_calls"][0]["id"], "call_abc");
    assert_eq!(message["tool_calls"][0]["type"], "function");
    assert_eq!(message["tool_calls"][0]["function"]["name"], "get_weather");
    assert_eq!(
        message["tool_calls"][0]["function"]["arguments"], r#"{"city":"Paris"}"#,
        "arguments must be a JSON string, not a nested object"
    );
    assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");

    let usage = &chat["usage"];
    assert_eq!(usage["prompt_tokens"], 8);
    assert_eq!(
        usage["completion_tokens"], 385,
        "completion = candidatesTokenCount(5) + thoughtsTokenCount(380)"
    );
    assert_eq!(usage["total_tokens"], 393);
    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 0);
    assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 380);

    // The value must satisfy the route layer's deserialization contract.
    let parsed: ChatCompletionResponse =
        serde_json::from_value(chat).expect("must deserialize into ChatCompletionResponse");
    assert_eq!(
        parsed.choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );
    let calls = parsed.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(calls[0].id, "call_abc");
    assert_eq!(calls[0].function.name, "get_weather");
}

#[test]
fn response_keeps_text_only_answer_and_generates_missing_tool_id() {
    let resp = json!({
        "candidates": [{
            "content": { "parts": [{ "text": "plain answer" }], "role": "model" },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 3,
            "candidatesTokenCount": 2,
            "totalTokenCount": 5,
            "cachedContentTokenCount": 3
        },
        "responseId": "resp-xyz"
    });
    let chat = gemini_to_chat_response(&resp, "gemini-2.5-flash");

    assert_eq!(chat["choices"][0]["message"]["content"], "plain answer");
    assert!(chat["choices"][0]["message"]
        .get("reasoning_content")
        .is_none());
    assert!(chat["choices"][0]["message"].get("tool_calls").is_none());
    assert_eq!(chat["choices"][0]["finish_reason"], "stop");
    assert_eq!(chat["usage"]["completion_tokens"], 2);
    assert_eq!(chat["usage"]["total_tokens"], 5);
    assert_eq!(chat["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(
        chat["usage"]["completion_tokens_details"]["reasoning_tokens"],
        0
    );

    // A function call without `id` gets a synthesized one, and content is null.
    let resp = json!({
        "candidates": [{
            "content": {
                "parts": [{ "functionCall": { "name": "lookup", "args": { "q": "x" } } }],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "responseId": "resp-noid"
    });
    let chat = gemini_to_chat_response(&resp, "gemini-3.6-flash");
    assert_eq!(
        chat["choices"][0]["message"]["content"],
        Value::Null,
        "empty text + tool calls must yield content=null"
    );
    let id = chat["choices"][0]["message"]["tool_calls"][0]["id"]
        .as_str()
        .unwrap();
    assert!(
        id.starts_with("call_"),
        "synthesized id must be recognizable: {id}"
    );
    assert_eq!(
        chat["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        r#"{"q":"x"}"#
    );
    assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");
    // No usageMetadata → no usage object rather than a fabricated zero one.
    assert!(chat.get("usage").is_none());
}

#[test]
fn response_tolerates_antigravity_envelope_and_blocked_prompt() {
    // Defensive: an inner-Antigravity-shaped body still translates.
    let wrapped = json!({ "response": live_response() });
    let chat = gemini_to_chat_response(&wrapped, "gemini-3.6-flash");
    assert_eq!(chat["id"], "chatcmpl-resp-123");
    assert_eq!(chat["choices"][0]["message"]["content"], "The answer ");

    // Prompt-level block: no candidates, only promptFeedback.
    let blocked = json!({
        "promptFeedback": { "blockReason": "SAFETY" },
        "usageMetadata": { "promptTokenCount": 7, "totalTokenCount": 7 },
        "responseId": "resp-blocked"
    });
    let chat = gemini_to_chat_response(&blocked, "gemini-3.6-flash");
    assert_eq!(chat["choices"][0]["finish_reason"], "content_filter");
    assert_eq!(chat["choices"][0]["message"]["content"], "");
}

#[test]
fn finish_reason_mapping_is_exhaustive() {
    assert_eq!(
        map_gemini_finish_reason(Some("STOP"), false),
        FinishReason::Stop
    );
    assert_eq!(
        map_gemini_finish_reason(Some("STOP"), true),
        FinishReason::ToolCalls
    );
    assert_eq!(
        map_gemini_finish_reason(None, true),
        FinishReason::ToolCalls,
        "missing reason with tool calls is still a tool-call stop"
    );
    assert_eq!(map_gemini_finish_reason(None, false), FinishReason::Stop);
    assert_eq!(
        map_gemini_finish_reason(Some("MAX_TOKENS"), true),
        FinishReason::Length,
        "truncation must win over the tool-call promotion"
    );
    for reason in [
        "SAFETY",
        "RECITATION",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "SPII",
        "IMAGE_SAFETY",
    ] {
        assert_eq!(
            map_gemini_finish_reason(Some(reason), false),
            FinishReason::ContentFilter,
            "finishReason={reason}"
        );
    }
    assert_eq!(
        map_gemini_finish_reason(Some("MALFORMED_FUNCTION_CALL"), false),
        FinishReason::Stop
    );
    assert_eq!(
        map_gemini_finish_reason(Some("SOMETHING_NEW"), false),
        FinishReason::Stop,
        "unknown reasons degrade to stop"
    );

    let chat = gemini_to_chat_response(
        &json!({
            "candidates": [{
                "content": { "parts": [{ "text": "cut" }], "role": "model" },
                "finishReason": "MAX_TOKENS",
                "index": 0
            }],
            "responseId": "resp-cut"
        }),
        "gemini-3.6-flash",
    );
    assert_eq!(chat["choices"][0]["finish_reason"], "length");
}

// ---------------------------------------------------------------------------
// Streaming chunks
// ---------------------------------------------------------------------------

#[test]
fn chunk_maps_text_thought_and_usage() {
    let frame = json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "thinking...", "thought": true },
                    { "text": "Hello" }
                ],
                "role": "model"
            },
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 12,
            "candidatesTokenCount": 3,
            "totalTokenCount": 20,
            "thoughtsTokenCount": 5,
            "cachedContentTokenCount": 4
        },
        "responseId": "resp-stream-1"
    });

    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-resp-stream-1")
        .expect("frame with candidates must produce a chunk");

    assert_eq!(chunk.id, "chatcmpl-resp-stream-1");
    assert_eq!(chunk.object, "chat.completion.chunk");
    assert_eq!(chunk.model, "gemini-3.6-flash");
    assert!(chunk.created > 0);
    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].index, 0);
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
    assert_eq!(
        chunk.choices[0].delta.reasoning_content.as_deref(),
        Some("thinking...")
    );
    assert!(chunk.choices[0].delta.tool_calls.is_none());
    assert_eq!(
        chunk.choices[0].finish_reason, None,
        "no finishReason in the frame → no finish_reason yet"
    );

    let usage = chunk
        .usage
        .as_ref()
        .expect("usageMetadata must map to usage");
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 8, "3 candidates + 5 thoughts");
    assert_eq!(usage.total_tokens, 20);
    assert_eq!(
        usage.prompt_tokens_details.as_ref().unwrap().cached_tokens,
        Some(4)
    );
    assert_eq!(
        usage
            .completion_tokens_details
            .as_ref()
            .unwrap()
            .reasoning_tokens,
        Some(5)
    );

    // Downstream deserialization contract (SSE writer re-serializes this).
    let raw = serde_json::to_value(&chunk).unwrap();
    let back: ChatCompletionChunk = serde_json::from_value(raw).unwrap();
    assert_eq!(back.choices[0].delta.content.as_deref(), Some("Hello"));
}

#[test]
fn chunk_maps_parallel_function_calls_with_contiguous_indexes() {
    let frame = json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "functionCall": { "name": "a", "args": { "x": 1 }, "id": "call_1" } },
                    { "functionCall": { "name": "b", "args": { "y": 2 } } }
                ],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }]
    });

    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-s").unwrap();
    let calls = chunk.choices[0]
        .delta
        .tool_calls
        .as_ref()
        .expect("tool calls must be projected");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].index, 0);
    assert_eq!(calls[1].index, 1, "indexes must be contiguous from 0");
    assert_eq!(
        calls[0].id.as_deref(),
        Some("call_1"),
        "id must be preserved"
    );
    let synthesized = calls[1].id.as_deref().unwrap();
    assert!(synthesized.starts_with("call_"));
    assert_ne!(synthesized, "call_1");
    assert_eq!(calls[0].r#type.as_deref(), Some("function"));
    assert_eq!(
        calls[0].function.as_ref().unwrap().name.as_deref(),
        Some("a")
    );
    assert_eq!(
        calls[0].function.as_ref().unwrap().arguments.as_deref(),
        Some(r#"{"x":1}"#)
    );
    assert!(chunk.choices[0].delta.content.is_none());
    assert_eq!(
        chunk.choices[0].finish_reason,
        Some(FinishReason::ToolCalls),
        "STOP + function calls → tool_calls"
    );
    assert!(chunk.usage.is_none());
}

#[test]
fn chunk_handles_signature_only_terminal_frame_and_empty_parts() {
    // Observed last frame: empty text + thoughtSignature + finishReason, and
    // usageMetadata maintained per frame.
    let frame = json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": "", "thoughtSignature": "c2lnbmF0dXJl" }],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 8,
            "candidatesTokenCount": 5,
            "totalTokenCount": 393,
            "thoughtsTokenCount": 380
        },
        "modelVersion": "gemini-3.6-flash-001",
        "responseId": "resp-last"
    });

    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-resp-last")
        .expect("signature-only frame is still a valid chunk");
    assert!(chunk.choices[0].delta.content.is_none());
    assert!(chunk.choices[0].delta.reasoning_content.is_none());
    assert!(chunk.choices[0].delta.tool_calls.is_none());
    assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::Stop));
    let usage = chunk.usage.as_ref().unwrap();
    assert_eq!(usage.prompt_tokens, 8);
    assert_eq!(usage.completion_tokens, 385);
    assert_eq!(usage.total_tokens, 393);

    // A frame whose candidate carries `parts` directly (no `content` wrapper)
    // is tolerated too — the signature sentinel must not become content.
    let frame = json!({
        "candidates": [{
            "parts": [{ "text": "", "thoughtSignature": "c2lnbmF0dXJl" }],
            "finishReason": "STOP",
            "index": 0
        }]
    });
    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-sig").unwrap();
    assert!(chunk.choices[0].delta.content.is_none());
    assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::Stop));

    // Candidate with no content at all (finish still delivered).
    let frame = json!({ "candidates": [{ "finishReason": "MAX_TOKENS", "index": 0 }] });
    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-x").unwrap();
    assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::Length));
    assert!(chunk.choices[0].delta.content.is_none());
}

#[test]
fn chunk_returns_none_without_candidates() {
    let usage_only = json!({
        "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 2, "totalTokenCount": 3 }
    });
    assert!(
        gemini_chunk_to_chat_chunk(&usage_only, "gemini-3.6-flash", "id").is_none(),
        "usage-only frames must not synthesize an empty delta"
    );

    let feedback_only = json!({ "promptFeedback": { "blockReason": "SAFETY" } });
    assert!(gemini_chunk_to_chat_chunk(&feedback_only, "gemini-3.6-flash", "id").is_none());

    let empty_candidates = json!({ "candidates": [] });
    assert!(gemini_chunk_to_chat_chunk(&empty_candidates, "gemini-3.6-flash", "id").is_none());
}

#[test]
fn chunk_tolerates_antigravity_response_envelope() {
    let frame = json!({
        "response": {
            "candidates": [{
                "content": { "parts": [{ "text": "wrapped" }], "role": "model" },
                "index": 0
            }]
        }
    });
    let chunk = gemini_chunk_to_chat_chunk(&frame, "gemini-3.6-flash", "chatcmpl-w")
        .expect("envelope-wrapped frame must still translate");
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("wrapped"));
}

// ---------------------------------------------------------------------------
// Terminal chunk + chat id
// ---------------------------------------------------------------------------

#[test]
fn terminal_chunk_closes_stream_with_finish_and_usage() {
    let usage = json!({
        "promptTokenCount": 8,
        "candidatesTokenCount": 5,
        "totalTokenCount": 393,
        "thoughtsTokenCount": 380,
        "cachedContentTokenCount": 0
    });
    let chunk = gemini_terminal_chunk(
        "gemini-3.6-flash",
        "chatcmpl-resp-terminal",
        Some(usage),
        FinishReason::Length,
    );

    assert_eq!(chunk.id, "chatcmpl-resp-terminal");
    assert_eq!(chunk.object, "chat.completion.chunk");
    assert_eq!(chunk.model, "gemini-3.6-flash");
    assert!(chunk.created > 0);
    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].index, 0);
    assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::Length));
    assert!(chunk.choices[0].delta.content.is_none());
    assert!(chunk.choices[0].delta.reasoning_content.is_none());
    assert!(chunk.choices[0].delta.tool_calls.is_none());

    let usage = chunk.usage.as_ref().expect("usage must be forwarded");
    assert_eq!(usage.prompt_tokens, 8);
    assert_eq!(usage.completion_tokens, 385);
    assert_eq!(usage.total_tokens, 393);
    assert_eq!(
        usage
            .completion_tokens_details
            .as_ref()
            .unwrap()
            .reasoning_tokens,
        Some(380)
    );

    // No collected usage → terminal chunk still terminates cleanly.
    let chunk = gemini_terminal_chunk(
        "gemini-3.6-flash",
        "chatcmpl-resp-terminal",
        None,
        FinishReason::ContentFilter,
    );
    assert!(chunk.usage.is_none());
    assert_eq!(
        chunk.choices[0].finish_reason,
        Some(FinishReason::ContentFilter)
    );

    // Already OpenAI-shaped usage is accepted as-is (no double conversion).
    let openai_usage = json!({
        "prompt_tokens": 10,
        "completion_tokens": 2,
        "total_tokens": 12
    });
    let chunk = gemini_terminal_chunk(
        "gemini-3.6-flash",
        "chatcmpl-resp-terminal",
        Some(openai_usage),
        FinishReason::Stop,
    );
    let usage = chunk.usage.as_ref().unwrap();
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 2);
    assert_eq!(usage.total_tokens, 12);
}

#[test]
fn chat_id_is_prefixed_and_stable() {
    assert_eq!(gemini_chat_id(Some("resp-abc")), "chatcmpl-resp-abc");
    assert_eq!(gemini_chat_id(Some("  resp-abc  ")), "chatcmpl-resp-abc");

    let generated = gemini_chat_id(None);
    assert!(generated.starts_with("chatcmpl-"));
    assert!(generated.len() > "chatcmpl-".len());
    assert_ne!(generated, gemini_chat_id(None), "random ids must differ");

    let blank = gemini_chat_id(Some("   "));
    assert!(blank.starts_with("chatcmpl-"));
    assert_ne!(blank, "chatcmpl-");
}
