//! 验收测试：egress 守护对显式代理目标跳过本地 DNS + 三态缓存 `(mode, host)` 分键。
//!
//! 契约（冻结）：`.agents/notes/proposed/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md`
//! 与团队任务板 task-1。本文件只依赖契约中的**新 API**（HEAD 上不存在，编译失败 = 红相）：
//!   - `ponyllm_server::egress::{DataPlaneRefusal, DnsLookupError}`
//!   - `check_data_plane_url(raw) -> Result<(), DataPlaneRefusal>`（direct 全检查内核）
//!   - `check_data_plane_url_proxied(raw) -> Result<(), DataPlaneRefusal>`（纯 fast path，零 DNS）
//!   - `check_data_plane_url_with_resolver(raw, F) -> Result<(), DataPlaneRefusal>`（注入 resolver）
//!   - `AppState::data_plane_egress_guard_for_target(provider, model, url) -> Result<(), String>`
//!   - `AppState::egress_guard_cache: Mutex<HashMap<(bool proxied, String lowercase host),
//!     EgressGuardVerdict>>`；`EgressGuardVerdict` 增 `pub transient`（保留 `ok`/`expires_at`）
//!
//! ## 红相说明
//! HEAD 上上述 API/字段/键型均不存在 ⇒ 本文件**编译失败** = 修复未实施的直接证据；
//! 编译通过即红相不成立，需退回核查（严禁声称测试"通过"）。
//!
//! ## DNS 依赖
//! `.invalid`（RFC 6761 保留 TLD）保证 NXDOMAIN 立即返回：用作"目标不可解析"的零依赖
//! 证据——若实现有缺陷走了解析路径，该调用必然失败（而非 Ok）。

use std::sync::Arc;
use std::time::Duration;

use ponyllm_server::egress::{
    check_data_plane_url, check_data_plane_url_proxied, check_data_plane_url_with_resolver,
    DataPlaneRefusal, DnsLookupError,
};
use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};

/// 构造带 provider `px` 的 AppState；`proxy: None` = 无代理（直连）。
fn state_with_proxy(proxy: Option<&str>) -> Arc<AppState> {
    let mut cfg = GatewayConfig::default();
    cfg.providers.insert(
        "px".to_string(),
        ProviderConfig {
            base_url: "http://203.0.113.88/v1".to_string(),
            default_model: "px-model".to_string(),
            proxy: proxy.map(|s| s.to_string()),
            ..Default::default()
        },
    );
    Arc::new(AppState::new(cfg))
}

fn cache_lookup(
    state: &AppState,
    key: (bool, &str),
) -> Option<(bool, bool, std::time::Instant)> {
    let cache = state.egress_guard_cache.lock().unwrap_or_else(|p| p.into_inner());
    cache
        .get(&(key.0, key.1.to_string()))
        .map(|v| (v.ok, v.transient, v.expires_at))
}

/// 用例 1：代理生效 + 不可解析目标域 ⇒ 放行且零 DNS（缓存 `(true, nope.invalid)` ok=true）。
/// `.invalid` 一旦被解析必然失败 —— 返回 Ok 即证明未走解析路径。
#[tokio::test]
async fn c1_proxied_fast_path_skips_dns_and_caches_ok() {
    let state = state_with_proxy(Some("http://127.0.0.1:8899"));
    let url = "https://nope.invalid/v1";

    let res = state
        .data_plane_egress_guard_for_target("px", "", url)
        .await;
    assert!(
        res.is_ok(),
        "代理生效 + 不可解析目标必须放行（零 DNS 快路径），实际 Err: {:?}",
        res.err()
    );

    let (ok, _transient, _expires) = cache_lookup(&state, (true, "nope.invalid"))
        .expect("proxied Ok 判定必须写入缓存 (true, nope.invalid)");
    assert!(ok, "缓存条目必须 ok=true");
}

