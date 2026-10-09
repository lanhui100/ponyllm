//! Phase-2b 安全修复验收（隔离测试，Test Agent 于修复实施前编写）。
//!
//! 契约矩阵：`.dev-team/report/FIX-CONTRACT.md` F3（VULN-12 XFF 收敛）
//! + Phase-2b 审查修复 **R1**（resolve_client_ip 契约修正）。
//!
//! ## R1 契约修正（Phase-2b，推翻原 f3_single_hop_no_trusted 冻结行为）
//!
//! 直连 TCP peer 不在 `trusted_proxies` 时必须**忽略全部转发头**：
//!
//! ```ignore
//! pub fn resolve_client_ip(
//!     xff: Option<&str>,
//!     x_real_ip: Option<&str>,
//!     remote: std::net::IpAddr,
//!     trusted: &[std::net::IpAddr],
//! ) -> std::net::IpAddr
//! // 1. if !trusted.contains(remote) { return remote; }   // 直连 peer 不可信 → 头一律忽略
//! // 2. 之后才解析 XFF（右向左、跳过 trusted 跳、格式校验弃非法段/剥端口）
//! //    全部段 trusted / XFF 缺失 → 回退 x_real_ip（合法才取）→ remote。
//! ```
//!
//! 攻击面：未接入可信代理时，攻击者伪造 `X-Forwarded-For` 即可模拟任意客户端
//! （绕过 admin 围栏 / 错配限流键 / 污染审计）。peer 校验把 XFF 的信任锚点
//! 收回到 TCP 对端。
//!
//! ## 红相说明
//! 当前 HEAD 的 `resolve_client_ip` 无 peer 校验（无条件解析 XFF）→
//! 本文件 peer-not-trusted 三用例返回 XFF/x_real_ip 而非 remote → 断言失败（红相）；
//! 修复后按上述契约返回 remote，用例转绿。

use ponyllm_server::auth::resolve_client_ip;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

fn v4(o: [u8; 4]) -> IpAddr {
    IpAddr::V4(Ipv4Addr::from(o))
}

fn v6(g: [u16; 8]) -> IpAddr {
    IpAddr::V6(Ipv6Addr::from(g))
}

/// 直连对端（测试中即本地回环）。
const DIRECT_PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
/// 可信代理段样例（192.0.2.1）。
const TRUSTED_PROXY: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));

// ---------------------------------------------------------------------------
// R1：peer 不在 trusted → 忽略全部转发头（红相用例）
// ---------------------------------------------------------------------------

#[test]
fn r1_peer_not_trusted_ignores_xff() {
    // 攻击者伪造 XFF；peer 127.0.0.1 非 trusted → 必须返回 peer（直视 XFF=203.0.113.7）
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7"), None, DIRECT_PEER, &[]),
        DIRECT_PEER,
        "R1: peer 非 trusted 时必须忽略 XFF（当前实现无 peer 校验 → 返回 XFF 值，红相成立）"
    );
}

#[test]
fn r1_peer_not_trusted_ignores_x_real_ip() {
    assert_eq!(
        resolve_client_ip(None, Some("203.0.113.8"), DIRECT_PEER, &[]),
        DIRECT_PEER,
        "R1: peer 非 trusted 时必须忽略 X-Real-IP（红相成立）"
    );
}

#[test]
fn r1_peer_not_trusted_ignores_headers_even_if_xff_hops_trusted() {
    // XFF 全部为 trusted 段、x_real_ip 亦提供 → 仍须返回 peer（guard 在解析任何头之前）
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(
            Some("192.0.2.1"),
            Some("203.0.113.8"),
            DIRECT_PEER,
            &trusted
        ),
        DIRECT_PEER,
        "R1: peer 非 trusted 时不得消费任何转发头（红相成立）"
    );
}

// ---------------------------------------------------------------------------
// F3 矩阵（peer trusted → 正常解析；伴生回归，HEAD 即绿）
// ---------------------------------------------------------------------------

#[test]
fn f3_peer_trusted_single_hop() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7"), None, TRUSTED_PROXY, &trusted),
        v4([203, 0, 113, 7])
    );
}

#[test]
fn f3_peer_trusted_multi_hop_skips_trusted_rightmost() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(
            Some("203.0.113.7, 192.0.2.1"),
            None,
            TRUSTED_PROXY,
            &trusted
        ),
        v4([203, 0, 113, 7])
    );
}

#[test]
fn f3_peer_trusted_invalid_segments_dropped() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(
            Some("not-an-ip, 203.0.113.7"),
            None,
            TRUSTED_PROXY,
            &trusted
        ),
        v4([203, 0, 113, 7])
    );
    assert_eq!(
        resolve_client_ip(Some("  ,  203.0.113.7 , "), None, TRUSTED_PROXY, &trusted),
        v4([203, 0, 113, 7])
    );
}

#[test]
fn f3_peer_trusted_port_injection_stripped() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7:9999"), None, TRUSTED_PROXY, &trusted),
        v4([203, 0, 113, 7])
    );
    assert_eq!(
        resolve_client_ip(Some("[2001:db8::1]:443"), None, TRUSTED_PROXY, &trusted),
        v6([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1])
    );
}

#[test]
fn f3_peer_trusted_ipv6_literal() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(Some("2001:db8::1"), None, TRUSTED_PROXY, &trusted),
        v6([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1])
    );
    assert_eq!(
        resolve_client_ip(Some("::ffff:10.0.0.1"), None, TRUSTED_PROXY, &trusted),
        v4([10, 0, 0, 1])
    );
}

#[test]
fn f3_peer_trusted_all_hops_trusted_falls_to_x_real_ip() {
    let trusted = [TRUSTED_PROXY];
    assert_eq!(
        resolve_client_ip(
            Some("192.0.2.1"),
            Some("203.0.113.8"),
            TRUSTED_PROXY,
            &trusted
        ),
        v4([203, 0, 113, 8])
    );
}

#[test]
fn f3_peer_trusted_fallback_chain() {
    let trusted = [TRUSTED_PROXY];
    // 无 XFF → x_real_ip
    assert_eq!(
        resolve_client_ip(None, Some("203.0.113.8"), TRUSTED_PROXY, &trusted),
        v4([203, 0, 113, 8])
    );
    // x_real_ip 非法 → remote
    assert_eq!(
        resolve_client_ip(None, Some("garbage"), TRUSTED_PROXY, &trusted),
        TRUSTED_PROXY
    );
    // 双缺失 → remote
    assert_eq!(
        resolve_client_ip(None, None, TRUSTED_PROXY, &trusted),
        TRUSTED_PROXY
    );
}
