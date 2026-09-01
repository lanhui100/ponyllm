use std::sync::Arc;
use std::time::Instant;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::telemetry::FlightFrame;
use ponyllm_protocol::openai::responses::CreateResponseRequest;
use crate::state::AppState;

pub async fn handle_responses(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateResponseRequest>,
) -> impl IntoResponse {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());

    let (provider_name, provider_cfg) = match state.config.providers.iter().next() {
        Some((name, cfg)) => (name.clone(), cfg.clone()),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": {"message": "No upstream provider configured"}})),
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
    let target_url = format!("{}/v1/responses", provider_cfg.base_url.trim_end_matches('/'));

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

    match executor.execute_json_request(&target_url, &req_val).await {
        Ok(resp_val) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/responses", latency, 0, 0, true);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/responses".to_string(),
                key_id: provider_name,
                raw_key: None,
                status_code: Some(200),
                latency,
                error: None,
                request_snippet: req_snippet,
                response_snippet: Some(resp_val.to_string()),
            });

            (StatusCode::OK, Json(resp_val)).into_response()
        }
        Err(err) => {
            let latency = start_time.elapsed();
            state.metrics.record_request("/v1/responses", latency, 0, 0, false);
            state.flight_recorder.record(FlightFrame {
                request_id,
                endpoint: "/v1/responses".to_string(),
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
