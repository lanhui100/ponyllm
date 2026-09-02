//! Custom Axum extractors that render protocol-standard JSON error envelopes
//! instead of default plain-text 400/422 responses.

use axum::extract::rejection::JsonRejection;
use axum::extract::FromRequest;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;
use serde_json::json;

/// `AppJson<T>` wraps Axum's `Json<T>` extractor to guarantee that any
/// deserialization or JSON framing errors are returned as structured JSON
/// compliant with OpenAI or Anthropic error specifications, rather than
/// Axum's default plain-text rejections.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request<axum::body::Body>, state: &S) -> Result<Self, Self::Rejection> {
        let uri_path = req.uri().path().to_string();
        match Json::<T>::from_request(req, state).await {
            Ok(Json(val)) => Ok(AppJson(val)),
            Err(rejection) => {
                let err_msg = match &rejection {
                    JsonRejection::JsonDataError(e) => format!("Invalid request payload: {}", e.body_text()),
                    JsonRejection::JsonSyntaxError(e) => format!("Invalid JSON syntax: {}", e.body_text()),
                    JsonRejection::MissingJsonContentType(_) => {
                        "Missing or invalid 'content-type: application/json' header".to_string()
                    }
                    _ => rejection.body_text(),
                };

                let is_anthropic = uri_path.ends_with("/messages") || uri_path.contains("/messages/");
                let body = if is_anthropic {
                    json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": err_msg
                        }
                    })
                } else {
                    json!({
                        "error": {
                            "message": err_msg,
                            "type": "invalid_request_error",
                            "code": "invalid_payload"
                        }
                    })
                };

                Err((StatusCode::BAD_REQUEST, Json(body)).into_response())
            }
        }
    }
}
