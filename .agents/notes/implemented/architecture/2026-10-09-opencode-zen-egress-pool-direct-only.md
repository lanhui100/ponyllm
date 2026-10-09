# ADR: opencode-zen egress_pool 降爆炸半径（仅 direct）

Status: implemented
Date: 2026-10-09
Authors: Team Lead (diagnosis), Test Agent (T4b 冻结预检), Executor-Config (实施)

## Context & Problem Statement

2026-10-09 波次诊断确认：pproxy 曾整体卡死（`crates/server/src/gateway.rs`
`serve_data_plane` 在 accept 循环内同步 `sem.acquire_owned().await`，
`MAX_CONCURRENT_CONNECTIONS=256`；CONNECT relay 持 permit 无超时；死隧道占满
permit → accept 停转 → 内核 accept 队列溢出 recv-Q 129/128、330 CLOSE-WAIT），
导致依赖 pproxy 的所有路径全线 10s 超时 503 `upstream_unavailable`：

1. **muse-spark-1.3-contributor-free** —— 模型级 `base_url` 指向
   `http://pproxy-host:8899/pony_.../opencode/zen/v1`（path-token 反向路由，
   必须经 pproxy 出海，受地域封锁 403 无法直连）；
2. **space-bunny-free / mimo / step-5** —— provider 级 `egress_pool =
   ["direct", "http://pproxy-host:8899"]` 按 `round_robin` 轮询，pproxy egress
   被选中时超时 503；
3. **antigravity（claude-sonnet-4-6 等）** —— 模型级 `proxy=` 走
   `pproxy-host.ponyllm.svc:8899` CONNECT 隧道。

T1（Lead 已执行）：重启 pproxy 后全链路恢复（accept 队列 0、端到端 3 模型 200、
doctor 10 passed）。

本 ADR 决策范围 = **T2 降爆炸半径**：对未受地域封锁的 zen 免费模型
（space-bunny / mimo / step-5）去掉 pproxy egress，仅保留 `direct`，缩小对
pproxy 的依赖面（pproxy 卡死时不再波及这三个模型）。muse 模型级 base_url 与
antigravity `proxy=` **不在本变更范围**（它们必须依赖 pproxy，属 T3 根治后仍受
保护的对象）。

## Decision

### 1. 仓库 manifest 同步（写域 deploy/）

`deploy/ponyllm-config.example.toml` 的 `[providers.opencode-zen]`：

- `egress_pool` 由 5 条目（direct + 4 个 pproxy 节点条目，2026-10-09 五出口
  核实形态）收敛为仅 `["direct"]`；
- `egress_strategy = "round_robin"` 保留（单条目时无实际分桶语义，但保持与
  live 一致、零迁移）；
- 注释块同步改写：记录 2026-10-10 wave-5 T2 决策动机，保留 2026-10-09 五出口
  核实的历史备忘（CGNAT 校验墙 / allowlist 依赖 / 出口 IP 同 VPS gate / 池只控
  客户端拨号出口）；
- grep 全仓确认无其他 manifest 镜像此段（唯一镜像点即本文件；`crates/**` 中
  `egress_pool` 为测试/代码构造，非部署 manifest）。

### 2. live Secret 更新（真相源，热加载）

- 真相源：Secret `ponyllm-live-config`（ns `ponyllm`），键 `ponyllm.toml` +
  `rotated_at`；网关以 `--config-backend=kubernetes` 启动，
  `config_poller` 每 2s 轮询 `ponyllm.toml` 内容哈希，命中变更即原子重建
  （无需重启 pod，`hot_reload_ms=2000` 已由 admin overview 确认）。
- 操作：改前完整备份 Secret 到 `.dev-team/backups/ponyllm-live-config.pre-t2-*.yaml`；
  `kubectl patch secret ponyllm-live-config --type merge` **仅替换
  `data["ponyllm.toml"]` 单键**（保持 base64），`rotated_at` 与其他键原样保留。
- 载荷 = 现网 toml 全文且**仅删 1 行** `"http://pproxy-host:8899",`（diff 核对
  为 569d568 单行删除；config_version 230 不变）。
- 变更后核对：`kubectl get secret ... -o jsonpath='{.data.ponyllm\.toml}' | base64 -d`
  diff 仅 egress_pool 一行；muse 模型级 base_url（pony_ 段）、antigravity
  base_url/proxy/default_protocol、zen-jev base_url 逐字节未动。

### 3. 验证（机器收据）

- T4b 冻结预检 `tests/pre-flight-config-egress.sh`（test-agent 编写，只读执行）：
  变更前红相 Exit 1（FAIL 1 live egress 含 pproxy + FAIL 3 example 5 条目），
  变更后 **Exit 0（GREEN）**，4 项断言全过：
  1) live egress_pool == ["direct"]；2) muse 模型级 base_url 保留 pony_ 路径；
  3) example 与 live egress_pool 一致；4) antigravity / zen-jev 未误改。
- 热加载生效：admin quota 各 key 行 `egress` 数组由
  `[direct, http://pproxy-host:8899]`（2 条目）变为 `[direct]`（单条目），
  config_version 仍 230 —— Secret 热加载已生效。
