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
