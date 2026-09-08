use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use crate::pool::antigravity::AntigravityTokenManager;

/// Process-wide jitter counter: mixed with wall-clock nanos so concurrent
/// instances and synchronized retries desynchronize (B1). Not cryptographic,
/// only backoff decorrelation.
static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn backoff_jitter_millis(spread: u64) -> u64 {
    let n = JITTER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Knuth multiplicative mix of counter and clock, then bound.
    let mixed = n
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(nanos.rotate_left(17));
    (mixed >> 11) % spread.max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Active,
    CoolingDown,
    Disabled,
}

#[derive(Debug, Clone)]
pub enum PoolErrorType {
    RateLimit { retry_after: Option<Duration> },
    /// Quota exhaustion cools the key down (never permanently disables):
    /// real quota recovers at `resetTime`, fake 429-style throttling clears
    /// on its own. `retry_after` defaults to a conservative 15 minutes when
    /// the upstream gave no explicit signal.
    QuotaExhausted { retry_after: Option<Duration> },
    AuthInvalid,
    PolicyViolation,
    ServerError,
    NetworkError,
}

#[derive(Debug)]
pub struct KeyStats {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub consecutive_failures: AtomicUsize,
    pub policy_violations: AtomicUsize,
    pub cooldown_until: RwLock<Option<Instant>>,
    pub disabled_reason: RwLock<Option<String>>,
}

impl Default for KeyStats {
    fn default() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            consecutive_failures: AtomicUsize::new(0),
            policy_violations: AtomicUsize::new(0),
            cooldown_until: RwLock::new(None),
            disabled_reason: RwLock::new(None),
        }
    }
}

#[derive(Clone)]
pub enum KeyAuth {
    Static(String),
    Antigravity(Arc<AntigravityTokenManager>),
}

impl std::fmt::Debug for KeyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(key) => {
                let masked = if key.len() > 8 {
                    format!("{}...{}", &key[..4], &key[key.len() - 4..])
                } else {
                    "***".to_string()
                };
                write!(f, "Static({})", masked)
            }
            Self::Antigravity(_) => write!(f, "Antigravity(TokenManager)"),
        }
    }
}

pub struct ApiKeyEntry {
    pub id: String,
    pub api_key: String,
    pub auth: KeyAuth,
    pub priority: u32,
    pub weight: u32,
    pub stats: KeyStats,
}

// Manual Debug: `#[derive(Debug)]` would print `api_key` verbatim into
// logs/dumps (P0-8). Only a short non-sensitive preview is emitted.
impl std::fmt::Debug for ApiKeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyEntry")
            .field("id", &self.id)
            .field("api_key", &crate::telemetry::FlightRecorder::sanitize_key(&self.api_key))
            .field("auth", &self.auth)
            .field("priority", &self.priority)
            .field("weight", &self.weight)
            .finish()
    }
}

impl ApiKeyEntry {
    pub fn new(id: impl Into<String>, api_key: impl Into<String>, priority: u32, weight: u32) -> Self {
        let k = api_key.into();
        Self {
            id: id.into(),
            api_key: k.clone(),
            auth: KeyAuth::Static(k),
            priority,
            weight,
            stats: KeyStats::default(),
        }
    }

    pub fn new_antigravity(
        id: impl Into<String>,
        manager: Arc<AntigravityTokenManager>,
        priority: u32,
        weight: u32,
    ) -> Self {
        Self {
            id: id.into(),
            api_key: String::new(),
            auth: KeyAuth::Antigravity(manager),
            priority,
            weight,
            stats: KeyStats::default(),
        }
    }

    pub fn is_antigravity(&self) -> bool {
        matches!(self.auth, KeyAuth::Antigravity(_))
    }

    pub fn antigravity_manager(&self) -> Option<Arc<AntigravityTokenManager>> {
        match &self.auth {
            KeyAuth::Antigravity(mgr) => Some(mgr.clone()),
            _ => None,
        }
    }

