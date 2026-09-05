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
use ponyllm_protocol::openai::responses::CreateResponseRequest;
use parking_lot::Mutex;
use std::str::FromStr;
use crate::extractors::AppJson;
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{anthropic_sse_to_responses_stream, chat_sse_to_responses_stream, extract_usage_tokens, passthrough_sse, wrap_telemetry_stream, StreamFailureContext};

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

    // Parse requested model (auto / [1m] / :strategy suffix) and resolve the
    // physical model + provider, mirroring chat/messages routing so virtual
    // model names are correctly mapped upstream.
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();
    let prompt_hint = match &req.input {
        ponyllm_protocol::openai::responses::ResponseInput::Text(t) => Some(t.clone()),
        ponyllm_protocol::openai::responses::ResponseInput::Items(_) => serde_json::to_string(&req.input).ok(),
    };
    let prompt_ref = prompt_hint.as_deref();

    let routing_start = Instant::now();
    let header_strategy = headers
        .get("x-pony-strategy")
        .or_else(|| headers.get("x-routing-strategy"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| GatewayRoutingStrategy::from_str(s).ok());
    let header_thinking = crate::extractors::parse_thinking_header(&headers);
    let targets = match state.resolve_routed_targets_with_prompt_and_protocol(&parsed, header_strategy, prompt_ref, crate::extractors::parse_protocol_header(&headers), Some(ponyllm_core::pool::UpstreamProtocol::Responses)) {
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
                ponyllm_core::error::CoreError::CapacityExhausted { .. } => (StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted"),
                ponyllm_core::error::CoreError::Internal(ref msg) if msg.contains("No provider configured") => {
                    (StatusCode::NOT_FOUND, "model_not_found")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
            };
            return (
                status,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("No upstream provider found for model '{}'", req.model),
                        "type": "invalid_request_error",
                        "code": code
                    }
                })),
            )
                .into_response();
        }
    };
    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(targets[0].provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: targets[0].upstream_protocol != ponyllm_core::pool::UpstreamProtocol::Responses,
            routing_ms,
        },
    );

    let mut last_error = String::new();
    let mut last_pool_exhausted = false;
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;
    let mut last_req_snippet: Option<String> = None;
    let is_streaming = req.stream.unwrap_or(false);

    for target in targets {
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

        let requested_thinking = header_thinking
            .or(parsed.thinking_override)
            .or_else(|| req.get_reasoning_effort());
        let effective_thinking = target.resolve_thinking(requested_thinking);

        if effective_thinking.is_active() {
            target_req.reasoning_effort = Some(effective_thinking);
            target_req.reasoning = Some(ponyllm_protocol::openai::responses::ResponseReasoningConfig {
                effort: Some(effective_thinking),
            });
        } else {
            target_req.reasoning_effort = None;
            target_req.reasoning = None;
            target_req.extra.remove("reasoning_effort");
            target_req.extra.remove("reasoning");
        }

        let (target_url, req_val) = match target.upstream_protocol {
            ponyllm_core::pool::UpstreamProtocol::Chat => {
                let mut chat_req = match ponyllm_protocol::translator::responses_to_chat_request(&target_req) {
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
                let mut ant_req = match ponyllm_protocol::translator::responses_to_anthropic_request(&target_req) {
                    Ok(ar) => ar,
                    Err(e) => {
                        last_error = format!("Translation error for {}: {}", provider_name, e);
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
                    ant_req.reasoning_effort = None;
                    ant_req.thinking = None;
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
        };

        let req_snippet = Some(req_val.to_string());
        last_req_snippet = req_snippet.clone();
        let sink_ctx = EventSinkCtx {
            request_id: request_id.clone(),
            endpoint: endpoint.clone(),
            provider: provider_name.clone(),
            start: start_time,
            stages: stages.clone(),
            request_snippet: req_snippet.clone(),
        };
        let executor = UpstreamExecutor::new(pool, max_retries)
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx));

        // Handle streaming request: pass through upstream SSE unchanged
        if is_streaming {
            match executor.execute_stream_request(&target_url, &req_val).await {
                Ok(upstream_resp) => {
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

                    let failure_ctx = StreamFailureContext {
                        bus: state.event_bus.clone(),
                        ctx: ctx.clone(),
                        provider: provider_name.clone(),
                        stages: stages.clone(),
                        request_snippet: req_snippet.clone(),
                    };
                    // Same-protocol upstreams stream through untouched;
                    // mismatched natives are translated into Responses events.
                    let body = match target.upstream_protocol {
                        ponyllm_core::pool::UpstreamProtocol::Chat => {
                            let stream = chat_sse_to_responses_stream(
                                upstream_resp.bytes_stream(),
                                &target.physical_model,
                            );
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Anthropic => {
                            let stream = anthropic_sse_to_responses_stream(
                                upstream_resp.bytes_stream(),
                                &target.physical_model,
                            );
                            let monitored = wrap_telemetry_stream(stream, failure_ctx);
                            axum::body::Body::from_stream(monitored)
                        }
                        ponyllm_core::pool::UpstreamProtocol::Responses => {
                            let stream = passthrough_sse(upstream_resp.bytes_stream());
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
                    tracing::warn!("Provider '{}' responses stream failed ({}). Attempting fallback...", provider_name, err);
                    last_kind = err.kind();
                    last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                    last_error = err.to_string();
                    continue;
                }
            }
        }

        match executor.execute_json_request(&target_url, &req_val).await {
            Ok(resp_val) => {
                let mut resp_val = match target.upstream_protocol {
                    ponyllm_core::pool::UpstreamProtocol::Chat => {
                        let chat_resp: ponyllm_protocol::openai::chat::ChatCompletionResponse =
                            match serde_json::from_value(resp_val) {
                                Ok(cr) => cr,
                                Err(e) => {
                                    last_error = format!("Invalid Chat response from {}: {}", provider_name, e);
                                    continue;
                                }
                            };
                        let resp_obj = match ponyllm_protocol::translator::chat_to_responses_response(&chat_resp) {
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
                                    last_error = format!("Invalid Anthropic response from {}: {}", provider_name, e);
                                    continue;
                                }
                            };
                        let resp_obj = match ponyllm_protocol::translator::anthropic_to_responses_response(&ant_resp) {
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
                };
                let latency = start_time.elapsed();
                let (prompt_tokens, completion_tokens) = extract_usage_tokens(&resp_val);
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
            Err(err) => {
                tracing::warn!("Provider '{}' responses request failed ({}). Attempting fallback...", provider_name, err);
                last_kind = err.kind();
                last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                last_error = err.to_string();
                continue;
            }
        }
    }

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

    let msg = crate::extractors::format_exhausted_message(&last_error, last_pool_exhausted, &request_id);
    let mut err_resp = crate::extractors::project_openai_error(&last_kind, &msg);
    inject_telemetry_headers(&mut err_resp, &request_id, &stages);
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
