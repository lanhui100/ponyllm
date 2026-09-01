use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use futures_util::StreamExt;
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::telemetry::FlightFrame;
use ponyllm_protocol::anthropic::messages::MessageRequest;
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_protocol::translator::{anthropic_to_chat_request, chat_to_anthropic_response};
use crate::state::AppState;

pub async fn handle_messages(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MessageRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());

    // Resolve provider dynamically based on model name
    let (provider_name, provider_cfg) = match state.resolve_provider(&req.model) {
        Some((name, cfg)) => (name, cfg),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": {"message": format!("No upstream provider found for model '{}'", req.model)}})),
            )
                .into_response();
        }
    };

    let pool = match state.get_pool(&provider_name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": {"message": format!("No key pool for provider '{}'", provider_name)}})),
            )
                .into_response();
        }
    };

    let executor = UpstreamExecutor::new(pool, state.config.max_retries);
    let target_url = format!("{}/v1/chat/completions", provider_cfg.base_url.trim_end_matches('/'));

    // Translate incoming Anthropic request to OpenAI Chat request
    let chat_req = match anthropic_to_chat_request(&req) {
        Ok(cr) => cr,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"message": format!("Translation error: {}", e)}})),
            )
                .into_response();
        }
    };

    let req_val = match serde_json::to_value(&chat_req) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"message": format!("Serialization error: {}", e)}})),
            )
                .into_response();
        }
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
                    key_id: provider_name,
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
                        serde_json::json!({"error": {"message": format!("Stream error: {}", e), "type": "upstream_error"}}).to_string(),
                    )),
                });

                return axum::response::Sse::new(stream).into_response();
            }
            Err(err) => {
                let latency = start_time.elapsed();
                state.metrics.record_request("/v1/messages", latency, 0, 0, false);
                state.flight_recorder.record(FlightFrame {
                    request_id,
                    endpoint: "/v1/messages".to_string(),
                    key_id: provider_name,
                    raw_key: None,
                    status_code: Some(502),
                    latency,
                    error: Some(err.to_string()),
                    request_snippet: req_snippet,
                    response_snippet: None,
                });

                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": {"message": err.to_string()}})),
                )
                    .into_response();
            }
        }
    }

    match executor.execute_json_request(&target_url, &req_val).await {
        Ok(resp_val) => {
            let latency = start_time.elapsed();
            let chat_resp: ChatCompletionResponse = match serde_json::from_value(resp_val) {
                Ok(cr) => cr,
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": {"message": format!("Invalid upstream response format: {}", e)}})),
                    )
                        .into_response();
                }
            };

            let ant_resp = match chat_to_anthropic_response(&chat_resp) {
                Ok(ar) => ar,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": {"message": format!("Egress translation error: {}", e)}})),
                    )
                        .into_response();
                }
            };

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
                key_id: provider_name,
                raw_key: None,
                status_code: Some(200),
                latency,
                error: None,
                request_snippet: req_snippet,
                response_snippet: resp_snippet,
            });

            (StatusCode::OK, Json(ant_resp)).into_response()
        }
        Err(err) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/messages", latency, 0, 0, false);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/messages".to_string(),
                key_id: provider_name,
                raw_key: None,
                status_code: Some(502),
                latency,
                error: Some(err.to_string()),
                request_snippet: req_snippet,
                response_snippet: None,
            });

            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": {"message": err.to_string()}})),
            )
                .into_response()
        }
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
