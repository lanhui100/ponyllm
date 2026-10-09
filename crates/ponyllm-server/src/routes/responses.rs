use crate::extractors::{format_request_snippet, AppJson};
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers, retry_unlock_hint};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{
    anthropic_sse_to_responses_stream, antigravity_sse_to_openai_stream,
    chat_sse_to_responses_stream, collect_antigravity_sse_to_json, extract_usage_tokens,
    is_transient_empty_stop_error, passthrough_sse, stall_guard, wrap_telemetry_stream,
    StreamFailureContext, DEFAULT_TAIL_STALL_IDLE,
};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use parking_lot::Mutex;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::{is_opencode_zen_target, EventSinkCtx, UpstreamExecutor};
use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};
use ponyllm_protocol::openai::responses::CreateResponseRequest;
use ponyllm_protocol::translator::chat_to_antigravity_request;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

pub async fn handle_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AppJson(req): AppJson<CreateResponseRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());
    let endpoint = "/v1/responses".to_string();
    let ctx = EventCtx {
        request_id: request_id.clone(),
        session_id: None,
        model: Some(req.model.clone()),
        endpoint: endpoint.clone(),
        start: start_time,
    };
    let stages = Arc::new(Mutex::new(StageTimings::default()));

    // Client-side validation: empty inputs are a client error
    let is_empty_input = match &req.input {
        ponyllm_protocol::openai::responses::ResponseInput::Text(t) => t.trim().is_empty(),
        ponyllm_protocol::openai::responses::ResponseInput::Items(items) => items.is_empty(),
    };
    if is_empty_input {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "input must not be empty",
                    "type": "invalid_request_error",
                    "code": "invalid_input"
                }
            })),
        )
            .into_response();
    }

    // User access & quota check
    let caller_user_id = headers
        .get("x-user-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string());
    if let Some(uid) = caller_user_id.as_deref() {
        if let Err(err) = state.user_tracker.check_access(uid, &req.model) {
            let (status, code) = match err {
                ponyllm_core::UserCheckError::QuotaExhausted { .. } => {
                    (StatusCode::TOO_MANY_REQUESTS, "user_quota_exhausted")
                }
                ponyllm_core::UserCheckError::ModelNotAllowed { .. } => {
                    (StatusCode::FORBIDDEN, "model_forbidden_for_user")
                }
                ponyllm_core::UserCheckError::UserDisabled { .. } => {
                    (StatusCode::FORBIDDEN, "user_disabled")
                }
                ponyllm_core::UserCheckError::UserNotFound { .. } => {
                    (StatusCode::FORBIDDEN, "user_not_found")
                }
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
    }

    // B002 token gate: per-key quota + model_limits intersection (second,
    // multiplicative gate after the user gate).
    if let Err(resp) = crate::routes::gate::token_gate(&state, &headers, &req.model) {
        return resp;
    }

    // Parse requested model (auto / [1m] / :strategy suffix) and resolve the
    // physical model + provider, mirroring chat/messages routing so virtual
    // model names are correctly mapped upstream.
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();
    let prompt_hint = match &req.input {
        ponyllm_protocol::openai::responses::ResponseInput::Text(t) => Some(t.clone()),
        ponyllm_protocol::openai::responses::ResponseInput::Items(_) => {
            serde_json::to_string(&req.input).ok()
        }
    };
    let prompt_ref = prompt_hint.as_deref();

    let routing_start = Instant::now();
    let header_strategy = headers
        .get("x-pony-strategy")
        .or_else(|| headers.get("x-routing-strategy"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| GatewayRoutingStrategy::from_str(s).ok());
    let header_thinking = crate::extractors::parse_thinking_header(&headers);
    let required_modalities = req.required_modalities();
    let targets = match state.resolve_routed_targets_full(
        &parsed,
        header_strategy,
        prompt_ref,
        crate::extractors::parse_protocol_header(&headers),
        Some(ponyllm_core::pool::UpstreamProtocol::Responses),
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
                ponyllm_core::error::CoreError::UnsupportedModality { .. } => {
                    (StatusCode::BAD_REQUEST, "unsupported_modality")
                }
                ponyllm_core::error::CoreError::CapacityExhausted { .. } => {
                    (StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted")
                }
                ponyllm_core::error::CoreError::Internal(ref msg)
                    if msg.contains("No provider configured") =>
                {
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
    // Antigravity is now bridged via Responses -> Chat -> Antigravity.
    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(targets[0].provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: targets[0].upstream_protocol
                != ponyllm_core::pool::UpstreamProtocol::Responses,
            routing_ms,
        },
    );

    let mut last_error = String::new();
    let mut last_pool_exhausted = false;
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;
    let mut last_retry_after: Option<u64> = None;
    let mut last_req_snippet: Option<String> = None;
    let is_streaming = req.stream.unwrap_or(false);

    // Quota boundary (bugfix): a quota-exhaustion failure on one provider must
    // not silently drain a second provider that carries the same model, unless
    // the operator explicitly opts back into cross-provider quota failover, OR
    // when the request is an auto-managed virtual model (`auto`) where zero-interruption
    // failover across providers is the explicit contract requested by downstream agents.
    let quota_failover_enabled =
        parsed.is_auto || state.config.read().cross_provider_quota_failover;

    // Request-level pre-commit empty-STOP retry wall-clock budget (contract
    // `2026-10-07-empty-stop-budget-contract`, ruling 1-2): one deadline taken
    // once per request and shared by every target's retry loops, so N targets
    // cannot accumulate past the downstream DSH ~300s idle watchdog. `None`
    // (config Some(0) / disabled) removes the gate entirely.
    let empty_stop_budget = state.config.read().effective_empty_stop_timeout();
    let empty_stop_deadline = empty_stop_budget.map(|b| tokio::time::Instant::now() + b);
    // Per-target budget slice (adversarial review FIX-2): each routed target
    // gets at most budget/N of the request wall-clock, so a slow first target
    // cannot starve failover attempts on later providers. The global deadline
    // still bounds the whole request (sum of slices <= budget).
    let per_target_budget = empty_stop_budget.map(|b| b / (targets.len().max(1) as u32));

    for target in targets {
        // If target is currently cooling down under the model circuit breaker, skip it
        if parsed.is_auto
            && state.is_model_cooling_down(&target.provider_name, &target.physical_model)
        {
            tracing::warn!(
                provider = %target.provider_name,
                model = %target.physical_model,
                "Model circuit breaker active: skipping cooled down model candidate"
            );
            continue;
        }

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
        let provider_name = target.provider_name.clone();
        let physical_model = target.physical_model.clone();
        let pool = match state.get_pool(&provider_name) {
            Some(p) => p,
            None => continue,
        };

        // Route to the provider endpoint matching its native protocol,
        // translating when the inbound Responses shape differs from it.
        let max_retries = state.config.read().max_retries;
        let mut target_req = req.clone();
        target_req.model = physical_model.clone();

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
            target_req.reasoning = Some(
                ponyllm_protocol::openai::responses::ResponseReasoningConfig {
                    effort: Some(effective_thinking),
                },
            );
            target_req.sanitize_thinking_extra();
        } else {
            target_req.reasoning_effort = None;
            target_req.reasoning = None;
            target_req.sanitize_thinking_extra();
            target_req.extra.remove("reasoning");
        }

        // Apply thinking-aware output token safeguard:
        // 1. Ensures max_output_tokens is floored to safe minimum if thinking is active to prevent zero-content choking.
        // 2. Clamps against model's declared max_output.
        target_req.max_output_tokens = ponyllm_core::pool::apply_thinking_output_safeguard(
            target_req.max_output_tokens,
            &target.max_output,
            effective_thinking,
        );

        let (target_url, req_val) = match target.upstream_protocol {
            ponyllm_core::pool::UpstreamProtocol::Chat => {
                let mut chat_req =
                    match ponyllm_protocol::translator::responses_to_chat_request(&target_req) {
                        Ok(cr) => cr,
                        Err(e) => {
                            last_error = format!("Translation error for {}: {}", provider_name, e);
                            continue;
                        }
                    };
                if effective_thinking.is_active() {
                    chat_req.reasoning_effort = Some(effective_thinking);
                } else {
                    chat_req.reasoning_effort = None;
                    chat_req.extra.remove("reasoning_effort");
                    chat_req.extra.remove("thinking");
                }
                let chat_val = match serde_json::to_value(&chat_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Serialization error for {}: {}", provider_name, e);
                        continue;
                    }
                };
                (target.chat_completions_url(), chat_val)
            }
            ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                let mut ant_req =
                    match ponyllm_protocol::translator::responses_to_anthropic_request(&target_req)
                    {
                        Ok(ar) => ar,
                        Err(e) => {
                            last_error = format!("Translation error for {}: {}", provider_name, e);
                            continue;
                        }
                    };
                let is_adaptive =
                    ponyllm_protocol::anthropic::messages::ThinkingConfig::is_adaptive_model(
                        &target.physical_model,
                    );
                if effective_thinking.is_active() {
                    ant_req.reasoning_effort = Some(effective_thinking);
                    if is_adaptive {
                        ant_req.thinking =
                            Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                                r#type: "adaptive".to_string(),
                                budget_tokens: None,
                                effort: None,
                            });
                        ant_req.output_config = Some(
                            ponyllm_protocol::anthropic::messages::AnthropicOutputConfig {
                                effort: Some(effective_thinking),
                            },
                        );
                    } else {
                        ant_req.thinking =
                            Some(ponyllm_protocol::anthropic::messages::ThinkingConfig {
                                r#type: "enabled".to_string(),
                                budget_tokens: None,
                                effort: Some(effective_thinking),
                            });
                        ant_req.output_config = None;
                    }
                } else {
                    ant_req.reasoning_effort = None;
                    ant_req.thinking = None;
                    ant_req.output_config = None;
                    ant_req.extra.remove("thinking");
                    ant_req.extra.remove("output_config");
                    ant_req.extra.remove("reasoning_effort");
                }
                let val = match serde_json::to_value(&ant_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error =
                            format!("Serialization error for {}: {}", target.provider_name, e);
                        continue;
                    }
                };
                (target.messages_url(), val)
            }
            ponyllm_core::pool::UpstreamProtocol::Responses => {
                let val = match serde_json::to_value(&target_req) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!("Invalid JSON for {}: {}", provider_name, e);
                        continue;
                    }
                };
                (target.responses_url(), val)
            }
            ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                let url = target.antigravity_url(is_streaming);
                let thinking = requested_thinking.map(|_| effective_thinking);
                let (ag_project, ag_salt) = state
                    .peek_antigravity_identity(&provider_name)
                    .unwrap_or_else(|| ("aicode-consumers".to_string(), String::new()));
                // First translate Responses request to Chat request, then to Antigravity envelope
                let mut chat_req =
                    match ponyllm_protocol::translator::responses_to_chat_request(&target_req) {
                        Ok(cr) => cr,
                        Err(e) => {
                            last_error = format!(
                                "Translation error (Responses->Chat) for {}: {}",
                                provider_name, e
                            );
                            continue;
                        }
                    };
                if effective_thinking.is_active() {
                    chat_req.reasoning_effort = Some(effective_thinking);
                } else {
                    chat_req.reasoning_effort = None;
                    chat_req.extra.remove("reasoning_effort");
                    chat_req.extra.remove("thinking");
                }
                let val = match chat_to_antigravity_request(
                    &chat_req,
                    &target.physical_model,
                    &ag_project,
                    thinking,
                    &ag_salt,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        last_error = format!(
                            "Translation error (Chat->Antigravity) for {}: {}",
                            provider_name, e
                        );
                        continue;
                    }
                };
                (url, val)
            }
            ponyllm_core::pool::UpstreamProtocol::Systemone => {
                last_error = format!(
                    "Systemone protocol cannot be served by responses endpoint for {}",
                    provider_name
                );
                continue;
            }
        };

        // VULN-07/F6: refuse to dial upstream URLs that resolve (or rebind)
        // to metadata/private/CGNAT/benchmark ranges — re-validated here at
        // dial time (cached per host), not only at provider write time.
        if let Err(reason) = state
            .data_plane_egress_guard_for_target(&provider_name, &target.physical_model, &target_url)
            .await
        {
            last_error = reason.to_string();
            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
            tracing::warn!(provider = %provider_name, url = %target_url, "data-plane egress guard refused upstream; skipping target");
            continue;
        }

        let req_snippet = Some(format_request_snippet(&req_val));
        last_req_snippet = req_snippet.clone();
        let sink_ctx = EventSinkCtx {
            request_id: request_id.clone(),
            endpoint: endpoint.clone(),
            provider: provider_name.clone(),
            model: Some(requested_raw_model.clone()),
            start: start_time,
            stages: stages.clone(),
            request_snippet: req_snippet.clone(),
        };
        let http_client = state.http_client_for_target(&provider_name, &target.physical_model);
        // Egress pool runtime (contract `2026-10-07-egress-pool-contract`):
        // per-attempt exit-IP rotation when the provider configured a pool;
        // both `None` = legacy single-proxy path, byte-identical to before.
        let (egress_pool, egress_clients) =
            state.egress_runtime_for_target(&provider_name, &target.physical_model);
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
                .get(&provider_name)
                .and_then(|p| p.effective_rate_limits(&parsed.clean_model_name));
            let ttfb = cfg.effective_ttfb_timeout(&provider_name);
            (rl, ttfb)
        };
        let executor = UpstreamExecutor::with_client(pool.clone(), http_client, max_retries)
            .with_downstream_headers(&headers)
            .with_opencode_zen(is_opencode_zen_target(&provider_name, &target_url))
            .with_rate_limits(rate_limits)
            .with_ttfb_timeout(ttfb_timeout)
            .with_egress(egress_pool.clone(), egress_clients.clone())
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));

        let empty_stop_tried_keys: Vec<String> = Vec::new();
        // Per-target deadline (adversarial review FIX-2): min(global request
        // deadline, this target's entry-time + budget/N slice). All wall-clock
        // `remaining` computations inside the retry loops below source from
        // this per-target deadline.
        let target_deadline = empty_stop_deadline.map(|global| {
            let slice = per_target_budget.unwrap_or_default();
            global.min(tokio::time::Instant::now() + slice)
        });
        // Handle streaming request: pass through upstream SSE unchanged
        if is_streaming {
            match executor
                .execute_stream_request_with_timing_and_key(&target_url, &req_val)
                .await
            {
                Ok((upstream_resp, attempt_start, winning_key_id)) => {
                    if let Some(p) = prompt_ref {
                        state.hot_cache.record_dispatch(p, &provider_name);
                    }
                    state.emit(
                        &ctx,
                        Some(provider_name.clone()),
                        GatewayEvent::StreamStarted {
                            request_snippet: req_snippet.clone(),
                        },
                    );

                    let est_prompt_tokens = serde_json::to_string(&req.input)
                        .map(|s| (s.len() as u64 / 4).max(1))
                        .unwrap_or(1);

                    let failure_ctx = StreamFailureContext {
                        bus: state.event_bus.clone(),
                        ctx: ctx.clone(),
                        provider: provider_name.clone(),
                        stages: stages.clone(),
                        request_snippet: req_snippet.clone(),
                        estimated_prompt_tokens: est_prompt_tokens,
                        attempt_start: Some(attempt_start),
                        key_pool: Some(pool.clone()),
                        key_id: Some(winning_key_id),
                        sentry: Some(state.sentry.clone()),
                    };
                    // Same-protocol upstreams stream through untouched;
                    // mismatched natives are translated into Responses events.
                    let body = match target.upstream_protocol {
                        ponyllm_core::pool::UpstreamProtocol::Chat => {
                            let stream = chat_sse_to_responses_stream(
                                stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE),
                                &target.physical_model,
                            );
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                            let stream = anthropic_sse_to_responses_stream(
                                stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE),
                                &target.physical_model,
                            );
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Responses => {
                            let stream = passthrough_sse(stall_guard(
                                upstream_resp.bytes_stream(),
                                DEFAULT_TAIL_STALL_IDLE,
                            ));
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                            let chat_stream = antigravity_sse_to_openai_stream(
                                stall_guard(upstream_resp.bytes_stream(), DEFAULT_TAIL_STALL_IDLE),
                                &target.physical_model,
                            );
                            let stream =
                                chat_sse_to_responses_stream(chat_stream, &target.physical_model);
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Systemone => {
                            return crate::extractors::render_openai_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_request_error",
                                "protocol_mismatch",
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
                    return resp;
                }
                Err(err) => {
                    tracing::warn!(
                        "Provider '{}' responses stream failed ({}). Attempting fallback...",
                        provider_name,
                        err
                    );
                    last_kind = err.kind();

                    // Auto resilience: if model failed with ModelNotFound or 404/400 model error, trigger model circuit breaker & PonySentry report
                    if parsed.is_auto
                        && matches!(
                            last_kind,
                            ponyllm_core::error::GatewayErrorKind::ModelNotFound
                        )
                    {
                        state.record_model_outage(
                            &target.provider_name,
                            &target.physical_model,
                            std::time::Duration::from_secs(600),
                        );

                        let mut tags = std::collections::HashMap::new();
                        tags.insert("event_type".to_string(), "auto_model_failover".to_string());
                        tags.insert("failed_provider".to_string(), target.provider_name.clone());
                        tags.insert("failed_model".to_string(), target.physical_model.clone());
                        state.sentry.capture_error(
                            "AutoModelFailover",
                            &format!("Auto routed model '{}:{}' failed with ModelNotFound, triggering failover", target.provider_name, target.physical_model),
                            Some(tags),
                            Some(serde_json::json!({
                                "request_id": request_id,
                                "error": err.to_string(),
                            })),
                        );
                    }

                    // H1: pool entirely cooled by quota exhaustion reads as a
                    // quota boundary, not a transient no-key error.
                    if !quota_failover_enabled
                        && crate::extractors::pool_quota_exhausted(&err, &pool)
                    {
                        last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                    }
                    last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_))
                        && empty_stop_tried_keys.is_empty();
                    last_retry_after = crate::extractors::retry_after_secs(
                        &last_kind,
                        retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()),
                    );
                    last_error = err.to_string();
                    continue;
                }
            }
        }

        let mut collect_tried_keys: Vec<String> = Vec::new();
        let (upstream_result, winning_key_id) = if target.upstream_protocol
            == ponyllm_core::pool::UpstreamProtocol::Antigravity
        {
            // Unified attempt budget (contract `2026-10-07-empty-stop-budget-
            // contract` ruling 4): shared with chat / messages — also fixes the
            // previous formula missing the per-key multiplier.
            let max_empty_stop_attempts = crate::streaming::empty_stop_attempt_budget(
                pool.total_key_count(),
                executor.max_retries,
            );
            // R2/R3: same policy as chat.rs (fresh key + fresh requestId,
            // deterministic early convergence).
            let mut collect_attempt = 0usize;
            let mut collect_req_val = req_val.clone();
            let mut collect_consecutive_first_frame: usize = 0;
            loop {
                collect_attempt += 1;
                // Wall-clock gate (contract ruling 7): shared request-level
                // deadline; never dial with no budget left.
                let remaining_c = target_deadline
                    .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                if collect_attempt > 1 {
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
                }
                let collect_executor = UpstreamExecutor::with_client(
                    pool.clone(),
                    executor.client.clone(),
                    executor.max_retries,
                )
                .with_downstream_headers(&headers)
                .with_opencode_zen(is_opencode_zen_target(&provider_name, &target_url))
                .with_rate_limits(rate_limits)
                .with_ttfb_timeout(if collect_attempt == 1 {
                    // FIX-1: first attempt keeps the configured TTFB verbatim.
                    ttfb_timeout
                } else {
                    ttfb_timeout.map(|t| remaining_c.map_or(t, |r| t.min(r)))
                })
                .with_excluded_keys(&collect_tried_keys)
                .with_egress(egress_pool.clone(), egress_clients.clone())
                .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));
                match collect_executor
                    .execute_stream_request_with_timing_and_key(&target_url, &collect_req_val)
                    .await
                {
                    Ok((resp, _instant, kid)) => {
                        if !collect_tried_keys.iter().any(|k| k == &kid) {
                            collect_tried_keys.push(kid.clone());
                        }
                        let raw_stream = resp.bytes_stream();
                        match collect_antigravity_sse_to_json(raw_stream).await {
                            Ok(v) => break (Ok(v), Some(kid)),
                            Err(e) if is_transient_empty_stop_error(&e) => {
                                match crate::routes::chat::collect_empty_stop_policy(
                                    &e,
                                    collect_attempt,
                                    max_empty_stop_attempts,
                                    &mut collect_consecutive_first_frame,
                                    &target.physical_model,
                                ) {
                                    Some(crate::routes::chat::CollectRetryAction::Retry {
                                        delay,
                                    }) => {
                                        tracing::warn!(
                                            provider = %provider_name,
                                            key_id = %kid,
                                            error = %e,
                                            collect_attempt,
                                            max_empty_stop_attempts,
                                            backoff_ms = delay.as_millis() as u64,
                                            "Non-stream Antigravity collect hit transient empty STOP in Responses route; backing off and retrying"
                                        );
                                        tokio::time::sleep(
                                            remaining_c.map_or(delay, |r| delay.min(r)),
                                        )
                                        .await;
                                        ponyllm_protocol::translator::refresh_antigravity_request_ids(&mut collect_req_val);
                                    }
                                    Some(
                                        crate::routes::chat::CollectRetryAction::Deterministic {
                                            message,
                                        },
                                    ) => {
                                        tracing::warn!(
                                            provider = %provider_name,
                                            collect_attempt,
                                            collect_consecutive_first_frame,
                                            "Non-stream Antigravity collect hit deterministic empty STOP in Responses route; converging early to trigger failover"
                                        );
                                        last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                                        last_error = message;
                                        break (
                                            Err(CoreError::Internal(last_error.clone())),
                                            Some(kid),
                                        );
                                    }
                                    None => {
                                        tracing::warn!(
                                            provider = %provider_name,
                                            error = %e,
                                            "Antigravity stream collection failed in Responses route"
                                        );
                                        break (
                                            Err(CoreError::Internal(format!(
                                                "Antigravity stream collect failed: {}",
                                                e
                                            ))),
                                            Some(kid),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    provider = %provider_name,
                                    error = %e,
                                    "Antigravity stream collection failed in Responses route"
                                );
                                break (
                                    Err(CoreError::Internal(format!(
                                        "Antigravity stream collect failed: {}",
                                        e
                                    ))),
                                    Some(kid),
                                );
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
                            let remaining_now = target_deadline
                                .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                            if let Some(r) = remaining_now {
                                if r.is_zero() {
                                    last_kind =
                                        ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
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
                                provider = %provider_name,
                                collect_attempt,
                                max_empty_stop_attempts,
                                backoff_ms = delay.as_millis() as u64,
                                "All eligible keys cycled during non-stream Antigravity empty-STOP retries in Responses route; resetting exclusion list to retry across pool with backoff"
                            );
                            collect_tried_keys.clear();
                            tokio::time::sleep(remaining_now.map_or(delay, |r| delay.min(r))).await;
                            ponyllm_protocol::translator::refresh_antigravity_request_ids(
                                &mut collect_req_val,
                            );
                            continue;
                        }
                        break (Err(e), None);
                    }
                }
            }
        } else if ponyllm_core::executor::zen_free_tier_forces_upstream_stream(
            &provider_name,
            &target_url,
            &target.physical_model,
        ) && matches!(
            target.upstream_protocol,
            ponyllm_core::pool::UpstreamProtocol::Chat
                | ponyllm_core::pool::UpstreamProtocol::Responses
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
            match executor
                .execute_stream_request_with_timing_and_key(&target_url, &streamed_val)
                .await
            {
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
                                provider = %provider_name,
                                error = %e,
                                "Zen free-tier upstream stream collection failed"
                            );
                            (
                                Err(CoreError::Internal(format!(
                                    "Zen stream collect failed: {}",
                                    e
                                ))),
                                Some(kid),
                            )
                        }
                    }
                }
                Err(e) => (Err(e), None),
            }
        } else {
            match executor
                .execute_json_request_with_key(&target_url, &req_val)
                .await
            {
                Ok((val, kid)) => (Ok(val), Some(kid)),
                Err(e) => (Err(e), None),
            }
        };

        match (upstream_result, winning_key_id) {
            (Ok(resp_val), winning_key_id) => {
                let mut resp_val = match target.upstream_protocol {
                    ponyllm_core::pool::UpstreamProtocol::Chat => {
                        let chat_resp: ponyllm_protocol::openai::chat::ChatCompletionResponse =
                            match serde_json::from_value(resp_val) {
                                Ok(cr) => cr,
                                Err(e) => {
                                    last_error = format!(
                                        "Invalid Chat response from {}: {}",
                                        provider_name, e
                                    );
                                    continue;
                                }
                            };
                        let resp_obj =
                            match ponyllm_protocol::translator::chat_to_responses_response(
                                &chat_resp,
                            ) {
                                Ok(ro) => ro,
                                Err(e) => {
                                    last_error = format!("Translation error: {}", e);
                                    continue;
                                }
                            };
                        match serde_json::to_value(&resp_obj) {
                            Ok(v) => v,
                            Err(e) => {
                                last_error = format!("Serialization error: {}", e);
                                continue;
                            }
                        }
                    }
                    ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                        let ant_resp: ponyllm_protocol::anthropic::messages::MessageResponse =
                            match serde_json::from_value(resp_val) {
                                Ok(ar) => ar,
                                Err(e) => {
                                    last_error = format!(
                                        "Invalid Anthropic response from {}: {}",
                                        provider_name, e
                                    );
                                    continue;
                                }
                            };
                        let resp_obj =
                            match ponyllm_protocol::translator::anthropic_to_responses_response(
                                &ant_resp,
                            ) {
                                Ok(ro) => ro,
                                Err(e) => {
                                    last_error = format!("Translation error: {}", e);
                                    continue;
                                }
                            };
                        match serde_json::to_value(&resp_obj) {
                            Ok(v) => v,
                            Err(e) => {
                                last_error = format!("Serialization error: {}", e);
                                continue;
                            }
                        }
                    }
                    ponyllm_core::pool::UpstreamProtocol::Responses => resp_val,
                    ponyllm_core::pool::UpstreamProtocol::Antigravity => {
                        let chat_resp_val =
                            ponyllm_protocol::translator::antigravity_to_chat_response(
                                &resp_val,
                                &target.physical_model,
                            );
                        let chat_resp: ponyllm_protocol::openai::chat::ChatCompletionResponse =
                            match serde_json::from_value(chat_resp_val) {
                                Ok(cr) => cr,
                                Err(e) => {
                                    last_error = format!(
                                        "Invalid Antigravity translated Chat response from {}: {}",
                                        provider_name, e
                                    );
                                    continue;
                                }
                            };
                        let resp_obj =
                            match ponyllm_protocol::translator::chat_to_responses_response(
                                &chat_resp,
                            ) {
                                Ok(ro) => ro,
                                Err(e) => {
                                    last_error = format!("Translation error: {}", e);
                                    continue;
                                }
                            };
                        match serde_json::to_value(&resp_obj) {
                            Ok(v) => v,
                            Err(e) => {
                                last_error = format!("Serialization error: {}", e);
                                continue;
                            }
                        }
                    }
                    ponyllm_core::pool::UpstreamProtocol::Systemone => resp_val,
                };
                let latency = start_time.elapsed();
                let (prompt_tokens, completion_tokens, cached_tokens) =
                    extract_usage_tokens(&resp_val);
                if let Some(uid) = caller_user_id.as_deref() {
                    state
                        .user_tracker
                        .record_tokens(uid, prompt_tokens + completion_tokens);
                }
                // B002 double settlement: token-leg accounting.
                crate::routes::gate::token_record_tokens(
                    &state,
                    &headers,
                    prompt_tokens + completion_tokens,
                );
                if let Some(kid) = winning_key_id.as_deref() {
                    let wall_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    pool.record_tokens(
                        kid,
                        wall_ms,
                        prompt_tokens,
                        completion_tokens,
                        cached_tokens,
                    );
                }
                let tps = if latency.as_secs_f64() > 0.05 && completion_tokens > 0 {
                    Some((completion_tokens as f64 / latency.as_secs_f64()).max(1.0))
                } else {
                    None
                };
                if let Some(p) = prompt_ref {
                    state.hot_cache.record_dispatch(p, &provider_name);
                }
                state.emit(
                    &ctx,
                    Some(provider_name.clone()),
                    GatewayEvent::RequestCompleted {
                        status_code: 200,
                        latency_ms: latency.as_secs_f64() * 1000.0,
                        prompt_tokens,
                        completion_tokens,
                        cached_tokens,
                        tps,
                        request_snippet: req_snippet.clone(),
                        response_snippet: Some(resp_val.to_string()),
                    },
                );

                // Model Echo Rule: strictly echo requested model name in response body
                if let Some(obj) = resp_val.as_object_mut() {
                    obj.insert("model".to_string(), serde_json::json!(requested_raw_model));
                }

                let mut response = (StatusCode::OK, Json(resp_val)).into_response();
                inject_routing_headers(&mut response, &target);
                inject_telemetry_headers(&mut response, &request_id, &stages);
                return response;
            }
            (Err(err), _) => {
                tracing::warn!(
                    "Provider '{}' responses request failed ({}). Attempting fallback...",
                    provider_name,
                    err
                );
                last_kind = err.kind();

                // Auto resilience: if model failed with ModelNotFound or 404/400 model error, trigger model circuit breaker & PonySentry report
                if parsed.is_auto
                    && matches!(
                        last_kind,
                        ponyllm_core::error::GatewayErrorKind::ModelNotFound
                    )
                {
                    state.record_model_outage(
                        &target.provider_name,
                        &target.physical_model,
                        std::time::Duration::from_secs(600),
                    );

                    let mut tags = std::collections::HashMap::new();
                    tags.insert("event_type".to_string(), "auto_model_failover".to_string());
                    tags.insert("failed_provider".to_string(), target.provider_name.clone());
                    tags.insert("failed_model".to_string(), target.physical_model.clone());
                    state.sentry.capture_error(
                        "AutoModelFailover",
                        &format!("Auto routed model '{}:{}' failed with ModelNotFound, triggering failover", target.provider_name, target.physical_model),
                        Some(tags),
                        Some(serde_json::json!({
                            "request_id": request_id,
                            "error": err.to_string(),
                        })),
                    );
                }

                // H1: pool entirely cooled by quota exhaustion reads as a
                // quota boundary, not a transient no-key error.
                if !quota_failover_enabled && crate::extractors::pool_quota_exhausted(&err, &pool) {
                    last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                }
                last_pool_exhausted =
                    matches!(err, CoreError::NoAvailableKey(_)) && collect_tried_keys.is_empty();
                last_retry_after = crate::extractors::retry_after_secs(
                    &last_kind,
                    retry_unlock_hint(&last_kind, &pool, rate_limits.as_ref()),
                );
                let err_text = err.to_string();
                // Adversarial review FIX-3: empty-STOP collect breaks must
                // carry Retry-After pacing even when the pool is healthy
                // (no unlock hint), matching the streaming-loop breaks.
                if err_text.contains("Antigravity stream collect failed")
                    || err_text.contains("Antigravity deterministic empty STOP")
                {
                    last_retry_after = last_retry_after.or(Some(1));
                }
                last_error = err_text;
                continue;
            }
        }
    }

    let msg = crate::extractors::format_exhausted_message(
        &requested_raw_model,
        &last_kind,
        &last_error,
        last_pool_exhausted,
        &request_id,
    );

    // PonySentry 埋点上报网关耗尽/失败事件（过滤下游客户端 4xx/invalid_request_error 参数错误）
    let is_client_bad_request = last_error.contains("invalid_request_error")
        || last_error.contains("400 Bad Request")
        || last_error.contains("Duplicate function_call_output");
    if !is_client_bad_request {
        let mut tags = std::collections::HashMap::new();
        tags.insert("requested_model".to_string(), requested_raw_model.clone());
        tags.insert("route".to_string(), "v1/responses".to_string());
        tags.insert("error_kind".to_string(), format!("{:?}", last_kind));
        state.sentry.capture_error(
            "GatewayExhaustedError",
            &format!(
                "Responses request failed for model '{}': {}",
                requested_raw_model, last_error
            ),
            Some(tags),
            Some(serde_json::json!({
                "request_id": request_id,
                "last_error": last_error,
                "pool_exhausted": last_pool_exhausted,
            })),
        );
    }

    let mut err_resp = crate::extractors::project_openai_error(&last_kind, &msg);
    if let Some(secs) = last_retry_after {
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            err_resp.headers_mut().insert("retry-after", v);
        }
    }
    inject_telemetry_headers(&mut err_resp, &request_id, &stages);

    let latency = start_time.elapsed();
    state.emit(
        &ctx,
        None,
        GatewayEvent::RequestFailed {
            status_code: err_resp.status().as_u16(),
            latency_ms: latency.as_secs_f64() * 1000.0,
            error: last_error.clone(),
            request_snippet: last_req_snippet,
        },
    );
    err_resp
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", now)
}
