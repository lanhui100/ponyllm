use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::{EventSinkCtx, UpstreamExecutor};
use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_protocol::translator::{anthropic_to_chat_request, chat_to_anthropic_response};
use parking_lot::Mutex;
use std::str::FromStr;
use crate::extractors::AppJson;
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{
    openai_sse_to_anthropic_stream, passthrough_sse, wrap_telemetry_stream,
    StreamFailureContext,
};
use ponyllm_protocol::anthropic::messages::{AnthropicSystem, AnthropicSystemBlock};

fn extract_anthropic_prompt(req: &MessageRequest) -> Option<String> {
    if let Some(sys) = &req.system {
        match sys {
            AnthropicSystem::Text(s) => return Some(s.clone()),
            AnthropicSystem::Blocks(blocks) => {
                let text = blocks.iter().map(|b| match b {
                    AnthropicSystemBlock::Text { text, .. } => text.as_str(),
                }).collect::<Vec<_>>().join("\n");
                return Some(text);
            }
        }
    }
    req.messages.first().map(|m| m.content.as_plain_text())
}

pub async fn handle_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AppJson(req): AppJson<MessageRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());
    let endpoint = "/v1/messages".to_string();
    let ctx = EventCtx {
        request_id: request_id.clone(),
        session_id: None,
        endpoint: endpoint.clone(),
        start: start_time,
    };
    let stages = Arc::new(Mutex::new(StageTimings::default()));

    // Client-side validation: empty messages are a client error, not an
    // upstream exhaustion (previously surfaced as 502 after hitting upstream).
    if req.messages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "messages must not be empty"
                }
            })),
        )
            .into_response();
    }

    // 1. Extract optional X-Pony-Strategy header
    let header_strategy = headers
        .get("x-pony-strategy")
        .or_else(|| headers.get("x-routing-strategy"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| GatewayRoutingStrategy::from_str(s).ok());

    // 2. Parse requested model with sanitization & auto/strategy/1m extraction
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();

    // 3. Resolve ranked target providers for multi-provider transparent failover (with hot cache probe)
    let prompt_hint = extract_anthropic_prompt(&req);
    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_with_prompt(&parsed, header_strategy, prompt_hint.as_deref()) {
        Ok(ts) if !ts.is_empty() => ts,
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "not_found_error",
                        "message": format!("model '{}' not found", req.model)
                    }
                })),
            )
                .into_response();
        }
        Err(err) => {
            let (status, err_type) = match err {
                CoreError::CapacityExhausted { .. } => (StatusCode::TOO_MANY_REQUESTS, "overloaded_error"),
                CoreError::Internal(ref msg) if msg.contains("No provider configured") => {
                    (StatusCode::NOT_FOUND, "not_found_error")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "api_error"),
            };
            return (
                status,
                Json(serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": err_type,
                        "message": err.to_string()
                    }
                })),
            )
                .into_response();
        }
    };

    let is_streaming = req.stream.unwrap_or(false);
    let mut last_error = String::new();
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;
    let mut last_req_snippet: Option<String> = None;

    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(targets[0].provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: !targets[0].is_anthropic_upstream,
            routing_ms,
        },
    );

    for target in targets {
        let pool = match state.get_pool(&target.provider_name) {
            Some(p) => p,
            None => continue,
        };

        let max_retries = state.config.read().max_retries;
        let is_anthropic_upstream = target.is_anthropic_upstream;

        let mut target_req = req.clone();
        target_req.model = target.physical_model.clone();

        let (target_url, req_val) = if is_anthropic_upstream {
            let url = crate::state::normalize_messages_url(&target.base_url);
            // Sanitize messages for strict Anthropic upstreams:
            // Extract any AnthropicRole::System messages into target_req.system,
            // and normalize Unknown roles to User so upstream never throws 400.
            let mut extracted_systems = Vec::new();
            let mut clean_messages = Vec::with_capacity(target_req.messages.len());
            for mut msg in target_req.messages {
                match msg.role {
                    ponyllm_protocol::anthropic::messages::AnthropicRole::System => {
                        extracted_systems.push(msg.content.as_plain_text());
                    }
                    ponyllm_protocol::anthropic::messages::AnthropicRole::Unknown => {
                        msg.role = ponyllm_protocol::anthropic::messages::AnthropicRole::User;
                        clean_messages.push(msg);
                    }
                    _ => clean_messages.push(msg),
                }
            }
            if !extracted_systems.is_empty() {
                let joined = extracted_systems.join("\n\n");
                target_req.system = match target_req.system {
                    Some(ponyllm_protocol::anthropic::messages::AnthropicSystem::Text(t)) => {
                        Some(ponyllm_protocol::anthropic::messages::AnthropicSystem::Text(format!("{}\n\n{}", t, joined)))
                    }
                    Some(ponyllm_protocol::anthropic::messages::AnthropicSystem::Blocks(mut blocks)) => {
                        blocks.push(ponyllm_protocol::anthropic::messages::AnthropicSystemBlock::Text {
                            text: joined,
                            cache_control: None,
                        });
                        Some(ponyllm_protocol::anthropic::messages::AnthropicSystem::Blocks(blocks))
                    }
                    None => Some(ponyllm_protocol::anthropic::messages::AnthropicSystem::Text(joined)),
                };
            }
            target_req.messages = clean_messages;

            let val = match serde_json::to_value(&target_req) {
                Ok(v) => v,
                Err(e) => {
                    last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            (url, val)
        } else {
            let url = crate::state::normalize_chat_completions_url(&target.base_url);
            let chat_req = match anthropic_to_chat_request(&target_req) {
                Ok(cr) => cr,
                Err(e) => {
                    last_error = format!("Translation error for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            let val = match serde_json::to_value(&chat_req) {
                Ok(v) => v,
                Err(e) => {
                    last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            (url, val)
        };

        let req_snippet = serde_json::to_string(&target_req).ok();
        last_req_snippet = req_snippet.clone();

        // Every per-key retry inside the executor reports through this observer,
        // so each failed attempt lands in the flight recorder with its own
        // status code, key id and upstream error body.
        let sink_ctx = EventSinkCtx {
            request_id: request_id.clone(),
            endpoint: endpoint.clone(),
            provider: target.provider_name.clone(),
            start: start_time,
            stages: stages.clone(),
            request_snippet: req_snippet.clone(),
        };
        let executor = UpstreamExecutor::new(pool, max_retries)
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx));

        if is_streaming {
            match executor.execute_stream_request(&target_url, &req_val).await {
                Ok(upstream_resp) => {
                    if let Some(p) = prompt_hint.as_deref() {
                        state.hot_cache.record_dispatch(p, &target.provider_name);
                    }
                    state.emit(
                        &ctx,
                        Some(target.provider_name.clone()),
                        GatewayEvent::StreamStarted {
                            request_snippet: req_snippet.clone(),
                        },
                    );

                    // Stream the raw upstream SSE body. For an OpenAI upstream,
                    // translate OpenAI chat chunks into Anthropic SSE events.
                    let raw_stream = upstream_resp.bytes_stream();
                    // Mid-stream failures (after this started event) are appended
                    // by the telemetry wrapper with the same request_id.
                    let failure_ctx = StreamFailureContext {
                        bus: state.event_bus.clone(),
                        ctx: ctx.clone(),
                        provider: target.provider_name.clone(),
                        stages: stages.clone(),
                        request_snippet: req_snippet.clone(),
                    };
                    let body = if is_anthropic_upstream {
                        let stream = passthrough_sse(raw_stream);
                        let monitored = wrap_telemetry_stream(stream, failure_ctx);
                        axum::body::Body::from_stream(monitored)
                    } else {
                        let stream = openai_sse_to_anthropic_stream(
                            raw_stream,
                            &target.physical_model,
                        );
                        let monitored = wrap_telemetry_stream(stream, failure_ctx);
                        axum::body::Body::from_stream(monitored)
                    };

                    let mut resp = axum::response::Response::new(body);
                    resp.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    inject_routing_headers(&mut resp, &target);
                    inject_telemetry_headers(&mut resp, &request_id, &stages);
                    return resp;
                }
                Err(err) => {
                    tracing::warn!("Provider '{}' stream failed ({}). Attempting fallback...", target.provider_name, err);
                    last_kind = err.kind();
                    last_error = err.to_string();
                    continue;
                }
            }
        } else {
            match executor.execute_json_request(&target_url, &req_val).await {
                Ok(resp_val) => {
                    let latency = start_time.elapsed();
                    let mut ant_resp: MessageResponse = if is_anthropic_upstream {
                        match serde_json::from_value(resp_val) {
                            Ok(ar) => ar,
                            Err(e) => {
                                last_error = format!("Invalid Anthropic response from {}: {}", target.provider_name, e);
                                continue;
                            }
                        }
                    } else {
                        let chat_resp: ChatCompletionResponse = match serde_json::from_value(resp_val) {
                            Ok(cr) => cr,
                            Err(e) => {
                                last_error = format!("Invalid response format from {}: {}", target.provider_name, e);
                                continue;
                            }
                        };

                        match chat_to_anthropic_response(&chat_resp) {
                            Ok(ar) => ar,
                            Err(e) => {
                                last_error = format!("Translation error: {}", e);
                                continue;
                            }
                        }
                    };

                    // Model Echo Rule: Strictly echo requested model name in response body
                    ant_resp.model = requested_raw_model.clone();

                    let cached_read = ant_resp.usage.cache_read_input_tokens.unwrap_or(0) as u64;
                    let cached_create = ant_resp.usage.cache_creation_input_tokens.unwrap_or(0) as u64;
                    let prompt_tokens = (ant_resp.usage.input_tokens as u64)
                        .saturating_add(cached_read)
                        .saturating_add(cached_create);
                    let completion_tokens = ant_resp.usage.output_tokens as u64;
                    let tps = if latency.as_secs_f64() > 0.05 && completion_tokens > 0 {
                        Some((completion_tokens as f64 / latency.as_secs_f64()).max(1.0))
                    } else {
                        None
                    };
                    // Non-streaming requests cannot observe true TTFT; pass None to avoid polluting TTFT EWMA
                    if let Some(p) = prompt_hint.as_deref() {
                        state.hot_cache.record_dispatch(p, &target.provider_name);
                    }
                    state.emit(
                        &ctx,
                        Some(target.provider_name.clone()),
                        GatewayEvent::RequestCompleted {
                            status_code: 200,
                            latency_ms: latency.as_secs_f64() * 1000.0,
                            prompt_tokens,
                            completion_tokens,
                            tps,
                            request_snippet: req_snippet,
                            response_snippet: serde_json::to_string(&ant_resp).ok(),
                        },
                    );

                    let mut response = (StatusCode::OK, Json(ant_resp)).into_response();
                    inject_routing_headers(&mut response, &target);
                    inject_telemetry_headers(&mut response, &request_id, &stages);
                    return response;
                }
                Err(err) => {
                    tracing::warn!("Provider '{}' json request failed ({}). Attempting fallback...", target.provider_name, err);
                    last_kind = err.kind();
                    last_error = err.to_string();
                    continue;
                }
            }
        }
    }

    // All candidate providers exhausted
    let latency = start_time.elapsed();
    state.emit(
        &ctx,
        None,
        GatewayEvent::RequestFailed {
            status_code: 502,
            latency_ms: latency.as_secs_f64() * 1000.0,
            error: last_error.clone(),
            request_snippet: last_req_snippet,
        },
    );

    // Correlate the client-visible error with the black-box frame: the
    // request_id is embedded in the message and exposed as a header, so
    // `ponyllm telemetry` output can be grepped for the failing request.
    let msg = crate::extractors::format_exhausted_message(&last_error, &request_id);
    let mut resp = crate::extractors::project_anthropic_error(&last_kind, &msg);
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        resp.headers_mut().insert("x-ponyllm-request-id", v);
    }
    resp
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos)
}
