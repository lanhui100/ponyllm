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
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_protocol::translator::{
    anthropic_to_chat_request, anthropic_to_responses_request,
    antigravity_to_messages_response, chat_to_anthropic_response,
    messages_to_antigravity_request, responses_to_anthropic_response,
};
use parking_lot::Mutex;
use std::str::FromStr;
use crate::extractors::{format_request_snippet, AppJson};
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers, retry_unlock_hint};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{
    antigravity_sse_to_anthropic_stream, collect_antigravity_sse_to_json,
    empty_stop_retry_delay, is_transient_empty_stop_error,
    openai_sse_to_anthropic_stream, passthrough_sse,
    responses_sse_to_anthropic_stream, stall_guard,
    verify_antigravity_stream_preamble_with_deadline,
    wrap_telemetry_stream, AntigravityPreambleResult, StreamFailureContext,
    DEFAULT_PREAMBLE_DEADLINE, DEFAULT_TAIL_STALL_IDLE,
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
        model: Some(req.model.clone()),
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

    let header_thinking = crate::extractors::parse_thinking_header(&headers);

    // 2. Parse requested model with sanitization & auto/strategy/1m extraction
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();

    // 3. Resolve ranked target providers for multi-provider transparent failover (with hot cache probe)
    let prompt_hint = extract_anthropic_prompt(&req);
    let required_modalities = req.required_modalities();
    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_full(
        &parsed,
        header_strategy,
        prompt_hint.as_deref(),
        crate::extractors::parse_protocol_header(&headers),
        Some(ponyllm_core::pool::UpstreamProtocol::Anthropic),
        &required_modalities,
    ) {
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
                CoreError::UnsupportedModality { .. } => (StatusCode::BAD_REQUEST, "invalid_request_error"),
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

    if !parsed.is_auto {
        for modality in &required_modalities {
            if !targets.iter().any(|t| t.supports_modality(modality)) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": format!(
                                "Model '{}' does not support modality '{}'. Supported input modalities: {:?}",
                                targets[0].physical_model, modality, targets[0].input_types
                            )
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

    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(targets[0].provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: targets[0].upstream_protocol != ponyllm_core::pool::UpstreamProtocol::Anthropic,
            routing_ms,
        },
    );

    // Quota boundary (bugfix): a quota-exhaustion failure on one provider must
    // not silently drain a second provider that carries the same model, unless
    // the operator explicitly opts back into cross-provider quota failover, OR
    // when the request is an auto-managed virtual model (`auto`) where zero-interruption
    // failover across providers is the explicit contract requested by downstream agents.
    let quota_failover_enabled = parsed.is_auto || state.config.read().cross_provider_quota_failover;

    // Request-level pre-commit empty-STOP retry wall-clock budget (contract
    // `2026-10-07-empty-stop-budget-contract`, ruling 1-2): one deadline taken
    // once per request and shared by every target's retry loops, so N targets
    // cannot accumulate past the downstream DSH ~300s idle watchdog. `None`
    // (config Some(0) / disabled) removes the gate entirely.
    let empty_stop_budget = state.config.read().effective_empty_stop_timeout();
    let empty_stop_deadline = empty_stop_budget.map(|b| tokio::time::Instant::now() + b);

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

        // Apply thinking-aware output token safeguard:
        // 1. Ensures max_tokens is floored to safe minimum if thinking is active to prevent zero-content choking.
        // 2. Clamps against model's declared max_output.
        target_req.max_tokens = ponyllm_core::pool::apply_thinking_output_safeguard(
            Some(target_req.max_tokens),
            &target.max_output,
            effective_thinking,
        ).unwrap_or(target_req.max_tokens);

        let is_adaptive = ponyllm_protocol::anthropic::messages::ThinkingConfig::is_adaptive_model(&target.physical_model);
        if effective_thinking.is_active() {
            target_req.reasoning_effort = Some(effective_thinking);
            if is_adaptive {
                target_req.thinking = Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                    r#type: "adaptive".to_string(),
                    budget_tokens: None,
                    effort: None,
                });
                target_req.output_config = Some(ponyllm_protocol::anthropic::messages::AnthropicOutputConfig {
                    effort: Some(effective_thinking),
                });
            } else {
                target_req.thinking = Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                    r#type: "enabled".to_string(),
                    budget_tokens: None,
                    effort: Some(effective_thinking),
                });
                target_req.output_config = None;
            }
        } else {
            target_req.reasoning_effort = None;
            target_req.thinking = None;
            target_req.output_config = None;
            target_req.extra.remove("thinking");
            target_req.extra.remove("output_config");
            target_req.extra.remove("reasoning_effort");
        }

        let (target_url, req_val) = match target.upstream_protocol {
            ponyllm_core::pool::UpstreamProtocol::Responses => {
                let url = target.responses_url();
                let mut resp_req = match anthropic_to_responses_request(&target_req) {
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
                let input_has_content = match &resp_req.input {
                    ponyllm_protocol::openai::responses::ResponseInput::Text(t) => !t.trim().is_empty(),
                    ponyllm_protocol::openai::responses::ResponseInput::Items(items) => !items.is_empty(),
                };
                if !input_has_content {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "type": "error",
                            "error": {
                                "type": "invalid_request_error",
                                "message": "Request input must not be empty"
                            }
                        })),
                    )
                        .into_response();
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
            ponyllm_core::pool::UpstreamProtocol::Chat => {
                let url = target.chat_completions_url();
                let mut chat_req = match anthropic_to_chat_request(&target_req) {
                    Ok(cr) => cr,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                if effective_thinking.is_active() {
                    chat_req.reasoning_effort = Some(effective_thinking);
                } else if requested_thinking == Some(ponyllm_protocol::common::ReasoningEffort::Off) {
                    chat_req.reasoning_effort = Some(ponyllm_protocol::common::ReasoningEffort::Off);
                } else {
                    chat_req.reasoning_effort = None;
                    chat_req.extra.remove("reasoning_effort");
                    chat_req.extra.remove("thinking");
                }
                let mut val = match serde_json::to_value(&chat_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                if requested_thinking == Some(ponyllm_protocol::common::ReasoningEffort::Off) {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("reasoning_effort".to_string(), serde_json::json!("none"));
                        obj.insert("thinking".to_string(), serde_json::json!({ "type": "disabled" }));
                    }
                }
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                let url = target.messages_url();
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
            }
            ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                let url = target.antigravity_url(is_streaming);
                // Explicit-only thinking passthrough (see chat.rs).
                let thinking = requested_thinking.map(|_| effective_thinking);
                // Per-credential project (P0-6, see chat.rs).
                let (ag_project, ag_salt) = state
                    .peek_antigravity_identity(&target.provider_name)
                    .unwrap_or_else(|| ("aicode-consumers".to_string(), String::new()));
                let val = match messages_to_antigravity_request(&target_req, &target.physical_model, &ag_project, thinking, &ag_salt) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Systemone => {
                last_error = format!("Systemone protocol cannot be served by messages endpoint for {}", target.provider_name);
                continue;
            }
        };

        // VULN-07/F6: refuse to dial upstream URLs that resolve (or rebind)
        // to metadata/private/CGNAT/benchmark ranges — re-validated here at
        // dial time (cached per host), not only at provider write time.
        if let Err(reason) = state
            .data_plane_egress_guard_for_target(&target.provider_name, &target.physical_model, &target_url)
            .await
        {
            last_error = reason;
            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
            tracing::warn!(provider = %target.provider_name, url = %target_url, "data-plane egress guard refused upstream; skipping target");
            continue;
        }

        // Forensics snippet must be the actual upstream wire JSON, not the
        // inbound Anthropic shape, so frames replay what was really sent.
        let req_snippet = Some(format_request_snippet(&req_val));
        last_req_snippet = req_snippet.clone();

        // Every per-key retry inside the executor reports through this observer,
        // so each failed attempt lands in the flight recorder with its own
        // status code, key id and upstream error body.
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
        // Egress pool runtime (contract `2026-10-07-egress-pool-contract`):
        // per-attempt exit-IP rotation when the provider configured a pool;
        // both `None` = legacy single-proxy path, byte-identical to before.
        let (egress_pool, egress_clients) =
            state.egress_runtime_for_target(&target.provider_name, &target.physical_model);
        // Resolve this target's effective short-window budget (provider default
        // merged with the model override) so budget-filtered key selection and
        // the window-exhaustion Retry-After are honest.
        // Resolve the budget under the CLEAN model name (matching pricing and
        // routing): a suffixed request (`deepseek-v4-flash[1m]:economy`) must
        // hit the same model-level rate_limits as the plain name.
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
            .with_egress(egress_pool.clone(), egress_clients.clone())
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));

        // Empty-STOP is an upstream transient unrelated to credential health
        // (fails pre-commit, fails fast): give it its own, larger budget so a
        // multi-second upstream blip cannot exhaust the generic retry budget.
        // Unified attempt budget (contract `2026-10-07-empty-stop-budget-contract`
        // ruling 4): shared by chat / messages / responses — floored at
        // MIN_EMPTY_STOP_ATTEMPTS, hard-capped at MAX_EMPTY_STOP_ATTEMPTS_CAP.
        let max_empty_stop_attempts =
            crate::streaming::empty_stop_attempt_budget(pool.total_key_count(), executor.max_retries);

        let mut collect_tried_keys: Vec<String> = Vec::new();
        if is_streaming {
            let current_executor = executor;
            let mut stream_attempt = 0;
            let max_stream_attempts = current_executor.max_retries.max(pool.total_key_count()).max(1);
            // R2: keys already tried by empty-STOP retries (mirrors chat.rs).
            let mut empty_stop_tried_keys: Vec<String> = Vec::new();
            // Per-key empty STOP attempt counter to allow in-place exponential backoff retries.
            let mut active_key_id: Option<String> = None;
            let mut active_key_empty_stop_count: usize = 0;
            // R2: mutable upstream envelope, refreshed per retry.
            let mut attempt_req_val = req_val.clone();
            // R3: consecutive early-frame empty STOPs.
            let mut consecutive_first_frame_stops: usize = 0;

            loop {
                stream_attempt += 1;
                // Request-level wall-clock gate (contract `2026-10-07-empty-stop-
                // budget-contract` ruling 3): never start an attempt with no
                // budget left — fail fast to UpstreamUnavailable.
                let remaining = empty_stop_deadline
                    .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                if let Some(r) = remaining {
                    if r.is_zero() {
                        tracing::warn!(
                            provider = %target.provider_name,
                            stream_attempt,
                            "Antigravity empty-STOP retry wall-clock budget exhausted; failing fast"
                        );
                        last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                        last_error = format!(
                            "Antigravity empty-STOP retry wall-clock budget ({}s) exhausted after {} attempts",
                            empty_stop_budget.map(|b| b.as_secs()).unwrap_or(0),
                            stream_attempt - 1
                        );
                        last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref())).or(Some(1));
                        break;
                    }
                }
                // R2: rebuild the attempt executor with the tried-keys list.
                let attempt_executor = UpstreamExecutor::with_client(
                    pool.clone(),
                    current_executor.client.clone(),
                    current_executor.max_retries,
                )
                .with_downstream_headers(&headers)
                .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
                .with_rate_limits(rate_limits)
                .with_ttfb_timeout(ttfb_timeout.map(|t| remaining.map_or(t, |r| t.min(r))))
                .with_excluded_keys(&empty_stop_tried_keys)
                .with_pinned_key(active_key_id.clone())
                .with_egress(egress_pool.clone(), egress_clients.clone())
                .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));
                match attempt_executor.execute_stream_request_with_timing_and_key(&target_url, &attempt_req_val).await {
                    Ok((upstream_resp, attempt_start, winning_key_id)) => {
                        let raw_stream = stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE);

                        // For Antigravity upstream, verify preamble before committing downstream headers.
                        let mut first_frame_stop = false;
                        let (final_raw_stream, is_empty_stop_retry) = if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                            match verify_antigravity_stream_preamble_with_deadline(
                                raw_stream,
                                std::time::Duration::from_secs(10),
                                remaining.map_or(DEFAULT_PREAMBLE_DEADLINE, |r| r.min(DEFAULT_PREAMBLE_DEADLINE)),
                            ).await {
                                Ok(AntigravityPreambleResult::Ready { buffered, tail }) => {
                                    let head_stream = futures_util::stream::iter(buffered.into_iter().map(Ok));
                                    let chained = head_stream.chain(tail);
                                    let boxed: Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, crate::streaming::StallError>> + Send + Unpin> = Box::new(chained);
                                    (boxed, false)
                                }
                                Ok(AntigravityPreambleResult::TransientEmptyStop { frames, shape }) => {
                                    // R1: one line must answer key / latency /
                                    // frame shape (mirrors chat.rs).
                                    // R3: `frames <= 1` means the terminal STOP
                                    // arrived in early warmup/first frame.
                                    first_frame_stop = frames <= 1;
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
                            // Update per-key empty-STOP accounting:
                            // Try in-place on the same key up to PER_KEY_EMPTY_STOP_MAX_ATTEMPTS before rotating.
                            if active_key_id.as_deref() == Some(&winning_key_id) {
                                active_key_empty_stop_count += 1;
                            } else {
                                active_key_id = Some(winning_key_id.clone());
                                active_key_empty_stop_count = 1;
                            }

                            let key_exhausted = active_key_empty_stop_count >= crate::streaming::PER_KEY_EMPTY_STOP_MAX_ATTEMPTS;
                            if key_exhausted {
                                if !empty_stop_tried_keys.iter().any(|k| k == &winning_key_id) {
                                    empty_stop_tried_keys.push(winning_key_id.clone());
                                }
                                active_key_id = None;
                                active_key_empty_stop_count = 0;
                            }

                            // R3: deterministic early convergence (mirrors chat.rs).
                            if first_frame_stop && key_exhausted {
                                consecutive_first_frame_stops += 1;
                            } else if !first_frame_stop {
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
                                // Contract ruling 6/C7: empty-STOP breaks carry
                                // Retry-After pacing (healthy pool unlock=None
                                // still gets a 1s floor via `.or(Some(1))`).
                                last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref())).or(Some(1));
                                break;
                            }
                            if stream_attempt < max_empty_stop_attempts {
                                let delay = empty_stop_retry_delay(active_key_empty_stop_count.max(1));
                                tracing::warn!(
                                    provider = %target.provider_name,
                                    key_id = %winning_key_id,
                                    stream_attempt,
                                    key_empty_stop_attempt = active_key_empty_stop_count,
                                    max_empty_stop_attempts,
                                    backoff_ms = delay.as_millis() as u64,
                                    "Antigravity empty-STOP before commit; backing off and retrying transparently"
                                );
                                // In-flight backoff never sleeps past the
                                // request-level wall-clock budget.
                                tokio::time::sleep(remaining.map_or(delay, |r| delay.min(r))).await;
                                // R2: mutate upstream identity, cut toxic KV-cache affinity and escalate reasoning depth
                                if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                                    ponyllm_protocol::translator::mutate_antigravity_request_on_empty_stop(&mut attempt_req_val, stream_attempt);
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
                            last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref())).or(Some(1));
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

                        // Stream the raw upstream SSE body. For an OpenAI upstream,
                        // translate OpenAI chat chunks into Anthropic SSE events.
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
                                let stream = passthrough_sse(final_raw_stream);
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Responses => {
                                let stream = responses_sse_to_anthropic_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Chat => {
                                let stream = openai_sse_to_anthropic_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                                let stream = antigravity_sse_to_anthropic_stream(
                                    final_raw_stream,
                                    &target.physical_model,
                                );
                                let monitored = wrap_telemetry_stream(stream, failure_ctx);
                                axum::body::Body::from_stream(monitored)
                            }
                            ponyllm_core::pool::UpstreamProtocol::Systemone => {
                                return crate::extractors::render_anthropic_error(
                                    StatusCode::BAD_REQUEST, "invalid_request_error",
                                    "Use /v1/systemone for systemone models",
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
                        if target.physical_model != parsed.clean_model_name {
                            resp.headers_mut().insert(
                                axum::http::header::HeaderName::from_static("x-ponyllm-fallback-triggered"),
                                HeaderValue::from_static("true"),
                            );
                            if let Ok(orig_val) = HeaderValue::from_str(&parsed.clean_model_name) {
                                resp.headers_mut().insert(
                                    axum::http::header::HeaderName::from_static("x-ponyllm-original-model"),
                                    orig_val,
                                );
                            }
                        }
                        return resp;
                    }
                    Err(err) => {
                        // If we failed with NoAvailableKey after having excluded keys due to empty STOPs,
                        // and we still have attempts remaining, clear the exclusion list so we can cycle
                        // and retry with backoff across all keys in the pool instead of prematurely failing.
                        if matches!(err, CoreError::NoAvailableKey(_))
                            && !empty_stop_tried_keys.is_empty()
                            && stream_attempt < max_empty_stop_attempts
                        {
                            // Wall-clock gate (contract ruling 7): with no
                            // budget left, cycling the pool again is pointless.
                            let remaining_now = empty_stop_deadline
                                .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                            if let Some(r) = remaining_now {
                                if r.is_zero() {
                                    last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                    last_error = format!(
                                        "Antigravity empty-STOP retry wall-clock budget ({}s) exhausted after {} attempts",
                                        empty_stop_budget.map(|b| b.as_secs()).unwrap_or(0),
                                        stream_attempt - 1
                                    );
                                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref())).or(Some(1));
                                    break;
                                }
                            }
                            let delay = crate::streaming::empty_stop_retry_delay(stream_attempt);
                            tracing::warn!(
                                provider = %target.provider_name,
                                stream_attempt,
                                max_empty_stop_attempts,
                                backoff_ms = delay.as_millis() as u64,
                                "All eligible keys cycled during Antigravity empty-STOP retries; resetting exclusion list to retry across pool with backoff"
                            );
                            empty_stop_tried_keys.clear();
                            tokio::time::sleep(remaining_now.map_or(delay, |r| delay.min(r))).await;
                            if target.upstream_protocol == ponyllm_core::pool::UpstreamProtocol::Antigravity {
                                ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut attempt_req_val);
                            }
                            continue;
                        }

                        tracing::warn!("Provider '{}' stream failed ({}). Attempting fallback...", target.provider_name, err);
                        last_kind = err.kind();
                        // H1: pool entirely cooled by quota exhaustion reads
                        // as a quota boundary, not a transient no-key error.
                        if !quota_failover_enabled && crate::extractors::pool_quota_exhausted(&err, &pool) {
                            last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                        }
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
                // R2/R3: same policy as chat.rs (fresh key + fresh requestId,
                // deterministic early convergence).
                let mut collect_attempt = 0usize;
                let mut collect_req_val = req_val.clone();
                let mut collect_consecutive_first_frame: usize = 0;
                loop {
                    collect_attempt += 1;
                    // Wall-clock gate (contract ruling 7): shared request-level
                    // deadline; never dial with no budget left.
                    let remaining_c = empty_stop_deadline
                        .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                    if let Some(r) = remaining_c {
                        if r.is_zero() {
                            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                            last_error = format!(
                                "Antigravity stream collect failed: empty-STOP retry wall-clock budget ({}s) exhausted after {} attempts",
                                empty_stop_budget.map(|b| b.as_secs()).unwrap_or(0),
                                collect_attempt - 1
                            );
                            break (Err(CoreError::Internal(last_error.clone())), None);
                        }
                    }
                    let collect_executor = UpstreamExecutor::with_client(
                        pool.clone(),
                        executor.client.clone(),
                        executor.max_retries,
                    )
                    .with_downstream_headers(&headers)
                    .with_opencode_zen(is_opencode_zen_target(&target.provider_name, &target_url))
                    .with_rate_limits(rate_limits)
                    .with_ttfb_timeout(ttfb_timeout.map(|t| remaining_c.map_or(t, |r| t.min(r))))
                    .with_excluded_keys(&collect_tried_keys)
                    .with_egress(egress_pool.clone(), egress_clients.clone())
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
                                    match crate::routes::chat::collect_empty_stop_policy(&e, collect_attempt, max_empty_stop_attempts, &mut collect_consecutive_first_frame, &target.physical_model) {
                                        Some(crate::routes::chat::CollectRetryAction::Retry { delay }) => {
                                            tracing::warn!(
                                                provider = %target.provider_name,
                                                key_id = %kid,
                                                error = %e,
                                                collect_attempt,
                                                max_empty_stop_attempts,
                                                backoff_ms = delay.as_millis() as u64,
                                                "Non-stream Antigravity collect hit transient empty STOP; backing off and retrying"
                                            );
                                            tokio::time::sleep(remaining_c.map_or(delay, |r| delay.min(r))).await;
                                            ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut collect_req_val);
                                        }
                                        Some(crate::routes::chat::CollectRetryAction::Deterministic { message }) => {
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
                        Err(e) => {
                            if matches!(e, CoreError::NoAvailableKey(_))
                                && !collect_tried_keys.is_empty()
                                && collect_attempt < max_empty_stop_attempts
                            {
                                // Wall-clock gate (contract ruling 7): with no
                                // budget left, cycling the pool again is pointless.
                                let remaining_now = empty_stop_deadline
                                    .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                                if let Some(r) = remaining_now {
                                    if r.is_zero() {
                                        last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                        last_error = format!(
                                            "Antigravity stream collect failed: empty-STOP retry wall-clock budget ({}s) exhausted after {} attempts",
                                            empty_stop_budget.map(|b| b.as_secs()).unwrap_or(0),
                                            collect_attempt - 1
                                        );
                                        break (Err(CoreError::Internal(last_error.clone())), None);
                                    }
                                }
                                let delay = crate::streaming::empty_stop_retry_delay(collect_attempt);
                                tracing::warn!(
                                    provider = %target.provider_name,
                                    collect_attempt,
                                    max_empty_stop_attempts,
                                    backoff_ms = delay.as_millis() as u64,
                                    "All eligible keys cycled during non-stream Antigravity empty-STOP retries; resetting exclusion list to retry across pool with backoff"
                                );
                                collect_tried_keys.clear();
                                tokio::time::sleep(remaining_now.map_or(delay, |r| delay.min(r))).await;
                                ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut collect_req_val);
                                continue;
                            }
                            break (Err(e), None);
                        }
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
                    let mut ant_resp: MessageResponse = match target.upstream_protocol {
                        ponyllm_core::pool::UpstreamProtocol::Responses => {
                            let resp_obj: ponyllm_protocol::openai::responses::ResponseObject =
                                match serde_json::from_value(resp_val) {
                                    Ok(ro) => ro,
                                    Err(e) => {
                                        last_error = format!("Invalid Responses object from {}: {}", target.provider_name, e);
                                        continue;
                                    }
                                };
                            match responses_to_anthropic_response(&resp_obj) {
                                Ok(ar) => ar,
                                Err(e) => {
                                    // Upstream Responses failure (status=failed with an
                                    // upstream code/message) must fail over, not fall
                                    // through as a success: project it as an
                                    // upstream fault (503, retryable) and try the
                                    // next routed target. Mirrors the chat route.
                                    last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                    last_retry_after = crate::extractors::retry_after_secs(&last_kind, retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()));
                                    last_error = format!("Upstream {} response failed: {}", target.provider_name, e);
                                    continue;
                                }
                            }
                        }
                        ponyllm_core::pool::UpstreamProtocol::Chat => {
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
                        }
                        ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                            match serde_json::from_value(resp_val) {
                                Ok(ar) => ar,
                                Err(e) => {
                                    last_error = format!("Invalid Anthropic response from {}: {}", target.provider_name, e);
                                    continue;
                                }
                            }
                        }
                        ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                            let ant_val = antigravity_to_messages_response(&resp_val, &target.physical_model);
                            match serde_json::from_value(ant_val) {
                                Ok(ar) => ar,
                                Err(e) => {
                                    last_error = format!("Invalid Antigravity translated response from {}: {}", target.provider_name, e);
                                    continue;
                                }
                            }
                        }
                        ponyllm_core::pool::UpstreamProtocol::Systemone => {
                            last_error = "Systemone protocol cannot be projected as Anthropic messages".to_string();
                            continue;
                        }
                    };

                    // Model Echo Rule: Strictly echo requested model name in response body
                    ant_resp.model = requested_raw_model.clone();

                    let cached_read = ant_resp.usage.cache_read_input_tokens.unwrap_or(0) as u64;
                    let cached_create = ant_resp.usage.cache_creation_input_tokens.unwrap_or(0) as u64;
                    let prompt_tokens = (ant_resp.usage.input_tokens as u64)
                        .saturating_add(cached_read)
                        .saturating_add(cached_create);
                    let cached_tokens = cached_read;
                    let completion_tokens = ant_resp.usage.output_tokens as u64;
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
                            tps,
                            request_snippet: req_snippet,
                            response_snippet: serde_json::to_string(&ant_resp).ok(),
                        },
                    );

                    let mut response = (StatusCode::OK, Json(ant_resp)).into_response();
                    inject_routing_headers(&mut response, &target);
                    inject_telemetry_headers(&mut response, &request_id, &stages);
                    if target.physical_model != parsed.clean_model_name {
                        response.headers_mut().insert(
                            axum::http::header::HeaderName::from_static("x-ponyllm-fallback-triggered"),
                            axum::http::HeaderValue::from_static("true"),
                        );
                        if let Ok(orig_val) = axum::http::HeaderValue::from_str(&parsed.clean_model_name) {
                            response.headers_mut().insert(
                                axum::http::header::HeaderName::from_static("x-ponyllm-original-model"),
                                orig_val,
                            );
                        }
                    }
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
    let mut resp = crate::extractors::project_anthropic_error(&last_kind, &msg);
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

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos)
}
