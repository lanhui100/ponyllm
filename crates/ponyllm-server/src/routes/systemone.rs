use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::executor::{is_opencode_zen_target, EventSinkCtx, UpstreamExecutor};
use ponyllm_core::pool::UpstreamProtocol;
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};

use crate::extractors::{format_exhausted_message, project_openai_error, AppJson};
use crate::routes::models::ParsedRequestModel;
use crate::state::AppState;
use crate::streaming::extract_usage_tokens;

pub const SYSTEMONE_MAX_JSON_BYTES: usize = 512 * 1024;
const SYSTEMONE_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Pure systemone passthrough. The gateway resolves model/provider, injects
/// authentication/Zen client headers, records telemetry, and keeps Jev's JSON
/// schema opaque to this route.
pub async fn handle_systemone(
    State(state): State<Arc<AppState>>,
    AppJson(body): AppJson<Value>,
) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = format!("req_{}", uuid_simple());
    let model = body.get("model").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let endpoint = "/v1/systemone".to_string();
    let ctx = EventCtx { request_id: request_id.clone(), session_id: None, model: Some(model.clone()), endpoint: endpoint.clone(), start };
    let stages = Arc::new(Mutex::new(StageTimings::default()));
    let body_size = serde_json::to_vec(&body).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    if body_size > SYSTEMONE_MAX_JSON_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error":{"message":"systemone request body exceeds 512 KiB","type":"invalid_request_error","code":"payload_too_large"}}))).into_response();
    }
    let request_snippet = Some(systemone_audit_snippet(&body));
    if model.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"model is required","type":"invalid_request_error","code":"invalid_input"}}))).into_response();
    }

    let parsed = ParsedRequestModel::parse(&model);
    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_full(&parsed, None, None, None, None, &[]) {
        Ok(ts) => ts.into_iter().filter(|target| target.upstream_protocol == UpstreamProtocol::Systemone).collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    let targets = match targets {
        ts if !ts.is_empty() => ts,
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error":{"message":format!("The model '{}' does not exist or you do not have access to it.", model),"type":"invalid_request_error","code":"model_not_found"}}))).into_response(),
    };
    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(&ctx, Some(targets[0].provider_name.clone()), GatewayEvent::RouteResolved { provider: targets[0].provider_name.clone(), translated: false, routing_ms });

    let mut last_error = String::new();
    let mut last_kind = GatewayErrorKind::Internal;
    let mut last_pool_exhausted = false;
    let mut last_retry_after = None;
    let mut last_provider: Option<String> = None;

    for target in targets {
        let provider = target.provider_name.clone();
        last_provider = Some(provider.clone());
        let Some(pool) = state.get_pool(&provider) else { last_error = format!("No key pool for provider '{}'", provider); continue; };
        let mut upstream_body = body.clone();
        if let Some(obj) = upstream_body.as_object_mut() { obj.insert("model".to_string(), Value::String(target.physical_model.clone())); }
        let target_url = target.systemone_url();
        // VULN-07/F6: refuse to dial upstream URLs that resolve (or rebind)
        // to metadata/private/CGNAT/benchmark ranges — re-validated here at
        // dial time (cached per host), not only at provider write time.
        if let Err(reason) = state.data_plane_egress_guard(&target_url).await {
            last_error = reason;
            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
            tracing::warn!(provider = %provider, url = %target_url, "data-plane egress guard refused upstream; skipping target");
            continue;
        }
        let sink_ctx = EventSinkCtx { request_id: request_id.clone(), endpoint: endpoint.clone(), provider: provider.clone(), model: Some(model.clone()), start, stages: stages.clone(), request_snippet: request_snippet.clone() };
        let client = state.http_client_for_target(&provider, &target.physical_model);
        let (max_retries, ttfb_timeout) = {
            let cfg = state.config.read();
            (cfg.max_retries, cfg.effective_ttfb_timeout(&provider))
        };
        let executor = UpstreamExecutor::with_client(pool.clone(), client, max_retries)
            .with_opencode_zen(is_opencode_zen_target(&provider, &target_url))
            .with_systemone(true)
            .with_ttfb_timeout(ttfb_timeout)
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx));

        match executor.execute_json_request_with_key(&target_url, &upstream_body).await {
            Ok((response, key_id)) => {
                let response_bytes = serde_json::to_vec(&response).unwrap_or_default();
                if response_bytes.len() > SYSTEMONE_MAX_RESPONSE_BYTES {
                    last_kind = GatewayErrorKind::UpstreamUnavailable;
                    last_error = "systemone upstream response exceeds 4 MiB".to_string();
                    continue;
                }
                let (prompt_tokens, completion_tokens, cached_tokens) = extract_usage_tokens(&response);
                let wall_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                pool.record_tokens(&key_id, wall_ms, prompt_tokens, completion_tokens, cached_tokens);
                let latency = start.elapsed();
                let tps = (completion_tokens > 0 && latency.as_secs_f64() > 0.05).then(|| completion_tokens as f64 / latency.as_secs_f64());
                state.emit(&ctx, Some(provider), GatewayEvent::RequestCompleted { status_code: 200, latency_ms: latency.as_secs_f64() * 1000.0, prompt_tokens, completion_tokens, cached_tokens, tps, request_snippet: request_snippet.clone(), response_snippet: Some(systemone_audit_snippet(&response)) });
                return (StatusCode::OK, Json(response)).into_response();
            }
            Err(err) => {
                if let CoreError::UpstreamStatusError { status, body: upstream_body } = &err {
                    if status.is_client_error() && *status != StatusCode::TOO_MANY_REQUESTS {
                        return systemone_upstream_error_response(*status, upstream_body);
                    }
                }
                last_kind = err.kind();
                last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                last_retry_after = crate::extractors::retry_after_secs(&last_kind, crate::routes::chat::retry_unlock_hint(&last_kind, &pool, None));
                last_error = err.to_string();
            }
        }
    }

    let message = format_exhausted_message(&model, &last_kind, &last_error, last_pool_exhausted, &request_id);
    let mut response = project_openai_error(&last_kind, &message);
    if let Some(secs) = last_retry_after { if let Ok(value) = secs.to_string().parse() { response.headers_mut().insert("retry-after", value); } }
    let latency = start.elapsed();
    state.emit(&ctx, last_provider, GatewayEvent::RequestFailed { status_code: response.status().as_u16(), latency_ms: latency.as_secs_f64() * 1000.0, error: last_error.clone(), request_snippet });
    response
}

