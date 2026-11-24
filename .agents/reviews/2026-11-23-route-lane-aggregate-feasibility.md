# 可行性评估：新增「聚合模型 / 路由链（lane）」——自定义跨 provider 优先级组合

Date: 2026-11-23
Status: assessment（未实施；结论待裁决）
Scope: `ponyllm-config` / `ponyllm-server` 路由解析层 + 模型面契约
前置：`.agents/reviews/2026-11-23-deepseek-flash-lane-feasibility.md`（同一目标的缺口 G1–G10）

## TL;DR

- **能，而且主改动集中在一个函数**：`AppState::resolve_routed_targets_full`（`state.rs:2350`）是五个协议的**唯一解析入口**，链在解析层展开即可被现有 `for target in targets` 循环消费，不必改计费/池/探活。
- **不要做成"新 provider"，要做成"一等路由链（lane）"**：新 provider 会被 `build_egress_pool`、`register_pool`、quota probe、`/v1/models` 全部按 provider 名遍历，语义污染面远大于收益。
- **聚合模型只解决"失败后换路"，不解决"sense 并发低"本身**：没有 provider 级 in-flight 门（G3）时，饱和仍表现为排队→超时→切换，agent 中断照旧。**两条必须同批做**。
- **真正的摩擦点不是代码而是模型面契约**：`wave-8-model-surface.md` C2-02 要求非 `auto` 的 id 首段必须是 provider 名、C3-01 要求 `auto` 恰在索引 0 —— lane 名要么不进 `/v1/models`，要么先改契约。
- 规模估计：**L1 MVP 0.5–1.5 人日**（解析 + 配置双结构 + 3 处闸门 + 测试）；L2（健康熔断/quota 溢出/观测）再 1–2 人日。

## 一、现成地基（可直接复用，勿重写）

| # | 能力 | 证据 | 复用方式 |
|---|---|---|---|
| 1 | 单点解析入口，多协议共享 | `state.rs:2350`；5 处消费者 `chat.rs:152/267`、`messages.rs:161/272`、`responses.rs:140/253`、`images.rs:269/348`、`systemone.rs:62/91` | lane 解析插在 `is_auto` 分支旁（`state.rs:2371`） |
| 2 | `RoutedTarget` 已承载 provider/physical/协议/定价/thinking/容量 | `state.rs:2502-2550` | 完全复用，无需新结构 |
| 3 | 显式顺序链的既有尝试 | `ModelSpec.fallbacks`（`config.rs:74`）+ 广度展开、visited 去重、`depth<3` 防环（`state.rs:2780-2896`） | 语义可借鉴，**但顺序不可复用**（见障碍②） |
| 4 | 模型级熔断 + 半开 | `record_model_outage` / `is_model_cooling_down`（`state.rs:833/871`） | 泛化门（G4）即可，无需新建状态 |
| 5 | 配置热加载 + 版本 + Admin 写 | `config_version`、`ConfigStore::save`、`If-Match`、模型 CRUD 已含 `fallbacks`（`admin.rs:547/2718`） | lane 走同一通道，无需新机制 |

## 二、三个真实障碍（按代价排序）

### ① 模型面契约（最贵，且是裁决点不是编码点）

- `wave-8-model-surface.md` C2-02：每个非 `auto` 条目 id 一定含 `/` 且首段 ∈ provider 名集合；C2-07 禁止裸 id；C3-01 要求 `auto` 恰一次且在索引 0。
- 因此 `deepseek-flash-lane`（裸名）**不能**出现在 `/v1/models`；`ponyllm/deepseek-flash-lane` 也不是合法 id（`ponyllm` 不是 provider）。
- 三条出路：
  1. **不进公开模型面**：lane 仅可由 `ponyllm/<lane>` 或管理台直连触发，`/v1/models` 零改动（MVP 推荐，契约零风险）。
  2. 把 lane 当 pseudo-provider 暴露 → 需改 C2/C3 + `owned_by` 语义 + 前端模型选择器（约 +1 人日）。
  3. 复用 `auto`（否决，见 §四）。

