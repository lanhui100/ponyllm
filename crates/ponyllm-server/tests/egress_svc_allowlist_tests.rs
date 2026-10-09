//! Wave-2 验收测试（红相）：`data_plane_blocked_name()` 的 `*.svc` 白名单豁免契约。
//!
//! ## 冻结契约（Lead 下发，不得自行加码或缩水）
//! 当主机名命中 `*.svc` / `*.svc.cluster.local` 封禁规则时，**若且仅当**该主机
//! 被 `probe_allowlisted()` 精确命中、**且**命中的白名单条目本身是 `.svc` 后缀名，
//! 才予以放行。三条硬约束：
//!   1. 云元数据主机（`METADATA_HOSTS` / `169.254.169.254`）判定**必须前置**，
//!      **永不可**被白名单豁免；
//!   2. 白名单条目若为裸 TLD（如 `svc`），**不得**解锁任意 `*.svc`
//!      （`probe_allowlisted` 的 `ends_with(".{entry}")` 语义会让裸 `svc`
//!      匹配一切 `*.svc`，修复必须显式挡住这个陷阱）；
//!   3. `check_probe_url_fast`（admin 写入闸门）现有行为**不得**改变。
//!
//! ## 为什么走这两个公开入口（纯黑盒）
//! `data_plane_blocked_name` 本身是私有 `fn`，`check_data_plane_policy_fast`
//! 是 `pub(crate)` —— 二者都**不可**从集成测试访问。本文件改用两条真实数据面
//! 入口，它们都经由 `check_data_plane_policy_fast` 抵达同一判定：
//!   - `check_data_plane_url_proxied` —— 纯 fast path，**零 DNS**，完全确定性；
//!   - `check_data_plane_url_with_resolver` —— 注入式 resolver，**零真实 DNS**。
//! 不使用 `check_data_plane_url`（真实 `getaddrinfo`）作为判定依据：它会把
//! 网络抖动混进协议契约，制造 Flaky。
//!
//! ## 环境变量纪律
//! `PONYLLM_PROBE_ALLOWLIST` 是运行时读取的进程级变量，多线程 `cargo test`
//! 会互相串味。因此每个用例通过 [`Allowlist`] 守卫：取全局互斥锁 → 设置 →
//! Drop 时（含 panic 路径）逐项还原。本组全部用例都持锁，天然串行。
//! 工作区 `edition = "2021"`，`std::env::set_var` 仍是安全 fn，故**无需 `unsafe`**
//! （edition ≥ 2024 才需要，且本文件未出现任何 `unsafe`）。
//!
//! 绝不使用 `#[ignore]`、绝不吞异常放行：每条断言都绑定具体判定语义。

use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard};

use ponyllm_server::egress::{
    check_data_plane_url_proxied, check_data_plane_url_with_resolver, check_probe_url_fast,
    DnsLookupError,
};

const ALLOWLIST_VAR: &str = "PONYLLM_PROBE_ALLOWLIST";
/// 全局 loopback 逃生口。与本组判定无关，但若外部二进制置了 `1`，会污染
/// 进程内所有用例；守卫期间强制清除，保证用例是自洽的。
const LOOPBACK_VAR: &str = "PONYLLM_ALLOW_LOOPBACK_PROBE";

