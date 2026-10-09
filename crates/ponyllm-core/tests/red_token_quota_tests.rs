//! 红相契约测试（B001）：TokenQuotaTracker（`ponyllm_core::token_quota`）。
//!
//! 冻结契约（protocol-agent 锁定，Lead 契约为准）：与 UserQuotaTracker 同构，
//! keyed by gateway key id；配额不存于 tracker，`check_quota(key_id, quota_limit)`
//! 检查时传入（`Option<u64>`，None=不限；used >= limit → Err(QuotaExhausted)）。
//! ```text
//! pub fn new() -> Self;
//! pub fn upsert(&self, key_id: impl Into<String>);       // 建行/刷新，保留已有 used
//! pub fn remove(&self, key_id: &str) -> Option<String>;
//! pub fn record_tokens(&self, key_id: &str, tokens: u64);
//! pub fn get_used_tokens(&self, key_id: &str) -> u64;
//! pub fn reset_usage(&self, key_id: &str) -> bool;
//! pub fn check_quota(&self, key_id: &str, quota_limit: Option<u64>)
//!     -> Result<(), TokenQuotaError>;
//! ```
//!
//! 红相状态：契约尚未实现，断言预期 FAIL。并发用例为确定性并发（Barrier 同步起步 +
//! 原子累加，无 sleep）。

use std::sync::{Arc, Barrier};

use ponyllm_core::token_quota::{TokenQuotaError, TokenQuotaTracker};

/// 基本流：upsert → record → get_used_tokens → check_quota（超限拒绝）→ reset_usage。
#[test]
fn basic_flow_upsert_record_check_reset() {
    // Arrange
    let tracker = TokenQuotaTracker::new();

    // Act
    tracker.upsert("tk-1");
    let qty = tracker.get_used_tokens("tk-1");

    // Assert —— 新行 used 从 0 开始
    assert_eq!(qty, 0, "fresh token must start at zero usage");
    assert_eq!(
        tracker.check_quota("tk-1", Some(100)),
        Ok(()),
        "0 < 100 must pass"
    );

    // 消耗 60
    tracker.record_tokens("tk-1", 60);
    assert_eq!(tracker.get_used_tokens("tk-1"), 60);
    assert_eq!(
        tracker.check_quota("tk-1", Some(100)),
        Ok(()),
        "60 < 100 must pass"
    );

    // 再消耗 50 → 110 ≥ 100，必须拒绝
    tracker.record_tokens("tk-1", 50);
    assert_eq!(tracker.get_used_tokens("tk-1"), 110);
    match tracker.check_quota("tk-1", Some(100)) {
        Err(TokenQuotaError::QuotaExhausted {
            key_id,
            used_tokens,
            quota_limit,
        }) => {
            assert_eq!(key_id, "tk-1");
            assert_eq!(used_tokens, 110);
            assert_eq!(quota_limit, 100);
        }
        other => panic!("used 110 >= limit 100 must be QuotaExhausted, got {other:?}"),
    }

    // 重置后恢复
    assert!(
        tracker.reset_usage("tk-1"),
        "reset on existing token must return true"
    );
    assert_eq!(tracker.get_used_tokens("tk-1"), 0);
    assert_eq!(
        tracker.check_quota("tk-1", Some(100)),
        Ok(()),
        "after reset must pass again"
    );
}

/// 未知 token：check_quota → TokenNotFound；get_used_tokens → 0；reset_usage → false。
#[test]
fn unknown_token_reports_not_found() {
    // Arrange
    let tracker = TokenQuotaTracker::new();

    // Act & Assert
    assert_eq!(tracker.get_used_tokens("no-such-token"), 0);
    assert_eq!(
        tracker.check_quota("no-such-token", Some(100)),
        Err(TokenQuotaError::TokenNotFound {
            key_id: "no-such-token".to_string(),
        })
    );
    assert!(
        !tracker.reset_usage("no-such-token"),
        "reset on unknown token must return false"
    );
}

/// remove：存在 → Some(key_id)；再次 remove / 未存在 → None。
#[test]
fn remove_deletes_entry() {
    // Arrange
    let tracker = TokenQuotaTracker::new();
    tracker.upsert("tk-rm");
    tracker.record_tokens("tk-rm", 7);

    // Act & Assert
    assert_eq!(tracker.remove("tk-rm"), Some("tk-rm".to_string()));
    assert_eq!(tracker.remove("tk-rm"), None, "second remove must be None");
    assert_eq!(tracker.remove("never-existed"), None);
    assert_eq!(
        tracker.get_used_tokens("tk-rm"),
        0,
        "removed token usage must be gone"
    );
}

/// upsert 幂等刷新：保留已有 used 计数（同构 upsert_user）。
#[test]
fn upsert_preserves_existing_usage() {
    // Arrange
    let tracker = TokenQuotaTracker::new();
    tracker.upsert("tk-2");
    tracker.record_tokens("tk-2", 5);

    // Act —— 再次 upsert（模拟配置热载刷新）
    tracker.upsert("tk-2");

    // Assert
    assert_eq!(
        tracker.get_used_tokens("tk-2"),
        5,
        "re-upsert must not reset usage"
    );
}

/// quota_limit = None：不限（无论 used 多大都放行）。
#[test]
fn no_quota_limit_means_unlimited() {
    // Arrange
    let tracker = TokenQuotaTracker::new();
    tracker.upsert("tk-∞");

    // Act
    tracker.record_tokens("tk-∞", 999_999);

    // Assert
    assert_eq!(
        tracker.check_quota("tk-∞", None),
        Ok(()),
        "None limit must never exhaust"
    );
    assert!(
        matches!(
            tracker.check_quota("tk-∞", Some(1_000)),
            Err(TokenQuotaError::QuotaExhausted { .. })
        ),
        "concrete limit must still be enforced when provided"
    );
}

/// 确定性并发：10 线程 Barrier 同步起步、各加 1 → 总数精确 = 10（原子累加，无 sleep）。
#[test]
fn concurrent_records_are_exactly_summed() {
    // Arrange
    const THREADS: u64 = 10;
    let tracker = Arc::new(TokenQuotaTracker::new());
    tracker.upsert("tk-conc");
    let barrier = Arc::new(Barrier::new(THREADS as usize));

    // Act —— 所有线程同时越过 Barrier 之后各 record 1
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            let tracker = Arc::clone(&tracker);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait(); // 确定性起步：禁止 sleep 盲等
                tracker.record_tokens("tk-conc", 1);
            });
        }
    });

    // Assert —— 无丢更新、无重复
    assert_eq!(
        tracker.get_used_tokens("tk-conc"),
        THREADS,
        "10 concurrent +1 records must sum to exactly 10"
    );
}
