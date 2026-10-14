use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::common::ReasoningEffort;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseReasoningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
}

/// OpenAI Responses API Create Request (`/v1/responses`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateResponseRequest {
    pub model: String,
    pub input: ResponseInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponseToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing)]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponseReasoningConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl CreateResponseRequest {
    pub fn get_reasoning_effort(&self) -> Option<ReasoningEffort> {
        if let Some(re) = self.reasoning_effort {
            return Some(re);
        }
        if let Some(ref r) = self.reasoning {
            if let Some(eff) = r.effort {
                return Some(eff);
            }
        }
        if let Some(val) = self.extra.get("reasoning_effort") {
            if let Some(s) = val.as_str() {
                return ReasoningEffort::from_str_loose(s);
            }
        }
        None
    }

    pub fn sanitize_thinking_extra(&mut self) {
        self.extra.remove("reasoning_effort");
        self.extra.remove("thinking");
    }

    pub fn required_modalities(&self) -> Vec<&'static str> {
        let mut mods = Vec::new();
        match &self.input {
            ResponseInput::Text(_) => mods.push("text"),
            ResponseInput::Items(items) => {
                for item in items {
                    if let ResponseInputItem::Message { content, .. } = item {
                        match content {
                            ResponseInputContent::Text(_) => mods.push("text"),
                            ResponseInputContent::Parts(parts) => {
                                for p in parts {
                                    match p {
                                        ResponseContentPart::InputImage { .. } => mods.push("image"),
                                        ResponseContentPart::InputAudio { .. } => mods.push("audio"),
                                        ResponseContentPart::InputVideo { .. } => mods.push("video"),
                                        ResponseContentPart::InputFile { .. } => mods.push("file"),
                                        ResponseContentPart::Text { .. } => mods.push("text"),
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        mods.sort_unstable();
        mods.dedup();
        if mods.is_empty() {
            mods.push("text");
        }
        mods
    }
}


#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseContentPart>),
}

impl<'de> Deserialize<'de> for ResponseInputContent {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawItem {
            Part(ResponseContentPart),
            Str(String),
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ContentHelper {
            Text(String),
            Array(Vec<RawItem>),
        }

        match ContentHelper::deserialize(deserializer)? {
            ContentHelper::Text(s) => Ok(ResponseInputContent::Text(s)),
            ContentHelper::Array(raw_items) => {
                let parts: Vec<ResponseContentPart> = raw_items
                    .into_iter()
                    .map(|item| match item {
                        RawItem::Part(p) => p,
                        RawItem::Str(s) => ResponseContentPart::Text { text: s },
                    })
                    .collect();
                Ok(ResponseInputContent::Parts(parts))
            }
        }
    }
}

impl ResponseInputContent {
    pub fn as_plain_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ResponseContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn is_non_empty(&self) -> bool {
        match self {
            Self::Text(text) => !text.trim().is_empty(),
            Self::Parts(parts) => parts.iter().any(|p| match p {
                ResponseContentPart::Text { text } => !text.trim().is_empty(),
                ResponseContentPart::InputImage { .. } => true,
                ResponseContentPart::InputAudio { .. } => true,
                ResponseContentPart::InputVideo { .. } => true,
                ResponseContentPart::InputFile { .. } => true,
                ResponseContentPart::Thought { thought } => !thought.trim().is_empty(),
                ResponseContentPart::Reasoning { reasoning } => !reasoning.trim().is_empty(),
                ResponseContentPart::SummaryText { text }
                | ResponseContentPart::ReasoningText { text } => !text.trim().is_empty(),
                ResponseContentPart::Refusal { refusal } => !refusal.trim().is_empty(),
                ResponseContentPart::Unknown => false,
            }),
        }
    }
}

impl From<String> for ResponseInputContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for ResponseInputContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl From<Vec<ResponseContentPart>> for ResponseInputContent {
    fn from(parts: Vec<ResponseContentPart>) -> Self {
        Self::Parts(parts)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Items(Vec<ResponseInputItem>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseInputItem {
    Message {
        role: String,
        content: ResponseInputContent,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output", alias = "function_response")]
    FunctionResponse {
        call_id: String,
        output: String,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ResponseContentPart>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<Vec<ResponseContentPart>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
    ItemReference {
        id: String,
    },
    #[serde(untagged)]
    Custom(serde_json::Value),
}

impl<'de> Deserialize<'de> for ResponseInputItem {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde_json::Value;
        let mut val = Value::deserialize(deserializer)?;
        if let Value::Object(ref mut map) = val {
            if !map.contains_key("type") {
                if map.contains_key("role") && map.contains_key("content") {
                    map.insert("type".to_string(), Value::String("message".to_string()));
                } else if map.contains_key("call_id") && map.contains_key("output") {
                    map.insert("type".to_string(), Value::String("function_call_output".to_string()));
                } else if map.contains_key("call_id") && map.contains_key("name") {
                    map.insert("type".to_string(), Value::String("function_call".to_string()));
                }
            }
        }
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum StandardItem {
            Message {
                role: String,
                content: ResponseInputContent,
            },
            FunctionCall {
                call_id: String,
                name: String,
                arguments: String,
            },
            #[serde(rename = "function_call_output", alias = "function_response")]
            FunctionResponse {
                call_id: String,
                output: String,
            },
            Reasoning {
                #[serde(default)]
                id: Option<String>,
                #[serde(default)]
                status: Option<String>,
                #[serde(default)]
                content: Option<Vec<ResponseContentPart>>,
                #[serde(default)]
                summary: Option<Vec<ResponseContentPart>>,
                #[serde(default)]
                encrypted_content: Option<String>,
            },
            ItemReference {
                id: String,
            },
        }

        match serde_json::from_value::<StandardItem>(val.clone()) {
            Ok(StandardItem::Message { role, content }) => Ok(ResponseInputItem::Message { role, content }),
            Ok(StandardItem::FunctionCall { call_id, name, arguments }) => Ok(ResponseInputItem::FunctionCall { call_id, name, arguments }),
            Ok(StandardItem::FunctionResponse { call_id, output }) => Ok(ResponseInputItem::FunctionResponse { call_id, output }),
            Ok(StandardItem::Reasoning { id, status, content, summary, encrypted_content }) => {
                Ok(ResponseInputItem::Reasoning { id, status, content, summary, encrypted_content })
            }
            Ok(StandardItem::ItemReference { id }) => Ok(ResponseInputItem::ItemReference { id }),
            Err(_) => {
                // Keep arbitrary custom/extension input items intact instead of rejecting with 400
                Ok(ResponseInputItem::Custom(val))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseToolDefinition {
    Function {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<serde_json::Value>,
    },
    FileSearch,
    CodeInterpreter,
}

/// Current Unix time in seconds. Never returns `0`, so `created_at > 0` always holds
/// (R-NS-03).
pub fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1)
        .max(1)
}

fn default_parallel_tool_calls() -> bool {
    true
}

fn default_text_config() -> serde_json::Value {
    serde_json::json!({ "format": { "type": "text" } })
}

fn default_tool_choice() -> serde_json::Value {
    serde_json::Value::String("auto".to_string())
}

fn default_truncation() -> String {
    "disabled".to_string()
}

fn id_tail(raw: &str) -> String {
    let stripped = raw
        .strip_prefix("chatcmpl-")
        .or_else(|| raw.strip_prefix("cmb-"))
        .or_else(|| raw.strip_prefix("resp_"))
        .or_else(|| raw.strip_prefix("msg_"))
        .or_else(|| raw.strip_prefix("rs_"))
        .unwrap_or(raw);
    stripped
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Normalizes an upstream response id into the official `resp_`-prefixed form (R-NS-01).
pub fn normalize_response_id(raw: &str) -> String {
    if raw.starts_with("resp_") && raw.len() > "resp_".len() {
        return raw.to_string();
    }
    let tail = id_tail(raw);
    if tail.is_empty() {
        format!("resp_{:x}", now_unix_secs())
    } else {
        format!("resp_{tail}")
    }
}

/// Normalizes an upstream item id into a `msg_`-prefixed assistant-message item id (R-NS-08).
pub fn normalize_message_item_id(raw: &str) -> String {
    if raw.starts_with("msg_") && raw.len() > "msg_".len() {
        return raw.to_string();
    }
    let tail = id_tail(raw);
    if tail.is_empty() {
        format!("msg_{:x}", now_unix_secs())
    } else {
        format!("msg_{tail}")
    }
}

/// Builds the top-level `reasoning` output item (R-NS-13/14/15). The reasoning text is carried
/// both as a `summary_text` part (MUST, R-NS-14) and as a `reasoning_text` content part (SHOULD,
/// R-NS-15), matching the official target shape.
pub fn reasoning_output_item(response_id: &str, reasoning: &str) -> ResponseOutputItem {
    let tail = id_tail(response_id);
    let item_id = if tail.is_empty() {
        format!("rs_{:x}", now_unix_secs())
    } else {
        format!("rs_{tail}")
    };
    ResponseOutputItem::Reasoning {
        id: Some(item_id),
        status: Some("completed".to_string()),
        encrypted_content: None,
        content: Some(vec![ResponseContentPart::ReasoningText {
            text: reasoning.to_string(),
        }]),
        summary: Some(vec![ResponseContentPart::SummaryText {
            text: reasoning.to_string(),
        }]),
    }
}

/// OpenAI Responses API response object (`/v1/responses` non-stream body and the payload of
/// every `response.*` streaming event).
///
/// Every field is serialized unconditionally: the contract requires the nullable set present as
/// explicit `null`s (R-NS-20/21/23), `metadata` as `{}` (R-NS-22) and `created_at` as a positive
/// integer (R-NS-03).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseObject {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    /// Unix seconds of creation. Stable for the whole lifetime of one response, including every
    /// event of one stream.
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<ResponseError>,
    #[serde(default)]
    pub incomplete_details: Option<serde_json::Value>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub output: Vec<ResponseOutputItem>,
    #[serde(default = "default_parallel_tool_calls")]
    pub parallel_tool_calls: bool,
    #[serde(default)]
    pub previous_response_id: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_text_config")]
    pub text: serde_json::Value,
    #[serde(default = "default_tool_choice")]
    pub tool_choice: serde_json::Value,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default = "default_truncation")]
    pub truncation: String,
    #[serde(default)]
    pub usage: Option<ResponseUsage>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub user: Option<String>,
}

impl Default for ResponseObject {
    fn default() -> Self {
        Self {
            id: String::new(),
            object: "response".to_string(),
            created_at: now_unix_secs(),
            completed_at: None,
            status: String::new(),
            error: None,
            incomplete_details: None,
            instructions: None,
            model: String::new(),
            output: Vec::new(),
            parallel_tool_calls: true,
            previous_response_id: None,
            temperature: None,
            text: default_text_config(),
            tool_choice: default_tool_choice(),
            tools: Vec::new(),
            top_p: None,
            truncation: default_truncation(),
            usage: None,
            metadata: HashMap::new(),
            user: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

impl ResponseObject {
    /// True when this object represents an upstream failure.
    ///
    /// The wire contract sets `status == "failed"` (and streaming triggers on
    /// the `response.failed` event variant regardless of status text), so
    /// non-streaming guards must not be stricter: a non-success status
    /// carrying a non-empty error payload also counts. Known success /
    /// in-flight statuses (`completed`, `incomplete`, `in_progress`,
    /// `queued`) are never failures, even if an error object happens to be
    /// attached.
    pub fn is_failed(&self) -> bool {
        if self.status == "failed" {
            return true;
        }
        match self.status.as_str() {
            "completed" | "incomplete" | "in_progress" | "queued" | "" => false,
            _ => match &self.error {
                Some(e) => !(e.code.trim().is_empty() && e.message.trim().is_empty()),
                None => false,
            },
        }
    }

    /// Shared upstream-failure reason for Failed projections.
    /// With an upstream error payload: `code=<code> message=<message>`;
    /// missing/empty falls back to the response status/id:
    /// `status=<status>[/<id>]`. Carries no credentials.
    pub fn failed_reason(&self) -> String {
        if let Some(err) = self.error.as_ref() {
            if !(err.code.trim().is_empty() && err.message.trim().is_empty()) {
                return format!("code={} message={}", err.code, err.message);
            }
        }
        if self.id.trim().is_empty() {
            format!("status={}", self.status)
        } else {
            format!("status={}/{}", self.status, self.id)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseOutputItem {
    Message {
        #[serde(default)]
        id: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        role: String,
        /// Serialized as `output_text` parts: the wire type for assistant output is
        /// `output_text`, never the input-side `input_text` (R-NS-11).
        #[serde(default, serialize_with = "serialize_output_content_parts")]
        content: Vec<ResponseContentPart>,
    },
    FunctionCall {
        #[serde(default)]
        id: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        arguments: String,
    },
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(default)]
        content: Option<Vec<ResponseContentPart>>,
        #[serde(default)]
        summary: Option<Vec<ResponseContentPart>>,
    },
    #[serde(other)]
    Unknown,
}

fn deserialize_image_url_field<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        Str(String),
        Obj { url: String },
    }
    match Helper::deserialize(deserializer)? {
        Helper::Str(s) => Ok(s),
        Helper::Obj { url } => Ok(url),
    }
}

pub fn default_audio_format() -> String {
    "wav".to_string()
}

/// Serializes one content part the way a **response** body must: assistant text is
/// `{"type":"output_text","text":…,"annotations":[],"logprobs":[]}`.
///
/// The input side keeps `ResponseContentPart::Text`'s own serialization (`input_text`) so a
/// Responses-native upstream still receives legal input content (contract §5.1: serialize strict
/// only inside a response body, deserialize permissive everywhere).
struct OutputContentPart<'a>(&'a ResponseContentPart);

impl Serialize for OutputContentPart<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self.0 {
            ResponseContentPart::Text { text } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("type", "output_text")?;
                map.serialize_entry("text", text)?;
                map.serialize_entry("annotations", &Vec::<serde_json::Value>::new())?;
                map.serialize_entry("logprobs", &Vec::<serde_json::Value>::new())?;
                map.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// `serialize_with` helper for a single response-side content part.
pub fn serialize_output_content_part<S>(
    part: &ResponseContentPart,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    OutputContentPart(part).serialize(serializer)
}

/// `serialize_with` helper for a response-side content-part list.
pub fn serialize_output_content_parts<S>(
    parts: &[ResponseContentPart],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(parts.len()))?;
    for part in parts {
        seq.serialize_element(&OutputContentPart(part))?;
    }
    seq.end()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentPart {
    #[serde(rename = "input_text", alias = "output_text", alias = "text")]
    Text {
        text: String,
    },
    #[serde(rename = "input_image", alias = "image_url", alias = "image")]
    InputImage {
        #[serde(deserialize_with = "deserialize_image_url_field")]
        image_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    #[serde(rename = "input_audio", alias = "audio")]
    InputAudio {
        #[serde(alias = "audio", alias = "data")]
        data: String,
        #[serde(default = "default_audio_format")]
        format: String,
    },
    #[serde(rename = "input_video", alias = "video_url", alias = "video")]
    InputVideo {
        video_url: String,
    },
    #[serde(rename = "input_file", alias = "file")]
    InputFile {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    Thought {
        thought: String,
    },
    Reasoning {
        reasoning: String,
    },
    Refusal {
        refusal: String,
    },
    /// Reasoning summary part of a top-level `reasoning` output item (R-NS-14).
    #[serde(rename = "summary_text")]
    SummaryText {
        #[serde(default)]
        text: String,
    },
    /// Reasoning content part of a top-level `reasoning` output item (R-NS-15).
    #[serde(rename = "reasoning_text")]
    ReasoningText {
        #[serde(default)]
        text: String,
    },
    #[serde(other)]
    Unknown,
}

impl Default for ResponseContentPart {
    fn default() -> Self {
        Self::Text {
            text: String::new(),
        }
    }
}

/// `usage.input_tokens_details` (R-NS-17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseInputTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
    #[serde(default)]
    pub cache_write_tokens: u32,
}

/// `usage.output_tokens_details` (R-NS-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseUsage {
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub input_tokens_details: ResponseInputTokensDetails,
    #[serde(default)]
    pub output_tokens_details: ResponseOutputTokensDetails,
}

impl ResponseUsage {
    /// Builds a usage object carrying the real upstream counts. `total_tokens` is always
    /// `input_tokens + output_tokens` (R-NS-19).
    pub fn new(
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: u32,
        reasoning_tokens: u32,
    ) -> Self {
        Self {
            total_tokens: input_tokens + output_tokens,
            input_tokens,
            output_tokens,
            input_tokens_details: ResponseInputTokensDetails {
                cached_tokens,
                cache_write_tokens: 0,
            },
            output_tokens_details: ResponseOutputTokensDetails { reasoning_tokens },
        }
    }
}

/// Streaming events for OpenAI Responses API.
///
/// Every variant carries a `sequence_number` (R-SS-04). The `response_id` fields retained on some
/// variants exist only for inbound tolerance of older upstream frames; they are never serialized,
/// because the official event schemas do not define them (R-SS-07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    /// Emitted once after `response.created` and before the first output item (R-SS-03).
    #[serde(rename = "response.in_progress")]
    ResponseInProgress {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    /// Legacy terminal event tolerated on inbound streams only.
    #[serde(rename = "response.done")]
    ResponseDone {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        #[serde(default)]
        sequence_number: u64,
        #[serde(default, skip_serializing)]
        response_id: String,
        #[serde(default)]
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        #[serde(default)]
        sequence_number: u64,
        #[serde(default, skip_serializing)]
        response_id: String,
        #[serde(default)]
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        #[serde(default)]
        sequence_number: u64,
        #[serde(default, skip_serializing)]
        response_id: String,
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: u32,
        #[serde(default)]
        content_index: u32,
        #[serde(serialize_with = "serialize_output_content_part")]
        part: ResponseContentPart,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        #[serde(default)]
        sequence_number: u64,
        #[serde(default, skip_serializing)]
        response_id: String,
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: u32,
        #[serde(default)]
        content_index: u32,
        #[serde(serialize_with = "serialize_output_content_part")]
        part: ResponseContentPart,
    },

    /// Legacy wire name tolerated on inbound streams only.
    #[serde(rename = "response.text.delta")]
    TextDelta(ResponseTextDelta),

    /// Real OpenAI wire name for assistant text deltas (R-SS-07).
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(ResponseOutputTextDelta),

    /// Full text of the assistant message, emitted before `response.content_part.done` (R-SS-08).
    #[serde(rename = "response.output_text.done")]
    OutputTextDone(ResponseOutputTextDone),

    /// OpenAI Responses reasoning delta events.
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta(ResponseTextDelta),

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded(ResponseReasoningSummaryPart),

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta(ResponseReasoningSummaryTextDelta),

    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone(ResponseReasoningSummaryTextDone),

    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone(ResponseReasoningSummaryPart),

    /// Real OpenAI terminal event carrying usage.
    #[serde(rename = "response.completed")]
    Completed {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    /// Real OpenAI incomplete/truncated event.
    #[serde(rename = "response.incomplete")]
    Incomplete {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    /// Real OpenAI failure event.
    #[serde(rename = "response.failed")]
    Failed {
        #[serde(default)]
        sequence_number: u64,
        response: ResponseObject,
    },

    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta(ResponseFunctionCallDelta),

    #[serde(other)]
    Unknown,
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseTextDelta {
    #[serde(default)]
    pub response_id: String,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub content_index: u32,
    pub delta: String,
}

/// `response.output_text.delta` payload (R-SS-07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseOutputTextDelta {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub content_index: u32,
    #[serde(default)]
    pub delta: String,
    #[serde(default)]
    pub logprobs: Vec<serde_json::Value>,
}

/// `response.output_text.done` payload (R-SS-08).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseOutputTextDone {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub content_index: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub logprobs: Vec<serde_json::Value>,
}

/// `response.reasoning_summary_part.added` / `.done` payload (R-SS-11/R-SS-23).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseReasoningSummaryPart {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub summary_index: u32,
    #[serde(default)]
    pub part: ResponseContentPart,
}

/// `response.reasoning_summary_text.delta` payload (R-SS-11/R-SS-23).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseReasoningSummaryTextDelta {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub summary_index: u32,
    #[serde(default)]
    pub delta: String,
}

/// `response.reasoning_summary_text.done` payload (R-SS-11/R-SS-23).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseReasoningSummaryTextDone {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub summary_index: u32,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseFunctionCallDelta {
    #[serde(default)]
    pub sequence_number: u64,
    #[serde(default, skip_serializing)]
    pub response_id: String,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub call_id: String,
    pub delta: String,
}