    pub async fn resolve_token(&self) -> crate::error::Result<String> {
        match &self.auth {
            KeyAuth::Static(key) => Ok(key.clone()),
            KeyAuth::Antigravity(mgr) => mgr.get_valid_token().await,
        }
    }

    /// Check the current effective state of the key with fast read-path
    pub fn current_state(&self) -> KeyState {
        if self.stats.disabled_reason.read().is_some() {
            return KeyState::Disabled;
        }

        // Fast read path: avoid write-lock contention under heavy concurrent reads
        {
            let cd_read = self.stats.cooldown_until.read();
            if let Some(until) = *cd_read {
                if Instant::now() < until {
                    return KeyState::CoolingDown;
                }
            } else {
                return KeyState::Active;
            }
        }

        // Slow path: upgrade to write lock only to reset expired cooldown
        let mut cd_write = self.stats.cooldown_until.write();
        if let Some(until) = *cd_write {
            if Instant::now() >= until {
                *cd_write = None;
                self.stats.consecutive_failures.store(0, Ordering::SeqCst);
                KeyState::Active
            } else {
                KeyState::CoolingDown
            }
        } else {
            KeyState::Active
        }
    }

    /// Record a successful request
    pub fn record_success(&self) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// Remaining cooldown, if still cooling.
    pub fn cooldown_remaining(&self) -> Option<Duration> {
        let guard = self.stats.cooldown_until.read();
        let until = (*guard)?;
        let now = Instant::now();
        if now < until {
            Some(until - now)
        } else {
            None
        }
    }

    /// Count a transient 429 without cooling, for singleton pools that
    /// always passthrough upstream instead of local isolation.
    pub fn record_transient_failure(&self) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.consecutive_failures.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a failed request and transition state accordingly
    pub fn record_failure(&self, err_type: PoolErrorType) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
        let consecutive = self.stats.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

        match err_type {
            PoolErrorType::RateLimit { retry_after } => {
                let duration = match retry_after {
                    Some(d) if d >= Duration::from_secs(300) => {
                        // Long policy-driven coolings (geo-gate #1008,
                        // quota): escalate 5m -> 30m -> 2h cap across
                        // consecutive hits so a sustained storm backs off
                        // instead of knocking every 5 minutes (B3).
                        let step = consecutive.min(4).saturating_sub(1) as u32;
                        d.saturating_mul(2u32.saturating_pow(step)).min(Duration::from_secs(7200))
                    }
                    Some(d) => d,
                    None => {
                        let base_multiplier = 2u64.saturating_pow((consecutive as u32).saturating_sub(1));
                        let base_secs = (3u64.saturating_mul(base_multiplier)).min(60);
                        Duration::from_millis(base_secs * 1000 + backoff_jitter_millis(500))
                    }
                };
                *self.stats.cooldown_until.write() = Some(Instant::now() + duration);
            }
            PoolErrorType::QuotaExhausted { retry_after } => {
                // Quota exhaustion is transient by nature (real quota
                // recovers at resetTime; throttling clears on its own), so
                // it cools down instead of permanently disabling the key.
                let duration = retry_after.unwrap_or(Duration::from_secs(15 * 60));
                *self.stats.cooldown_until.write() = Some(Instant::now() + duration);
            }
            PoolErrorType::AuthInvalid => {
                *self.stats.disabled_reason.write() = Some("Authentication failed (invalid key)".to_string());
            }
            PoolErrorType::PolicyViolation => {
                *self.stats.disabled_reason.write() = Some("Account policy violation / Terms of Service suspension (permanent isolate)".to_string());
            }
            PoolErrorType::ServerError | PoolErrorType::NetworkError => {
                if consecutive >= 3 {
                    let exp = (consecutive as u32).saturating_sub(3);
                    let secs = (1u64.saturating_mul(2u64.saturating_pow(exp))).min(30);
                    *self.stats.cooldown_until.write() = Some(Instant::now() + Duration::from_secs(secs));
                }
            }
        }
    }
}
