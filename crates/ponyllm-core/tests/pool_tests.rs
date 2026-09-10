use std::sync::Arc;
use std::time::{Duration, SystemTime};
use ponyllm_core::pool::*;

#[test]
fn test_key_pool_round_robin() {
    let pool = KeyPool::new("openai", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("key-1", "sk-test-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("key-2", "sk-test-2", 1, 10));

    let k1 = pool.select_key().unwrap();
    let k2 = pool.select_key().unwrap();
    let k3 = pool.select_key().unwrap();

    assert_ne!(k1.id, k2.id);
    assert_eq!(k1.id, k3.id);
}

#[test]
fn test_key_pool_priority_selection() {
    let pool = KeyPool::new("anthropic", RoutingStrategy::Priority);
    // Priority 1 is higher priority than Priority 2
    pool.add_key(ApiKeyEntry::new("primary", "sk-ant-primary", 1, 10));
    pool.add_key(ApiKeyEntry::new("backup", "sk-ant-backup", 2, 10));

    // When primary is active, always select primary
    for _ in 0..5 {
        let k = pool.select_key().unwrap();
        assert_eq!(k.id, "primary");
    }
}

#[test]
fn test_key_cooldown_on_429_and_automatic_failover() {
    let pool = KeyPool::new("openai", RoutingStrategy::Priority);
    pool.add_key(ApiKeyEntry::new("primary", "sk-openai-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("backup", "sk-openai-2", 2, 10));

    // Initially selects primary
    let k1 = pool.select_key().unwrap();
    assert_eq!(k1.id, "primary");

    // Primary hits 429 Rate Limit
    pool.record_error("primary", PoolErrorType::RateLimit { retry_after: Some(Duration::from_millis(50)) });

    // Next selection should automatically failover to backup!
    let k2 = pool.select_key().unwrap();
    assert_eq!(k2.id, "backup");

    // Wait for cooldown to expire
    std::thread::sleep(Duration::from_millis(60));

    // Primary should be recovered and selected again
    let k3 = pool.select_key().unwrap();
    assert_eq!(k3.id, "primary");
}

#[test]
fn test_key_cooling_on_quota_exhausted() {
    let pool = KeyPool::new("deepseek", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-ds-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-ds-2", 1, 10));

    // k1 hits quota exceeded -> cools down (never permanently disabled)
    pool.record_error("k1", PoolErrorType::QuotaExhausted { retry_after: Some(Duration::from_secs(60)) });
    assert_eq!(pool.get_key_status("k1"), Some(KeyState::CoolingDown));

    // Only k2 should be returned while k1 cools
    for _ in 0..5 {
        let k = pool.select_key().unwrap();
        assert_eq!(k.id, "k2");
    }
}

#[test]
fn test_all_keys_cooling_still_exhausts_pool() {
    let pool = KeyPool::new("openai", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));

    pool.record_error("k1", PoolErrorType::QuotaExhausted { retry_after: None });

    assert_eq!(pool.get_key_status("k1"), Some(KeyState::CoolingDown));
    let res = pool.select_key();
    assert!(res.is_err());
}

#[test]
fn test_quota_cooldown_exposes_advertised_reset() {
    let pool = KeyPool::new("antigravity", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("ag-solo", "sk-1", 1, 10));

    // 429 quota body advertised "Resets in 15h21m26s".
    let reset = Duration::from_secs(15 * 3600 + 21 * 60 + 26);
    pool.record_error(
        "ag-solo",
        PoolErrorType::QuotaExhausted {
            retry_after: Some(reset),
        },
    );
    assert_eq!(pool.get_key_status("ag-solo"), Some(KeyState::CoolingDown));

    let (remaining, reset_at) = pool.key_cooldown("ag-solo");
    let remaining = remaining.expect("cooling key must report remaining");
    assert!(
        remaining >= reset - Duration::from_secs(5) && remaining <= reset,
        "expected ~{:?} remaining, got {:?}",
        reset,
        remaining
    );
    assert!(
        reset_at.is_some(),
        "wall-clock reset must be exposed for the Web badge"
    );

    // Unknown key / healthy key expose nothing.
    assert_eq!(pool.key_cooldown("nope"), (None, None));
}

#[test]
fn test_cooldown_never_shortened_by_later_transient_error() {
    let entry = ApiKeyEntry::new("ag-1", "sk-1", 1, 10);
    let reset = Duration::from_secs(15 * 3600 + 21 * 60 + 26);

    entry.record_failure(PoolErrorType::QuotaExhausted {
        retry_after: Some(reset),
    });
    // An in-flight request failing with a short transient error must not
    // shorten the advertised quota window, or traffic resumes mid-window.
    entry.record_failure(PoolErrorType::RateLimit { retry_after: None });

    let remaining = entry.cooldown_remaining().expect("still cooling");
    assert!(
        remaining >= reset - Duration::from_secs(5),
        "quota window was shortened to {:?}",
        remaining
    );
    assert_eq!(entry.current_state(), KeyState::CoolingDown);
}

#[test]
fn test_concurrent_cooldowns_keep_mirror_in_sync() {
    // Regression guard for the two-field race: a longer deadline installed by
    // one thread must never coexist with an older wall-clock mirror, or the
    // Web badge under-reports and hides the hint while the key still cools.
    let entry = Arc::new(ApiKeyEntry::new("ag-1", "sk-1", 1, 10));
    let durations = [3600u64, 5400, 7200, 9000, 10800, 12600, 14400, 15 * 3600 + 21 * 60];
    let mut handles = Vec::new();
    for secs in durations {
        let e = entry.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..200 {
                e.record_failure(PoolErrorType::QuotaExhausted {
                    retry_after: Some(Duration::from_secs(secs)),
                });
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let remaining = entry.cooldown_remaining().expect("still cooling");
    let reset_at = entry
        .cooldown_reset_at()
        .expect("mirror must be present while cooling");
    let mirror_remaining = reset_at
        .duration_since(SystemTime::now())
        .expect("mirror must be in the future while cooling");
    let drift = remaining.abs_diff(mirror_remaining);
    assert!(
        drift <= Duration::from_secs(2),
        "mirror drifted from the deadline by {:?} (deadline {:?}, mirror {:?})",
        drift,
        remaining,
        mirror_remaining
    );
}

#[test]
fn test_mass_disable_breaker_downgrades_to_cooling() {
    // 2-key pool: permanently isolating one key would leave only 50%
    // alive, so the breaker must downgrade to a 5-minute cooling.
    let pool = KeyPool::new("ag", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));

    pool.record_error("k1", PoolErrorType::PolicyViolation);
    assert_eq!(
        pool.get_key_status("k1"),
        Some(KeyState::CoolingDown),
        "breaker must prevent the first permanent isolate in a 2-key pool"
    );

    // Single-key pools are exempt: nothing to protect, isolate directly.
    let solo = KeyPool::new("solo", RoutingStrategy::RoundRobin);
    solo.add_key(ApiKeyEntry::new("only", "sk-1", 1, 10));
    solo.record_error("only", PoolErrorType::PolicyViolation);
    assert_eq!(solo.get_key_status("only"), Some(KeyState::Disabled));
}

#[test]
fn test_breaker_allows_isolate_above_floor() {
    // 3-key pool: isolating the first key leaves 2/3 alive (> 50%),
    // so the permanent isolate goes through.
    let pool = KeyPool::new("ag", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));
    pool.add_key(ApiKeyEntry::new("k3", "sk-3", 1, 10));

    pool.record_error("k1", PoolErrorType::PolicyViolation);
    assert_eq!(pool.get_key_status("k1"), Some(KeyState::Disabled));

    // Isolating a second key would leave 1/3 alive: breaker trips.
    pool.record_error("k2", PoolErrorType::PolicyViolation);
    assert_eq!(pool.get_key_status("k2"), Some(KeyState::CoolingDown));
}

#[test]
fn test_entry_debug_never_prints_raw_key() {
    let entry = ApiKeyEntry::new("k1", "sk-live-secret-value-12345", 1, 10);
    let dbg = format!("{:?}", entry);
    assert!(!dbg.contains("sk-live-secret-value-12345"), "got {}", dbg);
    assert!(dbg.contains("k1"));
}

#[test]
fn test_rate_limit_default_cooldown_duration() {
    let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);
    // 429 with no retry_after from upstream (e.g. SenseNova)
    entry.record_failure(PoolErrorType::RateLimit { retry_after: None });

    assert_eq!(entry.current_state(), KeyState::CoolingDown);
    let cd_until = entry.stats.cooldown_until.read().unwrap();
    // Default cooldown should be ~3s (base) + jitter, cooperating with downstream retry curves
    let remaining = cd_until.saturating_duration_since(std::time::Instant::now());
    assert!(
        remaining >= std::time::Duration::from_secs(2) && remaining <= std::time::Duration::from_secs(4),
        "Expected cooldown ~3s for first 429 (cooperate with downstream 1.5-3s retry), got {:?}",
        remaining
    );
}

#[test]
fn test_rate_limit_exponential_backoff_progression() {
    let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);

    // First 429: ~3s
    entry.record_failure(PoolErrorType::RateLimit { retry_after: None });
    let cd1 = entry.stats.cooldown_until.read().unwrap();
    let r1 = cd1.saturating_duration_since(std::time::Instant::now());
    assert!(r1 >= Duration::from_secs(2) && r1 <= Duration::from_secs(4),
        "1st 429: expected ~3s, got {:?}", r1);

    // Second 429: ~6s
    entry.record_failure(PoolErrorType::RateLimit { retry_after: None });
    let cd2 = entry.stats.cooldown_until.read().unwrap();
    let r2 = cd2.saturating_duration_since(std::time::Instant::now());
    assert!(r2 >= Duration::from_secs(5) && r2 <= Duration::from_secs(7),
        "2nd 429: expected ~6s, got {:?}", r2);

    // Third 429: ~12s
    entry.record_failure(PoolErrorType::RateLimit { retry_after: None });
    let cd3 = entry.stats.cooldown_until.read().unwrap();
    let r3 = cd3.saturating_duration_since(std::time::Instant::now());
    assert!(r3 >= Duration::from_secs(11) && r3 <= Duration::from_secs(13),
        "3rd 429: expected ~12s, got {:?}", r3);
}

#[test]
fn test_rate_limit_cooldown_capped_at_60s() {
    let entry = ApiKeyEntry::new("k1", "sk-1", 1, 10);

    // Simulate 10 consecutive 429s — should cap at 60s, never exceed
    for _ in 0..10 {
        entry.record_failure(PoolErrorType::RateLimit { retry_after: None });
    }

    let cd = entry.stats.cooldown_until.read().unwrap();
    let remaining = cd.saturating_duration_since(std::time::Instant::now());
    assert!(remaining <= Duration::from_secs(61),
        "Cooldown should cap at 60s, got {:?}", remaining);
}

#[test]
fn test_single_key_429_exhaustion_triggers_cooldown() {
    let pool = KeyPool::new("solo", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("only", "sk-1", 1, 10));

    // First and second 429: transient retry allowed (stays Active)
    pool.record_error("only", PoolErrorType::RateLimit { retry_after: None });
    assert_eq!(pool.get_key_status("only"), Some(KeyState::Active));

    pool.record_error("only", PoolErrorType::RateLimit { retry_after: None });
    assert_eq!(pool.get_key_status("only"), Some(KeyState::Active));

    // Third 429: sustained rate limiting -> must cool down to prevent hammering storm
    pool.record_error("only", PoolErrorType::RateLimit { retry_after: None });
    assert_eq!(pool.get_key_status("only"), Some(KeyState::CoolingDown));
}

#[test]
fn test_two_key_pool_tos_second_strike_permanent_isolation() {
    let pool = KeyPool::new("ag", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-2", 1, 10));

    // First ToS strike on k1: breaker downgrades to cooling to prevent sudden panic
    pool.record_error("k1", PoolErrorType::PolicyViolation);
    assert_eq!(pool.get_key_status("k1"), Some(KeyState::CoolingDown));

    // Second ToS strike on k1: confirmed dead credential, breaker permits permanent isolation!
    pool.record_error("k1", PoolErrorType::PolicyViolation);
    assert_eq!(pool.get_key_status("k1"), Some(KeyState::Disabled));
}

