//! 短窗计量器 [`ShortWindowMeter`]：per-key 60s 滑动窗口计 requests/tokens + in-flight 并发计数。
//!
//! 属于"统一账户额度计量与治理内核"（M1 计量内核，见
//! `.agents/notes/proposed/architecture/2026-09-30-unified-quota-metering-governance-kernel.md`）：
//! 长窗四要素计量沿用 [`super::usage::KeyUsageTracker`]（保留不动），本模块只补 60s 短窗与并发口径。
//!
//! 口径约定：
//! - **尝试即记账（保守）**：`record_attempt` 在请求发起/结算时即计入窗口，无论成败；
//!   上游 429 等失败请求同样占用窗口预算，避免"被限流 key 显零用量反被选中"。
//! - **cached 可配**：`record_attempt` 的 `tokens` 为上游口径总量（含缓存命中）；
//!   缓存命中可经 [`ShortWindowMeter::record_cached_tokens`] 单独记录，
//!   `remaining(..., count_cached)` 决定 TPM 预算是否扣减缓存部分（默认按上游口径全计）。
//! - **in-flight 并发**：独立原子计数，与 rpm/tpm 预算正交，供调度层单独约束并发上限。
//!
//! 实现：12 × 5s 环形 slot 覆盖 60s（与 ADR 建议一致）。窗口查询按 slot 起始毫秒
//! 时间戳判定，纳入整 slot（保守口径：跨边界请求整个计入）。墙钟取系统时钟毫秒；
//! 内置 `fixed_clock_ms` 测试钩子（非 0 时固定墙钟）供确定性窗口滚动单测。
//! 线程安全：`parking_lot::Mutex` + 原子，与仓库 `usage.rs` 风格一致。

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 默认滑动窗口：60s（12 × 5s 环形 slot）。
const DEFAULT_WINDOW_SECS: u64 = 60;
/// 单个 slot 粒度：5s。
const SLOT_MS: u64 = 5_000;
/// 环形 slot 数：12 × 5s = 60s。
const SLOT_COUNT: usize = 12;

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// slot 起始毫秒时间戳（0 = 空槽）。
    start_ms: u64,
    requests: u64,
    tokens: u64,
    cached_tokens: u64,
}

#[derive(Debug, Default)]
struct Inner {
    slots: [Slot; SLOT_COUNT],
}

/// 60s 短窗计量器（per-key）。
#[derive(Debug)]
pub struct ShortWindowMeter {
    inner: Mutex<Inner>,
    /// 进行中（in-flight）并发请求数。
    in_flight: AtomicU32,
    /// 成功请求生命周期计数（`record_success`，不重复计入窗口 request）。
    successes: AtomicU64,
    /// 测试钩子：非 0 时固定墙钟（毫秒）；0 = 系统时钟。
    fixed_clock_ms: AtomicU64,
}

