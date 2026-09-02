use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use crate::state::AppState;

fn format_model_json(model_id: &str, provider_name: &str) -> serde_json::Value {
    json!({
        "id": model_id,
        "object": "model",
        "type": "model",
        "created": 1710000000,
        "created_at": "2024-03-01T00:00:00Z",
        "owned_by": provider_name,
        "display_name": format!("{} ({})", model_id, provider_name),
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
        .map(|(model_id, provider_name)| format_model_json(&model_id, &provider_name))
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
    if let Some((m_id, provider_name)) = models.into_iter().find(|(m, _)| m == &model_id) {
        (
            StatusCode::OK,
            Json(format_model_json(&m_id, &provider_name)),
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