### ② 排序会吃掉"优先级"语义

- 非 auto 路径无条件 `sort_candidates`（`state.rs:2822/2828`），主键是 `ModelSpec.priority`（`state.rs:3160`），次级是 Balanced 价格/延迟打分（`state.rs:3030-3117`）。
- 三个 provider 的同名模型如果 priority 相同，**声明顺序不保证**；靠 priority 调序则配置漂移后不可读。
- 结论：lane 分支必须**显式短路排序**（按声明序返回），不能靠 priority 模拟。这是唯一的"必须新增分支"，也是它区别于 `fallbacks` 的核心。

### ③ 三个闸门写死 `parsed.is_auto`

- 冷却跳过：`chat.rs:288` / `messages.rs:293` / `responses.rs:274`。
- 熔断写入：`chat.rs:1186/1700`、`messages.rs:1206/1685`，且仅 `ModelNotFound`（`executor` 层 429/超时不写）。
- quota 边界：`chat.rs:252` = `parsed.is_auto || config.cross_provider_quota_failover`；`cross_provider_quota_failover=false` 时 `last_kind.is_quota_exhausted()` 直接 `break`（`chat.rs:299-312`）——**sense 若回 quota 文案，第二跳永远不会被尝试**。
- 另有一处放大器：`x-ponyllm-fallback-triggered` 以"物理名 ≠ 请求名"判定（`chat.rs:1114` 等 4 处），lane 名必然误报。

## 三、建议设计（最小语义集）

```toml
# config.toml —— 与既有 gateway.auto_models 同级
[[gateway.lanes]]
name = "deepseek-flash-lane"      # 逻辑名，与任何物理模型名冲突时 fail-fast
allow_quota_spill = true          # 默认 false；免费→免费才允许
[[gateway.lanes.targets]]
provider = "sense"
model = "deepseek-flash"
[[gateway.lanes.targets]]
provider = "magpie"
model = "trae-cn/deepseek-v4.1-flash-max"
[[gateway.lanes.targets]]
provider = "workbuddy"
model = "deepseek-v4.1-flash"
```

语义（每条都有对应可测断言）：

1. **严格保序**：解析结果顺序 = 声明顺序，不参与 strategy 打分、不被 priority 重排。
2. **健康跳过 + 半开**：命中冷却 TTL 的 target 直接跳过；TTL 到期后首个请求回主道，成功即清（复用 `is_model_cooling_down`，TTL 建议 30–60s 而非当前 600s）。
3. **链内 quota 溢出按链放行**：`allow_quota_spill = false` 时保持全局现状（遇到 quota 即停），`true` 时链内继续下一跳；**跨计费方向默认禁止**（免费→付费需显式 `allow_paid_spill = true`）。
4. **配置校验 fail-fast**：target 的 `provider` 不存在、`model` 不在该 provider 的 `default_model ∪ models`、lane 名与物理模型/`auto` 重名 → 载入失败（含热加载），不静默降级。
5. **归因**：响应头增 `x-ponyllm-lane` + `x-ponyllm-chain-step`；`x-ponyllm-fallback-triggered` 改为"实际 target ≠ 首个 target"判定。

配置面需同步两处结构：`ponyllm_config::GatewaySection`（`config.rs:43`）与 `ponyllm_server::GatewayConfig`（`config.rs:469`），并补迁移/回环测试（旧 TOML 无 `lanes` 必须零变更）。

## 四、Alternatives considered

