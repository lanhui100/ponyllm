use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Native upstream wire protocol of a provider or model.
///
/// Inbound protocol is fixed by the HTTP path (`/v1/chat/completions`,
/// `/v1/responses`, `/v1/messages`); outbound protocol is resolved as
/// model override > provider default > legacy URL heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpstreamProtocol {
    /// OpenAI Chat Completions (`/v1/chat/completions`)
    #[default]
    Chat,
    /// OpenAI Responses (`/v1/responses`)
    Responses,
    /// Anthropic Messages (`/v1/messages`)
    Anthropic,
}

impl UpstreamProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn is_anthropic(&self) -> bool {
        matches!(self, Self::Anthropic)
    }
}

impl FromStr for UpstreamProtocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "chat.completions" | "chat_completions" | "openai-chat" | "openai" => {
                Ok(Self::Chat)
            }
            "responses" | "response" | "openai-responses" | "openai_responses" => {
                Ok(Self::Responses)
            }
            "anthropic" | "messages" | "claude" => Ok(Self::Anthropic),
            _ => Err(format!("Unknown upstream protocol '{}'", s)),
        }
    }
}

impl fmt::Display for UpstreamProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for UpstreamProtocol {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for UpstreamProtocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}
