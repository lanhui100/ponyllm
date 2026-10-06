# Agent Note: opencode-zen 上游改走 loopback 规避出口守卫 CGNAT 自锁

Status: implemented

## Problem

本地网关（`/tmp/ponyllm-local.toml`，bind 127.0.0.1:18080）的 `opencode-zen` provider 请求
`muse-spark-1.3-contributor-free` / `fledge-alpha-free` 恒报
`503 upstream_unavailable`，末尾错误
`data-plane upstream '100.95.193.103' resolves to a blocked address (100.95.193.103)`
（`All candidate upstream providers exhausted`，无一个候选上游可拨）。

根因链路：

1. `100.95.193.103` 是**本机自己的 Tailscale 地址**（`tailscale0 inet 100.95.193.103/32`），
   8899 端口跑的是本机 pproxy-server（`pony-proxy`，token 路径 `/{token}/opencode/zen/v1`）。
2. 数据面出口守卫（VULN-07/F6，`egress::check_data_plane_url`）把 `100.64.0.0/10`
   （CGNAT，RFC 6598）整体列入黑名单，与 loopback/私网/元数据同等 fail-closed 拒绝
   （`egress.rs::blocked_v4`）；字面 CGNAT IP 在拨号前即被拒，不建立连接。
3. 两个模型只挂这一个 provider → 所有候选上游同因失败 → executor 报上游侧耗尽 → 503。

## Decision

1. **`opencode-zen.base_url` 的 host 从 `100.95.193.103` 改为 `127.0.0.1`**
   （`http://127.0.0.1:8899/{token}/opencode/zen/v1`）。数据面策略**明确允许 loopback**
   （文档化了本地模型服务器形态，如本地 Ollama），因此不触发守卫、无需任何算子放行配置。
   服务与网关同机，loopback 直达等价于经 Tailscale 自环。
2. **`fledge-alpha-free` 的 model_config 补 `protocol = "chat"`**：provider 级
   `default_protocol = "responses"` 令其经 responses 通道上送，上游（zen）回
   `ModelProtocolUnsupported`；与 `mimo-v2.5-free` 已显式声明 `protocol = "chat"` 一致后
   同模型正常返回。

验证（网关重启后实测，网关 key `sk-pony-…`）：

- `muse-spark-1.3-contributor-free` ✅（chat/completions 返回 pong）
- `fledge-alpha-free` ✅（补 protocol 后 chat/completions 返回 pong）
- `mimo-v2.5-free` ✅（回归不受影响）

## Alternatives considered

- *`PONYLLM_PROBE_ALLOWLIST=100.95.193.103` 放行该 CGNAT 字面 IP*：可行（B7 算子逃生舱），
  但为一个"本机自环即可达"的目标放宽出口策略，且把 Tailscale 地址写死进环境变量，
  换机器/换网段即失效——拒绝。
- *保留原 base_url 不做改动*：模型持续 503，拒绝。
- *base_url 换成本机公网域名*：本机无该域名，引入 DNS/证书依赖，拒绝。

## Consequences

- 自建同机上游（本机 pproxy/zen）经 loopback 拨号，越过守卫是**设计内合法形态**，
  不削弱守卫对私网/元数据/CGNAT 外目标的 SSRF 防护；Tailscale 地址仍整体被拒。
- `/tmp/ponyllm-local.toml` 是易失配置：任何重新生成该文件的脚本/流程须镜像同一改动
  （loopback host 与 fledge 的 `protocol = "chat"`），否则故障复现（靠 review）。
- 若未来 zen 服务迁出本机（如别的 Tailscale 节点），loopback 方案失效，届时按需评估
  allowlist 或公网端点。