fn redact_upstream_error(text: &str) -> String {
    let mut out = ponyllm_core::telemetry::scrub_secrets(text);
    for marker in ["proxy-authenticate:", "proxy-authorization:", "authorization:", "x-api-key:"] {
        let lower = out.to_ascii_lowercase();
        if let Some(start) = lower.find(marker) {
            let end = out[start..].find('\n').map(|i| start + i).unwrap_or(out.len());
            out.replace_range(start..end, "[redacted-header]");
        }
    }
    out
}

fn systemone_upstream_error_response(status: StatusCode, body: &str) -> axum::response::Response {
    if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return project_openai_error(&GatewayErrorKind::UpstreamUnavailable, "systemone proxy authentication failed");
    }
    const MAX_ERROR_BYTES: usize = 64 * 1024;
    let bounded = if body.len() > MAX_ERROR_BYTES { &body[..body.floor_char_boundary(MAX_ERROR_BYTES)] } else { body };
    let bounded = redact_upstream_error(bounded);
    if let Ok(value) = serde_json::from_str::<Value>(&bounded) { return (status, Json(value)).into_response(); }
    (status, [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"))], bounded).into_response()
}

fn systemone_audit_snippet(value: &Value) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new(); hasher.update(&encoded);
    format!("systemone_json_bytes={} sha256={:x}", encoded.len(), hasher.finalize())
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    format!("{:x}", now)
}
