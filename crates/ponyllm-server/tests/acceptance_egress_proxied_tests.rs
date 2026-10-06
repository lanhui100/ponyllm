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

use std::sync::{Arc, Mutex};
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

// ---------------------------------------------------------------------------
// 回归验收（task-1 补充，Lead 修复三个对抗审查发现的绿相用例）
//
// 1. gateway-default 代理形状（HIGH）：InheritGateway → cfg.proxy 也必须走无 DNS
//    快路径（此前 gateway.proxy 客户端曾直连）。
// 2. 客户端确实走代理（BLOCKER）：黑盒 e2e —— 本地极简 TCP 代理断言收到 CONNECT，
//    钉死"守护以为走代理、客户端实际直连"的脱钩（此前非空代理 URL 曾被倒置成直连）。
// 3. use_system_proxy 降级（MED）：use_system_proxy=true 时 env/NO_PROXY 语义使字节
//    去向不可保证 ⇒ 保守降级为完整检查（DNS 照跑），不得走快路径。
// ---------------------------------------------------------------------------

/// AppState：provider `px` 无 proxy 字段（InheritGateway），网关级
/// `proxy` / `use_system_proxy` 可配；`default_model="px-model"` 存在。
fn state_with_gateway_proxy(gateway_proxy: Option<&str>, use_system_proxy: bool) -> Arc<AppState> {
    let mut cfg = GatewayConfig::default();
    cfg.proxy = gateway_proxy.map(|s| s.to_string());
    cfg.use_system_proxy = use_system_proxy;
    cfg.providers.insert(
        "px".to_string(),
        ProviderConfig {
            base_url: "http://203.0.113.88/v1".to_string(),
            default_model: "px-model".to_string(),
            ..Default::default()
        },
    );
    Arc::new(AppState::new(cfg))
}

/// 本地极简 TCP 代理：读首行 → 记录 → 以 `CONNECT` 开头回 `HTTP/1.1 200 OK` 并保持
/// 隧道 ~2s；其他首行回 `HTTP/1.1 400`。返回 `(代理 URL, 收到的首行记录)`。
async fn spawn_fake_proxy() -> (String, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    let records: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake proxy listener");
    let addr = listener.local_addr().expect("fake proxy local addr");
    let recs = records.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let recs = recs.clone();
            tokio::spawn(async move {
                let mut line = String::new();
                {
                    let mut reader = BufReader::new(&mut sock);
                    let _ =
                        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
                            .await;
                } // drop reader → `sock` 重新可写
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    recs.lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(trimmed.clone());
                }
                if trimmed.starts_with("CONNECT") {
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
                    // 保持隧道：客户端 TLS 失败或被 abort 后读至 EOF 自然收尾。
                    let mut buf = [0u8; 1024];
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        loop {
                            match sock.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    })
                    .await;
                } else {
                    let _ = sock.write_all(b"HTTP/1.1 400\r\n\r\n").await;
                }
            });
        }
    });
    (format!("http://{}", addr), records)
}

