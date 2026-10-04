//! OpenAI Images API — request/response wire types for the gateway's
//! `/v1/images/generations` and `/v1/images/edits` endpoints.
//!
//! The gateway always answers with `b64_json` (it has no object-storage URL
//! hosting), regardless of the requested `response_format`. Unknown fields are
//! preserved in `extra` and silently ignored by the Antigravity translator,
//! mirroring the chat/responses convention.

use serde::{Deserialize, Serialize};

/// `POST /v1/images/generations`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageGenerationRequest {
    pub model: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// `POST /v1/images/edits`
///
/// Accepted wire shapes:
/// - JSON: `image` / `mask` are base64 strings or `data:<mime>;base64,<...>`
///   data URIs.
/// - `multipart/form-data` (OpenAI SDK native): `image` / `mask` are file
///   parts, `model` / `prompt` / `n` / `size` / `response_format` are text
///   parts. Both shapes are normalized into this struct by the handler.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageEditRequest {
    pub model: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// OpenAI Images API success response. `data[].b64_json` carries the image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesResponse {
    pub created: u64,
    pub data: Vec<ImageData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_generation_request_with_unknown_fields() {
        let v: ImageGenerationRequest = serde_json::from_str(r#"{
            "model": "gemini-3.1-flash-image",
            "prompt": "a cat",
            "n": 1,
            "size": "1024x1024",
            "response_format": "b64_json",
            "style": "vivid",
            "quality": "hd"
        }"#).unwrap();
        assert_eq!(v.model, "gemini-3.1-flash-image");
        assert_eq!(v.prompt, "a cat");
        assert_eq!(v.size.as_deref(), Some("1024x1024"));
        assert!(v.extra.contains_key("style"));
        assert!(v.extra.contains_key("quality"));
    }

    #[test]
    fn parse_edit_request_data_uri() {
        let v: ImageEditRequest = serde_json::from_str(r#"{
            "model": "gemini-3.1-flash-image",
            "prompt": "make it red",
            "image": "data:image/png;base64,AAAA",
            "mask": "AAAA"
        }"#).unwrap();
        assert_eq!(v.image.as_deref(), Some("data:image/png;base64,AAAA"));
        assert_eq!(v.mask.as_deref(), Some("AAAA"));
    }

    #[test]
    fn roundtrip_response() {
        let r = ImagesResponse {
            created: 1710000000,
            data: vec![ImageData { b64_json: Some("AAAA".into()), url: None, revised_prompt: None }],
            model: Some("gemini-3.1-flash-image".into()),
        };
        let v = serde_json::to_value(r).unwrap();
        assert_eq!(v["data"][0]["b64_json"], "AAAA");
        assert_eq!(v["created"], 1710000000);
    }
}
