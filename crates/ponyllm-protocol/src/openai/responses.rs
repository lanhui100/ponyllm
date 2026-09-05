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
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseContentPart>),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    FunctionResponse {
        call_id: String,
        output: String,
    },
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseObject {
    pub id: String,
    pub object: String,
    pub status: String,
    pub model: String,
    pub output: Vec<ResponseOutputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
        id: String,
        status: String,
        role: String,
        content: Vec<ResponseContentPart>,
    },
    FunctionCall {
        id: String,
        status: String,
        call_id: String,
        name: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentPart {
    #[serde(rename = "input_text", alias = "output_text", alias = "text")]
    Text {
        text: String,
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
    pub total_tokens: u32,
    pub input_tokens: u32,
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
        response_id: String,
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        response_id: String,
        output_index: u32,
        item: ResponseOutputItem,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ResponseContentPart,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        response_id: String,
        item_id: String,
        output_index: u32,
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
    pub call_id: String,
    pub delta: String,
}
