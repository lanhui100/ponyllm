# Agent Note: egress guard 对显式代理目标跳过本地 DNS 并分类缓存瞬时失败

Status: implemented

## Problem

数据面出口防护（`crates/ponyllm-server/src/egress.rs::check_data_plane_url`）在拨号前对目标
host 做阻塞式本地 getaddrinfo，5s 超时 fail-closed；失败判定进 10s 负缓存
（`state.rs::data_plane_egress_guard`）。对经显式 forward proxy 出口的目标（如
antigravity → `pproxy-host.ponyllm.svc:8899` CONNECT 代理，代理自行解析目标域），本地解析
结果与字节流向无关，却承担了同一份 5s fail-closed + 10s 级联风险。

2026-10-06T10:59 实证（三网关同窗口）：共享上游间歇性静默 ~5s 丢包 → dev/proserver/tencent
网关各一次 `latency_ms≈5002/5015/5009`（tokio 5s 上限触顶）→ 10s 负缓存内后续请求保证 503
`(cached)`。故障是泛化上游丢包（同一窗口 coredns-proserver 对无关域名同样超时），非 Google
专属、非单节点本地抖动。

## Decision

1. **代理快路径（skip local DNS）**：新增 `AppState::data_plane_egress_guard_for_target(
   provider_name, model_name, url)`；与 `http_client_for_target` 共享同一"生效代理"计算
   （`GatewayConfig::effective_proxy_url_for`：model > provider > InheritGateway→gateway.proxy，
   同源杜绝漂移）。仅当**同时**满足才走无 DNS 快路径（`egress::proxy_fast_path_eligible`）：
   scheme ∈ {http, https} && `reqwest::Proxy::all(trimmed)` 可解析 && 目标 host 不在
   no_proxy 豁免（localhost/127.0.0.1）&& 代理 host 通过 `check_proxy_url_fast`。其余
   （socks5/socks5h——workspace reqwest 无 socks feature、`Proxy::all` 必失败、客户端会静默
   回退直连；代理 URL 解析失败；空串；仅 use_system_proxy=true）一律完整检查，杜绝"守护以为
   走代理、客户端实际直连"的 SSRF 脱钩。快路径命中打 trace 日志（可审计）。
2. **三态缓存**：`EgressGuardVerdict` 增 `transient` 字段（保留 `ok`/`expires_at` pub）；
   缓存键改为 `(proxied: bool, lowercase host)`——proxied 的 Ok 判定不污染 direct 拨号
   （DNS-rebinding 防护不被同 host 键静默关闭）。TTL：Ok=5s / 确定性拒绝（黑名单名、私网
   字面 IP、解析出私网 IP）=10s / 瞬时失败（DNS 超时/解析错误/空结果/Join 错误）=1s。
   per-(mode,host) 在途单飞去重（Notify 模式，owner 解析→入缓存→移除条目→notify_waiters，
   waiter 醒后重查缓存、6s 有界等待），避免持续故障期每次请求各付满 5s 并钉死
   spawn_blocking 线程（频率放大 DoS）。
3. **类型与可测性**：`check_data_plane_url → Result<(), DataPlaneRefusal{reason, transient}>`
   （唯一生产调用者是 state.rs 的 guard 内核）；DNS 解析抽成可注入 seam
   `check_data_plane_url_with_resolver(raw, resolver)`，超时/失败/空集/Join 错误映射为
   `DnsLookupError` 变体，单测无需真实墙钟。共享 `check_data_plane_policy_fast`（scheme +
   名字黑名单 + allowlist 优先 + 字面 IP）供 direct 与 proxied 复用。
4. **调用点**：chat/messages/responses/images/systemone 五处改为
   `data_plane_egress_guard_for_target(&provider_name, &target.physical_model, &url)`；
   保留 1 参 `data_plane_egress_guard` 作为 direct 内核（既有测试原样编译）。

## Alternatives considered

- **瞬时失败零缓存**（架构参谋初案）：持续解析失败的主机在零缓存下每个请求付满 5s、
  并发可打爆 spawn_blocking 池 → 采用 1s 短 TTL + 单飞去重替代（安全参谋指正）。
- **CONNECT 探测验证代理侧解析**：无法证明代理解析目标、每请求加延迟 → 否。
- **PONYLLM_PROBE_ALLOWLIST 通配豁免 googleapis 域**：削弱整个域名族校验；仅作临时运维
  止血（加 `daily-cloudcode-pa.googleapis.com` 一行），不作代码解 → 否。
- **仅缓存 Ok（负向不缓存）**：等价零缓存反模式 → 否。
- **提升 DNS 超时 / 加固 resolver / CoreDNS forward 显式上游**：治标；resolver 属
  cluster-infra 仓边界，跨仓成本高；代码侧"对代理目标不解析"才是根因消除 → 否。
- **缓存键保持 host-only**：proxied Ok 会污染 direct 拨号、静默关闭 DNS-rebinding 防护
  （架构参谋 high finding）→ 采用 (mode, host) 分键。
- **APIs：guard 传 proxy 参数 vs 自解析**：自解析与 `http_client_for_target` 原子一致，
  5 处调用点单行化，防遗漏漂移 → 采用自解析 + 共享 helper。

## Consequences

- 代理目标的安全判定边界移入受信代理的解析命名空间（split-horizon 内网名在集群内代理侧
  解析到内网不再被网关拦截）——现网代理为受信出海出口且自带 egress ACL，判定为显式
  Non-Goal 信任边界；代理 host 拨号时复验 `check_proxy_url_fast` 收窄残余面。
- use_system_proxy=true 且无显式代理 → 一律完整检查（保守侧，宁多解析不少解析）。
- **Review hardening（对抗审查 2 轮后落地，commit 7f28cf2）**：
  - 修复预存倒置 `proxy_opt = url.trim().is_empty().then(|| url)`（客户端缓存未命中即建
    直连客户端，与 guard 的 proxied 判定脱钩）→ 非空代理 URL 必建代理客户端，回归用例为
    本地 CONNECT 代理 e2e（provider 级 + gateway 级两形状断言客户端真实走代理）。
  - `use_system_proxy=true` 时 guard 强制完整检查（env NO_PROXY 可绕过显式代理，
    Non-Goal 保守覆盖，命中即 DNS 照跑）。
  - 单飞 owner 以 `InflightEntry` RAII Drop guard 兜底：owner 取消/panic 时移除
    (mode,host) 条目，waiter 有界等待后可接管为新 owner，杜绝 stale-Occupied 永久 hang。
- 一次 DNS 瞬时抖动不再造成 10s 保证 503：瞬时失败 1s 内快速重检，恢复即放行。
- 验收（implemented 时全绿）：acceptance_egress_proxied_tests（12 用例：代理零 DNS 放行/
  直连瞬时拒绝 TTL≤2s/socks5 与不可解析代理强制全检查/确定性拒绝缓存/mode 隔离/注入
  resolver 分类全表/fast-direct 签名钉死/gateway-default 代理形状/客户端真实走代理 e2e×2/
  use_system_proxy 降级）、acceptance_sec_egress_cache_tests（新键形态 ≤15s/≤30s 上界）、
  acceptance_sec_ssrf_tests、ponyllm-server 全量回归无失败。
- 单飞去重引入并发复杂度——以 RAII Drop guard + 有界等待兜底（消除取消泄漏），并以
  3 次熔断预算兜底失败隔离，可回滚至红相锚定提交（56a385f）。