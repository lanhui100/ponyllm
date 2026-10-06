# Agent Note: opencode-zen 出口池轮询（egress pool rotation）

Status: implemented
Date: 2026-10-07

## Problem

opencode-zen 免费档额度按客户端出口 IP 记账（上游 ipRateLimiter 的
`ratelimit:ip:<ip>:<YYYYMMDD>`，见 2026-10-06-zen-free-quota-ip-basis），与 key
无关。生产网关 opencode-zen 三把 key（含 `public`）全部经 devserver 隧道同一
出口出网 → 共享同一 IP 额度桶；现有 provider `strategy=round_robin` 只在 key 间
轮询，对额度拆分无意义：一个 IP 被上游 429 打爆时三把 key 同时冷却、请求全灭。

要拆分母桶，调度维度必须从"key 轮询"改为"**出口 IP 池轮询**"：provider 配置
多个出口（直连 + 各代理 URL），每次上游尝试按策略轮换出口；每个出口独立记账、
独立冷却——一个出口被打爆只冷却它自己，请求 failover 到下一个出口。

## Decision

1. **配置形态**：`[providers.opencode-zen]` 新增 `egress_pool: Vec<String>`（
   每条目 `direct`/`none`/空 = 网关本机出口，`http(s)://`/`socks5://` = 经该
   代理出网、每代理一独立 IP）与 `egress_strategy: String`（`round_robin` 默认
   | `priority`）。显式非空池**替代** provider/model 的 `proxy` 字段生效；空池
   = 维持单 proxy 语义**零迁移**。`ponyllm-config` 与
   `ponyllm-server::ProviderConfig` 双侧镜像字段。
2. **校验**：`ponyllm_config::validate_egress_entry` 是唯一真源（direct/none/空
   合法；其余与 `check_proxy_url_fast` 同策略：http/https/socks5 scheme；
   loopback 代理合法；私网/链路本地/元数据/`.svc`/IPv4-mapped 被拒；操作员
   `PONYLLM_PROBE_ALLOWLIST` 豁免保持同构）。**解析器与拨号器同一**：校验先过
   `url::Url::parse`（WHATWG，reqwest `Proxy::all` 同款），再按
   `Host::Ipv4/Ipv6/Domain` 判策略——hex/octal IPv4（`0xa000005`）、fragment
   特技（`10.0.0.5#@127.0.0.1`）、空白 host、userinfo 均由同一解析器裁决，
   杜绝校验/拨号解析漂移（对抗审查 SSRF-BYPASS-EGRESS-POOL）。端口缺省与拨号器
   对齐：http(s) 走 scheme 默认端口、socks 走 1080，缺端口不构成 fail-open
   （Proxy::all 同样接受），只拒绝 Proxy::all 用不了的形态（未知 scheme、
   不可解析/空 host）。admin PUT 经 `egress::check_egress_pool_entry` 薄包装
   复用同一判定，违者 400 `egress_blocked`；`egress_pool = []` 清空回退 proxy。
3. **调度**（`ponyllm-core::pool::egress::EgressPool`）：条目按插入序存
   `Arc<EgressEntry>`（`id` 调用方自选内部标识 + `url: Option<String>`），每条目
   自带冷却态（monotonic deadline + wall-clock `cooldown_reset_at` 镜像，镜像
   `ApiKeyEntry::set_cooldown` 的 MAX 钳制 + keep-max）。`select_egress()`：
   round_robin 以 AtomicUsize 计数器取模、跳过冷却条目（计数器每次选择都推进，
   冷却解除后轮转自然恢复）；priority 取首个非冷却条目；空池/全冷却 →
   `CoreError::NoAvailableKey(provider)`。
4. **执行器**（`executor/upstream.rs`）：`UpstreamExecutor` 新增
   `egress_pool: Option<Arc<EgressPool>>` + `egress_clients`（代理 URL → 预建
   client 映射；reqwest 0.12 的 `RequestBuilder` 无 per-request `proxy()`，故
   逐出口预建 client，直接条目用基 client）。每次尝试 `select_egress()` 决定
   出口；429 `FreeUsageLimitError`/quota 措辞/402（均已分类
   `QuotaExhausted`）→ `record_quota_exhausted(id, body Resets in >
   Retry-After > 900s)` 只冷却该出口；网络/5xx 只 `record_transient_failure`
   计数、failover 下一出口；全冷却 → fail-fast `quota_exhausted`。未配池时
   `egress_pool=None`，发送路径与现状逐字节一致。
