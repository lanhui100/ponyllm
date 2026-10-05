# Agent Note: 数据面 SSRF 防护——拨号复检 + 禁重定向 + egress 阻断表补段（VULN-07/F6）

Status: implemented

## Problem

网关数据面（inference chat/messages/responses/images/systemone）拨号用户配置的上游时，仅在 provider 写入时刻做一次 egress 校验（`check_probe_url`），拨号时刻不复检；且数据面 `reqwest` client 未设 `redirect(Policy::none)`，跟随上游重定向。组合效果：拥有 admin-write（或开放模式）的攻击者可在写入后把域名 DNS 重绑定到内网/元数据地址，或让上游 302 到内网目标，使网关成为 SSRF 跳板并把内网响应回显给调用方。此外 `blocked_v4` 未覆盖 100.64/10（CGNAT）与 198.18/15（基准测试段），此类地址可作内网资产旁路。合约：FIX-CONTRACT.md F6（VULN-07）。

## Decision

1. **egress 阻断表补段**：`crates/ponyllm-server/src/egress.rs` `blocked_v4` 增加 `100.64/10`（RFC 6598 CGNAT）与 `198.18/15`（RFC 2544 基准测试段）；探针校验（`check_probe_url*`）与代理校验（`check_proxy_url_fast`）共享该函数自动生效，IPv4-mapped IPv6 经内嵌规则同步覆盖。
2. **数据面禁重定向**：`crates/ponyllm-core/src/executor/upstream.rs` `try_create_upstream_http_client_with_timeout` 的 builder 与代理解析失败时的 fallback builder 均设置 `.redirect(reqwest::redirect::Policy::none())`（与探针 client 一致），3xx 显式报错、绝不跟随。
3. **拨号逐请求复检**：
   - `egress.rs` 新增 `check_data_plane_url`：与探针同族策略（http/https 白名单；字面 IP 按共享阻断表判定；主机名 `getaddrinfo` 全地址复检、5s 超时 fail-closed；`*.svc`/云元数据名按名阻断；`PONYLLM_PROBE_ALLOWLIST` 提升 LAN 模型服务器名），唯一差异：回环字面 IP 与 `localhost` 名保持合法——数据面产品文档形态（本地 Ollama on 127.0.0.1）。
   - `state.rs` 新增 `AppState::data_plane_egress_guard(url)`：逐请求调用 `check_data_plane_url`，外包主机键 TTL 缓存（正 60s / 负 10s，未命中 fail-closed，锁中毒自恢复），热路径零额外 DNS。
   - 在 `routes/{chat,messages,responses,images,systemone}.rs` 的 per-target 拨号点（target_url 构造后、首个 execute 前）插入守卫；拒绝时置 `last_error`/`UpstreamUnavailable` 并 `continue`（不发拨号）。
4. 只改生产代码；验收测试由 Test Agent 冻结维护，本次未触碰任何测试文件。

## Alternatives considered

- **维持现状（仅写入时校验）**：写入后 DNS 重绑定窗口存在于整个上游生命周期，且 302 跟随可绕过校验；否决。
- **数据面套用探针回环禁令**（回环仅 `PONYLLM_ALLOW_LOOPBACK_PROBE=1` 放行）：会破坏"本地 Ollama"这一文档化数据面形态，并让既有的 127.0.0.1 mock 上游 e2e 在通过鉴权后仍被拒；否决，改为数据面回环默认合法、其余阻断表全量生效。
- **复检下沉 ponyllm-core executor 单点**：需把 IP 策略迁入 core 或引入 server→core 反向依赖；本期选 server 侧 helper + 5 路由调用点（每 target 每请求一次，collect/重试分支复用同一 target_url，一次守卫覆盖全部 execute），改动面小且策略与探针同文件可维护。
- **`Client::resolve` 钉 IP 完全消除 check→connect TOCTOU**：成本高、改动面大；逐请求复检已把窗口缩到毫秒级（与探针路径同水平，egress.rs 既有注释自认该残余），列为下期候选。
- **数据面复检做成默认关的配置开关**：默认关会使修复在未配置部署上静默失效；默认 on + 逃生口 env（回环默认放行、LAN 名走 `PONYLLM_PROBE_ALLOWLIST`）是更安全默认，本期不引入开关。

## Consequences

- 行为变化：依赖上游 3xx 跳转的 provider 现在显式报错（不再静默跟随）；升级后 LAN 模型服务器上游需设置 `PONYLLM_PROBE_ALLOWLIST`（本地 Ollama 回环默认不受影响）。
- 性能：冷缓存首次请求每个新 host 多一次有界 DNS（≤5s、fail-closed），热路径由 60s 正缓存吸收；chat 首包延迟无感知劣化（缓存命中即零 DNS）。
- 已知测试缺陷（非实现缺陷）：`acceptance_sec_ssrf_tests.rs::f6_egress_boundaries_outside_remain_open` 断言 `100.65.0.1` 放行，而该地址属于 100.64/10 CGNAT 段（100.64.0.0–100.127.255.255），与同文件 `f6_egress_blocks_cgnat_100_64_10` 自相矛盾；按合约实现 /10 后该断言失败，需 Test Agent 将该项改为 `100.128.0.1`（已上报 Lead 协调）。
- 回归状态：server/core lib 全绿；鉴权通过的数据面测试（gateway_keys_api_tests、auth_compat_tests）全绿；未鉴权 e2e 的 401 失败属 F1（open 模式 fail-closed，auth-lane 在途）与本项无关。
