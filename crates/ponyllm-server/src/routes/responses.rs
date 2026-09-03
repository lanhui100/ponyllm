use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::telemetry::FlightFrame;
use ponyllm_protocol::openai::responses::CreateResponseRequest;
use crate::extractors::AppJson;
use crate::routes::chat::inject_routing_headers;
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::passthrough_sse;

pub async fn handle_responses(
    State(state): State<Arc<AppState>>,
    AppJson(mut req): AppJson<CreateResponseRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());

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

    let (provider_name, physical_model, target) = match state.resolve_routed_target(&parsed, None) {
        Ok(t) => (t.provider_name.clone(), t.physical_model.clone(), t),
        Err(err) => {
            let (status, code) = match err {
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

    // Responses API is OpenAI-protocol only; route to the provider's
    // /v1/responses endpoint and set the physical model in the body.
    let max_retries = state.config.read().max_retries;
    let executor = UpstreamExecutor::new(pool, max_retries);
    let target_url = normalize_responses_url(&target.base_url);

    req.model = physical_model.clone();
    let req_val = match serde_json::to_value(&req) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"message": format!("Invalid JSON: {}", e)}})),
            )
                .into_response();
        }
    };

    let req_snippet = Some(req_val.to_string());

    // Handle streaming request: pass through upstream SSE unchanged
    if req.stream.unwrap_or(false) {
        match executor.execute_stream_request(&target_url, &req_val).await {
            Ok(upstream_resp) => {
                let latency = start_time.elapsed();
                state.metrics.record_request("/v1/responses", latency, 0, 0, true);
                state.flight_recorder.record(FlightFrame {
                    request_id,
                    endpoint: "/v1/responses".to_string(),
                    key_id: provider_name.clone(),
                    raw_key: None,
                    status_code: Some(200),
                    latency,
                    error: None,
                    request_snippet: req_snippet,
                    response_snippet: Some("[STREAM_STARTED]".to_string()),
                });

                let body = axum::body::Body::from_stream(passthrough_sse(upstream_resp.bytes_stream()));
                let mut resp = axum::response::Response::new(body);
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                inject_routing_headers(&mut resp, &target);
                return resp;
            }
            Err(err) => {
                let latency = start_time.elapsed();
                state.metrics.record_request("/v1/responses", latency, 0, 0, false);
                state.flight_recorder.record(FlightFrame {
                    request_id,
                    endpoint: "/v1/responses".to_string(),
                    key_id: provider_name.clone(),
                    raw_key: None,
                    status_code: Some(502),
                    latency,
                    error: Some(err.to_string()),
                    request_snippet: req_snippet,
                    response_snippet: None,
                });

                return crate::extractors::project_openai_error(&err.kind(), &err.to_string());
            }
        }
    }

    match executor.execute_json_request(&target_url, &req_val).await {
        Ok(mut resp_val) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/responses", latency, 0, 0, true);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/responses".to_string(),
                key_id: provider_name.clone(),
                raw_key: None,
                status_code: Some(200),
                latency,
                error: None,
                request_snippet: req_snippet,
                response_snippet: Some(resp_val.to_string()),
            });

            // Model Echo Rule: strictly echo requested model name in response body
            if let Some(obj) = resp_val.as_object_mut() {
                obj.insert("model".to_string(), serde_json::json!(requested_raw_model));
            }

            let mut response = (StatusCode::OK, Json(resp_val)).into_response();
            inject_routing_headers(&mut response, &target);
            response
        }
        Err(err) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/responses", latency, 0, 0, false);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/responses".to_string(),
                key_id: provider_name.clone(),
                raw_key: None,
                status_code: Some(502),
                latency,
                error: Some(err.to_string()),
                request_snippet: req_snippet,
                response_snippet: None,
            });

            crate::extractors::project_openai_error(&err.kind(), &err.to_string())
        }
    }
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
