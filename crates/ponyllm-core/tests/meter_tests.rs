//! ShortWindowMeter 公共 API 集成测试（真实墙钟）。
//!
//! 确定性窗口滚动（时钟注入）用例在内联 `#[cfg(test)]` 单测中（meter.rs），
//! 本文件覆盖真实时钟路径：并发计数、remaining 计算、count_cached 开关、
//! tokens=0 只计请求、成功计数，以及一条 5s 级真实时间滚动用例。

use ponyllm_core::pool::meter::ShortWindowMeter;

#[test]
fn test_in_flight_concurrency_public_api() {
    let m = ShortWindowMeter::new();
    assert_eq!(m.in_flight(), 0);
    m.in_flight_inc();
    m.in_flight_inc();
    m.in_flight_inc();
    assert_eq!(m.in_flight(), 3);
    m.in_flight_dec();
    assert_eq!(m.in_flight(), 2);
    // 防下溢：已为 0 时保持不变
    m.in_flight_dec();
    m.in_flight_dec();
    m.in_flight_dec();
    assert_eq!(m.in_flight(), 0);
}

#[test]
fn test_remaining_public_api() {
    let m = ShortWindowMeter::new();
    // 无限额
    assert_eq!(m.remaining(None, None, 60, true), (None, None));
    // 零用量
    assert_eq!(m.remaining(Some(10), Some(1000), 60, true), (Some(10), Some(1000)));

    m.record_attempt(250);
    m.record_attempt(0); // 只计请求
    let (req, tok) = m.remaining(Some(10), Some(1000), 60, true);
    assert_eq!(req, Some(8));
    assert_eq!(tok, Some(750));

    // 超出限额 → 饱和到 0
    assert_eq!(m.remaining(Some(1), Some(100), 60, true), (Some(0), Some(0)));
}

#[test]
fn test_remaining_count_cached_toggle_public_api() {
    let m = ShortWindowMeter::new();
    m.record_attempt(500); // 总量（含 200 缓存）
    m.record_cached_tokens(200);

    // true：按上游口径全计；false：剔除缓存命中
    assert_eq!(m.remaining(None, Some(1000), 60, true).1, Some(500));
    assert_eq!(m.remaining(None, Some(1000), 60, false).1, Some(700));
}

#[test]
fn test_record_attempt_tokens_zero_counts_request_only() {
    let m = ShortWindowMeter::new();
    m.record_attempt(0);
    m.record_attempt(0);
    assert_eq!(m.requests_in_window(), 2);
    assert_eq!(m.tokens_in_window(), 0);
}

#[test]
fn test_successes_public_api() {
    let m = ShortWindowMeter::new();
    m.record_success();
    m.record_success();
    assert_eq!(m.successes(), 2);
    // 成功计数不重复计入窗口请求
    assert_eq!(m.requests_in_window(), 0);
}

#[test]
fn test_add_tokens_public_api() {
    let m = ShortWindowMeter::new();
    // 准入：请求即计（RPM 槽立即可见）；结算：add_tokens 只补 token 不增请求。
    m.record_attempt(0);
    m.add_tokens(250);
    assert_eq!(m.requests_in_window(), 1);
    assert_eq!(m.tokens_in_window(), 250);
    // add_tokens(0) 无效果
    m.add_tokens(0);
    assert_eq!(m.tokens_in_window(), 250);
    m.add_tokens(50);
    assert_eq!(m.tokens_in_window(), 300);
    assert_eq!(m.requests_in_window(), 1);
}

#[test]
fn test_earliest_expiry_public_api() {
    let m = ShortWindowMeter::new();
    // 无窗口用量：None
    assert_eq!(m.earliest_expiry(), None);
    m.record_attempt(100);
    let expiry = m.earliest_expiry().expect("recorded usage -> expiry");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    // 过期点 = slot 起始 + 60s：在 (now, now+60s] 内（5s slot 粒度保守下界）。
    assert!(expiry > now && expiry <= now + 60_000, "expiry={expiry}, now={now}");
}

/// 真实墙钟滚动：5s slot 粒度下，旧 slot 在 `remaining(window_secs=5)` 中过期。
/// 睡眠 ~5.2s 越过一个 slot 边界，验证整 slot 过期语义与 60s 默认窗口保留语义。
#[test]
fn test_window_rolling_real_clock() {
    let m = ShortWindowMeter::new();
    m.record_attempt(100);
    std::thread::sleep(std::time::Duration::from_millis(5_200));
    m.record_attempt(50);

    // 5s 窗口：仅最近一次（旧 slot 已过期）
    let (req, tok) = m.remaining(Some(10), Some(100_000), 5, true);
    assert_eq!(req, Some(9));
    assert_eq!(tok, Some(99_950));

    // 默认 60s 窗口：两次都保留
    assert_eq!(m.requests_in_window(), 2);
    assert_eq!(m.tokens_in_window(), 150);
}
