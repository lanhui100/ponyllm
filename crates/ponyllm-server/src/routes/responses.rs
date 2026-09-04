use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::executor::{EventSinkCtx, UpstreamExecutor};
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};
use ponyllm_protocol::openai::responses::CreateResponseRequest;
use parking_lot::Mutex;
use crate::extractors::AppJson;
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::{passthrough_sse, wrap_telemetry_stream, StreamFailureContext};

pub async fn handle_responses(
    State(state): State<Arc<AppState>>,
    AppJson(mut req): AppJson<CreateResponseRequest>,
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

    let routing_start = Instant::now();
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

    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: provider_name.clone(),
            translated: false,
            routing_ms,
        },
    );

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
    if req.stream.unwrap_or(false) {
        match executor.execute_stream_request(&target_url, &req_val).await {
            Ok(upstream_resp) => {
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
                let latency = start_time.elapsed();
                state.emit(
                    &ctx,
                    Some(provider_name.clone()),
                    GatewayEvent::RequestFailed {
                        status_code: 502,
                        latency_ms: latency.as_secs_f64() * 1000.0,
                        error: err.to_string(),
                        request_snippet: req_snippet.clone(),
                    },
                );

                let mut err_resp =
                    crate::extractors::project_openai_error(&err.kind(), &err.to_string());
                inject_telemetry_headers(&mut err_resp, &request_id, &stages);
                return err_resp;
            }
        }
    }

    match executor.execute_json_request(&target_url, &req_val).await {
        Ok(mut resp_val) => {
            let latency = start_time.elapsed();
            // Token accounting stays at HEAD behavior (no usage split).
            state.emit(
                &ctx,
                Some(provider_name.clone()),
                GatewayEvent::RequestCompleted {
                    status_code: 200,
                    latency_ms: latency.as_secs_f64() * 1000.0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    tps: None,
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
            response
        }
        Err(err) => {
            let latency = start_time.elapsed();
            state.emit(
                &ctx,
                Some(provider_name.clone()),
                GatewayEvent::RequestFailed {
                    status_code: 502,
                    latency_ms: latency.as_secs_f64() * 1000.0,
                    error: err.to_string(),
                    request_snippet: req_snippet.clone(),
                },
            );

            let mut err_resp =
                crate::extractors::project_openai_error(&err.kind(), &err.to_string());
            inject_telemetry_headers(&mut err_resp, &request_id, &stages);
            err_resp
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
