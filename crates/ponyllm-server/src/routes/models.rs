use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use crate::state::AppState;

/// Handler for `GET /v1/models`
pub async fn handle_list_models(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let models = state.list_all_models();
    let data: Vec<serde_json::Value> = models
        .into_iter()
        .map(|(model_id, provider_name)| {
            json!({
                "id": model_id,
                "object": "model",
                "created": 1710000000,
                "owned_by": provider_name,
                "permission": [],
                "root": model_id,
                "parent": null
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": data
    }))
}

/// Handler for `GET /v1/models/:model_id`
pub async fn handle_get_model(
    State(state): State<Arc<AppState>>,
    Path(model_id): Path<String>,
) -> impl IntoResponse {
    let models = state.list_all_models();
    if let Some((m_id, provider_name)) = models.into_iter().find(|(m, _)| m == &model_id) {
        (
            StatusCode::OK,
            Json(json!({
                "id": m_id,
                "object": "model",
                "created": 1710000000,
                "owned_by": provider_name,
                "permission": [],
                "root": m_id,
                "parent": null
            })),
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
