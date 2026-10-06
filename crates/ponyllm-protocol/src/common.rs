use serde::{Deserialize, Serialize};

/// Stop condition: either a single string or an array of strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopCondition {
    Single(String),
    Multiple(Vec<String>),
}

impl From<String> for StopCondition {
    fn from(s: String) -> Self {
        StopCondition::Single(s)
    }
}

impl From<Vec<String>> for StopCondition {
    fn from(v: Vec<String>) -> Self {
        StopCondition::Multiple(v)
    }
}

impl StopCondition {
    pub fn as_slice(&self) -> &[String] {
        match self {
            StopCondition::Single(ref s) => std::slice::from_ref(s),
            StopCondition::Multiple(ref v) => v.as_slice(),
        }
    }
}

use std::fmt;
use std::str::FromStr;
use serde::{Deserializer, Serializer};

/// Unified reasoning effort scale (Off, Low, Medium, High, Max).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub enum ReasoningEffort {
    /// Zero reasoning chain / disabled. Lowest latency and token cost.
    Off = 0,
    /// Light reasoning.
    Low = 1,
    /// Standard / balanced reasoning.
    #[default]
    Medium = 2,
    /// Deep reasoning / high cognitive allocation.
    High = 3,
    /// Maximum cognitive allocation / xhigh / ultra / max reasoning.
    Max = 4,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    pub fn is_active(&self) -> bool {
        *self != Self::Off
    }

    /// Convert to OpenAI-compatible reasoning effort string representation.
    pub fn to_openai_str(&self) -> Option<&'static str> {
        match self {
            Self::Off => Some("none"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("high"), // OpenAI standard protocol caps at high, Max maps to high
        }
    }

    /// Tolerant parser from diverse client string representations.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" | "false" | "disabled" | "disable" | "no" => Some(Self::Off),
            "low" | "minimal" | "1" | "fast" | "light" => Some(Self::Low),
            "medium" | "standard" | "default" | "2" | "balanced" | "med" => Some(Self::Medium),
            "high" | "deep" | "3" | "true" | "full" => Some(Self::High),
            "max" | "xhigh" | "extra_high" | "extra-high" | "ultra" | "4" | "extreme" => Some(Self::Max),
            _ => None,
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::from_str_loose(s).ok_or_else(|| format!("Unknown reasoning effort: '{}'", s))
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for ReasoningEffort {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str_loose(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("Invalid reasoning effort value: '{}'", s))
        })
    }
}

/// Sanitize a tool or function name for upstream wire compatibility.
///
/// Most upstreams (OpenAI, Anthropic, Gemini, OpenCode Zen Responses) require
/// names matching `^[a-zA-Z0-9_.-]+$` with a length cap (typically 64 chars).
/// Any invalid character (e.g. `:` from hallucinated `git_diff:bash`, spaces, `/`)
/// is mapped to `_`. Empty or whitespace names fall back to a safe identifier.
pub fn sanitize_wire_tool_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "tool_call".to_string();
    }

    let is_valid_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-';
    let needs_cleaning = trimmed.chars().any(|c| !is_valid_char(c));

    let sanitized = if needs_cleaning {
        trimmed
            .chars()
            .map(|c| if is_valid_char(c) { c } else { '_' })
            .collect::<String>()
    } else {
        trimmed.to_string()
    };

    const MAX_TOOL_NAME_LEN: usize = 64;
    if sanitized.len() > MAX_TOOL_NAME_LEN {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        raw.hash(&mut hasher);
        let hash_suffix = format!("_{:08x}", hasher.finish() as u32);
        let keep_len = MAX_TOOL_NAME_LEN.saturating_sub(hash_suffix.len());
        format!("{}{}", &sanitized[..keep_len], hash_suffix)
    } else {
        sanitized
    }
}