/// 用例 2：直连 + 不可解析目标域 ⇒ transient 拒绝，缓存 `(false, nope.invalid)` TTL ≤ 2s
/// （契约：瞬时失败 1s TTL；HEAD 为 10s 负缓存 ⇒ 红相可观察）。
#[tokio::test]
async fn c2_direct_unresolvable_is_transient_refusal() {
    let state = state_with_proxy(None); // 无代理 ⇒ 直连
    let url = "https://nope.invalid/v1";

    let res = state.data_plane_egress_guard(url).await;
    let err = res.expect_err("直连 + 不可解析目标必须拒绝");
    assert!(
        err.contains("nope.invalid"),
        "refusal reason 应含目标 host（transient 语义），实际: {err}"
    );

    let (ok, transient, expires_at) = cache_lookup(&state, (false, "nope.invalid"))
        .expect("直连瞬时失败必须写入缓存 (false, nope.invalid)");
    assert!(!ok, "负向判定 ok=false");
    assert!(transient, "DNS 失败是瞬时失败（transient=true），不得落入 10s 确定性负缓存");
    let ttl = expires_at.saturating_duration_since(std::time::Instant::now());
    assert!(
        ttl <= Duration::from_secs(2),
        "瞬时失败 TTL 必须 ≤ 2s（契约 1s），实际 {ttl:?}"
    );
}

/// 用例 3：socks5 代理 ⇒ 不得走快路径（workspace reqwest 无 socks feature，
/// `Proxy::all` 必失败 ⇒ 完整检查、DNS 照跑）⇒ 不可解析目标必须 Err，不得 Ok。
#[tokio::test]
async fn c3_socks5_proxy_forces_full_check() {
    let state = state_with_proxy(Some("socks5://127.0.0.1:1080"));

    let res = state
        .data_plane_egress_guard_for_target("px", "", "https://nope.invalid/v1")
        .await;
    assert!(
        res.is_err(),
        "socks5 不得走无 DNS 快路径（否则客户端静默回退直连 = SSRF 脱钩），必须拒绝"
    );
}

/// 用例 4：代理为不可解析 URL（"not a url"）⇒ 完整检查 ⇒ 不可解析目标必须 Err。
#[tokio::test]
async fn c4_unparseable_proxy_forces_full_check() {
    let state = state_with_proxy(Some("not a url"));

    let res = state
        .data_plane_egress_guard_for_target("px", "", "https://nope.invalid/v1")
        .await;
    assert!(
        res.is_err(),
        "代理 URL 解析失败不得走快路径，完整检查必须拒绝"
    );
}

/// 用例 5：代理生效 + 确定性拒绝（不依赖 DNS）：`.svc` 名 / 私网字面 IP 均拒绝，
/// 且写入 `(true, host)` 缓存，TTL ≤ 30s 上界保留。
#[tokio::test]
async fn c5_deterministic_refusals_cached_under_proxied_mode() {
    let state = state_with_proxy(Some("http://127.0.0.1:8899"));

    for (url, host) in [
        ("http://api.svc:8080/", "api.svc"),
        ("http://192.168.1.5/", "192.168.1.5"),
    ] {
        let res = state.data_plane_egress_guard_for_target("px", "", url).await;
        assert!(
            res.is_err(),
            "确定性拒绝目标 {url} 必须被拒（不依赖 DNS）"
        );

        let (ok, transient, expires_at) = cache_lookup(&state, (true, host))
            .unwrap_or_else(|| panic!("确定性拒绝必须写入缓存 (true, {host})"));
        assert!(!ok, "{host}: 负向判定 ok=false");
        assert!(!transient, "{host}: 确定性拒绝 transient=false");
        let ttl = expires_at.saturating_duration_since(std::time::Instant::now());
        assert!(
            ttl <= Duration::from_secs(30),
            "{host}: 确定性拒绝 TTL 必须 ≤ 30s，实际 {ttl:?}"
        );
    }
}

/// 用例 6：mode 隔离 —— 同 host 先 proxied Ok（入 `(true, host)`），再 direct 必须 Err
/// （不得误食 proxied 缓存），且 `(false, host)` 无 ok 缓存。
#[tokio::test]
async fn c6_mode_isolation_proxied_ok_never_poisons_direct() {
    let state = state_with_proxy(Some("http://127.0.0.1:8899"));
    let url = "https://nope.invalid/v1";

    // 1) proxied 先行 ⇒ Ok 入 (true, nope.invalid)
    assert!(
        state
            .data_plane_egress_guard_for_target("px", "", url)
            .await
            .is_ok(),
        "proxied 前置判定应 Ok"
    );

    // 2) 同 host direct 后行 ⇒ 必须重新解析并拒绝，不得食 proxied 的 Ok 缓存
    let direct = state.data_plane_egress_guard(url).await;
    assert!(
        direct.is_err(),
        "direct 不得复用 (true, host) 的 Ok 判定（mode 隔离）"
    );

    let (direct_ok, _, _) = cache_lookup(&state, (false, "nope.invalid"))
        .expect("direct 拒绝必须写入 (false, nope.invalid)");
    assert!(!direct_ok, "(false, host) 不得为 ok=true");

    let (proxied_ok, _, _) = cache_lookup(&state, (true, "nope.invalid"))
        .expect("proxied Ok 判定仍应在 (true, host)");
    assert!(proxied_ok, "(true, host) 保持 ok=true");
}