/// 序列化所有改写进程级环境变量的用例（`cargo test` 默认多线程）。
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII 环境守卫：持锁 → 覆盖 `PONYLLM_PROBE_ALLOWLIST` → Drop 还原全部快照。
/// `None` = 清除该变量（等价于"未配置白名单"的干净基线）。
struct Allowlist {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl Allowlist {
    fn set(value: Option<&str>) -> Self {
        // 某条用例红相 panic 时锁会被 poison；恢复 inner guard 避免把一次
        // 预期内的红相放大成后续所有用例的级联失败。
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut saved = Vec::new();
        for var in [ALLOWLIST_VAR, LOOPBACK_VAR] {
            saved.push((var, std::env::var(var).ok()));
        }
        match value {
            Some(v) => std::env::set_var(ALLOWLIST_VAR, v),
            None => std::env::remove_var(ALLOWLIST_VAR),
        }
        std::env::remove_var(LOOPBACK_VAR);
        Self { _lock: lock, saved }
    }
}

impl Drop for Allowlist {
    fn drop(&mut self) {
        for (var, value) in self.saved.drain(..) {
            match value {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }
}

/// 注入式 resolver：一旦被调用就 fail-closed。
///
/// 用途有二：
///   - 对"白名单精确命中应当放行"的用例，resolver **根本不该被调用**；
///     一旦被调用即说明白名单没有前置生效，用例会拿到明确失败而不是靠 DNS 运气绿。
///   - 对"策略拒绝"的用例，拒绝必须发生在解析之前，这里同样保证了这一点。
fn resolver_must_not_run(host: &str) -> Result<Vec<IpAddr>, DnsLookupError> {
    Err(DnsLookupError::Failure(format!(
        "resolver must not be consulted for policy-decided host '{host}'"
    )))
}

/// 断言 URL 在**两条数据面入口**上都被确定性策略拒绝，且原因可读、非瞬态。
async fn assert_dp_refused(url: &str, needle: &str) {
    // 入口 1：proxied 快路径（零 DNS）。
    let proxied = check_data_plane_url_proxied(url).await;
    let proxied_err = match proxied {
        Ok(()) => panic!("proxied 快路径必须拒绝 {url}（规则 {needle}），实际放行"),
        Err(e) => e,
    };
    assert!(
        proxied_err.reason.contains(needle),
        "proxied 拒绝原因应包含 {needle:?}，实际: {}",
        proxied_err.reason
    );
    assert!(
        !proxied_err.transient,
        "策略拒绝必须是确定性拒绝（transient=false），否则会落入错误的负缓存策略: {}",
        proxied_err.reason
    );

    // 入口 2：直连路径（注入 resolver，零真实 DNS）。策略拒绝必须先于解析发生。
    let direct = check_data_plane_url_with_resolver(url, resolver_must_not_run).await;
    let direct_err = match direct {
        Ok(()) => panic!("直连路径必须拒绝 {url}（规则 {needle}），实际放行"),
        Err(e) => e,
    };
    assert!(
        direct_err.reason.contains(needle),
        "直连拒绝原因应包含 {needle:?}，实际: {}",
        direct_err.reason
    );
    assert!(
        !direct_err.transient,
        "策略拒绝必须是确定性拒绝（transient=false）: {}",
        direct_err.reason
    );
}

/// 断言 URL 在**两条数据面入口**上都被放行，且全程零 DNS。
async fn assert_dp_allowed(url: &str) {
    let proxied = check_data_plane_url_proxied(url).await;
    assert!(
        proxied.is_ok(),
        "proxied 快路径必须放行 {url}，实际拒绝: {:?}",
        proxied.err()
    );

    let direct = check_data_plane_url_with_resolver(url, resolver_must_not_run).await;
    assert!(
        direct.is_ok(),
        "直连路径必须放行 {url}（白名单命中后不得再依赖 DNS），实际拒绝: {:?}",
        direct.err()
    );
}

// ---------------------------------------------------------------------------
// G1 — 未白名单的 `*.svc` 仍被拒
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g1_non_allowlisted_svc_host_is_still_refused() {
    // Arrange
    let _env = Allowlist::set(None);

    // Act + Assert
    assert_dp_refused("http://pproxy-other.svc:8080/", "kubernetes in-cluster").await;
    assert_dp_refused(
        "http://pproxy-other.svc.cluster.local:8080/",
        "kubernetes in-cluster",
    )
    .await;
}

// ---------------------------------------------------------------------------
// G2 — 白名单精确命中 `.svc` 后缀条目 -> 放行（**红相**）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g2_allowlisted_svc_host_is_admitted() {
    // Arrange
    let _env = Allowlist::set(Some("pproxy-host.ponyllm.svc"));

    // Act + Assert
    assert_dp_allowed("http://pproxy-host.ponyllm.svc:8080/").await;
}

// ---------------------------------------------------------------------------
// G3 — 白名单以 FQDN 形态（`.svc.cluster.local`）命中 -> 放行（**红相**）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g3_allowlisted_svc_fqdn_host_is_admitted() {
    // Arrange
    let _env = Allowlist::set(Some("pproxy-host.ponyllm.svc.cluster.local"));

    // Act + Assert
    assert_dp_allowed("http://pproxy-host.ponyllm.svc.cluster.local:8080/").await;
}

// ---------------------------------------------------------------------------
// G4 — 对抗：白名单里塞进元数据主机也必须无效（判定前置，永不可豁免）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g4_metadata_hosts_are_never_exempted_by_the_allowlist() {
    // Arrange
    let _env = Allowlist::set(Some("169.254.169.254,metadata.google.internal,foo.svc"));

