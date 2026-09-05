use ponyllm_protocol::common::ReasoningEffort;
use serde::{Deserialize, Serialize};

/// Thinking capability specification for a model.
///
/// Implements dual-guard resolution:
/// - Baseline fallback: uses `default_effort` when the client does not specify an effort.
/// - Ceiling clamping: strictly clamps requested effort to `max_effort`, preventing upstream 400s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelThinkingSpec {
    /// Baseline effort when none requested by client (e.g. Medium for reasoner, Off for non-reasoner)
    #[serde(default = "default_thinking_default")]
    pub default_effort: ReasoningEffort,
    /// Ceiling maximum effort allowed by this model (e.g. High for deep-reasoner, Off for non-reasoner)
    #[serde(default = "default_thinking_max")]
    pub max_effort: ReasoningEffort,
}

pub fn default_thinking_default() -> ReasoningEffort {
    ReasoningEffort::Off
}

pub fn default_thinking_max() -> ReasoningEffort {
    ReasoningEffort::Off
}

impl Default for ModelThinkingSpec {
    fn default() -> Self {
        Self {
            default_effort: ReasoningEffort::Off,
            max_effort: ReasoningEffort::Off,
        }
    }
}

impl ModelThinkingSpec {
    pub fn new(default_effort: ReasoningEffort, max_effort: ReasoningEffort) -> Self {
        Self {
            default_effort: default_effort.min(max_effort),
            max_effort,
        }
    }

    /// Pure reasoning model (default=Medium, max=High)
    pub fn standard_reasoner() -> Self {
        Self {
            default_effort: ReasoningEffort::Medium,
            max_effort: ReasoningEffort::High,
        }
    }

    /// Lightweight reasoning model (default=Low, max=Medium)
    pub fn lightweight_reasoner() -> Self {
        Self {
            default_effort: ReasoningEffort::Low,
            max_effort: ReasoningEffort::Medium,
        }
    }

    /// Non-reasoning model (default=Off, max=Off)
    pub fn non_reasoner() -> Self {
        Self {
            default_effort: ReasoningEffort::Off,
            max_effort: ReasoningEffort::Off,
        }
    }

    /// Calculate effective effort based on requested effort.
    ///
    /// - If `requested` is None: falls back to `default_effort` (guaranteed <= max_effort).
    /// - If `requested` is Some(effort): strictly clamped by `min(max_effort)`.
    pub fn resolve(&self, requested: Option<ReasoningEffort>) -> ReasoningEffort {
        let ceiling = self.max_effort;
        match requested {
            None => self.default_effort.min(ceiling),
            Some(effort) => effort.min(ceiling),
        }
    }

    /// Whether this model supports any reasoning (i.e. max_effort > Off)
    pub fn supports_reasoning(&self) -> bool {
        self.max_effort > ReasoningEffort::Off
    }

    /// Heuristic to infer reasonable thinking spec from model name if not configured.
    pub fn infer_from_model_name(model_name: &str) -> Self {
        let lower = model_name.trim().to_ascii_lowercase();
        let is_reasoner = lower.contains("o1")
            || lower.contains("o3")
            || lower.contains("o4")
            || lower.contains("opus-5")
            || lower.contains("fable")
            || lower.contains("r1")
            || lower.contains("reasoner")
            || lower.contains("qwq")
            || lower.contains("thinking");

        if is_reasoner {
            if lower.contains("mini") || lower.contains("flash") || lower.contains("lite") {
                Self::lightweight_reasoner()
            } else {
                Self::standard_reasoner()
            }
        } else {
            Self::non_reasoner()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_thinking_spec_clamping_and_fallback() {
        // Standard reasoner (default Medium, max High)
        let spec = ModelThinkingSpec::standard_reasoner();
        assert_eq!(spec.resolve(None), ReasoningEffort::Medium);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Off)), ReasoningEffort::Off);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Low)), ReasoningEffort::Low);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Medium)), ReasoningEffort::Medium);
        assert_eq!(spec.resolve(Some(ReasoningEffort::High)), ReasoningEffort::High);

        // Lightweight reasoner (default Low, max Medium)
        let light = ModelThinkingSpec::lightweight_reasoner();
        assert_eq!(light.resolve(None), ReasoningEffort::Low);
        assert_eq!(light.resolve(Some(ReasoningEffort::High)), ReasoningEffort::Medium); // Clamped!
        assert_eq!(light.resolve(Some(ReasoningEffort::Low)), ReasoningEffort::Low);

        // Non reasoner (default Off, max Off)
        let non = ModelThinkingSpec::non_reasoner();
        assert_eq!(non.resolve(None), ReasoningEffort::Off);
        assert_eq!(non.resolve(Some(ReasoningEffort::High)), ReasoningEffort::Off); // Clamped to Off!
        assert!(!non.supports_reasoning());

        // Name inference
        let o3 = ModelThinkingSpec::infer_from_model_name("o3-mini");
        assert_eq!(o3.default_effort, ReasoningEffort::Low);
        assert_eq!(o3.max_effort, ReasoningEffort::Medium);

        let opus = ModelThinkingSpec::infer_from_model_name("claude-opus-5");
        assert_eq!(opus.default_effort, ReasoningEffort::Medium);
        assert_eq!(opus.max_effort, ReasoningEffort::High);

        let gpt4o = ModelThinkingSpec::infer_from_model_name("gpt-4o");
        assert_eq!(gpt4o.default_effort, ReasoningEffort::Off);
        assert_eq!(gpt4o.max_effort, ReasoningEffort::Off);
    }
}
