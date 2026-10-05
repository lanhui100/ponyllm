//! Phase-2b 安全修复验收（隔离测试，Test Agent 于修复实施前编写）。
//!
//! 契约矩阵：Phase-2b 审查修复 **R2**（限流桶回收：prune 需按时回收锁定过期条目；
//! `lockout_count > 0` 不再永久驻留 → 内存有界）。
//!
//! ## 红相说明
//! HEAD 上 `AuthRateLimiter::prune` 的 retain 条件含 `lockout_count > 0` → 锁定过期后
//! 条目仍永久驻留（内存无界）；且 prune 为私有、无存活条目计数可观察。
//! 本文件需要两个尚不存在的 pub 方法（编译失败 = R2 未实现/不可观察的直接证据）：
//!
//! ## 接口契约（Executor 按此实现）
//! ```ignore
//! impl AuthRateLimiter {
//!     pub fn prune_expired(&self);              // 公开 prune：丢弃窗口外失败与过期锁定的空条目
//!     pub fn live_budget_count(&self) -> usize; // 当前存活 (ip,prefix) 条数（内存有界断言）
//! }
//! ```
//! 语义：锁定过期后且失败记录为空 → 条目必须被清理（不得因 `lockout_count > 0` 驻留）。

use std::net::IpAddr;
use std::time::Duration;

use ponyllm_server::auth_ratelimit::AuthRateLimiter;

const PREFIX: &str = "sk-pony-admin";

#[test]
fn r2_lockout_entries_reclaimed_after_expiry() {
    let rl = AuthRateLimiter::new(1, 3, 1); // window=1s, limit=3, lockout=1s
    let ip: IpAddr = "203.0.113.55".parse().unwrap();

    // 3 次失败 → 触发锁定
    for _ in 0..3 {
        rl.record_failure(ip, PREFIX);
    }
    assert_eq!(rl.live_budget_count(), 1, "锁定后应恰有 1 条预算");
    assert_eq!(rl.check(ip, PREFIX), Err(()), "锁定中必须 429");

    // 锁定与失败窗口同时过期（window=1s, lockout=1s）
    std::thread::sleep(Duration::from_secs(2));

    rl.prune_expired();
    assert_eq!(
        rl.live_budget_count(),
        0,
        "R2: 锁定过期且无失败记录后条目必须可清理（lockout_count>0 不得永久驻留 → 内存有界），实际 {} 条",
        rl.live_budget_count()
    );
}

#[test]
fn r2_pruned_budget_is_reusable() {
    let rl = AuthRateLimiter::new(1, 3, 1);
    let ip: IpAddr = "203.0.113.56".parse().unwrap();

    for _ in 0..3 {
        rl.record_failure(ip, PREFIX);
    }
    assert_eq!(rl.check(ip, PREFIX), Err(()));

    std::thread::sleep(Duration::from_secs(2));
    rl.prune_expired();

    // 清理后：同 (ip,prefix) 重新获得完整预算
    assert_eq!(rl.check(ip, PREFIX), Ok(()), "R2: 回收后的条目应回到可用预算");
}

#[test]
fn r2_active_budgets_are_not_reclaimed() {
    let rl = AuthRateLimiter::new(10, 3, 10);
    let ip: IpAddr = "203.0.113.57".parse().unwrap();
    rl.record_failure(ip, PREFIX); // 1 次失败仍在窗口内（window=10s）
    rl.prune_expired();
    assert_eq!(
        rl.live_budget_count(),
        1,
        "窗口内活跃条目不得被误回收（防止 prune 过激导致限流失效）"
    );
}