//! Phase-2b 安全修复验收（隔离测试，Test Agent 于修复实施前编写）。
//!
//! 契约矩阵：Phase-2b 审查修复 **R3**（egress 正向缓存缩短/复检 → DNS rebinding
//! 窗口收窄）。
//!
//! ## 背景
//! `AppState::data_plane_egress_guard` 以 host 为键缓存判定结果；HEAD 上正向（Ok）
//! 缓存 TTL = 60s，DNS rebinding 窗口即 60s。R3 要求收窄正向 TTL，使窗口有界。
//!
//! ## 红相说明
//! HEAD 上 `EgressGuardVerdict::{ok, expires_at}` 字段为私有 → 集成测试无法读取
//! 缓存 TTL（编译失败 = R3 不可观察/未实现的直接证据）。修复需把 verdict 的
//! `expires_at` 改为 pub（或提供等价的 pub accessor），并收窄正向 TTL。
//!
//! ## 接口契约（Executor 按此实现）
//! ```ignore
//! // state.rs
//! pub struct EgressGuardVerdict {
//!     pub ok: bool,
//!     pub expires_at: std::time::Instant,   // ← 需 pub（本测试读取）
//! }
//! ```
//!
//! 正向缓存 TTL 上限取 **15s**（DNS rebinding 窗口有界；具体数值以本文件断言为准，
//! 如 Lead 有更小目标请同步收紧）。

use std::sync::Arc;
use std::time::Duration;

use ponyllm_server::{AppState, GatewayConfig};

/// 正向判定（放行字面量，免 DNS）后，缓存条目 TTL 必须 ≤ 15s。
#[tokio::test]
async fn r3_positive_egress_verdict_ttl_short() {
    let state = Arc::new(AppState::new(GatewayConfig::default()));
    let url = "http://203.0.113.88/"; // TEST-NET-3 字面量：放行且无需 DNS

    assert!(state.data_plane_egress_guard(url).await.is_ok(), "正向判定应放行");

    let cache = state.egress_guard_cache.lock().unwrap_or_else(|p| p.into_inner());
    let entry = cache
        .get("203.0.113.88")
        .cloned()
        .expect("正向判定应写入缓存");
    let ttl = entry
        .expires_at
        .saturating_duration_since(std::time::Instant::now());
    assert!(
        ttl <= Duration::from_secs(15),
        "R3: 正向缓存 TTL 必须 ≤ 15s（收窄 DNS rebinding 窗口），实际 {:?}（HEAD 为 60s，红相成立）",
        ttl
    );
}

/// 负向判定（阻断）TTL 也必须有界（不随 R3 放宽）。
#[tokio::test]
async fn r3_negative_egress_verdict_ttl_bounded() {
    let state = Arc::new(AppState::new(GatewayConfig::default()));
    let url = "http://10.0.0.9/"; // 私有段：阻断

    assert!(state.data_plane_egress_guard(url).await.is_err(), "负向判定应阻断");

    let cache = state.egress_guard_cache.lock().unwrap_or_else(|p| p.into_inner());
    let entry = cache
        .get("10.0.0.9")
        .cloned()
        .expect("负向判定应写入缓存");
    let ttl = entry
        .expires_at
        .saturating_duration_since(std::time::Instant::now());
    assert!(
        ttl <= Duration::from_secs(30),
        "R3: 负向缓存 TTL 必须有界，实际 {:?}",
        ttl
    );
}