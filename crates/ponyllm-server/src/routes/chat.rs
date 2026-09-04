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
use ponyllm_protocol::anthropic::messages::MessageResponse;
use ponyllm_protocol::openai::chat::ChatCompletionRequest;
use ponyllm_protocol::translator::{anthropic_to_chat_response, chat_to_anthropic_request};
use parking_lot::Mutex;
use std::str::FromStr;
use crate::extractors::AppJson;
use crate::routes::models::ParsedRequestModel;
use crate::state::{AppState, RoutedTarget};
use crate::streaming::{
    anthropic_sse_to_openai_stream, extract_usage_tokens, passthrough_sse,
    wrap_telemetry_stream, StreamFailureContext,
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
    let prompt_hint = req.messages.first().and_then(extract_chat_prompt);
    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_with_prompt(&parsed, header_strategy, prompt_hint.as_deref()) {
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

    let is_streaming = req.stream.unwrap_or(false);
    let mut last_error = String::new();
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;
    let mut last_req_snippet: Option<String> = None;

    // Journey start: routing cost is the first attributable segment.
    // (Pre-routing validation rejections stay silent, as before.)
    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    let first_provider = targets[0].provider_name.clone();
    let first_translated = targets[0].is_anthropic_upstream;
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
        let is_anthropic_upstream = target.is_anthropic_upstream;

        let mut target_req = req.clone();
        target_req.model = target.physical_model.clone();

        let (target_url, req_val) = if is_anthropic_upstream {
            let url = crate::state::normalize_messages_url(&target.base_url);
            let ant_req = match chat_to_anthropic_request(&target_req) {
                Ok(ar) => ar,
                Err(e) => {
                    last_error = format!("Translation error for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            let val = match serde_json::to_value(&ant_req) {
                Ok(v) => v,
                Err(e) => {
                    last_error = format!("Serialization error for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            (url, val)
        } else {
            let url = crate::state::normalize_chat_completions_url(&target.base_url);
            let val = match serde_json::to_value(&target_req) {
                Ok(v) => v,
                Err(e) => {
                    last_error = format!("Invalid JSON for {}: {}", target.provider_name, e);
                    continue;
                }
            };
            (url, val)
        };

        let req_snippet = Some(req_val.to_string());
        last_req_snippet = req_snippet.clone();

        // Every per-key retry inside the executor appends attempt events to
        // the bus; metrics and frames derive from the same single-append log.
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

                    // Stream the raw upstream SSE body. For an Anthropic upstream,
                    // translate Anthropic SSE events into OpenAI chat chunks.
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
                        let stream = anthropic_sse_to_openai_stream(
                            raw_stream,
                            &target.physical_model,
                        );
                        let monitored = wrap_telemetry_stream(stream, failure_ctx);
                        axum::body::Body::from_stream(monitored)
                    } else {
                        let stream = passthrough_sse(raw_stream);
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
                    let mut final_val = if is_anthropic_upstream {
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
                    } else {
                        resp_val
                    };

                    // Model Echo Rule: Strictly echo requested model name in response body
                    if let Some(obj) = final_val.as_object_mut() {
                        obj.insert("model".to_string(), serde_json::json!(requested_raw_model));
                    }

                    let (prompt_tokens, completion_tokens) = extract_usage_tokens(&final_val);
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
    let mut resp = crate::extractors::project_openai_error(&last_kind, &msg);
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