5. **管理面**：`GET /api/admin/quota` 每 key 行新增 `egress` 数组（provider 级，
   `index`/`entry`/`state`/`cooldown_reset_at`，未配池缺省）；`PUT providers`
   载荷可写 `egress_pool`/`egress_strategy`，沿用 `admin_write_enabled` 门禁；
   写后经 `rebuild_egress_pool_for` 重建 live 池（shape 未变则保留冷却态）。
   **凭据脱敏**：`egress_pool`/`entry` 的视图回显经
   `sanitize_egress_entry_for_view` 去掉 userinfo（`http://user:pass@host` →
   `http://host:port`），执行器仍持 raw URL 供 pproxy 认证拨号（对抗审查
   VIEW-CREDENTIAL-ECHO）。**策略双侧一致**：非法 `egress_strategy` 在 config
   构建时拒绝建池（回退 proxy 语义 + warn），与 PUT 400 一致（对抗审查
   STRATEGY-DRIFT）。
6. **网络/SSRF 一致性**：配池 provider 的 `http_client_for_target` 返回直连基
   client（池替代 proxy 语义）；`data_plane_egress_guard_for_target` 对配池
   provider 强制全量 direct 校验——每次尝试的出口在 guard 时未知，宁多解析不少
   解析，绝不因池内 direct 条目跳过本地 DNS。

## Alternatives considered

- **key 维度继续轮询**：已被 2026-10-06 实证否决——同出口多 key 共享同一 IP
  桶，key 间轮询不拆额度。否决。
- **每请求动态建 proxy client**：reqwest 0.12 `RequestBuilder` 无
  per-request proxy 覆盖；动态建 client 每尝试一次 TLS/连接池重建，热路径不可
  接受。改为构造期按出口预建 client、`egress_clients` URL→client 映射对齐。采纳。
- **池内条目在 executor 处即时校验（dial-time）**：SSRF 防护放在 admin PUT 写
  入校验 + 配置构建期防御性丢弃（`build_egress_pool_for_cfg` 对非法条目 warn 并
  跳过）+ `data_plane_egress_guard_for_target` 全量直连校验三层；dial-time 逐
  条目复查收益低、热路径成本高。采纳三层方案。
- **校验逻辑复制进 ponyllm-config**：`ponyllm-config` 无法依赖
  `ponyllm-server`，C3 配置侧测试需要 `ponyllm_config::validate_egress_entry`；
  在 config 侧实现同策略镜像，server 侧 `check_egress_pool_entry` 薄包装复用，
  单一真源在 config（与 `check_proxy_url_fast` 的既有探针策略保持同构而非同码，
  靠 C3 双侧测试钉死）。采纳。
- **手动解析校验 vs url crate 同解析器**：初版手写 `rsplit('@')+split(':')`
  仅拦截 std 可解析的十进制字面量——hex/octal IPv4 与 fragment 特技可把私网
  host 偷渡进校验而拨号器直拨该地址（对抗审查实证 SSRF-BYPASS-EGRESS-POOL）。
  改为先 `url::Url::parse`（与 `Proxy::all` 同一 WHATWG 解析器），按
  `Host::Ipv4/Ipv6/Domain` 判策略；ponyllm-config 新增 `url` 依赖。采纳。
- **视图回显原文 vs 脱敏**：管理面 echo 原样回显 `user:pass@` 会泄漏代理
  凭据；视图统一过 `sanitize_egress_entry_for_view` 脱 userinfo 保留
  host:port，执行器保留 raw URL。采纳。
- **热重载清空冷却**：每次 reload 全量重建 egress pools 会复活一个正在冷却的
  出口、重演撞 429。改为 shape（strategy + 条目）未变时保留原 `Arc` 与冷却态
  （与 key pool `inherit_runtime_state` 同原则）。采纳。

## Consequences

- opencode-zen 可配 `egress_pool = ["direct", "http://127.0.0.1:8899", ...]`
  把额度桶按出口拆分；任一出口 429 只冷却自身。
- 未配池 provider：`egress_pool` 空 → 执行器/客户端/guard 全部走原路径；
  C8 回归测试钉死 `effective_proxy_url_for` 语义不变。
- 管理面：quota 行可见每出口 state/cooldown_reset_at；PUT 可写池；空数组清空。

## Verification

- 契约验收（Test Agent 冻结，红相 → 绿相）：
  `cargo test -p ponyllm-config --test egress_pool_config_tests`（9/9，含对抗
  审查新增的 `c3_validate_egress_rejects_ssrf_bypass_forms`）
  `cargo test -p ponyllm-core --test egress_pool_tests`（11/11）
  `cargo test -p ponyllm-server --test egress_pool_admin_tests`（8/8，含
  `c7_egress_views_redact_userinfo_credentials`）
- 回归：`cargo test --workspace` 全量 854 项通过、0 失败；web typecheck +
  vitest 157 全过。
- 对抗审查修复回归：`rejects_parser_mismatch_bypass_tricks`（hex/octal/
  fragment/空白 host/空 host 拒绝 + 端口缺省与拨号器对齐）+ 视图脱敏 +
  strategy 双侧一致，全部钉死在 config 单测与冻结 C3/C7 用例。