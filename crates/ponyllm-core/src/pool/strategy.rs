use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingStrategy {
    RoundRobin,
    Priority,
    WeightedRoundRobin,
}

/// Global or per-request gateway strategy mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GatewayRoutingStrategy {
    /// Economy first: Plan nodes ($0) > Hot Cache hit > Lowest metered price (Default)
    #[default]
    Economy,
    /// Speed first: Minimum End-to-End Latency = TTFT + (ExpectedTokens / TPS)
    Speed,
    /// Reliability first: High SLA & avoid 429 rate-limited nodes
    Reliable,
    /// Balanced: Pareto weighted score
    Balanced,
}

impl FromStr for GatewayRoutingStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "economy" | "cheap" | "cheapest" | "e" => Ok(Self::Economy),
            "speed" | "fastest" | "fast" | "s" => Ok(Self::Speed),
            "reliable" | "ha" | "stability" | "r" => Ok(Self::Reliable),
            "balanced" | "auto" | "b" => Ok(Self::Balanced),
            _ => Err(format!("Unknown routing strategy '{}'", s)),
        }
    }
}

impl fmt::Display for GatewayRoutingStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Economy => write!(f, "economy"),
            Self::Speed => write!(f, "speed"),
            Self::Reliable => write!(f, "reliable"),
            Self::Balanced => write!(f, "balanced"),
        }
    }
}

/// Capability Tier for AI models with single-letter shorthand (F/S/L)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ModelTier {
    /// Light (L): Ultra-fast single-line completion & quick tasks
    Light,
    /// Standard (S): Balanced high-throughput daily driver
    Standard,
    /// Flagship (F): Top-tier reasoning & code intelligence (Default)
    #[default]
    Flagship,
}

impl ModelTier {
    pub fn shorthand(&self) -> &'static str {
        match self {
            Self::Flagship => "F",
            Self::Standard => "S",
            Self::Light => "L",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Flagship => "Flagship (旗舰)",
            Self::Standard => "Standard (主力)",
            Self::Light => "Light (轻量)",
        }
    }
}

impl FromStr for ModelTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "F" | "FLAGSHIP" | "TOP" | "FLAG" => Ok(Self::Flagship),
            "S" | "STANDARD" | "MID" | "MAIN" => Ok(Self::Standard),
            "L" | "LIGHT" | "SMALL" | "MINI" => Ok(Self::Light),
            _ => Err(format!("Invalid model tier '{}', must be F, S, or L", s)),
        }
    }
}

impl fmt::Display for ModelTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.shorthand())
    }
}

impl Serialize for ModelTier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.shorthand())
    }
}

impl<'de> Deserialize<'de> for ModelTier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Parse context window string (e.g. "1M", "1024K", "256K", "128K", "32K") to numeric token count
pub fn parse_context_capacity_tokens(s: &str) -> usize {
    let clean = s.trim().to_ascii_uppercase();
    if clean.ends_with('M') {
        let num: f64 = clean.trim_end_matches('M').parse().unwrap_or(1.0);
        (num * 1024.0 * 1024.0) as usize
    } else if clean.ends_with('K') {
        let num: f64 = clean.trim_end_matches('K').parse().unwrap_or(128.0);
        (num * 1024.0) as usize
    } else {
        clean.parse::<usize>().unwrap_or(131072) // Default 128K
    }
}

/// Check if context capacity transition is valid (Only allows equal or increasing capacity)
pub fn is_context_capacity_compatible(source_capacity_str: &str, target_capacity_str: &str) -> bool {
    let src = parse_context_capacity_tokens(source_capacity_str);
    let tgt = parse_context_capacity_tokens(target_capacity_str);
    tgt >= src
}
