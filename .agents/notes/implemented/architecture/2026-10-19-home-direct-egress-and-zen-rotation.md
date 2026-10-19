# Agent Note: 为 opencode-zen 建真实住宅直连出口与双 IP 轮换

Status: implemented
Date: 2026-10-19

## Problem

DSH 调用 `opencode-zen/deepseek-v4.1-flash-free` 持续报「当前请求的额度已用尽」，
用户质疑"有两个家庭 IP 出口，为什么没有轮换"。

排查得到的机器事实：

1. **上游返回体**：`HTTP 429 {"type":"error","error":{"type":"FreeUsageLimitError",...}}`
   以及 `HTTP 403 {"type":"FreeTierError","message":"...detected abusive traffic from this IP address..."}`。
   按 `2026-10-06-zen-free-quota-ip-basis` 已在源码层证实的记账规则，免费档额度挂在
   **出口公网 IP 的每日窗口桶**上（`ratelimit:ip:<ip>:<YYYYMMDD>`），与 key 无关。
   网关解析到的冷却截止 `cooldown_reset_at=2026-10-20T00:00:00Z`（UTC 午夜）与之一致。
2. **"两个出口"实际是链式中继，不是两个出口**：
   - `pproxy-host` Service 只有 1 个 Endpoint：`100.95.193.103:8899`（devserver）；
   - preprod 的 8899 是 `pproxy ha-forwarder --peers 100.105.241.39:8899`，
     tencent 的 8899 又是 `ha-forwarder --peers 100.95.193.103:8899`；
   - devserver 的 pproxy `route_upstreams.opencode = "vps"`（`rn.ponygo.fun`），
     经该代理 CONNECT 到 ipinfo 实测出口 = `192.210.231.8`（US/HostPapa）。
   即：所有路径最终收敛到**同一个 VPS 出口 IP**，天然共用一个额度桶，不存在轮换。
3. **两个家庭 IP 本身是健康的**：在 devserver / preprod 上**不经代理**直发上游，
   实测均 `200`（devserver 公网 `115.63.109.35`，preprod 公网 `115.63.109.7`）。
   被耗尽/被标记的是 VPS 出口 `192.210.231.8`。

## Decision

1. **新增两台家庭节点上的直连 CONNECT 出口**：`/usr/local/bin/home-direct-proxy.py`
   （asyncio，仅接受 `CONNECT`，Basic 鉴权，`HDP_ALLOW=opencode.ai`，强制 `AF_INET`
   以免 IPv6 出口造成 IP 漂移），由 systemd `home-direct-proxy.service` 托管，
   监听 `0.0.0.0:8898`。
2. **集群侧接线**：`deploy/egress-home-direct.yaml` 定义 `egress-home-dev`（→100.95.193.103:8898）
   与 `egress-home-pre`（→100.97.143.121:8898）两个 selector-less Service+Endpoints，
   并把两个 Service 名写入 4 个 Deployment 的 `PONYLLM_PROBE_ALLOWLIST`
   （否则 `validate_egress_entry` 以 `*.svc are not allowed` 丢弃条目、静默退化直连）。
3. **opencode-zen 使用两条目 egress_pool**（`egress_home_pre` / `egress_home_dev`，
   `egress_strategy=round_robin`），并把同一上游凭据重复登记为两个 key id
   （`zen-final-1` / `zen-final-1b`），使一次 403 只冷却一个 key，第二次尝试仍可命中
   另一出口/另一 key。
4. **当前运行态**：devserver 住宅 IP 被上游间歇性判为 abusive（抽样 6 次命中 3 次 403），
   preprod 住宅 IP 抽样 6 次命中 1 次，故 `egress_pool` **暂时只启用 `egress-home-pre`**；
   dev 出口保留在清单与允许列表里，待上游标记消退后加回数组即可恢复双出口轮换。

## Alternatives considered

- **把 opencode-zen 改成 `egress_pool = ["direct"]`**：云节点（tencent/proserver 之外的
  tencent、aliyun）直连会被上游判机房滥用并 403，而 403 会按 `is_zen_free_tier_gate_body`
  冷却 key 900s，等价于把四个副本里的两个变成"毒副本"。否决。
- **直接改 pproxy 的 `route_upstreams.opencode` 去掉 vps 路由**：pproxy 的上游枚举只有
  `worker|vercel|Named`，没有 "direct" 形态（`crates/core/src/route.rs`），改不掉；
  且 pproxy 属外部平台仓职责，本仓不宜越界改生产代理拓扑。否决。
- **让 preprod 的 ha-forwarder 指回 devserver 并期待"多点冗余"**：那正是既有形态，
  所有流量仍收敛到 192.210.231.8，轮换是假的。否决（这正是本次要拆掉的假设）。
- **只保留 preprod 单出口、不做双侧接线**：能立刻恢复，但下一次该 IP 被标记时又会
  全量不可用；保留 dev 侧 Service/代理只需一行数组改动即可重新启用，成本更低。采纳
  （作为"当前运行态"），双出口接线能力保留。

## Consequences

- 机器收据（2026-10-19 23:5x–次日 00:0x）：
  - 两个家庭节点直连上游均 `200`；经各自 `home-direct-proxy` 亦均 `200`；
  - 公共入口 `tokens.ponyjob.top` 连续 15 次 `deepseek-v4.1-flash-free` 流式请求
    **15/15 成功**；
  - 7 个 gateway 副本逐个探测 **7/7 成功**。
- 双出口同时启用时若任一 IP 被上游判 abusive，403 会冷却 key（900s），因此"轮换"
  只在出口健康时生效；上游滥用门禁是外部约束，网关侧只能降低单 IP 依赖度。
- `home-direct-proxy` 是新增的宿主机常驻服务（两台家庭节点，systemd），属集群基础设施；
  其代码与 unit 的权威副本建议同步到 `cluster-infra` 仓，本仓只保留接线清单与 ADR。
- 回滚：把 `[providers.opencode-zen]` 的 `egress_pool` 改回
  `["http://user:<token>@pproxy-host:8899"]`（原 pproxy 路径），并 `systemctl disable --now
  home-direct-proxy` 即可；Deployment 的 allowlist 多出的条目无害可留。
