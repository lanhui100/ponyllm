//! OpenAI Images API endpoints — `/v1/images/generations` and `/v1/images/edits`.
//!
//! Routes the request to a provider whose resolved protocol is Antigravity and
//! whose model declares `image` in `output_types`, translates to the upstream
//! `v1internal:generateContent` wire shape, and answers with the OpenAI Images
//! response format (`{created, data: [{b64_json}], model}`).
//!
//! Known constraints (probe-verified upstream, see the interface ADR):
//! - `n > 1` is rejected: the model does not support `candidateCount`.
//! - `response_format` is always answered as `b64_json` (the gateway has no
//!   object-storage URL hosting); requesting `url` returns `b64_json` too.
//! - OpenAI's `mask` has no Antigravity equivalent and is accepted-but-ignored.

use std::ops::Deref;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use parking_lot::Mutex;
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::{is_opencode_zen_target, EventSinkCtx, UpstreamExecutor};
use ponyllm_core::pool::UpstreamProtocol;
use ponyllm_core::telemetry::{EventCtx, GatewayEvent, StageTimings};
use ponyllm_protocol::openai::images::ImageEditRequest;
use ponyllm_protocol::translator::{
    antigravity_to_images_response, images_to_antigravity_request,
    openai_size_to_antigravity_aspect_ratio,
};

use crate::extractors::{format_request_snippet, AppJson};
use crate::routes::chat::{inject_routing_headers, inject_telemetry_headers};
use crate::routes::models::ParsedRequestModel;
use crate::state::{AppState, RoutedTarget};

fn uuid_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn bad_request(message: &str, code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": code
            }
        })),
    )
        .into_response()
}

/// Normalize an OpenAI image input (raw base64 or `data:<mime>;base64,<data>`)
/// into `(mime, base64)` for the Antigravity inlineData part.
fn extract_image_part(raw: &str, default_mime: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix("data:") {
        if let Some((mime, b64)) = stripped.split_once(";base64,") {
            return Some((mime.trim().to_string(), b64.trim().to_string()));
        }
    }
    Some((default_mime.to_string(), raw.to_string()))
}

/// Normalized `/v1/images/edits` input from either wire shape.
#[derive(Debug, Default)]
pub struct ImageEditInput {
    model: String,
    prompt: String,
    image: Option<String>,
    size: Option<String>,
    n: Option<u32>,
}

/// Accepts `application/json` (base64 / data-URI `image` field) and
/// `multipart/form-data` (OpenAI SDK native: `image`/`mask` file parts).
/// `mask` is accepted-but-ignored (no Antigravity equivalent).
impl<S> FromRequest<S> for ImageEditInput
where
    S: Send + Sync + Deref<Target = AppState>,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_multipart = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.starts_with("multipart/form-data"))
            .unwrap_or(false);

        if is_multipart {
            let mut mp = match Multipart::from_request(request, state).await {
                Ok(m) => m,
                Err(_) => return Err(bad_request("invalid multipart body", "invalid_input")),
            };
            let mut out = ImageEditInput::default();
            while let Ok(Some(field)) = mp.next_field().await {
                let name = field.name().unwrap_or("").to_string();
                match name.as_str() {
                    "model" => {
                        if let Ok(v) = field.text().await {
                            out.model = v;
                        }
                    }
                    "prompt" => {
                        if let Ok(v) = field.text().await {
                            out.prompt = v;
                        }
                    }
                    "size" => {
                        if let Ok(v) = field.text().await {
                            out.size = Some(v);
                        }
                    }
                    "n" => {
                        if let Ok(v) = field.text().await {
                            out.n = v.trim().parse::<u32>().ok();
                        }
                    }
                    "image" => {
                        let mime = field
                            .content_type()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "image/png".to_string());
                        if let Ok(bytes) = field.bytes().await {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                            out.image = Some(format!("data:{};base64,{}", mime, b64));
                        }
                    }
                    // `mask` accepted-but-ignored.
                    _ => {}
                }
            }
            Ok(out)
        } else {
            let limit = state.deref().config.read().request_body_limit;
            let bytes = match axum::body::to_bytes(request.into_body(), limit).await {
                Ok(b) => b,
                Err(_) => return Err(bad_request("failed to read request body", "invalid_input")),
            };
            let req: ImageEditRequest = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(e) => {
                    return Err(bad_request(&format!("invalid JSON body: {}", e), "invalid_input"));
                }
            };
            Ok(ImageEditInput {
                model: req.model,
                prompt: req.prompt,
                image: req.image,
                size: req.size,
                n: req.n,
            })
        }
    }
}