impl Default for ShortWindowMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl ShortWindowMeter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            in_flight: AtomicU32::new(0),
            successes: AtomicU64::new(0),
            fixed_clock_ms: AtomicU64::new(0),
        }
    }

    /// 当前墙钟（毫秒）。`fixed_clock_ms` 非 0 时（测试钩子）返回固定值。
    fn now_ms(&self) -> u64 {
        let fixed = self.fixed_clock_ms.load(Ordering::Relaxed);
        if fixed != 0 {
            return fixed;
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 取 `now` 对应的 slot；slot 起始时间戳不匹配（时钟前进越过 5s 边界或环形回绕）时重置。
    fn slot_for_locked(inner: &mut Inner, now: u64) -> &mut Slot {
        let slot_start = (now / SLOT_MS) * SLOT_MS;
        let idx = ((now / SLOT_MS) % SLOT_COUNT as u64) as usize;
        let slot = &mut inner.slots[idx];
        if slot.start_ms != slot_start {
            *slot = Slot {
                start_ms: slot_start,
                ..Default::default()
            };
        }
        slot
    }

    /// 尝试即记账（保守口径）：窗口计 1 个 request 与 `tokens` 个 token。
    /// `tokens = 0` 表示只计请求（如失败尝试无用量上报）。
    pub fn record_attempt(&self, tokens: u64) {
        let now = self.now_ms();
        let mut inner = self.inner.lock();
        let slot = Self::slot_for_locked(&mut inner, now);
        slot.requests = slot.requests.saturating_add(1);
        slot.tokens = slot.tokens.saturating_add(tokens);
    }

    /// 只累加窗口 token、不再增请求数。
    ///
    /// 与 [`Self::record_attempt`] 拆分请求/令牌记账：请求在准入时即
    /// `record_attempt(0)` 计入（RPM 槽位立即可见，消除准入-结算间竞态），
    /// 令牌在结算时经此方法补录（上游 `usage` 可得时才调用）。
    /// `tokens = 0` 时不产生任何效果。
    pub fn add_tokens(&self, tokens: u64) {
        if tokens == 0 {
            return;
        }
        let now = self.now_ms();
        let mut inner = self.inner.lock();
        let slot = Self::slot_for_locked(&mut inner, now);
        slot.tokens = slot.tokens.saturating_add(tokens);
    }

    /// 记录缓存命中 token（计入窗口 cached 桶，供 `remaining(count_cached=false)` 剔除）。
    /// 扩展方法（非 M2/M3 必需契约）：仅在需要按 count_cached 扣减缓存时调用，
    /// 应与对应的 [`Self::record_attempt`] 尽量落在同一 5s slot 内。
    pub fn record_cached_tokens(&self, tokens: u64) {
        let now = self.now_ms();
        let mut inner = self.inner.lock();
        let slot = Self::slot_for_locked(&mut inner, now);
        slot.cached_tokens = slot.cached_tokens.saturating_add(tokens);
    }

    /// 仅用于成功计数（不重复计窗口 request）。
    pub fn record_success(&self) {
        self.successes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn in_flight_inc(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn in_flight_dec(&self) {
        // 防下溢：已为 0 时保持不变（调用方应保证 inc/dec 成对）。
        let _ = self.in_flight.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v == 0 {
                None
            } else {
                Some(v - 1)
            }
        });
    }

    /// 默认 60s 窗口内的请求数（含失败尝试）。
    pub fn requests_in_window(&self) -> u64 {
        self.query_window(DEFAULT_WINDOW_SECS).0
    }

    /// 默认 60s 窗口内记录的 token 总量（按 `record_attempt` 传入口径，含缓存）。
    pub fn tokens_in_window(&self) -> u64 {
        self.query_window(DEFAULT_WINDOW_SECS).1
    }

    /// 进行中并发请求数。
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// 成功请求生命周期计数（不重复计入窗口 request）。
    pub fn successes(&self) -> u64 {
        self.successes.load(Ordering::Relaxed)
    }

    /// 当前窗口内（60s 环形窗）非空 slot 的**最早过期毫秒时间戳**：
    /// 该时刻之后，窗口内至少一份用量将滑出窗口——即预算回填的最早时刻。
    ///
    /// 只扫描仍在窗口内的非空 slot（已过期但未被回绕覆盖的 slot 不计），
    /// 已持锁、O(12)。无窗口用量（含全部过期）时返回 `None`。
    /// 供 [`super::pool::KeyPool::window_refill_in_with_limits`] 求最早回填时刻。
    pub fn earliest_expiry(&self) -> Option<u64> {
        let now = self.now_ms();
        let window_ms = DEFAULT_WINDOW_SECS.saturating_mul(1000);
        let cutoff = now.saturating_sub(window_ms);
        let inner = self.inner.lock();
        inner
            .slots
            .iter()
            .filter(|s| s.start_ms != 0 && s.start_ms >= cutoff && s.start_ms <= now)
            .map(|s| s.start_ms + window_ms)
            .min()
    }

    /// 查询 `window_secs` 秒窗口内的 (requests, tokens, cached_tokens)。
    /// 窗口小于 5s（slot 粒度）时按 5s 计（保守：整 slot 计入）。
    fn query_window(&self, window_secs: u64) -> (u64, u64, u64) {
        let now = self.now_ms();
        let window_ms = window_secs.saturating_mul(1000).clamp(SLOT_MS, u64::MAX);
        let cutoff = now.saturating_sub(window_ms);
        let inner = self.inner.lock();
        let mut requests = 0u64;
        let mut tokens = 0u64;
        let mut cached = 0u64;
        for slot in &inner.slots {
            if slot.start_ms != 0 && slot.start_ms >= cutoff && slot.start_ms <= now {
                requests = requests.saturating_add(slot.requests);
                tokens = tokens.saturating_add(slot.tokens);
                cached = cached.saturating_add(slot.cached_tokens);
            }
        }
        (requests, tokens, cached)
    }

    /// 计算窗口预算剩余：(剩余请求数, 剩余 token 数)；`None` 限额 = 不限。
    ///
    /// - `rpm` / `tpm`：每 `window_secs` 秒的请求 / token 限额。
    /// - `count_cached`：`true`（默认，按上游口径）TPM 按窗口全部记录 token 计；
    ///   `false` 时剔除 [`Self::record_cached_tokens`] 记录的缓存命中部分。
    pub fn remaining(
        &self,
        rpm: Option<u32>,
        tpm: Option<u64>,
        window_secs: u64,
        count_cached: bool,
    ) -> (Option<u64>, Option<u64>) {
        let (requests, tokens, cached) = self.query_window(window_secs);
        let charged = if count_cached {
            tokens
        } else {
            tokens.saturating_sub(cached)
        };
        let remaining_requests = rpm.map(|r| (r as u64).saturating_sub(requests));
        let remaining_tokens = tpm.map(|t| t.saturating_sub(charged));
        (remaining_requests, remaining_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 固定墙钟的计量器（T0 = 1_000_000 ms，恰好 5s 对齐，便于 slot 计算）。
    fn clocked() -> ShortWindowMeter {
        let meter = ShortWindowMeter::new();
        meter.fixed_clock_ms.store(1_000_000, Ordering::Relaxed);
        meter
    }

    fn advance(meter: &ShortWindowMeter, ms: u64) {
        meter.fixed_clock_ms.fetch_add(ms, Ordering::Relaxed);
    }

    #[test]
    fn test_window_rolling_expires_old_requests() {
        let m = clocked();
        // t0: 两次尝试（300 + 200 token）
        m.record_attempt(300);
        m.record_attempt(200);
        assert_eq!(m.requests_in_window(), 2);
        assert_eq!(m.tokens_in_window(), 500);

        // t0 + 59s：仍在 60s 窗口内 → 3 次请求
        advance(&m, 59_000);
        m.record_attempt(50);
        assert_eq!(m.requests_in_window(), 3);
        assert_eq!(m.tokens_in_window(), 550);

        // t0 + 61s：最早的两次尝试（t0、t0+5s..t0+55s slot）全部过期
        advance(&m, 2_000);
        assert_eq!(m.requests_in_window(), 1);
        assert_eq!(m.tokens_in_window(), 50);

        // 继续前进 60s：全部过期
        advance(&m, 60_000);
        assert_eq!(m.requests_in_window(), 0);
        assert_eq!(m.tokens_in_window(), 0);
    }

    #[test]
    fn test_window_rolling_ring_wraparound() {
        let m = clocked();
        m.record_attempt(100);
        // 越过整个环（12 × 5s = 60s）再写：旧 slot 被回绕重置
        advance(&m, 60_000);
        m.record_attempt(7);
        assert_eq!(m.requests_in_window(), 1);
        assert_eq!(m.tokens_in_window(), 7);
    }

    #[test]
    fn test_concurrency_counting() {
        let m = ShortWindowMeter::new();
        for _ in 0..5 {
            m.in_flight_inc();
        }
        assert_eq!(m.in_flight(), 5);
        m.in_flight_dec();
        m.in_flight_dec();
        m.in_flight_dec();
        assert_eq!(m.in_flight(), 2);
        // 防下溢：多扣不会变成负数
        for _ in 0..10 {
            m.in_flight_dec();
        }
        assert_eq!(m.in_flight(), 0);
    }

    #[test]
    fn test_remaining_no_limits_and_full_limits() {
        let m = clocked();
        // 无限额 → (None, None)
        assert_eq!(m.remaining(None, None, 60, true), (None, None));
        // 零用量 → 限额全额剩余
        assert_eq!(m.remaining(Some(10), Some(1000), 60, true), (Some(10), Some(1000)));
        // rpm 无限额、tpm 有限额
        assert_eq!(m.remaining(None, Some(500), 60, true), (None, Some(500)));
    }

    #[test]
    fn test_remaining_subtracts_window_usage() {
        let m = clocked();
        m.record_attempt(300); // 请求 +300 token
        m.record_attempt(0); // 只计请求
        let (req, tok) = m.remaining(Some(10), Some(1000), 60, true);
        assert_eq!(req, Some(8));
        assert_eq!(tok, Some(700));

        // 超出限额 → 饱和到 0，不为负
        let (req, tok) = m.remaining(Some(1), Some(200), 60, true);
        assert_eq!(req, Some(0));
        assert_eq!(tok, Some(0));
    }

    #[test]
    fn test_remaining_count_cached_toggle() {
        let m = clocked();
        // 总量 500（含 200 缓存命中）
        m.record_attempt(500);
        m.record_cached_tokens(200);

        // count_cached = true：按上游口径全计 → 剩余 500
        assert_eq!(m.remaining(Some(100), Some(1000), 60, true).1, Some(500));
        // count_cached = false：剔除缓存 → 剩余 700
        assert_eq!(m.remaining(Some(100), Some(1000), 60, false).1, Some(700));
        // 未记录缓存时开关无差异
        let n = clocked();
        n.record_attempt(500);
        assert_eq!(n.remaining(None, Some(1000), 60, true).1, Some(500));
        assert_eq!(n.remaining(None, Some(1000), 60, false).1, Some(500));
    }

    #[test]
    fn test_remaining_honors_window_secs() {
        let m = clocked();
        m.record_attempt(100); // t0
        advance(&m, 30_000);
        m.record_attempt(50); // t0 + 30s

        // 60s 窗口：两次都计入 → 剩余请求 0
        assert_eq!(m.remaining(Some(2), None, 60, true).0, Some(0));
        // 10s 窗口：只计最近一次 → 剩余请求 1
        assert_eq!(m.remaining(Some(2), None, 10, true).0, Some(1));
    }

    #[test]
    fn test_add_tokens_only_adds_tokens() {
        let m = clocked();
        // 请求准入时只计请求，令牌结算时补录：二者合计 = 1 请求 + tokens。
        m.record_attempt(0);
        m.add_tokens(250);
        assert_eq!(m.requests_in_window(), 1);
        assert_eq!(m.tokens_in_window(), 250);
        // add_tokens(0) 无效果；与完整 record_attempt 累加。
        m.add_tokens(0);
        m.record_attempt(100);
        assert_eq!(m.requests_in_window(), 2);
        assert_eq!(m.tokens_in_window(), 350);
    }

    #[test]
    fn test_earliest_expiry_scans_live_slots() {
        let m = clocked(); // T0 = 1_000_000 ms（5s 对齐）
        // 无用量：None。
        assert_eq!(m.earliest_expiry(), None);

        m.record_attempt(100); // slot @1_000_000 → 60s 后过期
        assert_eq!(m.earliest_expiry(), Some(1_060_000));

        // 更新的 slot 过期更晚：最早的（最旧）slot 胜出。
        advance(&m, 10_000);
        m.record_attempt(50); // slot @1_010_000 → 过期 @1_070_000
        assert_eq!(m.earliest_expiry(), Some(1_060_000));

        // 最旧 slot 滑出 60s 窗：剩余最早过期点变为 @1_070_000。
        advance(&m, 55_000);
        assert_eq!(m.earliest_expiry(), Some(1_070_000));

        // 全部过期：None。
        advance(&m, 10_000);
        assert_eq!(m.earliest_expiry(), None);
    }

    #[test]
    fn test_record_success_counts_separately() {
        let m = ShortWindowMeter::new();
        assert_eq!(m.successes(), 0);
        m.record_success();
        m.record_success();
        m.record_success();
        assert_eq!(m.successes(), 3);
        // 成功计数不进入窗口请求数
        assert_eq!(m.requests_in_window(), 0);
        m.record_attempt(10);
        m.record_success();
        assert_eq!(m.successes(), 4);
        assert_eq!(m.requests_in_window(), 1);
    }

    #[test]
    fn test_arc_smoke() {
        let _ = Arc::new(ShortWindowMeter::new());
    }
}