- **新建一个聚合 provider（如 `[providers.lane]`）**：否决。`KeyPool` 注册、`build_egress_pool_for_cfg`、quota probe、计费模式、`/v1/models` 全部以 provider 为单位；假 provider 会污染配额与账务，且 target 仍要二次路由。
- **复用 `auto`（把 lane 塞进 `auto_models`）**：否决。auto 的排序键是 `unhealthy → free → auto_models 序 → tier → priority`（`state.rs:3119-3158`），会让价格/tier 参与决策；且 `auto` 语义是"任意可用模型"，与"同一逻辑模型的固定三跳"冲突。
- **只靠 `ModelSpec.fallbacks` 拼链（现状）**：可作为 0 代码止血，但不满足需求：primary 恒在 fallback 前（`state.rs:2887-2896`），裸名会被付费同名通道插队（G2），且排序仍走 priority + 打分。
- **靠 `priority` 排序表达优先级**：否决。priority 是全局每模型字段，三跳要跨模型名，且同 priority 无稳定语义；配置漂移不可审计。
- **做 provider 级并发门之前先上 lane**：否决为先手。lane 只换"失败后"，不解决"失败前排队"；两者同批才有用户可感收益。
- **把 lane 暴露进 `/v1/models`（MVP）**：推迟。需改模型面契约（C2-02/C2-07/C3-01）与前端选择器，成本大于路由本身。

## 五、落地分层与验收

| 层 | 内容 | 估计 | 覆盖目标 |
|---|---|---|---|
| L0 | 纯配置止血：sense 加 `fallbacks` + `ttfb_timeout_secs` + `rate_limits`；`pre_commit_total_timeout_secs ≈ 150` | 0.5h | ~70% 提交前切换 |
| L1 | lane 解析 + 配置双结构 + 3 处闸门泛化 + fail-fast 校验 + 解析层测试 | 0.5–1.5 人日 | 严格三跳保序、不被付费插队 |
| L2 | provider 级 `max_in_flight`（RAII guard，饱和即跳过）、熔断 TTL 泛化到 429/TTFB、按链 quota 溢出、`x-ponyllm-lane/step` + per-provider 指标 | 1–2 人日 | 并发低不再中断；可观测 |
| L3 | 会话粘性（tool-loop 锁后端）、链内容量守卫（token 估算）、首帧缓冲 | 2–3 人日 | 提交后中断的残余 |

验收（机器可查，非零退出即失败）：

```bash
# 1) 解析层：lane 严格保序 + 不参与打分 + 冲突 fail-fast + 旧配置零变更
cargo test -p ponyllm-server --test request_routing_tests --test model_fallbacks_tests

# 2) 路由层：三跳失败转移（mock 上游依次 429/timeout/200）
cargo test -p ponyllm-server --test model_fallbacks_e2e_tests

# 3) 配置回环：旧 TOML（无 lanes）序列化后字节等价
cargo test -p ponyllm-config

# 4) live 归因（落点必须属于声明链，且 step 单调）
curl -s -D- -o/dev/null -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"model":"ponyllm/deepseek-flash-lane","messages":[{"role":"user","content":"ok"}],"max_tokens":8}' \
  https://tokens.ponyjob.top/v1/chat/completions | grep -iE 'x-ponyllm-(provider|lane|chain-step)'
```

## 残留风险（靠 review / 观察）

1. 五处循环同构（chat/messages/responses/images/systemone）：闸门泛化若只改 3 处，images/systemone 行为不一致；应显式声明"L1 只覆盖主协议"并加注释，或抽共享 helper（建议 L2 附带）。
2. 同名不同体：链内 thinking 默认档不同（off vs high），多轮工具循环切换会改变行为；无会话粘性前靠 review。
3. 容量单调性只覆盖 `[1m]` 标签（G7）：>256k 从 sense 溢出到 workbuddy 可能 400。
4. 上游模型 id 漂移（`trae-cn/...-max`）：fail-fast 校验能把"静默换道"变成载入期报错，但仍需定期 config 巡检。
5. `pre_commit_total_timeout_secs` 注释写 140s、代码回落 75s 的文档漂移（`config.rs:491`）会直接决定三跳是否被切片误杀，需随 L1 一并修正并加测试。