    // Act + Assert
    assert_dp_refused("http://169.254.169.254/latest/meta-data/", "cloud metadata").await;
    assert_dp_refused("http://metadata.google.internal/", "cloud metadata").await;
    assert_dp_refused("http://metadata.google.com/computeMetadata/v1/", "cloud metadata").await;
    assert_dp_refused("http://instance-data/latest/meta-data/", "cloud metadata").await;
}

// ---------------------------------------------------------------------------
// G5 — 对抗：裸 TLD 条目不得解锁任意 `*.svc`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g5_bare_tld_allowlist_entry_must_not_unlock_any_svc_host() {
    // Arrange：条目就是裸 `svc` —— `probe_allowlisted` 的
    // `host.ends_with(".{entry}")` 语义会让它匹配一切 `*.svc`，修复必须显式挡住。
    let _env = Allowlist::set(Some("svc"));

    // Act + Assert
    assert_dp_refused("http://anything.svc:8080/", "kubernetes in-cluster").await;
    assert_dp_refused("http://pproxy-host.ponyllm.svc:8080/", "kubernetes in-cluster").await;
    assert_dp_refused(
        "http://anything.svc.cluster.local:8080/",
        "kubernetes in-cluster",
    )
    .await;
}

// ---------------------------------------------------------------------------
// G6 — 未白名单时 `kubernetes.default.svc` 仍被拒
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g6_kubernetes_default_svc_is_refused_without_allowlist() {
    // Arrange
    let _env = Allowlist::set(None);

    // Act + Assert
    assert_dp_refused("http://kubernetes.default.svc:443/api", "kubernetes in-cluster").await;
    // 裸 `svc` 主机名本身也仍在封禁列表里。
    assert_dp_refused("http://svc:8080/", "kubernetes in-cluster").await;
}

// ---------------------------------------------------------------------------
// G7 — 公网主机判定不受本次改动影响
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g7_public_hosts_are_unaffected() {
    // Arrange：注入一个公网 IP 的 resolver（本组唯一需要真实解析结果的形态）。
    let public_ip: IpAddr = IpAddr::from([203, 0, 113, 10]);

    // Act + Assert — 无白名单时公网主机照常放行。
    {
        let _env = Allowlist::set(None);
        let proxied = check_data_plane_url_proxied("https://opencode.ai/zen/v1").await;
        assert!(
            proxied.is_ok(),
            "公网主机 opencode.ai 必须照常放行，实际: {:?}",
            proxied.err()
        );
        let direct =
            check_data_plane_url_with_resolver("https://opencode.ai/zen/v1", move |_h| Ok(vec![public_ip]))
                .await;
        assert!(
            direct.is_ok(),
            "公网主机直连路径必须放行，实际: {:?}",
            direct.err()
        );
    }

    // 公网主机判定不得被无关的 `*.svc` 白名单条目带偏。
    {
        let _env = Allowlist::set(Some("pproxy-host.ponyllm.svc"));
        let proxied = check_data_plane_url_proxied("https://opencode.ai/zen/v1").await;
        assert!(
            proxied.is_ok(),
            "无关的 svc 白名单条目不得影响公网主机判定，实际: {:?}",
            proxied.err()
        );
        let direct =
            check_data_plane_url_with_resolver("https://opencode.ai/zen/v1", move |_h| Ok(vec![public_ip]))
                .await;
        assert!(
            direct.is_ok(),
            "无关的 svc 白名单条目不得影响公网主机判定，实际: {:?}",
            direct.err()
        );
    }

    // 反向确认：公网主机解析到私有 IP 时**仍必须被拒**，证明本次改动没有把
    // 公网路径整体短路掉。
    {
        let _env = Allowlist::set(None);
        let private_ip: IpAddr = IpAddr::from([10, 0, 0, 5]);
        let direct = check_data_plane_url_with_resolver("https://opencode.ai/zen/v1", move |_h| {
            Ok(vec![private_ip])
        })
        .await;
        let err = direct.expect_err("公网主机解析到私网地址时必须 fail-closed");
        assert!(
            err.reason.contains("blocked address"),
            "拒绝原因应为私网地址，实际: {}",
            err.reason
        );
        assert!(
            !err.transient,
            "私网解析拒绝是确定性的，不应标记为 transient: {}",
            err.reason
        );
    }
}

