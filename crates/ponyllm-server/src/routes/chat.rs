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

    // Quota boundary (bugfix): a quota-exhaustion failure on one provider must
    // not silently drain a second provider that carries the same model, unless
    // the operator explicitly opts back into cross-provider quota failover.
    let quota_failover_enabled = state.config.read().cross_provider_quota_failover;

    for target in targets {
        // Stop before touching the next provider's quota when the previous
        // provider exhausted its account quota (402 / balance-wording 429 /
        // balance-wording 403 / antigravity quota frames / a pool cooled
        // entirely by quota). Transient faults still fail over normally.
        if !quota_failover_enabled && last_kind.is_quota_exhausted() {
            tracing::warn!(
                provider = %target.provider_name,
                "quota boundary stop: previous provider exhausted account quota; NOT failing over to '{}' to preserve its quota",
                target.provider_name
            );
            break;
        }
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
                let is_adaptive = ponyllm_protocol::anthropic::messages::ThinkingConfig::is_adaptive_model(&target.physical_model);
                if effective_thinking.is_active() {
                    ant_req.reasoning_effort = Some(effective_thinking);
                    if is_adaptive {
                        ant_req.thinking = Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                            r#type: "adaptive".to_string(),
                            budget_tokens: None,
                            effort: None,
                        });
                        ant_req.output_config = Some(ponyllm_protocol::anthropic::messages::AnthropicOutputConfig {
                            effort: Some(effective_thinking),
                        });
                    } else {
                        ant_req.thinking = Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                            r#type: "enabled".to_string(),
                            budget_tokens: None,
                            effort: Some(effective_thinking),
                        });
                        ant_req.output_config = None;
                    }
                } else {
                    ant_req.thinking = None;
                    ant_req.output_config = None;
                    ant_req.reasoning_effort = None;
                    ant_req.extra.remove("thinking");
                    ant_req.extra.remove("output_config");
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
            ponyllm_core::pool::UpstreamProtocol::Systemone => {
                last_error = format!("Systemone protocol cannot be served by chat endpoint for {}", target.provider_name);
                continue;
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
        // Resolve this target's effective short-window budget (provider default
        // merged with the model override) so budget-filtered key selection and
        // the window-exhaustion Retry-After are honest.
        // Resolve the budget under the CLEAN model name (matching pricing and
        // routing): a suffixed request (`deepseek-v4-flash[1m]:economy`) must
        // hit the same model-level rate_limits as the plain name, otherwise
        // the model-level budget silently never applies.
        let (rate_limits, ttfb_timeout) = {
            let cfg = state.config.read();
            let rl = cfg
                .providers
                .get(&target.provider_name)
                .and_then(|p| p.effective_rate_limits(&parsed.clean_model_name));
            let ttfb = cfg.effective_ttfb_timeout(&target.provider_name);
            (rl, ttfb)
        };
        let executor = UpstreamExecutor::with_client(pool.clone(), http_client, max_retries)
            .with_downstream_headers(&headers)
            .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
            .with_rate_limits(rate_limits)
            .with_ttfb_timeout(ttfb_timeout)
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));

        // Empty-STOP is an upstream transient unrelated to credential health
        // (fails pre-commit, fails fast): give it its own, larger budget so a
        // multi-second upstream blip cannot exhaust the generic retry budget.
        let max_empty_stop_attempts = executor
            .max_retries
            .max(pool.total_key_count())
            .max(MIN_EMPTY_STOP_ATTEMPTS);

        let mut collect_tried_keys: Vec<String> = Vec::new();
        if is_streaming {
            let current_executor = executor;
            let mut stream_attempt = 0;
            let max_stream_attempts = current_executor.max_retries.max(pool.total_key_count()).max(1);
            // R2: keys already tried by empty-STOP retries — fed back into
            // the executor so the next attempt selects a fresh key.
            let mut empty_stop_tried_keys: Vec<String> = Vec::new();
            // R2: mutable upstream envelope — refreshed with a new
            // requestId/trajectory per retry so each attempt is an
            // independent upstream trial (sessionId stays stable for KV cache).
            let mut attempt_req_val = req_val.clone();
            // R3: consecutive first-frame empty STOPs — a deterministic
            // prompt×model signature that must converge early instead of
            // burning the full 12-attempt budget.
            let mut consecutive_first_frame_stops: usize = 0;

            loop {
                stream_attempt += 1;
                // R2: rebuild the attempt executor with the tried-keys list.
                // `UpstreamExecutor` is cheap (Arc pool + client clone); the
                // event sink/observer is re-attached so telemetry is unchanged.
                let attempt_executor = UpstreamExecutor::with_client(
                    pool.clone(),
                    current_executor.client.clone(),
                    current_executor.max_retries,
                )
                .with_downstream_headers(&headers)
                .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
                .with_rate_limits(rate_limits)
                .with_ttfb_timeout(ttfb_timeout)
                .with_excluded_keys(&empty_stop_tried_keys)
                .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));
                match attempt_executor.execute_stream_request_with_timing_and_key(&target_url, &attempt_req_val).await {
                    Ok((upstream_resp, attempt_start, winning_key_id)) => {
                        // R2: this key is now consumed for empty-STOP
                        // purposes even if the preamble below succeeds.
                        if !empty_stop_tried_keys.iter().any(|k| k == &winning_key_id) {
                            empty_stop_tried_keys.push(winning_key_id.clone());
                        }
                        let raw_stream = stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE);

                        // For Antigravity upstream, verify preamble before committing downstream headers.
                        // If upstream emitted an immediate empty STOP, retry with another key.
                        // `first_frame_stop` feeds the R3 deterministic detector.
                        let mut first_frame_stop = false;
                        let (final_raw_stream, is_empty_stop_retry) = if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                            match verify_antigravity_stream_preamble(raw_stream, std::time::Duration::from_secs(10)).await {
                                Ok(AntigravityPreambleResult::Ready { buffered, tail }) => {
                                    let head_stream = futures_util::stream::iter(buffered.into_iter().map(Ok));
                                    let chained = head_stream.chain(tail);
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(chained);
                                    (boxed, false)
                                }
                                Ok(AntigravityPreambleResult::TransientEmptyStop { frames, shape }) => {
                                    // R1: one line must answer key / latency /
                                    // frame shape. `attempt_start` is the
                                    // winning attempt's dispatch instant;
                                    // elapsed ≈ single-attempt upstream cost.
                                    // R3: `frames == 0` means the terminal STOP
                                    // was the first significant frame.
                                    first_frame_stop = frames == 0;
                                    tracing::warn!(
                                        provider = %target.provider_name,
                                        key_id = %winning_key_id,
                                        frames,
                                        stream_attempt,
                                        max_stream_attempts,
                                        attempt_ms = attempt_start.elapsed().as_millis() as u64,
                                        finish_reason = ?shape.finish_reason,
                                        signature_only = shape.signature_only,
                                        had_usage = shape.had_usage,
                                        skipped_frames = shape.skipped_frames,
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
                            // R3: consecutive first-frame stops => the prompt×
                            // model deterministically yields zero content.
                            // Converge early and fail over to the next routed
                            // target instead of burning the full budget.
                            if first_frame_stop {
                                consecutive_first_frame_stops += 1;
                            } else {
                                consecutive_first_frame_stops = 0;
                            }
                            if consecutive_first_frame_stops >= crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD {
                                tracing::warn!(
                                    provider = %target.provider_name,
                                    attempts = stream_attempt,
                                    consecutive_first_frame_stops,
                                    "Antigravity deterministic empty STOP (first-frame, zero content); converging early to trigger failover"
                                );
                                last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                last_error = format!(
                                    "Antigravity deterministic empty STOP for model '{}' ({} consecutive first-frame zero-content STOPs across distinct keys; gateway converged early, try a different model or prompt)",
                                    target.physical_model, consecutive_first_frame_stops
                                );
                                break;
                            }
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
                                // R2: fresh upstream identity for the next
                                // attempt (sessionId preserved for KV cache).
                                if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                                    ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut attempt_req_val);
                                }
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
                            ponyllm_core::pool::UpstreamProtocol::Systemone => {
                                return crate::extractors::render_openai_error(
                                    StatusCode::BAD_REQUEST, "invalid_request_error",
                                    "protocol_mismatch", "Use /v1/systemone for systemone models",
                                );
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
                        // H1: pool entirely cooled by quota exhaustion reads
                        // as a quota boundary, not a transient no-key error.
                        if !quota_failover_enabled && crate::extractors::pool_quota_exhausted(&err, &pool) {
                            last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                        }
                        // R2: an empty-STOP retry loop that consumed every key
                        // surfaces NoAvailableKey from the executor, but pool
                        // keys are NOT cooling — the request simply tried them
                        // all. Report it as an upstream failure (failover),
                        // never as local pool exhaustion.
                        last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_)) && empty_stop_tried_keys.is_empty();
                        last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()));
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
                // R2: fresh key per attempt (excluded list) + fresh upstream
                // requestId per attempt. R3: deterministic early convergence
                // via the shared collect_empty_stop_policy.
                let mut collect_attempt = 0usize;
                let mut collect_req_val = req_val.clone();
                let mut collect_consecutive_first_frame: usize = 0;
                loop {
                    collect_attempt += 1;
                    let collect_executor = UpstreamExecutor::with_client(
                        pool.clone(),
                        executor.client.clone(),
                        executor.max_retries,
                    )
                    .with_downstream_headers(&headers)
                    .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
                    .with_rate_limits(rate_limits)
                    .with_ttfb_timeout(ttfb_timeout)
                    .with_excluded_keys(&collect_tried_keys)
                    .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));
                    match collect_executor.execute_stream_request_with_timing_and_key(&target_url, &collect_req_val).await {
                        Ok((resp, _instant, kid)) => {
                            if !collect_tried_keys.iter().any(|k| k == &kid) {
                                collect_tried_keys.push(kid.clone());
                            }
                            let raw_stream = resp.bytes_stream();
                            match collect_antigravity_sse_to_json(raw_stream).await {
                                Ok(v) => break (Ok(v), Some(kid)),
                                Err(e) if is_transient_empty_stop_error(&e) => {
                                    match collect_empty_stop_policy(&e, collect_attempt, max_empty_stop_attempts, &mut collect_consecutive_first_frame, &target.physical_model) {
                                        Some(CollectRetryAction::Retry { delay }) => {
                                            tracing::warn!(
                                                provider = %target.provider_name,
                                                key_id = %kid,
                                                error = %e,
                                                collect_attempt,
                                                max_empty_stop_attempts,
                                                backoff_ms = delay.as_millis() as u64,
                                                "Non-stream Antigravity collect hit transient empty STOP; backing off and retrying"
                                            );
                                            tokio::time::sleep(delay).await;
                                            ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut collect_req_val);
                                        }
                                        Some(CollectRetryAction::Deterministic { message }) => {
                                            tracing::warn!(
                                                provider = %target.provider_name,
                                                collect_attempt,
                                                collect_consecutive_first_frame,
                                                "Non-stream Antigravity collect hit deterministic empty STOP; converging early to trigger failover"
                                            );
                                            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                            last_error = message;
                                            break (Err(CoreError::Internal(last_error.clone())), Some(kid));
                                        }
                                        None => {
                                            tracing::warn!(
                                                provider = %target.provider_name,
                                                error = %e,
                                                "Antigravity stream collection failed"
                                            );
                                            break (Err(CoreError::Internal(format!("Antigravity stream collect failed: {}", e))), Some(kid));
                                        }
                                    }
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
                                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()));
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
                    // H1: pool entirely cooled by quota exhaustion reads as a
                    // quota boundary, not a transient no-key error.
                    if !quota_failover_enabled && crate::extractors::pool_quota_exhausted(&err, &pool) {
                        last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                    }
                    last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_)) && collect_tried_keys.is_empty();
                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()));
                    last_error = err.to_string();
                    continue;
                }
            }
        }
    }

    // All candidate providers exhausted
    // Correlate the client-visible error with the black-box frame: the
    // request_id is embedded in the message and exposed as a header, so
    // `ponyllm telemetry` output can be grepped for the failing request.
    let msg = crate::extractors::format_exhausted_message(&requested_raw_model, &last_kind, &last_error, last_pool_exhausted, &request_id);
    let mut resp = crate::extractors::project_openai_error(&last_kind, &msg);
    if let Some(secs) = last_retry_after {
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            resp.headers_mut().insert("retry-after", v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        resp.headers_mut().insert("x-ponyllm-request-id", v);
    }

    let latency = start_time.elapsed();
    state.emit(
        &ctx,
        None,
        GatewayEvent::RequestFailed {
            status_code: resp.status().as_u16(),
            latency_ms: latency.as_secs_f64() * 1000.0,
            error: last_error.clone(),
            request_snippet: last_req_snippet,
        },
    );
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

/// Honest pool-wide unlock hint for `Retry-After`: the LONGEST per-key
/// cooldown still outstanding, so the downstream client knows when the whole
/// pool can serve again rather than when the *first* key recovers.
///
/// The executor's transparent wait uses the earliest unlock (it holds the
/// request until a key frees); once that wait is exhausted and the gateway
/// answers 429, `max(unlock)` is the honest "pool as a whole" recovery time.
/// Still clamped at 60s by [`crate::extractors::retry_after_secs`].
///
/// Used ONLY for window-type failures (rate-limit / quota exhaustion) via
/// [`retry_unlock_hint`]: an unrelated 5xx with a cooling key must not inflate
/// `Retry-After` with the whole-pool estimate.
/// Window refill is folded in as `max(cooldown, longest_window_refill)` so a
/// window-exhausted pool (keys Active but at RPM/TPM budget, no cooldown)
/// still advertises an honest Retry-After.
pub(crate) fn pool_longest_unlock(
    pool: &ponyllm_core::pool::KeyPool,
    limits: Option<&ponyllm_core::pool::RateLimits>,
) -> Option<std::time::Duration> {
    // Longest per-key cooldown still outstanding (classic cooling recovery).
    let cooldown_max = pool
        .snapshot_keys()
        .iter()
        .filter_map(|k| k.cooldown_remaining())
        .max();
    // Longest wait until the WHOLE pool is schedulable again under the
    // budget (covers keys that are Active-but-window-exhausted and have no
    // cooldown). `0` = at least one key schedulable now (healthy), so it is
    // filtered out here to avoid advertising a spurious Retry-After on
    // non-exhausted failures.
    let window_max = pool
        .longest_window_refill_in_with_limits(limits)
        .filter(|d| !d.is_zero());
    match (cooldown_max, window_max) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Unlock hint feeding [`crate::extractors::retry_after_secs`], resolved by
/// the terminal failure kind:
///
/// - **Window-type failures** ([`GatewayErrorKind::RateLimitExceeded`] /
///   [`GatewayErrorKind::QuotaExhausted`]): the whole pool is budget- or
///   cooldown-blocked, so advertise the LONGEST unlock (`max` semantics via
///   [`pool_longest_unlock`]) — an honest "pool as a whole" recovery time.
/// - **Every other failure** (unrelated 5xx/502/upstream fault): a cooling
///   key must not inflate `Retry-After` for a failure that is not about the
///   quota, so keep the earliest unlock (`min` semantics via
///   `KeyPool::earliest_unlock`) and never leak the window-refill estimate.
///
/// Shared by the chat/messages/responses routes (single home in this module).
/// Outcome of one non-stream Antigravity collect retry round (R2/R3 shared
/// by the chat/messages/responses routes): whether to keep retrying, and the
/// terminal error when the loop must stop.
pub(crate) enum CollectRetryAction {
    /// Sleep already done by the caller contract — actually the delay is
    /// returned here so tests can assert without sleeping; routes sleep then
    /// `continue`.
    Retry { delay: std::time::Duration },
    /// Deterministic empty STOP: stop retrying, fail over with this message.
    Deterministic { message: String },
}

/// Shared R2/R3 policy for the non-stream Antigravity collect loops.
///
/// - Parses the collector error: transient empty STOP with `frames<=1` counts
///   toward the deterministic threshold; any other error (or a legacy string
///   without the trailer) is purely transient.
/// - `consecutive_first_frame_stops` is updated in place; reaching
///   [`crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD`] yields
///   [`CollectRetryAction::Deterministic`].
/// - Otherwise yields `Retry` while `attempt < max_attempts`, else `None`
///   (caller falls through to its terminal-error branch).
pub(crate) fn collect_empty_stop_policy(
    collector_error: &str,
    attempt: usize,
    max_attempts: usize,
    consecutive_first_frame_stops: &mut usize,
    physical_model: &str,
) -> Option<CollectRetryAction> {
    if !crate::streaming::is_transient_empty_stop_error(collector_error) || attempt >= max_attempts {
        return None;
    }
    // R3: only a first-frame stop (frames 0/1) is deterministic evidence. A
    // legacy string without the trailer parses to None => transient.
    let first_frame = crate::streaming::empty_stop_frame_count(collector_error)
        .map(|n| n <= 1)
        .unwrap_or(false);
    if first_frame {
        *consecutive_first_frame_stops += 1;
    } else {
        *consecutive_first_frame_stops = 0;
    }
    if *consecutive_first_frame_stops >= crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD {
        return Some(CollectRetryAction::Deterministic {
            message: format!(
                "Antigravity deterministic empty STOP for model '{}' ({} consecutive first-frame zero-content STOPs across distinct keys; gateway converged early, try a different model or prompt)",
                physical_model, *consecutive_first_frame_stops
            ),
        });
    }
    Some(CollectRetryAction::Retry {
        delay: crate::streaming::empty_stop_retry_delay(attempt),
    })
}

pub(crate) fn retry_unlock_hint(
    kind: &ponyllm_core::error::GatewayErrorKind,
    pool: &ponyllm_core::pool::KeyPool,
    limits: Option<&ponyllm_core::pool::RateLimits>,
) -> Option<std::time::Duration> {
    use ponyllm_core::error::GatewayErrorKind;
    match kind {
        GatewayErrorKind::RateLimitExceeded { .. } | GatewayErrorKind::QuotaExhausted => {
            pool_longest_unlock(pool, limits)
        }
        GatewayErrorKind::LockContention => {
            // Under cross-replica lock contention, the key is healthy (not cooled down),
            // so earliest_unlock() is None. The lock holder finishes refresh+persist in ~2-3s.
            // Providing a 2s hint (+1s ceiling in retry_after_secs = 3s) enables standard
            // client SDKs to successfully back off and retry.
            Some(std::time::Duration::from_secs(2))
        }
        _ => pool.earliest_unlock(),
    }
}

#[cfg(test)]
mod route_wait_tests {
    use super::*;
    use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RateLimits, RoutingStrategy};

    #[test]
    fn longest_unlock_takes_max_of_cooldown_and_window_refill() {
        // Healthy pool: no cooldown, no window exhaust -> None (no Retry-After).
        let healthy = KeyPool::new("p", RoutingStrategy::RoundRobin);
        healthy.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        assert_eq!(pool_longest_unlock(&healthy, None), None);

        // A cooling key dominates the window warning (cooldown decays in
        // real time, so assert a bounded range instead of an exact value).
        let cooling = KeyPool::new("p", RoutingStrategy::RoundRobin);
        cooling.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        cooling.set_key_cooldown("k1", std::time::Duration::from_secs(30));
        let cold = pool_longest_unlock(&cooling, None).expect("cooling pool reports a Retry-After");
        assert!(cold >= std::time::Duration::from_secs(29) && cold <= std::time::Duration::from_secs(30), "got {cold:?}");

        // An Active-but-window-exhausted pool (limits on) folds the longest
        // refill in so Retry-After is still honest.
        let windowed = KeyPool::new("p", RoutingStrategy::RoundRobin);
        windowed.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        windowed.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        let limits = RateLimits { rpm: Some(1), tpm: None, window_secs: Some(60), concurrency: None, count_cached: None };
        // Consume each key's whole RPM=1 budget so both are Active but
        // window-blocked (~until their 60s window ages out).
        for k in windowed.snapshot_keys() {
            k.meter().record_attempt(1);
        }
        let got = pool_longest_unlock(&windowed, Some(&limits));
        assert!(got.is_some(), "window-exhausted pool should advertise a Retry-After");
        assert!(!got.is_none());
    }

    #[test]
    fn retry_unlock_hint_uses_max_only_for_window_type_failures() {
        use ponyllm_core::error::GatewayErrorKind;
        // Window-exhausted pool (Active but at budget, no cooldown): the
        // window-type failure must advertise the refill estimate.
        let windowed = KeyPool::new("p", RoutingStrategy::RoundRobin);
        windowed.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        let limits = RateLimits { rpm: Some(1), tpm: None, window_secs: Some(60), concurrency: None, count_cached: None };
        windowed.snapshot_keys()[0].meter().record_attempt(1);
        let max_hint = retry_unlock_hint(
            &GatewayErrorKind::RateLimitExceeded { retry_after: None },
            &windowed,
            Some(&limits),
        );
        assert!(max_hint.is_some(), "window-type failure advertises the pool unlock");

        // Same window-exhausted pool, but an UNRELATED failure: the
        // window-refill estimate must NOT leak in (min semantics) — no
        // cooldown exists, so no Retry-After from the pool side.
        let min_hint = retry_unlock_hint(&GatewayErrorKind::Internal, &windowed, Some(&limits));
        assert_eq!(min_hint, None, "non-window failure must ignore window refill");

        // QuotaExhausted is also window-type: max semantics apply.
        let quota_hint = retry_unlock_hint(&GatewayErrorKind::QuotaExhausted, &windowed, Some(&limits));
        assert!(quota_hint.is_some(), "QuotaExhausted advertises the pool unlock");
    }

    #[test]
    fn retry_unlock_hint_keeps_earliest_for_unrelated_cooldown() {
        use ponyllm_core::error::GatewayErrorKind;
        // Two cooling keys (30s and 5s): an unrelated 5xx failure must
        // advertise the EARLIEST unlock (~5s), not the longest (30s).
        let pool = KeyPool::new("p", RoutingStrategy::RoundRobin);
        pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
        pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
        pool.set_key_cooldown("k1", std::time::Duration::from_secs(30));
        pool.set_key_cooldown("k2", std::time::Duration::from_secs(5));
        let hint = retry_unlock_hint(&GatewayErrorKind::UpstreamUnavailable, &pool, None)
            .expect("a cooling key still advertises an unlock");
        assert!(
            hint >= std::time::Duration::from_secs(4) && hint <= std::time::Duration::from_secs(5),
            "got {hint:?}"
        );
    }

    #[test]
    fn collect_empty_stop_policy_transient_then_deterministic() {
        // R3: first-frame stops ([frames=0/1]) count up; the Kth consecutive
        // one flips to Deterministic. A late stop (frames=5) resets.
        let mut consec = 0usize;
        let first = "Antigravity stream completed with zero text and zero tool calls (transient empty STOP) [frames=0]";
        let late = "Antigravity stream completed with zero text and zero tool calls (transient empty STOP) [frames=5]";

        for attempt in 1..crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD {
            match collect_empty_stop_policy(first, attempt, 12, &mut consec, "m") {
                Some(CollectRetryAction::Retry { .. }) => {}
                other => panic!("attempt {attempt} must stay Retry, got {}", other.is_some()),
            }
        }
        assert_eq!(consec, crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD - 1);
        match collect_empty_stop_policy(first, crate::streaming::DETERMINISTIC_EMPTY_STOP_THRESHOLD, 12, &mut consec, "gemini-3.8-flash-high") {
            Some(CollectRetryAction::Deterministic { message }) => {
                assert!(message.contains("deterministic"), "message: {message}");
                assert!(message.contains("gemini-3.8-flash-high"), "message: {message}");
            }
            _ => panic!("threshold attempt must converge"),
        }

        // Late-frame stop resets the streak and stays transient.
        let mut consec2 = 2usize;
        match collect_empty_stop_policy(late, 3, 12, &mut consec2, "m") {
            Some(CollectRetryAction::Retry { .. }) => {}
            _ => panic!("late stop must reset to Retry"),
        }
        assert_eq!(consec2, 0);

        // Legacy string without trailer: transient, never deterministic.
        let mut consec3 = 2usize;
        let legacy = "Antigravity stream completed with zero text and zero tool calls (transient empty STOP)";
        match collect_empty_stop_policy(legacy, 3, 12, &mut consec3, "m") {
            Some(CollectRetryAction::Retry { .. }) => {}
            _ => panic!("legacy string must stay Retry"),
        }
        assert_eq!(consec3, 0);

        // Budget exhausted or non-empty-stop error: None (caller terminal path).
        let mut consec4 = 0usize;
        assert!(collect_empty_stop_policy(first, 12, 12, &mut consec4, "m").is_none());
        assert!(collect_empty_stop_policy("boom", 1, 12, &mut consec4, "m").is_none());
    }
}