/// 黑盒断言：`state.http_client_for_target("px", "px-model")` 发 `https://nope.invalid/v1`
/// 必须经代理以 CONNECT 首行触达（5s 内）。BLOCKER：守护以为走代理、客户端直连即红。
async fn assert_connect_received(state: &AppState, records: &Mutex<Vec<String>>) {
    let client = state.http_client_for_target("px", "px-model");
    let handle = tokio::spawn(async move {
        let _ = client.get("https://nope.invalid/v1").send().await;
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut connect_line = String::new();
    loop {
        let recs = records.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(l) = recs.iter().find(|l| l.starts_with("CONNECT")) {
            connect_line = l.clone();
            break;
        }
        drop(recs);
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();
    assert!(
        connect_line.starts_with("CONNECT"),
        "客户端必须经代理发 CONNECT（BLOCKER：守护以为走代理、客户端直连的脱钩），代理收到的首行: {connect_line:?}"
    );
    assert!(
        connect_line.contains("nope.invalid"),
        "CONNECT 必须点名目标 host，实际: {connect_line:?}"
    );
}

/// 回归 1（对抗审查 HIGH）：网关级代理 + InheritGateway 形状必须走无 DNS 快路径。
/// provider 无 proxy、模型经 default_model 解析 ⇒ `effective_proxy_url_for` 落到
/// `cfg.proxy` ⇒ 快路径 ⇒ 不可解析目标 Ok + 缓存 `(true, nope.invalid)` ok=true。
#[tokio::test]
async fn reg1_gateway_default_proxy_inherit_shape_fast_path() {
    let state = state_with_gateway_proxy(Some("http://127.0.0.1:8899"), false);
    let res = state
        .data_plane_egress_guard_for_target("px", "px-model", "https://nope.invalid/v1")
        .await;
    assert!(
        res.is_ok(),
        "InheritGateway→gateway.proxy 必须走快路径放行（零 DNS），实际 Err: {:?}",
        res.err()
    );
    let (ok, _, _) = cache_lookup(&state, (true, "nope.invalid"))
        .expect("必须写入 (true, nope.invalid) 缓存");
    assert!(ok, "缓存条目必须 ok=true");
}

/// 回归 2（对抗审查 BLOCKER）· provider 级代理：客户端必须真的以 CONNECT 走代理。
#[tokio::test]
async fn reg2_client_actually_dials_proxy_provider_level() {
    let (proxy_url, records) = spawn_fake_proxy().await;
    let mut cfg = GatewayConfig::default();
    cfg.providers.insert(
        "px".to_string(),
        ProviderConfig {
            base_url: "http://203.0.113.88/v1".to_string(),
            default_model: "px-model".to_string(),
            proxy: Some(proxy_url),
            ..Default::default()
        },
    );
    let state = Arc::new(AppState::new(cfg));
    assert_connect_received(&state, &records).await;
}

/// 回归 2（对抗审查 BLOCKER）· gateway 级代理（无 provider proxy）：同断言。
#[tokio::test]
async fn reg2_client_actually_dials_proxy_gateway_level() {
    let (proxy_url, records) = spawn_fake_proxy().await;
    let state = state_with_gateway_proxy(Some(&proxy_url), false);
    assert_connect_received(&state, &records).await;
}

/// 回归 3（对抗审查 MED）：use_system_proxy=true 必须保守降级为完整检查。
/// 即使 gateway.proxy 显式设置，env/NO_PROXY 语义使字节去向不可保证 ⇒ 不得走快路径 ⇒
/// 不可解析目标必须 Err，且缓存 `(false, nope.invalid)` 为瞬时失败（TTL ≤ 2s）。
#[tokio::test]
async fn reg3_use_system_proxy_conservative_downgrade() {
    let state = state_with_gateway_proxy(Some("http://127.0.0.1:8899"), true);
    let res = state
        .data_plane_egress_guard_for_target("px", "px-model", "https://nope.invalid/v1")
        .await;
    assert!(
        res.is_err(),
        "use_system_proxy=true 必须完整检查并拒绝（不得走快路径），实际 Ok = 降级缺失"
    );
    let (ok, transient, expires_at) = cache_lookup(&state, (false, "nope.invalid"))
        .expect("完整检查结果必须写入 (false, nope.invalid)");
    assert!(!ok, "负向判定 ok=false");
    assert!(transient, "DNS 失败分类为瞬时失败（transient=true）");
    let ttl = expires_at.saturating_duration_since(std::time::Instant::now());
    assert!(
        ttl <= Duration::from_secs(2),
        "瞬时失败 TTL 必须 ≤ 2s（契约 1s），实际 {ttl:?}"
    );
}

/// 回归 4（终审低危一致性提示）：`*.localhost` 子域（如 evil.localhost）与客户端静态
/// no_proxy（"localhost,127.0.0.1"，hyper-util 按域 + *.localhost 子域匹配）对齐 ⇒
/// no-proxy 豁免：即使显式代理生效，此类目标也会被客户端**直连**，故 guard 不得走
/// 无 DNS 快路径（否则"守护以为走代理跳过解析、客户端直连"脱钩）。
///
/// 环境注记：本机 systemd-resolved 把 `*.localhost` 解析为 `::1`（数据面 loopback
/// 允许）⇒ 完整检查结果为 Ok；若某环境解析失败（NXDOMAIN）则为 Err。断言按
/// **环境中立**方式写：(1) 不得出现 `(true, evil.localhost)` 快路径判定缓存；
/// (2) 判定必须落 direct 全检查 `(false, host)`；(3) 结果与 1 参直连内核一致。
#[tokio::test]
async fn reg4_localhost_subdomain_no_proxy_exempt_full_check() {
    let state = state_with_proxy(Some("http://127.0.0.1:8899"));
    let url = "https://evil.localhost/v1";

    let res = state
        .data_plane_egress_guard_for_target("px", "", url)
        .await;
    assert!(
        cache_lookup(&state, (true, "evil.localhost")).is_none(),
        "*.localhost 属 no_proxy 豁免：不得出现 (true, evil.localhost) 快路径判定缓存"
    );
    let _ = cache_lookup(&state, (false, "evil.localhost")).expect(
        "判定必须走 direct 完整检查（DNS 照跑）并写入 (false, evil.localhost)",
    );

    // 结果必须与 1 参直连内核一致：快路径被误用（本环境应 Err 时）此处对不齐。
    let direct_state = Arc::new(AppState::new(GatewayConfig::default()));
    let direct = direct_state.data_plane_egress_guard(url).await;
    assert_eq!(
        res.is_ok(),
        direct.is_ok(),
        "for_target 结果必须与 direct 全检查一致（不得误走快路径）"
    );
}
