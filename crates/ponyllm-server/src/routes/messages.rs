use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_core::telemetry::FlightFrame;
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_protocol::translator::{anthropic_to_chat_request, chat_to_anthropic_response};
use std::str::FromStr;
use crate::extractors::AppJson;
use crate::routes::chat::inject_routing_headers;
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{openai_sse_to_anthropic_stream, passthrough_sse};

pub async fn handle_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AppJson(req): AppJson<MessageRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());

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

    // 3. Resolve ranked target providers for multi-provider transparent failover
    let targets = match state.resolve_routed_targets(&parsed, header_strategy) {
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

    for target in targets {
        let pool = match state.get_pool(&target.provider_name) {
            Some(p) => p,
            None => continue,
        };

        let executor = UpstreamExecutor::new(pool, state.config.max_retries);
        let is_anthropic_upstream = target.is_anthropic_upstream;

        let mut target_req = req.clone();
        target_req.model = target.physical_model.clone();

        let (target_url, req_val) = if is_anthropic_upstream {
            let url = crate::state::normalize_messages_url(&target.base_url);
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

        if is_streaming {
            match executor.execute_stream_request(&target_url, &req_val).await {
                Ok(upstream_resp) => {
                    let latency = start_time.elapsed();
                    state.metrics.record_request("/v1/messages", latency, 0, 0, true);
                    state.flight_recorder.record(FlightFrame {
                        request_id: request_id.clone(),
                        endpoint: "/v1/messages".to_string(),
                        key_id: target.provider_name.clone(),
                        raw_key: None,
                        status_code: Some(200),
                        latency,
                        error: None,
                        request_snippet: req_snippet,
                        response_snippet: Some("[STREAM_STARTED]".to_string()),
                    });

                    // Stream the raw upstream SSE body. For an OpenAI upstream,
                    // translate OpenAI chat chunks into Anthropic SSE events.
                    let raw_stream = upstream_resp.bytes_stream();
                    let body = if is_anthropic_upstream {
                        axum::body::Body::from_stream(passthrough_sse(raw_stream))
                    } else {
                        axum::body::Body::from_stream(openai_sse_to_anthropic_stream(
                            raw_stream,
                            &target.physical_model,
                        ))
                    };

                    let mut resp = axum::response::Response::new(body);
                    resp.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    inject_routing_headers(&mut resp, &target);
                    return resp;
                }
                Err(err) => {
                    tracing::warn!("Provider '{}' stream failed ({}). Attempting fallback...", target.provider_name, err);
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

                    state.metrics.record_request("/v1/messages", latency, 0, 0, true);
                    state.flight_recorder.record(FlightFrame {
                        request_id: request_id.clone(),
                        endpoint: "/v1/messages".to_string(),
                        key_id: target.provider_name.clone(),
                        raw_key: None,
                        status_code: Some(200),
                        latency,
                        error: None,
                        request_snippet: req_snippet,
                        response_snippet: serde_json::to_string(&ant_resp).ok(),
                    });

                    let mut response = (StatusCode::OK, Json(ant_resp)).into_response();
                    inject_routing_headers(&mut response, &target);
                    return response;
                }
                Err(err) => {
                    tracing::warn!("Provider '{}' json request failed ({}). Attempting fallback...", target.provider_name, err);
                    last_error = err.to_string();
                    continue;
                }
            }
        }
    }

    // All candidate providers exhausted
    let latency = start_time.elapsed();
    state.metrics.record_request("/v1/messages", latency, 0, 0, false);
    state.flight_recorder.record(FlightFrame {
        request_id,
        endpoint: "/v1/messages".to_string(),
        key_id: "all_providers_failed".to_string(),
        raw_key: None,
        status_code: Some(502),
        latency,
        error: Some(last_error.clone()),
        request_snippet: None,
        response_snippet: None,
    });

    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": format!("All candidate upstream providers exhausted. Last error: {}", last_error)
            }
        })),
    )
        .into_response()
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos)
}
