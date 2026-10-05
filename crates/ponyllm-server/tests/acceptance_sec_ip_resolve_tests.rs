//! Phase-2 安全修复验收（隔离测试，Test Agent 于业务实施前编写）。
//!
//! 契约矩阵：`.dev-team/report/FIX-CONTRACT.md` F3（VULN-12 XFF 收敛）。
//!
//! 红相要求：`ponyllm_server::auth::resolve_client_ip` 在未修复 HEAD 上不存在
//! （当前 auth_middleware 直接取 XFF 首段），本文件编译失败 = F3 未实现的直接证据。
//!
//! ## F3 接口契约（测试定义，Executor 按此实现；限流键/围栏/审计三处统一调用）
//!
//! ```ignore
//! pub fn resolve_client_ip(
//!     xff: Option<&str>,          // X-Forwarded-For 原始值（可为多跳 "a, b, c"）
//!     x_real_ip: Option<&str>,    // X-Real-IP 原始值
//!     remote: std::net::IpAddr,   // 直连对端（TCP peer）
//!     trusted: &[std::net::IpAddr], // 受信代理 IP（取自 trusted_proxies 配置）
//! ) -> std::net::IpAddr
//! ```
//!
//! 语义（右向左、跳过 trusted 跳、格式校验弃非法段）：
//! 1. XFF 存在且非空：按 ',' 切段、trim；丢弃空段与非法段（非 IP、含端口则剥端口）；
//!    自右向左跳过 `trusted` 中的地址；首个非 trusted 段即为客户端 IP；
//!    全部段均 trusted → 落到 2。
//! 2. XFF 缺失/为空/全部 trusted：取 x_real_ip（须为合法 IP，剥端口）→ 否则取 remote。
//!
//! 矩阵覆盖：多跳 / trusted 跳段 / 非法段丢弃 / IPv6 / 端口注入 / 回退链。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use ponyllm_server::auth::resolve_client_ip;

fn v4(o: [u8; 4]) -> IpAddr {
    IpAddr::V4(Ipv4Addr::from(o))
}

fn v6(g: [u16; 8]) -> IpAddr {
    IpAddr::V6(Ipv6Addr::from(g))
}

const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

#[test]
fn f3_single_hop_no_trusted() {
    // 单跳、无 trusted：取唯一/最右 XFF 段
    assert_eq!(resolve_client_ip(Some("203.0.113.7"), None, REMOTE, &[]), v4([203, 0, 113, 7]));
}

#[test]
fn f3_multi_hop_skips_trusted_rightmost() {
    // 双跳，最右跳为受信代理 → 取左侧真实客户端
    let trusted = [v4([192, 0, 2, 1])];
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7, 192.0.2.1"), None, REMOTE, &trusted),
        v4([203, 0, 113, 7])
    );
}

#[test]
fn f3_multi_hop_no_trusted_takes_rightmost() {
    // 无 trusted 时取最右段（紧邻代理眼中的客户端）
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7, 198.51.100.9"), None, REMOTE, &[]),
        v4([198, 51, 100, 9])
    );
}

#[test]
fn f3_invalid_segments_dropped() {
    // 非法段（非 IP）必须丢弃，不破坏后续解析
    assert_eq!(
        resolve_client_ip(Some("not-an-ip, 203.0.113.7"), None, REMOTE, &[]),
        v4([203, 0, 113, 7])
    );
    assert_eq!(
        resolve_client_ip(Some("  ,  203.0.113.7 , "), None, REMOTE, &[]),
        v4([203, 0, 113, 7])
    );
}

#[test]
fn f3_port_injection_stripped() {
    // 端口注入（攻击者写 "1.2.3.4:6666"）必须剥离为纯 IP
    assert_eq!(
        resolve_client_ip(Some("203.0.113.7:9999"), None, REMOTE, &[]),
        v4([203, 0, 113, 7])
    );
    assert_eq!(
        resolve_client_ip(Some("[2001:db8::1]:443"), None, REMOTE, &[]),
        v6([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1])
    );
}

#[test]
fn f3_ipv6_literal() {
    assert_eq!(
        resolve_client_ip(Some("2001:db8::1"), None, REMOTE, &[]),
        v6([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1])
    );
    // IPv4-mapped IPv6 保留内嵌语义
    assert_eq!(
        resolve_client_ip(Some("::ffff:10.0.0.1"), None, REMOTE, &[]),
        v4([10, 0, 0, 1])
    );
}

#[test]
fn f3_all_hops_trusted_falls_through() {
    // 全部段 trusted → 落到 x_real_ip
    let trusted = [v4([192, 0, 2, 1])];
    assert_eq!(
        resolve_client_ip(Some("192.0.2.1"), Some("203.0.113.8"), REMOTE, &trusted),
        v4([203, 0, 113, 8])
    );
}

#[test]
fn f3_no_xff_falls_back_to_x_real_ip_then_remote() {
    assert_eq!(
        resolve_client_ip(None, Some("203.0.113.8"), REMOTE, &[]),
        v4([203, 0, 113, 8])
    );
    // x_real_ip 非法 → remote
    assert_eq!(
        resolve_client_ip(None, Some("garbage"), REMOTE, &[]),
        REMOTE
    );
    // 双缺失 → remote
    assert_eq!(resolve_client_ip(None, None, REMOTE, &[]), REMOTE);
}
