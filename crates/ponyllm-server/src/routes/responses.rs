use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
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
use crate::streaming::{extract_usage_tokens, passthrough_sse, wrap_telemetry_stream, StreamFailureContext};

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
    let targets = match state.resolve_routed_targets_with_prompt(&parsed, header_strategy, prompt_ref) {
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
            translated: false,
            routing_ms,
        },
    );

    let mut last_error = String::new();
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

        // Responses API is OpenAI-protocol only; route to the provider's
        // /v1/responses endpoint and set the physical model in the body.
        let max_retries = state.config.read().max_retries;
        let target_url = normalize_responses_url(&target.base_url);

        let mut target_req = req.clone();
        target_req.model = physical_model.clone();
        let req_val = match serde_json::to_value(&target_req) {
            Ok(v) => v,
            Err(e) => {
                last_error = format!("Invalid JSON for {}: {}", provider_name, e);
                continue;
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

                    let stream = passthrough_sse(upstream_resp.bytes_stream());
                    let failure_ctx = StreamFailureContext {
                        bus: state.event_bus.clone(),
                        ctx: ctx.clone(),
                        provider: provider_name.clone(),
                        stages: stages.clone(),
                        request_snippet: req_snippet.clone(),
                    };
                    let monitored = wrap_telemetry_stream(stream, failure_ctx);
                    let body = axum::body::Body::from_stream(monitored);
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
                    last_error = err.to_string();
                    continue;
                }
            }
        }

        match executor.execute_json_request(&target_url, &req_val).await {
            Ok(mut resp_val) => {
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

    let msg = crate::extractors::format_exhausted_message(&last_error, &request_id);
    let mut err_resp = crate::extractors::project_openai_error(&last_kind, &msg);
    inject_telemetry_headers(&mut err_resp, &request_id, &stages);
    err_resp
}

fn normalize_responses_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1/responses") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{}/responses", trimmed)
    } else {
        format!("{}/v1/responses", trimmed)
    }
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", now)
}