/// Validate the resolved target can serve an images request.
fn image_target_error(target: &RoutedTarget) -> Option<(&'static str, &'static str)> {
    if target.upstream_protocol != UpstreamProtocol::Antigravity {
        return Some((
            "Images API is only supported on the antigravity protocol provider",
            "protocol_mismatch",
        ));
    }
    if !target.output_types.iter().any(|t| t.eq_ignore_ascii_case("image")) {
        return Some((
            "Model does not declare image output; images endpoints require output_types = [\"image\"]",
            "unsupported_output_type",
        ));
    }
    None
}

/// `POST /v1/images/generations` — text prompt → image.
pub async fn handle_image_generations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AppJson(req): AppJson<ponyllm_protocol::openai::images::ImageGenerationRequest>,
) -> Response {
    run_images_request(
        state,
        headers,
        &req.model,
        &req.prompt,
        None,
        req.size.as_deref(),
        req.n,
        "/v1/images/generations",
    )
    .await
}

/// `POST /v1/images/edits` — image + prompt → edited image.
pub async fn handle_image_edits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    input: ImageEditInput,
) -> Response {
    if input.image.as_deref().unwrap_or("").trim().is_empty() {
        return bad_request("image is required for image editing", "missing_image");
    }
    run_images_request(
        state,
        headers,
        &input.model,
        &input.prompt,
        input.image.as_deref(),
        input.size.as_deref(),
        input.n,
        "/v1/images/edits",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_images_request(
    state: Arc<AppState>,
    headers: HeaderMap,
    model: &str,
    prompt: &str,
    image: Option<&str>,
    size: Option<&str>,
    n: Option<u32>,
    endpoint: &str,
) -> Response {
    let start_time = Instant::now();
    let request_id = format!("req_{}", uuid_simple());
    let ctx = EventCtx {
        request_id: request_id.clone(),
        session_id: None,
        model: Some(model.to_string()),
        endpoint: endpoint.to_string(),
        start: start_time,
    };
    let stages = Arc::new(Mutex::new(StageTimings::default()));

    if prompt.trim().is_empty() {
        return bad_request("prompt must not be empty", "invalid_input");
    }
    if let Some(nv) = n {
        if nv != 1 {
            return bad_request(
                "n > 1 is not supported: the upstream image model does not enable multiple candidates",
                "unsupported_n",
            );
        }
    }

    let parsed = ParsedRequestModel::parse(model);
    let requested_raw_model = parsed.raw_requested_model.clone();

    let routing_start = Instant::now();
    let targets = match state.resolve_routed_targets_full(&parsed, None, Some(prompt), None, None, &["text"]) {
        Ok(ts) if !ts.is_empty() => ts,
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("The model '{}' does not exist or you do not have access to it.", model),
                        "type": "invalid_request_error",
                        "code": "model_not_found"
                    }
                })),
            )
                .into_response();
        }
        Err(err) => {
            let (status, code) = match err {
                CoreError::UnsupportedModality { .. } => (StatusCode::BAD_REQUEST, "unsupported_modality"),
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

    // Images endpoints are antigravity-only: reject non-image-capable targets
    // before touching any upstream.
    if let Some((msg, code)) = image_target_error(&targets[0]) {
        return bad_request(msg, code);
    }

    let routing_ms = routing_start.elapsed().as_secs_f64() * 1000.0;
    stages.lock().routing_ms = Some(routing_ms);
    state.emit(
        &ctx,
        Some(targets[0].provider_name.clone()),
        GatewayEvent::RouteResolved {
            provider: targets[0].provider_name.clone(),
            translated: true,
            routing_ms,
        },
    );

    let image_part = image.and_then(|raw| extract_image_part(raw, "image/png"));
    // For image editing, do not force an aspect ratio if not explicitly needed, or preserve upstream image ratio
    let aspect_ratio = if image_part.is_some() && size.is_none() {
        None
    } else {
        openai_size_to_antigravity_aspect_ratio(size)
    };

    let mut last_error = String::new();
    let mut last_pool_exhausted = false;
    let mut last_kind = ponyllm_core::error::GatewayErrorKind::Internal;

    for target in targets {
        let provider_name = target.provider_name.clone();
        let pool = match state.get_pool(&provider_name) {
            Some(p) => p,
            None => continue,
        };
        let max_retries = state.config.read().max_retries;

        let (ag_project, ag_salt) = state
            .peek_antigravity_identity(&provider_name)
            .unwrap_or_else(|| ("aicode-consumers".to_string(), String::new()));

        let req_val = images_to_antigravity_request(
            &target.physical_model,
            &ag_project,
            prompt,
            image_part.as_ref().map(|(m, b)| (m.as_str(), b.as_str())),
            aspect_ratio.as_deref(),
            &ag_salt,
        );

        let target_url = format!(
            "{}/v1internal:generateContent",
            target
                .endpoint_base
                .as_deref()
                .unwrap_or(&target.base_url)
                .trim_end_matches('/')
        );

        // VULN-07/F6: refuse to dial upstream URLs that resolve (or rebind)
        // to metadata/private/CGNAT/benchmark ranges — re-validated here at
        // dial time (cached per host), not only at provider write time.
        if let Err(reason) = state
            .data_plane_egress_guard_for_target(&provider_name, &target.physical_model, &target_url)
            .await
        {
            last_error = reason;
            last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
            tracing::warn!(provider = %provider_name, url = %target_url, "data-plane egress guard refused upstream; skipping target");
            continue;
        }

        let req_snippet = Some(format_request_snippet(&req_val));

        let sink_ctx = EventSinkCtx {
            request_id: request_id.clone(),
            endpoint: endpoint.to_string(),
            provider: provider_name.clone(),
            model: Some(requested_raw_model.clone()),
            start: start_time,
            stages: stages.clone(),
            request_snippet: req_snippet.clone(),
        };
        let http_client = state.http_client_for_target(&provider_name, &target.physical_model);
        let (rate_limits, ttfb_timeout) = {
            let cfg = state.config.read();
            let rl = cfg
                .providers
                .get(&provider_name)
                .and_then(|p| p.effective_rate_limits(&parsed.clean_model_name));
            let ttfb = cfg.effective_ttfb_timeout(&provider_name);
            (rl, ttfb)
        };
        let executor = UpstreamExecutor::with_client(pool.clone(), http_client, max_retries)
            .with_downstream_headers(&headers)
            .with_opencode_zen(is_opencode_zen_target(&provider_name, &target_url))
            .with_rate_limits(rate_limits)
            .with_ttfb_timeout(ttfb_timeout)
            .with_event_sink(sink_ctx.clone(), state.event_sink(sink_ctx.clone()));

        match executor.execute_json_request_with_key(&target_url, &req_val).await {
            Ok((resp_val, winning_key_id)) => {
                let latency = start_time.elapsed();
                let final_val = match antigravity_to_images_response(&resp_val, &requested_raw_model) {
                    Some(v) => v,
                    None => {
                        last_error = format!(
                            "Antigravity image response contained no inlineData image for model '{}' (upstream returned text/thought only)",
                            target.physical_model
                        );
                        last_kind = ponyllm_core::error::GatewayErrorKind::UpstreamUnavailable;
                        tracing::warn!(provider = %provider_name, model = %target.physical_model, "images request: upstream response had no image part");
                        pool.record_error(&winning_key_id, ponyllm_core::pool::PoolErrorType::ServerError);
                        continue;
                    }
                };

                // Best-effort token accounting from the upstream usage block.
                let (prompt_tokens, completion_tokens, cached_tokens) = {
                    let usage = resp_val
                        .get("response")
                        .and_then(|r| r.get("usageMetadata"))
                        .or_else(|| resp_val.get("usageMetadata"));
                    let p = usage.and_then(|u| u.get("promptTokenCount")).and_then(|t| t.as_u64()).unwrap_or(0);
                    let c = usage.and_then(|u| u.get("candidatesTokenCount")).and_then(|t| t.as_u64()).unwrap_or(0);
                    let ca = usage.and_then(|u| u.get("cachedContentTokenCount")).and_then(|t| t.as_u64()).unwrap_or(0);
                    (p, c, ca)
                };
                let wall_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                pool.record_tokens(&winning_key_id, wall_ms, prompt_tokens, completion_tokens, cached_tokens);
                state.emit(
                    &ctx,
                    Some(provider_name.clone()),
                    GatewayEvent::RequestCompleted {
                        status_code: 200,
                        latency_ms: latency.as_secs_f64() * 1000.0,
                        prompt_tokens,
                        completion_tokens,
                        cached_tokens,
                        tps: None,
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
                tracing::warn!(provider = %provider_name, "images request failed ({}). Attempting fallback...", err);
                last_kind = err.kind();
                if !state.config.read().cross_provider_quota_failover
                    && crate::extractors::pool_quota_exhausted(&err, &pool)
                {
                    last_kind = ponyllm_core::error::GatewayErrorKind::QuotaExhausted;
                }
                last_pool_exhausted = matches!(err, CoreError::NoAvailableKey(_));
                last_error = err.to_string();
            }
        }
    }

    let msg = crate::extractors::format_exhausted_message(
        &requested_raw_model,
        &last_kind,
        &last_error,
        last_pool_exhausted,
        &request_id,
    );
    let mut resp = crate::extractors::project_openai_error(&last_kind, &msg);
    inject_telemetry_headers(&mut resp, &request_id, &stages);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_image_part_handles_data_uri_and_raw() {
        assert_eq!(
            extract_image_part("data:image/png;base64,AAAA", "image/png"),
            Some(("image/png".to_string(), "AAAA".to_string()))
        );
        assert_eq!(
            extract_image_part("AAAA", "image/png"),
            Some(("image/png".to_string(), "AAAA".to_string()))
        );
        assert_eq!(extract_image_part("  ", "image/png"), None);
        assert_eq!(extract_image_part("", "image/png"), None);
    }
}
