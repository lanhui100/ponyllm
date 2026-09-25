use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::StreamExt;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::{is_opencode_zen_target, EventSinkCtx, UpstreamExecutor};
use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};
use ponyllm_protocol::anthropic::messages::MessageResponse;
use ponyllm_protocol::openai::chat::ChatCompletionRequest;
use ponyllm_protocol::translator::{
    anthropic_to_chat_response, antigravity_to_chat_response,
    chat_to_antigravity_request, chat_to_anthropic_request,
    chat_to_responses_request, responses_to_chat_response,
};
use parking_lot::Mutex;
use std::str::FromStr;
use crate::extractors::{format_request_snippet, AppJson};
use crate::routes::models::ParsedRequestModel;
use crate::state::{AppState, RoutedTarget};
use crate::streaming::{
    antigravity_sse_to_openai_stream, anthropic_sse_to_openai_stream,
    collect_antigravity_sse_to_json, empty_stop_retry_delay, is_transient_empty_stop_error,
    verify_antigravity_stream_preamble, AntigravityPreambleResult, MIN_EMPTY_STOP_ATTEMPTS,
    extract_usage_tokens, passthrough_sse, responses_sse_to_chat_stream,
    stall_guard, wrap_telemetry_stream, StreamFailureContext,
    DEFAULT_TAIL_STALL_IDLE,
};

use ponyllm_protocol::openai::chat::ChatMessage;

fn extract_chat_prompt(msg: &ChatMessage) -> Option<String> {
    match msg {
        ChatMessage::System(m) => Some(m.content.as_plain_text()),
        ChatMessage::User(m) => Some(m.content.as_plain_text()),
        ChatMessage::Developer(m) => Some(m.content.as_plain_text()),
        ChatMessage::Assistant(m) => m.content.as_ref().map(|c| c.as_plain_text()),
        ChatMessage::Tool(m) => Some(m.content.as_plain_text()),
        ChatMessage::Function(m) => m.content.clone(),
    }
}

