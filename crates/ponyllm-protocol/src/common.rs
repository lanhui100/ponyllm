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

/// Unified 4-tier reasoning effort scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub enum ReasoningEffort {
    /// Zero reasoning chain / disabled. Lowest latency and token cost.
    Off = 0,
    /// Light reasoning.
    Low = 1,
    /// Standard / balanced reasoning.
    #[default]
    Medium = 2,
    /// Deep reasoning / maximum cognitive allocation.
    High = 3,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
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
        }
    }

    /// Tolerant parser from diverse client string representations.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" | "false" | "disabled" | "disable" | "no" => Some(Self::Off),
            "low" | "minimal" | "1" | "fast" | "light" => Some(Self::Low),
            "medium" | "standard" | "default" | "2" | "balanced" | "med" => Some(Self::Medium),
            "high" | "deep" | "max" | "ultra" | "3" | "true" | "full" => Some(Self::High),
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

