use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_core::pool::{GatewayRoutingStrategy, ModelTier};
use serde_json::json;
use std::str::FromStr;
use crate::state::AppState;

/// Parsed request model structure with stripped tags and extracted strategy / tier overrides
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRequestModel {
    /// Exact raw model string requested by client (e.g. "deepseek-v4-flash[1m]:economy")
    pub raw_requested_model: String,
    /// Clean physical or base model name (e.g. "deepseek-v4-flash" or "llama3:70b" or "auto")
    pub clean_model_name: String,
    /// Whether this request uses virtual auto routing
    pub is_auto: bool,
    /// Explicit target tier (e.g. Standard or Flagship), or None for default
    pub explicit_tier: Option<ModelTier>,
    /// Strategy override from model name (e.g. Economy or Speed)
    pub strategy_override: Option<GatewayRoutingStrategy>,
    /// Whether 1M context is requested (e.g. [1m])
    pub is_1m_context: bool,
}

impl ParsedRequestModel {
    pub fn parse(raw: &str) -> Self {
        let raw_str = raw.trim();
        let (without_1m, is_1m_context) = strip_1m_tag(raw_str);

        let mut parts: Vec<&str> = without_1m
            .split(':')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let mut strategy_override = None;
        let mut explicit_tier = None;

        // Pop recognized strategy and tier modifiers from the right tail
        while parts.len() > 1 {
            let last = parts.last().unwrap();
            if let Ok(strat) = GatewayRoutingStrategy::from_str(last) {
                strategy_override = Some(strat);
                parts.pop();
            } else if let Ok(tier) = ModelTier::from_str(last) {
                explicit_tier = Some(tier);
                parts.pop();
            } else {
                break;
            }
        }

        let base_name = if parts.is_empty() {
            "auto".to_string()
        } else {
            parts.join(":")
        };

        let is_auto = base_name.eq_ignore_ascii_case("auto");
        let clean_model_name = if is_auto {
            "auto".to_string()
        } else {
            base_name
        };

        Self {
            raw_requested_model: raw_str.to_string(),
            clean_model_name,
            is_auto,
            explicit_tier,
            strategy_override,
            is_1m_context,
        }
    }
}

/// Robust case-insensitive and whitespace-tolerant [1m] tag stripper (Unicode-safe)
pub fn strip_1m_tag(s: &str) -> (String, bool) {
    let mut has_1m = false;
    let mut clean = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '[' {
            let mut tag_content = String::new();
            let mut matched_close = false;
            while let Some(&inner) = chars.peek() {
                chars.next();
                if inner == ']' {
                    matched_close = true;
                    break;
                }
                tag_content.push(inner);
            }

            let trimmed_lower = tag_content.trim().to_ascii_lowercase();
            if matched_close && (trimmed_lower == "1m" || trimmed_lower == "1024k") {
                has_1m = true;
                // Tag is stripped, do not write into clean
            } else {
                // Not a 1m tag, restore original bracketed segment
                clean.push('[');
                clean.push_str(&tag_content);
                if matched_close {
                    clean.push(']');
                }
            }
        } else {
            clean.push(c);
        }
    }

    (clean.trim().to_string(), has_1m)
}

pub fn format_model_json(model_id: &str, provider_name: &str, display_name: Option<&str>, protocol: &str) -> serde_json::Value {
    let display = display_name
        .map(|d| d.to_string())
        .unwrap_or_else(|| format!("{} ({})", model_id, provider_name));

    json!({
        "id": model_id,
        "object": "model",
        "type": "model",
        "created": 1710000000,
        "created_at": "2024-03-01T00:00:00Z",
        "owned_by": provider_name,
        "display_name": display,
        "protocol": protocol,
        "permission": [],
        "root": model_id,
        "parent": null
    })
}

/// Handler for `GET /v1/models` and `GET /models`
pub async fn handle_list_models(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let models = state.list_all_models();
    let data: Vec<serde_json::Value> = models
        .into_iter()
        .map(|(model_id, provider_name, display_name, protocol)| {
            format_model_json(&model_id, &provider_name, display_name.as_deref(), &protocol)
        })
        .collect();

    let first_id = data.first().and_then(|d| d.get("id")).and_then(|v| v.as_str());
    let last_id = data.last().and_then(|d| d.get("id")).and_then(|v| v.as_str());

    Json(json!({
        "object": "list",
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id
    }))
}

/// Handler for `GET /v1/models/:model_id` and `GET /models/:model_id`
pub async fn handle_get_model(
    State(state): State<Arc<AppState>>,
    Path(model_id): Path<String>,
) -> impl IntoResponse {
    let models = state.list_all_models();
    if let Some((m_id, provider_name, display_name, protocol)) = models.into_iter().find(|(m, _, _, _)| m == &model_id) {
        (
            StatusCode::OK,
            Json(format_model_json(&m_id, &provider_name, display_name.as_deref(), &protocol)),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": format!("Model '{}' does not exist or is not configured", model_id),
                    "type": "invalid_request_error",
                    "code": "model_not_found"
                }
            })),
        )
            .into_response()
    }
}
