use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::StreamExt;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_core::telemetry::FlightFrame;
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_protocol::translator::{anthropic_to_chat_request, chat_to_anthropic_response};
use std::str::FromStr;
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;

pub async fn handle_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut req): Json<MessageRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());

    // 1. Extract optional X-Pony-Strategy header
    let header_strategy = headers
        .get("x-pony-strategy")
        .or_else(|| headers.get("x-routing-strategy"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| GatewayRoutingStrategy::from_str(s).ok());

    // 2. Parse requested model with sanitization & auto/strategy/1m extraction
    let parsed = ParsedRequestModel::parse(&req.model);
    let requested_raw_model = parsed.raw_requested_model.clone();

    // 3. Resolve target provider and physical model
    let target = match state.resolve_routed_target(&parsed, header_strategy) {
        Ok(t) => t,
        Err(err) => {
            let status = match err {
                CoreError::CapacityExhausted { .. } => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            return (
                status,
                Json(serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "overloaded_error",
                        "message": err.to_string()
                    }
                })),
            )
                .into_response();
        }
    };

    let pool = match state.get_pool(&target.provider_name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "api_error",
                        "message": format!("No key pool for provider '{}'", target.provider_name)
                    }
                })),
            )
                .into_response();
        }
    };

    let executor = UpstreamExecutor::new(pool, state.config.max_retries);
    let is_anthropic_upstream = target.is_anthropic_upstream;

    // Substitute model with clean physical model for upstream
    req.model = target.physical_model.clone();

    let (target_url, req_val) = if is_anthropic_upstream {
        let url = crate::state::normalize_messages_url(&target.base_url);
        let val = match serde_json::to_value(&req) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": format!("Serialization error: {}", e)
                        }
                    })),
                )
                    .into_response();
            }
        };
        (url, val)
    } else {
        let url = crate::state::normalize_chat_completions_url(&target.base_url);
        let chat_req = match anthropic_to_chat_request(&req) {
            Ok(cr) => cr,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": format!("Translation error: {}", e)
                        }
                    })),
                )
                    .into_response();
            }
        };
        let val = match serde_json::to_value(&chat_req) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": format!("Serialization error: {}", e)
                        }
                    })),
                )
                    .into_response();
            }
        };
        (url, val)
    };

    let req_snippet = serde_json::to_string(&req).ok();

    // Handle streaming request
    if req.stream.unwrap_or(false) {
        match executor.execute_stream_request(&target_url, &req_val).await {
            Ok(upstream_resp) => {
                let latency = start_time.elapsed();
                state.metrics.record_request("/v1/messages", latency, 0, 0, true);
                state.flight_recorder.record(FlightFrame {
                    request_id,
                    endpoint: "/v1/messages".to_string(),
                    key_id: target.provider_name.clone(),
                    raw_key: None,
                    status_code: Some(200),
                    latency,
                    error: None,
                    request_snippet: req_snippet,
                    response_snippet: Some("[STREAM_STARTED]".to_string()),
                });

                let stream = upstream_resp.bytes_stream().map(|res| match res {
                    Ok(bytes) => Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(String::from_utf8_lossy(&bytes).to_string())),
                    Err(e) => Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(
                        serde_json::json!({
                            "type": "error",
                            "error": {
                                "type": "api_error",
                                "message": format!("Stream error: {}", e)
                            }
                        }).to_string(),
                    )),
                });

                let mut sse_resp = axum::response::Sse::new(stream).into_response();
                inject_routing_headers(&mut sse_resp, &target);
                return sse_resp;
            }
            Err(err) => {
                let latency = start_time.elapsed();
                state.metrics.record_request("/v1/messages", latency, 0, 0, false);
                state.flight_recorder.record(FlightFrame {
                    request_id,
                    endpoint: "/v1/messages".to_string(),
                    key_id: target.provider_name.clone(),
                    raw_key: None,
                    status_code: Some(502),
                    latency,
                    error: Some(err.to_string()),
                    request_snippet: req_snippet,
                    response_snippet: None,
                });

                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "api_error",
                            "message": err.to_string()
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    match executor.execute_json_request(&target_url, &req_val).await {
        Ok(resp_val) => {
            let latency = start_time.elapsed();
            let mut ant_resp: MessageResponse = if is_anthropic_upstream {
                match serde_json::from_value(resp_val) {
                    Ok(ar) => ar,
                    Err(e) => {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "type": "error",
                                "error": {
                                    "type": "api_error",
                                    "message": format!("Invalid Anthropic upstream response: {}", e)
                                }
                            })),
                        )
                            .into_response();
                    }
                }
            } else {
                let chat_resp: ChatCompletionResponse = match serde_json::from_value(resp_val) {
                    Ok(cr) => cr,
                    Err(e) => {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "type": "error",
                                "error": {
                                    "type": "api_error",
                                    "message": format!("Invalid upstream response format: {}", e)
                                }
                            })),
                        )
                            .into_response();
                    }
                };

                match chat_to_anthropic_response(&chat_resp) {
                    Ok(ar) => ar,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "type": "error",
                                "error": {
                                    "type": "api_error",
                                    "message": format!("Egress translation error: {}", e)
                                }
                            })),
                        )
                            .into_response();
                    }
                }
            };

            // Model Echo Rule: Strictly echo requested model name in MessageResponse
            ant_resp.model = requested_raw_model;

            let resp_snippet = serde_json::to_string(&ant_resp).ok();
            state.metrics.record_request(
                "/v1/messages",
                latency,
                ant_resp.usage.input_tokens as u64,
                ant_resp.usage.output_tokens as u64,
                true,
            );
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/messages".to_string(),
                key_id: target.provider_name.clone(),
                raw_key: None,
                status_code: Some(200),
                latency,
                error: None,
                request_snippet: req_snippet,
                response_snippet: resp_snippet,
            });

            let mut response = (StatusCode::OK, Json(ant_resp)).into_response();
            inject_routing_headers(&mut response, &target);
            response
        }
        Err(err) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/messages", latency, 0, 0, false);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/messages".to_string(),
                key_id: target.provider_name.clone(),
                raw_key: None,
                status_code: Some(502),
                latency,
                error: Some(err.to_string()),
                request_snippet: req_snippet,
                response_snippet: None,
            });

            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "api_error",
                        "message": err.to_string()
                    }
                })),
            )
                .into_response()
        }
    }
}

fn inject_routing_headers(resp: &mut axum::response::Response, target: &crate::state::RoutedTarget) {
    let headers = resp.headers_mut();
    if let Ok(val) = HeaderValue::from_str(&target.physical_model) {
        headers.insert("x-ponyllm-routed-model", val);
    }
    if let Ok(val) = HeaderValue::from_str(&target.provider_name) {
        headers.insert("x-ponyllm-provider", val);
    }
    if let Ok(val) = HeaderValue::from_str(&target.strategy.to_string()) {
        headers.insert("x-ponyllm-strategy", val);
    }
    if let Ok(val) = HeaderValue::from_str(&target.tier.to_string()) {
        headers.insert("x-ponyllm-tier", val);
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