// ---------------------------------------------------------------------------
// 约束 3 — `check_probe_url_fast`（admin 写入闸门）现有行为锁定
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g8_check_probe_url_fast_behaviour_is_unchanged() {
    // 场景 A：白名单条目命中 `*.svc` 主机时，admin 写入闸门当前**放行**
    // （`check_probe_url_fast` 先求值 `probe_allowlisted` 再走名称策略）。
    // 这是行为锁而非背书：Wave-2 的修复只准动数据面，禁止顺带改变 admin 闸门。
    {
        let _env = Allowlist::set(Some("pproxy-host.ponyllm.svc"));

        // A1: 精确命中 -> 放行
        let allowlisted = check_probe_url_fast("http://pproxy-host.ponyllm.svc:8080/");
        assert!(
            allowlisted.is_ok(),
            "check_probe_url_fast 现有行为锁定：白名单精确命中时放行，实际: {:?}",
            allowlisted.err()
        );

        // A2: 未命中白名单的 `*.svc` 依旧被 admin 写入闸门拒绝
        let unlisted = check_probe_url_fast("http://pproxy-other.svc:8080/");
        let err = unlisted.expect_err("未白名单的 *.svc 必须继续被 admin 写入闸门拒绝");
        assert!(
            err.contains("kubernetes in-cluster"),
            "拒绝原因应包含 kubernetes in-cluster，实际: {err}"
        );

        // A3: 未命中白名单的 `.svc.cluster.local` 同样被拒
        let unlisted_fqdn = check_probe_url_fast("http://pproxy-other.svc.cluster.local:8080/");
        let err_fqdn = unlisted_fqdn.expect_err("未白名单的 *.svc.cluster.local 必须继续被拒绝");
        assert!(
            err_fqdn.contains("kubernetes in-cluster"),
            "拒绝原因应包含 kubernetes in-cluster，实际: {err_fqdn}"
        );
    }

    // 场景 B：白名单精确命中元数据 IP 时，admin 写入闸门当前**也放行**。
    // 注意这与数据面 G4「元数据永不可豁免」并不冲突：那条约束只约束数据面
    // （`data_plane_blocked_name` 的判定顺序），此处锁的是 admin 闸门现状，
    // 防止修复外溢。冻结契约明确要求 `check_probe_url_fast` 行为不得改变。
    {
        let _env = Allowlist::set(Some("169.254.169.254"));

        let allowlisted_ip = check_probe_url_fast("http://169.254.169.254/latest/meta-data/");
        assert!(
            allowlisted_ip.is_ok(),
            "check_probe_url_fast 现有行为锁定：白名单精确命中元数据 IP 时放行，实际: {:?}",
            allowlisted_ip.err()
        );

        // 同场景下未命中白名单的元数据主机名依旧被拒。
        let metadata_name = check_probe_url_fast("http://metadata.google.internal/");
        let err_name = metadata_name.expect_err("未白名单的元数据主机名必须继续被拒绝");
        assert!(
            err_name.contains("cloud metadata"),
            "拒绝原因应包含 cloud metadata，实际: {err_name}"
        );
    }
}

/// 用例间不串味：守卫 Drop 后 `PONYLLM_PROBE_ALLOWLIST` 必须回到原值。
#[tokio::test]
async fn g9_allowlist_guard_restores_previous_environment() {
    // Arrange：先人为制造一个"脏"基线，验证守卫能原样还原。
    std::env::set_var(ALLOWLIST_VAR, "baseline-entry.example.org");
    {
        let _env = Allowlist::set(Some("pproxy-host.ponyllm.svc"));
        assert_eq!(
            std::env::var(ALLOWLIST_VAR).ok().as_deref(),
            Some("pproxy-host.ponyllm.svc"),
            "守卫生效期间变量应为新值"
        );
    }
    // Assert
    assert_eq!(
        std::env::var(ALLOWLIST_VAR).ok().as_deref(),
        Some("baseline-entry.example.org"),
        "守卫 Drop 后必须还原为进入前的值，否则会污染同进程其它用例"
    );
    // 清掉人为基线，交还干净环境。
    std::env::remove_var(ALLOWLIST_VAR);
}