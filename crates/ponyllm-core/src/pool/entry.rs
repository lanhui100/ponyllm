use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use parking_lot::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Active,
    CoolingDown,
    Disabled,
}

#[derive(Debug, Clone)]
pub enum PoolErrorType {
    RateLimit { retry_after: Option<Duration> },
    QuotaExhausted,
    AuthInvalid,
    ServerError,
    NetworkError,
}

#[derive(Debug)]
pub struct KeyStats {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub consecutive_failures: AtomicUsize,
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
            cooldown_until: RwLock::new(None),
            disabled_reason: RwLock::new(None),
        }
    }
}

#[derive(Debug)]
pub struct ApiKeyEntry {
    pub id: String,
    pub api_key: String,
    pub priority: u32,
    pub weight: u32,
    pub stats: KeyStats,
}

impl ApiKeyEntry {
    pub fn new(id: impl Into<String>, api_key: impl Into<String>, priority: u32, weight: u32) -> Self {
        Self {
            id: id.into(),
            api_key: api_key.into(),
            priority,
            weight,
            stats: KeyStats::default(),
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

    /// Record a failed request and transition state accordingly
    pub fn record_failure(&self, err_type: PoolErrorType) {
        self.stats.total_requests.fetch_add(1, Ordering::Relaxed);
        self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
        let consecutive = self.stats.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

        match err_type {
            PoolErrorType::RateLimit { retry_after } => {
                let duration = retry_after.unwrap_or_else(|| {
                    // Exponential backoff with light jitter (base 2s * 2^(consecutive - 1) + jitter)
                    let base_secs = (2u64.saturating_pow((consecutive as u32).saturating_sub(1))).min(60);
                    let jitter_millis = (consecutive as u64 * 37) % 500;
                    Duration::from_millis(base_secs * 1000 + jitter_millis)
                });
                *self.stats.cooldown_until.write() = Some(Instant::now() + duration);
            }
            PoolErrorType::QuotaExhausted => {
                *self.stats.disabled_reason.write() = Some("Quota exceeded".to_string());
            }
            PoolErrorType::AuthInvalid => {
                *self.stats.disabled_reason.write() = Some("Authentication failed (invalid key)".to_string());
            }
            PoolErrorType::ServerError | PoolErrorType::NetworkError => {
                if consecutive >= 3 {
                    // Temporarily cooldown for 10s after 3 consecutive failures
                    *self.stats.cooldown_until.write() = Some(Instant::now() + Duration::from_secs(10));
                }
            }
        }
    }
}
