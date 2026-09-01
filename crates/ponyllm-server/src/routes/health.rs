use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

pub async fn handle_health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "ponyllm",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