/// 用例 7a：分类全表（注入 resolver，无真实 DNS）——
///   10.0.0.5 ⇒ 确定性拒绝 transient=false；203.0.113.88 ⇒ Ok；
///   Err(Timeout) / Err(Failure(_)) / Ok([]) ⇒ transient=true。
#[tokio::test]
async fn c7_classification_with_injected_resolver() {
    use std::net::IpAddr;

    // 解析出私网字面 ⇒ 确定性拒绝（transient=false）
    let res = check_data_plane_url_with_resolver(
        "http://example.com/",
        |_h: &str| -> Result<Vec<IpAddr>, DnsLookupError> { Ok(vec![IpAddr::from([10, 0, 0, 5])]) },
    )
    .await;
    let err = res.expect_err("解析出私网 IP 必须拒绝");
    assert!(!err.transient, "私网解析是确定性拒绝（transient=false）");
    assert!(err.reason.contains("10.0.0.5"), "reason 应点名解析出的 IP，实际: {}", err.reason);

    // 解析出公共 TEST-NET-3 字面 ⇒ Ok
    assert!(
        check_data_plane_url_with_resolver(
            "http://example.com/",
            |_h: &str| -> Result<Vec<IpAddr>, DnsLookupError> {
                Ok(vec![IpAddr::from([203, 0, 113, 88])])
            },
        )
        .await
        .is_ok(),
        "解析出公共 IP 应放行"
    );

    // DNS 超时 ⇒ transient
    let res = check_data_plane_url_with_resolver(
        "http://example.com/",
        |_h: &str| -> Result<Vec<IpAddr>, DnsLookupError> { Err(DnsLookupError::Timeout) },
    )
    .await;
    assert!(res.expect_err("DNS 超时必须拒绝").transient, "Timeout ⇒ transient=true");

    // 解析器失败 ⇒ transient
    let res = check_data_plane_url_with_resolver(
        "http://example.com/",
        |_h: &str| -> Result<Vec<IpAddr>, DnsLookupError> {
            Err(DnsLookupError::Failure("boom".to_string()))
        },
    )
    .await;
    assert!(
        res.expect_err("解析器失败必须拒绝").transient,
        "Failure(_) ⇒ transient=true"
    );

    // 空结果集 ⇒ transient
    let res = check_data_plane_url_with_resolver(
        "http://example.com/",
        |_h: &str| -> Result<Vec<IpAddr>, DnsLookupError> { Ok(vec![]) },
    )
    .await;
    assert!(res.expect_err("空解析结果必须拒绝").transient, "Ok([]) ⇒ transient=true");

    // 契约：DataPlaneRefusal → String（供 1 参 guard / 路由层用）
    let refusal = DataPlaneRefusal {
        reason: "refused".to_string(),
        transient: true,
    };
    assert_eq!(String::from(refusal), "refused");
}

/// 用例 7b：`check_data_plane_url_proxied` = 纯 fast path（零 DNS，字面/IP 名直接判定）；
/// `check_data_plane_url` = direct 全检查内核，返回类型已切换为 `DataPlaneRefusal`。
#[tokio::test]
async fn c7b_proxied_fast_only_and_direct_core_signatures() {
    // fast path：名黑名单 / 私网字面 —— 无 DNS
    let err = check_data_plane_url_proxied("http://api.svc:8080/")
        .await
        .expect_err(".svc 名必须被 fast path 拒绝");
    assert!(!err.transient, "名黑名单拒绝是确定性拒绝");
    assert!(check_data_plane_url_proxied("http://192.168.1.5/").await.is_err());
    assert!(check_data_plane_url_proxied("http://203.0.113.88/").await.is_ok());

    // direct 内核：字面 IP 判定零 DNS
    let err = check_data_plane_url("http://10.0.0.5/")
        .await
        .expect_err("私网字面必须被 direct 内核拒绝");
    assert!(!err.transient);
    assert!(check_data_plane_url("http://203.0.113.88/").await.is_ok());
}
