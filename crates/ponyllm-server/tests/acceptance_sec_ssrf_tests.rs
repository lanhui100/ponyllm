//! Phase-2 安全修复验收（隔离测试，Test Agent 于业务实施前编写）。
//!
//! 契约矩阵：`.dev-team/report/FIX-CONTRACT.md` F6（VULN-07 SSRF 数据面）。
//!
//! 红相要求：本文件在未修复 HEAD 上必须失败，失败原因=对应缺失实现：
//! - `egress::blocked_v4` 未含 100.64.0.0/10（CGNAT）与 198.18.0.0/15（benchmark）
//!   → `check_probe_url_fast` 对这两段返回 Ok，断言 is_err 失败；
//! - 数据面 HTTP client（`create_upstream_http_client_*`）未设 `redirect(Policy::none)`
//!   → 对本地 302 上游自动跟随，断言 302 原样返回失败。
//!
//! 说明：`state.rs` 拨号复检（TTL 缓存）以 `check_probe_url` 的既有调用点为基座，
//! 此处以边界阻断断言覆盖其可观测面；复检自身的 TTL 为内部优化，不在此断言。

use ponyllm_server::egress::check_probe_url_fast;

// ---------------------------------------------------------------------------
// F6a  egress 阻断表补段：100.64/10（CGNAT）与 198.18/15（benchmark）
// ---------------------------------------------------------------------------

#[test]
fn f6_egress_blocks_cgnat_100_64_10() {
    for bad in [
        "http://100.64.0.1/",
        "http://100.64.255.255/",
        "http://100.64.0.1:8080/v1",
        // IPv4-mapped IPv6 也必须按内嵌 IPv4 判定（H2 既有先例）
        "http://[::ffff:100.64.0.1]/",
    ] {
        assert!(
            check_probe_url_fast(bad).is_err(),
            "F6: 100.64.0.0/10 (CGNAT) 必须被 egress 阻断：{}（HEAD 上 blocked_v4 未含该段 → Ok，红相成立）",
            bad
        );
    }
}

#[test]
fn f6_egress_blocks_benchmarking_198_18_15() {
    for bad in [
        "http://198.18.0.1/",
        "http://198.19.255.255/", // 198.18.0.0/15 右边界
        "http://198.18.7.7:443/",
        "http://[::ffff:198.18.0.1]/",
    ] {
        assert!(
            check_probe_url_fast(bad).is_err(),
            "F6: 198.18.0.0/15 (benchmark) 必须被 egress 阻断：{}（HEAD 上 blocked_v4 未含该段 → Ok，红相成立）",
            bad
        );
    }
}

/// 伴生回归：边界之外保持放行（100.63.255.255、198.17.255.255、198.20.0.1
/// 均不在任何阻断段内）。HEAD 上 Ok（绿）；修复后须保持 Ok。
#[test]
fn f6_egress_boundaries_outside_remain_open() {
    for good in [
        "http://100.63.255.255/",
        "http://100.65.0.1/",
        "http://198.17.255.255/",
        "http://198.20.0.1/",
        "http://203.0.113.10/",
    ] {
        assert!(
            check_probe_url_fast(good).is_ok(),
            "F6: 边界外地址必须放行：{}",
            good
        );
    }
}

// ---------------------------------------------------------------------------
// F6b  数据面 client 禁重定向（redirect(Policy::none)）
// ---------------------------------------------------------------------------

/// 数据面 client 对 302 必须原样返回（绝不跟随到 Location）。
/// HEAD 上 client 默认跟随重定向 → 最终 200（红相）。
#[tokio::test]
async fn f6_data_plane_client_never_follows_redirects() {
    use axum::routing::get;

    // 目标服务器：被 302 指过去后返回 200 "TARGET"
    let target = axum::Router::new().route(
        "/",
        get(|| async { (axum::http::StatusCode::OK, "TARGET") }),
    );
    let t_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let t_addr = t_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(t_listener, target).await.unwrap();
    });

    let target_url = format!("http://{}/", t_addr);
    // 重定向服务器：302 → target_url
    let redirector = axum::Router::new().route(
        "/",
        get(move || async move {
            (
                axum::http::StatusCode::FOUND,
                [(axum::http::header::LOCATION, target_url)],
                "redirect",
            )
        }),
    );
    let r_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let r_addr = r_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(r_listener, redirector).await.unwrap();
    });

    // 数据面 client（与 `AppState::http_client_for_provider` 同源构建）
    let client = ponyllm_core::executor::create_upstream_http_client_with_options(None, false);
    let resp = client
        .get(format!("http://{}/", r_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::FOUND,
        "F6: 数据面 client 必须 redirect(Policy::none)：302 应原样返回，实际 {}（HEAD 上自动跟随 → 200，红相成立）",
        resp.status()
    );
}