- 端到端实测（详见 Consequences 的收据）：antigravity claude-sonnet-4-6 经网关
  200；space-bunny/muse 经网关当前返回 429 —— 原因是 wave-3 起的 zen 免费档
  日窗口 in-memory 冷却（cooldown_reset_at=2026-10-10T00:00:00Z，非本变更引入），
  同一出口直连上游实测 200（上游健康，冷却为网关侧旧状态）。

## Alternatives considered

- **A. 全部模型都去掉 pproxy（muse 也改 direct）**：muse-spark 受上游地域封锁
  （直连 403 `This model is not available in your country`，实测 cf-placement
  remote-ORD），必须经 pproxy 出海；去掉其模型级 base_url 等于下线该模型。
  否决（保留 muse 路径隧道）。
- **B. 仅非封锁模型（space-bunny/mimo/step-5）去掉 pproxy egress**：即本决策。
  这些模型上游不做地域封锁，direct（网关本机 CN 出口）即可达；pproxy 卡死不再
  通过 round_robin 波及它们，降爆炸半径直接成立。采纳。
- **C. 保留现状（["direct","http://pproxy-host:8899"]）**：pproxy 一卡死，
  round_robin 约一半请求命中 pproxy egress 而 10s 超时 503，T1 靠重启 pproxy
  才恢复；不改变即把可用性继续押在单点代理上。否决。
- **D. egress_strategy 改 priority（direct 优先）**：与 A/B 的目标部分重叠，但
  池内保留 pproxy 条目即保留依赖面；且 priority 语义变化属行为变更，本波次
  不做多余动作（最小闭包）。否决（未采纳）。
- **E. 通过 Admin PUT 写配置而非 patch Secret**：wave-3 曾实测 Admin PUT 可写
  并持久于 PVC，但现网真相源已迁移为 Secret kubernetes 后端（config_version
  230 = Secret 内容），PUT 路径与 Secret 轮询可能双写漂移。本变更统一走
  Secret 单键 patch（真相源一致 + rotated_at 保留）。采纳。

## Consequences

- 未受封锁的 zen 免费模型（space-bunny / mimo / step-5）egress 固定 direct，
  不再依赖 pproxy 可用性；pproxy 再卡死时该类请求直接直连上游（CN 出口）。
- muse（pproxy 路径）与 antigravity（proxy=）维持依赖 pproxy，其可用性由
  T3 根治（accept 循环非阻塞 + relay 有界生命周期 + 看门狗）保护。
- `egress_pool` 单条目后 `round_robin` 分桶退化为单桶；如未来恢复多出口需
  重新加入条目（仍受 validate_egress_entry / PONYLLM_PROBE_ALLOWLIST 约束）。
- 验证期噪音：space-bunny/muse 当前经网关 429 系 wave-3 起 zen 免费档日窗口
  in-memory 冷却（cooldown_reset_at=2026-10-10T00:00:00Z），与本次 egress 变更
  无关；2026-10-10 窗口重置后应按 T4b 预检 + 实测 200 复验（复验命令见下）。

## Verification

- 冻结预检（机器）：`bash tests/pre-flight-config-egress.sh` → **Exit 0 / GREEN**。
- Secret diff（机器）：`kubectl -n ponyllm get secret ponyllm-live-config \
  -o jsonpath='{.data.ponyllm\.toml}' | base64 -d | diff - <(kubectl -n ponyllm \
  get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' | base64 -d)`
  （改前快照已备份于 `.dev-team/backups/`；diff 语义 = 仅删 1 行 pproxy 条目）。
- 热加载（机器）：admin overview config_version=230、quota egress 数组仅 direct。
- 端到端（收据）：
  - `claude-sonnet-4-6`（antigravity）经网关 `/v1/chat/completions` →
    **HTTP 200**，`content="agy-ok"`（time=1.06s）。
  - `space-bunny-free` / `muse-spark-1.3-contributor-free` 经网关当前 →
    **HTTP 429**（zen 日窗口冷却，reset 2026-10-10T00:00:00Z，非 egress 故障）；
    同出口直连上游 `space-bunny-free` → **HTTP 200**（上游健康证明）。

## 回滚（恢复原 egress_pool）

```bash
# 1) 从备份还原 ponyllm.toml 并 patch（恢复 ["direct","http://pproxy-host:8899"]）
kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
  | base64 -d > /tmp/ponyllm-current.toml
# 在 [providers.opencode-zen] 的 egress_pool 块中加回 "http://pproxy-host:8899", 条目
#（恢复为两条目形态，其余行不动）
# 2) 单键 merge patch（保留 rotated_at 与其他键）
NEW_B64=$(base64 -w0 /tmp/ponyllm-current.toml)
kubectl -n ponyllm patch secret ponyllm-live-config --type merge \
  -p "{\"data\":{\"ponyllm.toml\":\"$NEW_B64\"}}"
# 3) 校验：仅 egress_pool 两条目；config_version 不变；muse/antigravity 未动
kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
  | base64 -d | sed -n '/egress_pool/,/]/p'
# 4) 预检应回到红相 Exit 1（恢复后 egress 含 pproxy 条目），作为回滚成功信号
bash tests/pre-flight-config-egress.sh; echo "exit=$?"
```