pub async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AppJson(req): AppJson<ChatCompletionRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());
    let endpoint = "/v1/chat/completions".to_string();
    let ctx = EventCtx {
        request_id: request_id.clone(),
        session_id: None,
        model: Some(req.model.clone()),
        endpoint: endpoint.clone(),
        start: start_time,
    };
    let stages = Arc::new(Mutex::new(StageTimings::default()));

    // Client-side validation: empty messages are a client error, not an
    // upstream failure. Return standard 400 Bad Request immediately.
    if req.messages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "messages must not be empty",
                    "type": "invalid_request_error",
                    "code": "invalid_input"
                }
            })),
        )
            .into_response();
    }

    // 1. Extract optional X-Pony-Strategy header and X-Pony-Thinking header
    let header_strategy = headers
        .get("x-pony-strategy")
        .or_else(|| headers.get("x-routing-strategy"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| GatewayRoutingStrategy::from_str(s).ok());
    let header_thinking = crate::extractors::parse_thinking_header(&headers);


    // 2. Parse requested model with sanitization & auto/strategy/1m extraction
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();

    // 3. Resolve ranked target providers for multi-provider transparent failover (with hot cache probe)
    let prompt_hint = req.messages.first().and_then(extract_chat_prompt);
    let required_modalities = req.required_modalities();
    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_full(
        &parsed,
        header_strategy,
        prompt_hint.as_deref(),
        crate::extractors::parse_protocol_header(&headers),
        Some(ponyllm_core::pool::UpstreamProtocol::Chat),
        &required_modalities,
    ) {
        Ok(ts) if !ts.is_empty() => ts,
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("The model '{}' does not exist or you do not have access to it.", req.model),
                        "type": "invalid_request_error",
                        "code": "model_not_found"
                    }
                })),
            )
                .into_response();
        }
        Err(err) => {
            let (status, code) = match err {
                CoreError::UnsupportedModality { .. } => (StatusCode::BAD_REQUEST, "unsupported_modality"),
                CoreError::CapacityExhausted { .. } => (StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted"),
                CoreError::Internal(ref msg) if msg.contains("No provider configured") => {
                    (StatusCode::NOT_FOUND, "model_not_found")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
            };
            return (
                status,
                Json(serde_json::json!({
                    "error": {
                        "message": err.to_string(),
                        "type": "invalid_request_error",
                        "code": code
                    }
                })),
            )
                .into_response();
        }
    };

    if !parsed.is_auto {
        for modality in &required_modalities {
            if !targets.iter().any(|t| t.supports_modality(modality)) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!(
                                "Model '{}' does not support modality '{}'. Supported input modalities: {:?}",
                                targets[0].physical_model, modality, targets[0].input_types
                            ),
                            "type": "invalid_request_error",
                            "code": "unsupported_modality"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    let is_streaming = req.stream.unwrap_or(false);
    let mut last_error = String::new();
    let mut last_pool_exhausted = false;
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;
    let mut last_retry_after: Option<u64> = None;
    let mut last_req_snippet: Option<String> = None;

    // Journey start: routing cost is the first attributable segment.
    // (Pre-routing validation rejections stay silent, as before.)
    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    let first_provider = targets[0].provider_name.clone();
    let first_translated = targets[0].upstream_protocol != ponyllm_core::pool::UpstreamProtocol::Chat;
    state.emit(
        &ctx,
        Some(first_provider),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: first_translated,
            routing_ms,
        },
    );

    for target in targets {
        let pool = match state.get_pool(&target.provider_name) {
            Some(p) => p,
            None => continue,
        };

        let max_retries = state.config.read().max_retries;

        let mut target_req = req.clone();
        target_req.model = target.physical_model.clone();

        // Model-level default sampling: request value wins when present.
        if target_req.temperature.is_none() {
            target_req.temperature = target.temperature;
        }
        if target_req.top_p.is_none() {
            target_req.top_p = target.top_p;
        }

        let requested_thinking = header_thinking
            .or(parsed.thinking_override)
            .or_else(|| req.get_reasoning_effort());
        let effective_thinking = target.resolve_thinking(requested_thinking);

        if effective_thinking.is_active() {
            target_req.reasoning_effort = Some(effective_thinking);
        } else {
            target_req.reasoning_effort = None;
            target_req.extra.remove("reasoning_effort");
            target_req.extra.remove("thinking");
        }

        // Apply thinking-aware output token safeguard:
        // 1. Ensures max_tokens is floored to safe minimum if thinking is active to prevent zero-content choking.
        // 2. Clamps against model's declared max_output.
        target_req.max_tokens = ponyllm_core::pool::apply_thinking_output_safeguard(
            target_req.max_tokens,
            &target.max_output,
            effective_thinking,
        );
        if let Some(ref mut mt) = target_req.max_completion_tokens {
            *mt = ponyllm_core::pool::apply_thinking_output_safeguard(
                Some(*mt),
                &target.max_output,
                effective_thinking,
            ).unwrap_or(*mt);
        }

        let (target_url, req_val) = match target.upstream_protocol {
            ponyllm_core::pool::UpstreamProtocol::Responses => {
                let url = target.responses_url();
                let mut resp_req = match chat_to_responses_request(&target_req) {
                    Ok(rr) => rr,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                if effective_thinking.is_active() {
                    resp_req.reasoning_effort = Some(effective_thinking);
                    resp_req.reasoning = Some(ponyllm_protocol::openai::responses::ResponseReasoningConfig {
                        effort: Some(effective_thinking),
                    });
                    resp_req.sanitize_thinking_extra();
                } else {
                    resp_req.reasoning_effort = None;
                    resp_req.reasoning = None;
                    resp_req.sanitize_thinking_extra();
                    resp_req.extra.remove("reasoning");
                }
                let val = match serde_json::to_value(&resp_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                let url = target.messages_url();
                let mut ant_req = match chat_to_anthropic_request(&target_req) {
                    Ok(ar) => ar,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                if effective_thinking.is_active() {
                    ant_req.reasoning_effort = Some(effective_thinking);
                    ant_req.thinking = Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                        r#type: "enabled".to_string(),
                        budget_tokens: None,
                        effort: Some(effective_thinking),
                    });
                } else {
                    ant_req.thinking = None;
                    ant_req.reasoning_effort = None;
                    ant_req.extra.remove("thinking");
                    ant_req.extra.remove("reasoning_effort");
                }
                let val = match serde_json::to_value(&ant_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Chat => {
                let url = target.chat_completions_url();
                let val = match serde_json::to_value(&target_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Invalid JSON for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                let url = target.antigravity_url(is_streaming);
                // Explicit-only thinking passthrough: ceiling-enforced effort
                // reaches the translator solely when the caller asked for it
                // (header / model suffix / body); otherwise the legacy wire
                // shape is preserved.
                let thinking = requested_thinking.map(|_| effective_thinking);
                // Per-credential project: hardcoding one project bills every
                // credential to the same account and links them on the
                // risk-control side (P0-6). The key id salts the session
                // hash so identical prompts don't cluster (B7).
                let (ag_project, ag_salt) = state
                    .peek_antigravity_identity(&target.provider_name)
                    .unwrap_or_else(|| ("aicode-consumers".to_string(), String::new()));
                let val = match chat_to_antigravity_request(&target_req, &target.physical_model, &ag_project, thinking, &ag_salt) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (url, val)
            }
        };


        let req_snippet = Some(format_request_snippet(&req_val));
        last_req_snippet = req_snippet.clone();

        // Every per-key retry inside the executor appends attempt events to
        // the bus; metrics and frames derive from the same single-append log.
        let sink_ctx = EventSinkCtx {
            request_id: request_id.clone(),
            endpoint: endpoint.clone(),
            provider: target.provider_name.clone(),
            model: Some(requested_raw_model.clone()),
            start: start_time,
            stages: stages.clone(),
            request_snippet: req_snippet.clone(),
        };
        let http_client = state.http_client_for_target(&target.provider_name, &target.physical_model);
        let executor = UpstreamExecutor::with_client(pool.clone(), http_client, max_retries)
            .with_downstream_headers(&headers)
            .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx));

        // Empty-STOP is an upstream transient unrelated to credential health
        // (fails pre-commit, fails fast): give it its own, larger budget so a
        // multi-second upstream blip cannot exhaust the generic retry budget.
        let max_empty_stop_attempts = executor
            .max_retries
            .max(pool.total_key_count())
            .max(MIN_EMPTY_STOP_ATTEMPTS);

        if is_streaming {
            let current_executor = executor;
            let mut stream_attempt = 0;
            let max_stream_attempts = current_executor.max_retries.max(pool.total_key_count()).max(1);

            loop {
                stream_attempt += 1;
                match current_executor.execute_stream_request_with_timing_and_key(&target_url, &req_val).await {
                    Ok((upstream_resp, attempt_start, winning_key_id)) => {
                        let raw_stream = stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE);

                        // For Antigravity upstream, verify preamble before committing downstream headers.
                        // If upstream emitted an immediate empty STOP, retry with another key.
                        let (final_raw_stream, is_empty_stop_retry) = if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                            match verify_antigravity_stream_preamble(raw_stream, std::time::Duration::from_secs(10)).await {
                                Ok(AntigravityPreambleResult::Ready { buffered, tail }) => {
                                    let head_stream = futures_util::stream::iter(buffered.into_iter().map(Ok));
                                    let chained = head_stream.chain(tail);
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(chained);
                                    (boxed, false)
                                }
                                Ok(AntigravityPreambleResult::TransientEmptyStop { frames }) => {
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        frames,
                                        stream_attempt,
                                        max_stream_attempts,
                                        "Antigravity stream preamble completed with empty STOP! Triggering transparent gateway retry."
                                    );
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(futures_util::stream::empty());
                                    (boxed, true)
                                }
                                Ok(AntigravityPreambleResult::DeterministicBlock { reason }) => {
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        reason = %reason,
                                        "Antigravity prompt blocked by upstream safety during preamble"
                                    );
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(futures_util::stream::empty());
                                    (boxed, false)
                                }
                                Ok(AntigravityPreambleResult::AbruptTermination) => {
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        "Antigravity stream preamble terminated abruptly before content"
                                    );
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(futures_util::stream::empty());
                                    (boxed, true)
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        error = %e,
                                        "Antigravity stream preamble transport error"
                                    );
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(futures_util::stream::empty());
                                    (boxed, true)
                                }
                            }
                        } else {
                            let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(raw_stream);
                            (boxed, false)
                        };

                        if is_empty_stop_retry {
                            if stream_attempt < max_empty_stop_attempts {
                                let delay = empty_stop_retry_delay(stream_attempt);
                                tracing::warn!(
                                    provider = %target.provider_name,
                                    stream_attempt,
                                    max_empty_stop_attempts,
                                    backoff_ms = delay.as_millis() as u64,
                                    "Antigravity empty-STOP before commit; backing off and retrying transparently"
                                );
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                            tracing::warn!(
                                provider = %target.provider_name,
                                attempts = stream_attempt,
                                "Antigravity empty-STOP persisted across all transparent retries; failing attempt to trigger failover or standard error"
                            );
                            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                            last_error = format!(
                                "Antigravity stream preamble returned empty STOP across all {} attempts",
                                stream_attempt
                            );
                            break;
                        }

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

                        // Stream the upstream SSE body. Mismatched native protocols
                        // are translated into OpenAI chat chunks.
                        let est_prompt_tokens = serde_json::to_string(&req.messages).map(|s| (s.len() as u64 / 4).max(1)).unwrap_or(1);

                        // Mid-stream failures (after this started event) are appended
                        // by the telemetry wrapper with the same request_id.
                        let failure_ctx = StreamFailureContext {
                            bus: state.event_bus.clone(),
                            ctx: ctx.clone(),
                            provider: target.provider_name.clone(),
                            stages: stages.clone(),
                            request_snippet: req_snippet.clone(),
                            estimated_prompt_tokens: est_prompt_tokens,
                            attempt_start: Some(attempt_start),
                            key_pool: Some(pool.clone()),
                            key_id: Some(winning_key_id),
                        };
                        let body = match target.upstream_protocol {
                            ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                                let stream = anthropic_sse_to_openai_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Responses => {
                                let stream = responses_sse_to_chat_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Chat => {
                                let stream = passthrough_sse(final_raw_stream);
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                                let stream = antigravity_sse_to_openai_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
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
                        last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                        last_retry_after = crate::extractors::retry_after_secs(&last_kind, pool.earliest_unlock());
                        last_error = err.to_string();
                        break;
                    }
                }
            }
        } else {
            let (upstream_result, winning_key_id) = if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                tracing::debug!(
                    provider = %target.provider_name,
                    target_url = %target_url,
                    physical_model = %target.physical_model,
                    "Dispatching non-stream Antigravity request via stream collector"
                );
                // Transparent same-target retry: the stream collector reports an
                // upstream transient empty STOP as an error string; failover alone
                // would 502 a single-provider setup for a blithe upstream blip.
                let mut collect_attempt = 0usize;
                loop {
                    collect_attempt += 1;
                    match executor.execute_stream_request_with_timing_and_key(&target_url, &req_val).await {
                        Ok((resp, _instant, kid)) => {
                            let raw_stream = resp.bytes_stream();
                            match collect_antigravity_sse_to_json(raw_stream).await {
                                Ok(v) => break (Ok(v), Some(kid)),
                                Err(e) if is_transient_empty_stop_error(&e) && collect_attempt < max_empty_stop_attempts => {
                                    let delay = empty_stop_retry_delay(collect_attempt);
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        error = %e,
                                        collect_attempt,
                                        max_empty_stop_attempts,
                                        backoff_ms = delay.as_millis() as u64,
                                        "Non-stream Antigravity collect hit transient empty STOP; backing off and retrying"
                                    );
                                    tokio::time::sleep(delay).await;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        error = %e,
                                        "Antigravity stream collection failed"
                                    );
                                    break (Err(CoreError::Internal(format!("Antigravity stream collect failed: {}", e))), Some(kid));
                                }
                            }
                        }
                        Err(e) => break (Err(e), None),
                    }
                }
            } else if ponyllm_core::executor::zen_free_tier_forces_upstream_stream(
                &target.provider_name,
                &target_url,
                &target.physical_model,
            ) && matches!(
                target.upstream_protocol,
                ponyllm_core::pool::UpstreamProtocol::Chat | ponyllm_core::pool::UpstreamProtocol::Responses
            ) {
                // Zen free tier: the Console gate rejects non-stream upstream
                // bodies even with the tool gate satisfied. Force an upstream
                // stream and aggregate to the JSON shape the downstream asked
                // for (mirrors the Antigravity stream-collector pattern).
                let mut streamed_val = req_val.clone();
                if let Some(obj) = streamed_val.as_object_mut() {
                    obj.insert("stream".to_string(), serde_json::Value::Bool(true));
                    if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Chat {
                        obj.entry("stream_options".to_string())
                            .or_insert_with(|| serde_json::json!({"include_usage": true}));
                    }
                }
                match executor.execute_stream_request_with_timing_and_key(&target_url, &streamed_val).await {
                    Ok((resp, _instant, kid)) => {
                        let raw_stream = resp.bytes_stream();
                        let collected = match target.upstream_protocol {
                            ponyllm_core::pool::UpstreamProtocol::Responses => {
                                crate::streaming::collect_responses_sse_to_json(raw_stream).await
                            }
                            _ => crate::streaming::collect_chat_sse_to_json(raw_stream).await,
                        };
                        match collected {
                            Ok(v) => (Ok(v), Some(kid)),
                            Err(e) => {
                                tracing::warn!(
                                    provider = %target.provider_name,
                                    error = %e,
                                    "Zen free-tier upstream stream collection failed"
                                );
                                (Err(CoreError::Internal(format!("Zen stream collect failed: {}", e))), Some(kid))
                            }
                        }
                    }
                    Err(e) => (Err(e), None),
                }
            } else {
                match executor.execute_json_request_with_key(&target_url, &req_val).await {
                    Ok((val, kid)) => (Ok(val), Some(kid)),
                    Err(e) => (Err(e), None),
                }
            };

            match (upstream_result, winning_key_id) {
                (Ok(resp_val), winning_key_id) => {
                    let latency = start_time.elapsed();
                    let mut final_val = match target.upstream_protocol {
                        ponyllm_core::pool::UpstreamProtocol::Responses => {
                            let resp_obj: ponyllm_protocol::openai::responses::ResponseObject =
                                match serde_json::from_value(resp_val) {
                                    Ok(ro) => ro,
                                    Err(e) => {
                                        last_error = format!("Invalid Responses object from {}: {}", target.provider_name, e);
                                        continue;
                                    }
                                };
                            // Upstream Responses failure (e.g. status=failed with an
                            // upstream code/message) must fail over, not fall
                            // through as a client error: project it as an
                            // upstream transport fault (503, retryable) and try
                            // the next routed target.
                            let chat_resp = match responses_to_chat_response(&resp_obj) {
                                Ok(cr) => cr,
                                Err(e) => {
                                    last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, pool.earliest_unlock());
                                    last_error = format!("Upstream {} response failed: {}", target.provider_name, e);
                                    continue;
                                }
                            };
                            match serde_json::to_value(&chat_resp) {
                                Ok(v) => v,
                                Err(e) => {
                                    last_error = format!("Serialization error: {}", e);
                                    continue;
                                }
                            }
                        }
                        ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                            let ant_resp: MessageResponse = match serde_json::from_value(resp_val) {
                                Ok(ar) => ar,
                                Err(e) => {
                                    last_error = format!("Invalid Anthropic response from {}: {}", target.provider_name, e);
                                    continue;
                                }
                            };
                            let chat_resp = match anthropic_to_chat_response(&ant_resp) {
                                Ok(cr) => cr,
                                Err(e) => {
                                    last_error = format!("Translation error: {}", e);
                                    continue;
                                }
                            };
                            match serde_json::to_value(&chat_resp) {
                                Ok(v) => v,
                                Err(e) => {
                                    last_error = format!("Serialization error: {}", e);
                                    continue;
                                }
                            }
                        }
                        ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                            antigravity_to_chat_response(&resp_val, &target.physical_model)
                        }
                        _ => resp_val,
                    };

                    // Model Echo Rule: Strictly echo requested model name in response body
                    if let Some(obj) = final_val.as_object_mut() {
                        obj.insert("model".to_string(), serde_json::json!(requested_raw_model));
                    }

                    let (prompt_tokens, completion_tokens, cached_tokens) = extract_usage_tokens(&final_val);
                    if let Some(kid) = winning_key_id.as_deref() {
                        let wall_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        pool.record_tokens(kid, wall_ms, prompt_tokens, completion_tokens, cached_tokens);
                    }
                    let tps = if latency.as_secs_f64() > 0.05 && completion_tokens > 0 {
                        Some((completion_tokens as f64 / latency.as_secs_f64()).max(1.0))
                    } else {
                        None
                    };
                    // Non-streaming requests cannot observe true TTFT; pass None to avoid polluting TTFT EWMA
                    let tps_for_event = tps;
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
                            cached_tokens,
                            tps: tps_for_event,
                            request_snippet: req_snippet,
                            response_snippet: Some(final_val.to_string()),
                        },
                    );

                    let mut response = (StatusCode::OK, Json(final_val)).into_response();
                    inject_routing_headers(&mut response, &target);
                    inject_telemetry_headers(&mut response, &request_id, &stages);
                    return response;
                }
                (Err(err), _) => {
                    tracing::warn!("Provider '{}' json request failed ({}). Attempting fallback...", target.provider_name, err);
                    last_kind = err.kind();
                    last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, pool.earliest_unlock());
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
    let msg = crate::extractors::format_exhausted_message(&requested_raw_model, &last_error, last_pool_exhausted, &request_id);
    let mut resp = crate::extractors::project_openai_error(&last_kind, &msg);
    if let Some(secs) = last_retry_after {
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            resp.headers_mut().insert("retry-after", v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        resp.headers_mut().insert("x-ponyllm-request-id", v);
    }
    resp
}

