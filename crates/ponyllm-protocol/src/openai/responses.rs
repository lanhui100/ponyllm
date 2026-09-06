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
    #[serde(skip_serializing_if = "Option::is_none")]
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
        }

        match serde_json::from_value::<StandardItem>(val) {
            Ok(StandardItem::Message { role, content }) => Ok(ResponseInputItem::Message { role, content }),
            Ok(StandardItem::FunctionCall { call_id, name, arguments }) => Ok(ResponseInputItem::FunctionCall { call_id, name, arguments }),
            Ok(StandardItem::FunctionResponse { call_id, output }) => Ok(ResponseInputItem::FunctionResponse { call_id, output }),
            Err(e) => Err(serde::de::Error::custom(format!("Invalid response input item: {}", e))),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseObject {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub output: Vec<ResponseOutputItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
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
        #[serde(default)]
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
        #[serde(default)]
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
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseUsage {
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

/// Streaming events for OpenAI Responses API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated { response: ResponseObject },

    #[serde(rename = "response.done")]
    ResponseDone { response: ResponseObject },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        #[serde(default)]
        response_id: String,
        #[serde(default)]
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        #[serde(default)]
        response_id: String,
        #[serde(default)]
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        #[serde(default)]
        response_id: String,
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: u32,
        #[serde(default)]
        content_index: u32,
        part: ResponseContentPart,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        #[serde(default)]
        response_id: String,
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: u32,
        #[serde(default)]
        content_index: u32,
        part: ResponseContentPart,
    },

    #[serde(rename = "response.text.delta")]
    TextDelta(ResponseTextDelta),

    /// Real OpenAI wire name for text deltas (emitted by translators).
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(ResponseTextDelta),

    /// Real OpenAI terminal event carrying usage.
    #[serde(rename = "response.completed")]
    Completed { response: ResponseObject },

    /// Real OpenAI incomplete/truncated event.
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponseObject },

    /// Real OpenAI failure event.
    #[serde(rename = "response.failed")]
    Failed { response: ResponseObject },

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponseFunctionCallDelta {
    #[serde(default)]
    pub response_id: String,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub output_index: u32,
    #[serde(default)]
    pub call_id: String,
    pub delta: String,
}
