use std::time::Duration;
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
fn test_key_disabled_on_quota_exceeded() {
    let pool = KeyPool::new("deepseek", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-ds-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-ds-2", 1, 10));

    // k1 hits quota exceeded
    pool.record_error("k1", PoolErrorType::QuotaExhausted);

    // Only k2 should be returned from now on
    for _ in 0..5 {
        let k = pool.select_key().unwrap();
        assert_eq!(k.id, "k2");
    }
}

#[test]
fn test_all_keys_exhausted() {
    let pool = KeyPool::new("openai", RoutingStrategy::RoundRobin);
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));

    pool.record_error("k1", PoolErrorType::QuotaExhausted);

    let res = pool.select_key();
    assert!(res.is_err());
}