/// Client-auditable trace headers on every response (not only errors):
/// the request id stitches the server-side event journey, and `Server-Timing`
/// exposes the attributable pre-stream segments so external bench harnesses
/// can split routing vs upstream time without log access.
pub fn inject_telemetry_headers(
    response: &mut axum::response::Response,
    request_id: &str,
    stages: &Arc<Mutex<StageTimings>>,
) {
    if let Ok(v) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-ponyllm-request-id", v);
    }
    let st = stages.lock();
    let mut parts = Vec::new();
    if let Some(r) = st.routing_ms {
        parts.push(format!("routing;dur={:.1}", r));
    }
    if let Some(t) = st.upstream_ttfb_ms {
        parts.push(format!("upstream-ttfb;dur={:.1}", t));
    }
    if let Some(ttft) = st.upstream_ttft_ms {
        parts.push(format!("upstream-ttft;dur={:.1}", ttft));
    }
    if let Some(d_ttft) = st.downstream_ttft_ms {
        parts.push(format!("downstream-ttft;dur={:.1}", d_ttft));
    }
    if !parts.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&parts.join(", ")) {
            response.headers_mut().insert("server-timing", v);
        }
    }
}

pub fn inject_routing_headers(response: &mut axum::response::Response, target: &RoutedTarget) {
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&target.physical_model) {
        headers.insert("x-ponyllm-routed-model", v);
    }
    if let Ok(v) = HeaderValue::from_str(&target.provider_name) {
        headers.insert("x-ponyllm-provider", v);
    }
    if let Ok(v) = HeaderValue::from_str(&target.upstream_protocol.to_string()) {
        headers.insert("x-ponyllm-protocol", v);
    }
    if let Ok(v) = HeaderValue::from_str(&target.strategy.to_string()) {
        headers.insert("x-ponyllm-strategy", v);
    }
    if let Ok(v) = HeaderValue::from_str(target.tier.shorthand()) {
        headers.insert("x-ponyllm-tier", v);
    }
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos)
}
