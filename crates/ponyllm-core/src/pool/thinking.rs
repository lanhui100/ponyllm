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
    ReasoningEffort::High
}

pub fn default_thinking_max() -> ReasoningEffort {
    ReasoningEffort::High
}

impl Default for ModelThinkingSpec {
    fn default() -> Self {
        Self {
            default_effort: ReasoningEffort::High,
            max_effort: ReasoningEffort::High,
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

    /// Standard reasoning effort levels supported by the project (Off, Low, Medium, High, Max).
    pub const ALL_EFFORT_TIERS: [ReasoningEffort; 5] = [
        ReasoningEffort::Off,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Max,
    ];

    /// Match an arbitrary client string or tier name (including low, medium, high, max, xhigh, ultra) into ReasoningEffort.
    /// xhigh / ultra / max map to Max (or High). Falls back to default (High) if unrecognized or empty.
    pub fn match_4tier_effort(val: Option<&str>) -> ReasoningEffort {
        match val {
            Some(s) => ReasoningEffort::from_str_loose(s).unwrap_or(ReasoningEffort::High),
            None => ReasoningEffort::High,
        }
    }

    /// Pure reasoning model (default=High, max=High)
    pub fn standard_reasoner() -> Self {
        Self {
            default_effort: ReasoningEffort::High,
            max_effort: ReasoningEffort::High,
        }
    }

    /// Lightweight reasoning model (default=High, max=High)
    pub fn lightweight_reasoner() -> Self {
        Self {
            default_effort: ReasoningEffort::High,
            max_effort: ReasoningEffort::High,
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
    /// Default is High for all models unless explicitly non-reasoning or configured.
    pub fn infer_from_model_name(model_name: &str) -> Self {
        let lower = model_name.trim().to_ascii_lowercase();
        // Non-reasoning models: gpt-4o, gpt-3.5, embedding, whisper, tts, dall-e, etc.
        let is_non_reasoner = lower.contains("gpt-4o")
            || lower.contains("gpt-4-turbo")
            || lower.contains("gpt-3.5")
            || lower.contains("embedding")
            || lower.contains("whisper")
            || lower.contains("tts")
            || lower.contains("dall-e")
            || lower.contains("imagen");

        if is_non_reasoner {
            Self::non_reasoner()
        } else if lower.contains("gemini-3") {
            // For gemini-3 models, default is tiered (represented as Off for effort to avoid forcing -high),
            // while supporting full reasoning range (Off through Max) so client can request any effort.
            Self {
                default_effort: ReasoningEffort::Off,
                max_effort: ReasoningEffort::Max,
            }
        } else {
            Self::standard_reasoner()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_thinking_spec_clamping_and_fallback() {
        // Standard reasoner (default High, max High)
        let spec = ModelThinkingSpec::standard_reasoner();
        assert_eq!(spec.resolve(None), ReasoningEffort::High);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Off)), ReasoningEffort::Off);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Low)), ReasoningEffort::Low);
        assert_eq!(spec.resolve(Some(ReasoningEffort::Medium)), ReasoningEffort::Medium);
        assert_eq!(spec.resolve(Some(ReasoningEffort::High)), ReasoningEffort::High);

        // Custom clamped reasoner
        let light = ModelThinkingSpec::new(ReasoningEffort::Low, ReasoningEffort::Medium);
        assert_eq!(light.resolve(None), ReasoningEffort::Low);
        assert_eq!(light.resolve(Some(ReasoningEffort::High)), ReasoningEffort::Medium); // Clamped!
        assert_eq!(light.resolve(Some(ReasoningEffort::Low)), ReasoningEffort::Low);

        // Non reasoner (default Off, max Off)
        let non = ModelThinkingSpec::non_reasoner();
        assert_eq!(non.resolve(None), ReasoningEffort::Off);
        assert_eq!(non.resolve(Some(ReasoningEffort::High)), ReasoningEffort::Off); // Clamped to Off!
        assert!(!non.supports_reasoning());

        // Name inference defaults to High
        let o3 = ModelThinkingSpec::infer_from_model_name("o3-mini");
        assert_eq!(o3.default_effort, ReasoningEffort::High);
        assert_eq!(o3.max_effort, ReasoningEffort::High);

        let opus = ModelThinkingSpec::infer_from_model_name("claude-opus-5");
        assert_eq!(opus.default_effort, ReasoningEffort::High);
        assert_eq!(opus.max_effort, ReasoningEffort::High);

        let gpt4o = ModelThinkingSpec::infer_from_model_name("gpt-4o");
        assert_eq!(gpt4o.default_effort, ReasoningEffort::Off);
        assert_eq!(gpt4o.max_effort, ReasoningEffort::Off);

        let embed = ModelThinkingSpec::infer_from_model_name("text-embedding-3-small");
        assert_eq!(embed.default_effort, ReasoningEffort::Off);
        assert_eq!(embed.max_effort, ReasoningEffort::Off);

        // 4-tier match
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("low")), ReasoningEffort::Low);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("standard")), ReasoningEffort::Medium);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("deep")), ReasoningEffort::High);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("xhigh")), ReasoningEffort::Max);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("ultra")), ReasoningEffort::Max);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("max")), ReasoningEffort::Max);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(Some("off")), ReasoningEffort::Off);
        assert_eq!(ModelThinkingSpec::match_4tier_effort(None), ReasoningEffort::High);
    }
}
