use serde::{Deserialize, Serialize};

/// Pricing mode: Uniform (flat rate) or PeakValley (time-of-use rate)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PricingMode {
    #[default]
    Uniform,
    PeakValley,
}

/// Peak pricing period with start/end time in "HH:MM" 24h format,
/// and an option to include weekends (Saturdays & Sundays).
/// Times are interpreted in Beijing Time (UTC+8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricingPeriod {
    #[serde(default)]
    pub name: String,
    pub start_time: String,
    pub end_time: String,
    pub input_price: f64,
    pub cached_price: f64,
    pub output_price: f64,
    /// Whether peak pricing applies on weekends. If false, weekends use valley baseline.
    #[serde(default)]
    pub include_weekends: bool,
}

impl PricingPeriod {
    /// Check whether a given "HH:MM" time string falls into this period (inclusive start, exclusive end).
    /// Supports wrapping around midnight (e.g. 22:00 to 06:00).
    pub fn matches_time(&self, time_hm: &str) -> bool {
        let cur = time_hm.trim();
        let start = self.start_time.trim();
        let end = self.end_time.trim();
        if start <= end {
            cur >= start && cur < end
        } else {
            cur >= start || cur < end
        }
    }

    /// Check whether this peak period applies at the given Beijing datetime
    pub fn matches_datetime(&self, bj_dt: &chrono::DateTime<chrono::FixedOffset>) -> bool {
        use chrono::Datelike;
        let weekday = bj_dt.weekday();
        let is_weekend = weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun;
        if is_weekend && !self.include_weekends {
            return false;
        }
        let cur_hm = bj_dt.format("%H:%M").to_string();
        self.matches_time(&cur_hm)
    }
}

/// Pricing specification for a model provider (Units: USD per 1 Million Tokens)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricingConfig {
    /// Pricing mode: uniform or peak_valley
    #[serde(default)]
    pub mode: PricingMode,

    /// Regular input price per 1M tokens (cache miss)
    #[serde(default = "default_input_price")]
    pub input_price: f64,

    /// Cached input price per 1M tokens (cache hit read, usually 10%~50% of input_price)
    #[serde(default = "default_cached_price")]
    pub cached_price: f64,

    /// Output generation price per 1M tokens
    #[serde(default = "default_output_price")]
    pub output_price: f64,

    /// Peak-valley / time-of-use pricing periods (active when mode == PeakValley)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_periods: Vec<PricingPeriod>,
}

pub fn default_input_price() -> f64 {
    0.50
}

pub fn default_cached_price() -> f64 {
    0.25
}

pub fn default_output_price() -> f64 {
    1.00
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            mode: PricingMode::Uniform,
            input_price: default_input_price(),
            cached_price: default_cached_price(),
            output_price: default_output_price(),
            pricing_periods: Vec::new(),
        }
    }
}

impl PricingConfig {
    /// Check if this provider is genuinely free (prices explicitly 0.0 with epsilon precision)
    pub fn is_free(&self) -> bool {
        if self.mode == PricingMode::PeakValley && !self.pricing_periods.is_empty() {
            return self.pricing_periods.iter().all(|p| {
                p.input_price.abs() < 1e-6 && p.cached_price.abs() < 1e-6 && p.output_price.abs() < 1e-6
            });
        }
        self.input_price.abs() < 1e-6 && self.cached_price.abs() < 1e-6 && self.output_price.abs() < 1e-6
    }

    /// Resolve effective prices (input, cached, output) for the current moment or fallback to baseline (valley price)
    pub fn resolve_current_prices(&self) -> (f64, f64, f64) {
        if self.mode == PricingMode::PeakValley && !self.pricing_periods.is_empty() {
            // Use Beijing time UTC+8 as standard convention for peak/valley tariffs
            let tz_offset = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
            let bj_dt = chrono::Utc::now().with_timezone(&tz_offset);
            if let Some(period) = self.pricing_periods.iter().find(|p| p.matches_datetime(&bj_dt)) {
                return (period.input_price, period.cached_price, period.output_price);
            }
            // During off-peak (valley) or non-peak weekends, baseline (uniform input/cached/output) applies
        }
        (self.input_price, self.cached_price, self.output_price)
    }

    /// Estimate total cost for given input tokens, cached state, and expected output tokens
    pub fn estimate_cost(&self, input_tokens: usize, is_cached: bool, expected_output_tokens: usize) -> f64 {
        if self.is_free() {
            return 0.0;
        }
        let (in_p, ca_p, out_p) = self.resolve_current_prices();
        let in_rate = if is_cached { ca_p.max(0.0) } else { in_p.max(0.0) };
        let out_rate = out_p.max(0.0);
        let in_cost = (input_tokens as f64 / 1_000_000.0) * in_rate;
        let out_cost = (expected_output_tokens as f64 / 1_000_000.0) * out_rate;
        in_cost + out_cost
    }
}

/// Provider billing model: Metered (Pay-as-you-go) or Plan (Periodic fixed quota)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    #[default]
    Metered,
    Plan,
    Free,
}
