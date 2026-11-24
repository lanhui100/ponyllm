# Agent Note: 聚合模型（显式优先级链 + 链项冷却回归）

Status: implemented

## Problem

`sense/deepseek-flash` 是当前唯一的免费 DeepSeek Flash 通道，但上游并发限制很低：
限流一触发，下游 Agent（Claude Code / Codex / DSH）不会换模型，而是直接中断任务。
同时 `magpie` 与 `workbuddy` 也提供同一族模型（名称有 `deepseek-v4.1-flash` 及带后缀变体），
可以顶上，但运维上无法表达"先用 sense，撑不住用 magpie，再不行用 workbuddy，
冷却到期后自动回到 sense"这条链。

现有机制已覆盖大部分能力，缺的不是新概念，而是三处断点（均以代码为证）：

1. **同名扩展能覆盖，跨名不能**：`resolve_pinned_targets` 阶段 4b
   （`crates/ponyllm-server/src/state.rs:2836-2893`）按 `ModelSpec.fallbacks`
   （`crates/ponyllm-server/src/config.rs:74-76`）依次解析，但只做**逐字同名**匹配
   （`p_cfg.default_model == fb_model || p_cfg.models.contains(fb_model)`）。
   magpie / workbuddy 的模型名与 sense 不完全一致，只能靠 `canonicalize_model_name`
   写全局别名（已有 free 后缀条目的教训，见
   `2026-10-18-free-suffix-transparent-routing`），且无法把"同一物理名限定到某个 provider"。
2. **冷却只服务于 `auto`**：模型级熔断 `record_model_outage` / `is_model_cooling_down`
   （`state.rs:833-879`）在 chat 路由里被 `parsed.is_auto` 门控
   （`crates/ponyllm-server/src/routes/chat.rs:288` 跳过、`1186` 记录；messages.rs:293/1206、
   responses.rs:274/715 同构）。pinned 或 fallback 链请求既不会记冷却，也不会跳过冷却项，
   于是每次请求都先撞一遍 sense——这正是"并发过低导致中断"的直接原因。
   冷却 TTL 也硬编码为 600s（chat.rs:1192），对免费额度的分钟级恢复过慢。
3. **管理面写读不对称、前端零入口**：`CreateModelPayload` / `UpdateModelPayload` 已接受
   `fallbacks`（`routes/admin.rs:547,608`，写入见 `2718-2720`），但读侧 `ModelView`
   （`admin.rs:206-253`）不回显该字段；前端 `web/src` 全树无 `fallbacks` 引用，
   `web/src/components/governance/ModelSubSection.vue` 也没有编辑控件。
   即：运维只能改配置文件，界面上无法"添加一个聚合模型"。

另有一处与企业既有声明不符，需在本条一并纠正：旧条
`2026-10-04-model-level-fallback-chains-and-disaster-recovery` 声称"默认禁止自动跨计费模式降级"，
但 4b 解析段（`state.rs:2836-2893`）实际只做了 1M 上下文单调性过滤，
**没有**跨计费模式（免费 → 付费）防护。聚合链里 sense 免费、magpie/workbuddy 未必免费，
因此该防护必须显式补上，否则"兜底"会静默产生费用。

## Decision

网关提供一层**聚合模型（aggregate model）**配置，语义是"一个对客户端可见的模型名 + 一条显式优先级链"，
执行层复用既有 `RoutedTarget` 与 failover 循环（零新执行路径）：

1. **配置层**：`GatewayConfig` 顶层新增 `[[aggregate_models]]`，与 `providers` 正交：
   ```toml
   [[aggregate_models]]
   name = "deepseek-free"
   # 顺序即优先级；条目形如 "provider/model"，用于消解跨 provider 命名差异
   chain = ["sense/deepseek-flash", "magpie/deepseek-v4.1-flash", "workbuddy/deepseek-v4.1-flash"]
   cooldown_secs = 90          # 链项失败后的冷却窗口（到期自动回归）
   # 链即授权：链里写了谁就按顺序用谁（含付费项），不设额外开关
   ```
2. **路由层**：`resolve_routed_targets_full`（`state.rs:2350`）在 `parsed.is_auto` 判断旁增加
   分支——请求名命中聚合名时，按 `chain` 顺序解析成有序 `RoutedTarget` 列表并直接返回；
   链项命中失败（provider 无该模型）时跳过并留 WARN，不整体 400。
   跳过 `sort_candidates` 的策略重排，保证"运维写的顺序就是执行顺序"。
3. **冷却与回归**：把"记录冷却 / 跳过冷却项"的门控从 `parsed.is_auto` 放宽为
   `parsed.is_auto || 请求命中聚合模型`；冷却 TTL 取自聚合配置（默认 90s，上限沿用现有 breaker 约束）。
   冷却到期后自动重新把该项排回链首，实现"下次调用回到 sense"。
4. **计费可见性（不设独立开关，2026-11-23 修订）**：链即授权——运维在链里显式写下的项
   （含付费项）就按声明顺序使用。原 `allow_paid_fallback` 开关已移除：它判定的是
   `billing_mode` 配置声明，既挡不住"声明免费、实际收费"的漂移，又对同一意图二次确认。
   成本可见性改由两处承担：链项自身的 `billing_mode`（目标解析与 `aggregate_fallback` 事件）
   与管理面/前端展示；需要止血时直接改链（删/停用付费项），无需额外布尔。
5. **管理面与前端**：
   - 读侧 `ModelView` 补 `fallbacks` 回显，保证"改过能看到"；
   - 新增聚合模型 CRUD（`GET/POST/PUT/DELETE /api/admin/aggregate-models`），
     写操作走既有 `admin_write` 鉴权与 config version 冲突检查；
   - 前端 `AggregateModelSection.vue`（治理页策略页签）增"聚合模型"卡片：provider+model
     下拉添加链项、上移/下移排序、冷却秒数，以及静态"链即授权"提示；
     现有 `fallbacks` 字段也一并补一个有序输入框，消除写读不对称。
6. **可观测性**：沿用既有 `x-ponyllm-served-model` 与 attempt observer；
   聚合链每次降级额外打 `aggregate_fallback` 事件，含链项下标与失败类别。

明确不做：不改 `canonicalize_model_name` 全局别名表；不引入虚拟 provider
（无 key / base_url，会污染 pool、计费与探活语义）。

## Alternatives considered

1. **只扩 `fallbacks` 的同名匹配 + 逐条全局别名**：否决。命名差异每加一个模型就要在
   `canonicalize_model_name` 里手写一条硬编码（`-free` 后缀条目已暴露该不可持续性），
   且无法表达"同一物理名只指向某个 provider"。
2. **直接用 `auto` + `auto_models` 偏好列表**：否决。`auto` 是全局排序
   （`state.rs:3117-3165` 按 健康 / 免费 / 偏好 / tier / priority 多维排序），
   无法表达"固定顺序 + 冷却后回到首位"的链语义，会把价格与 tier 噪声混进确定性序列。
3. **新建虚拟 provider 承载聚合名**：否决。provider 在 `build_egress_pool_for_cfg`、
   `get_or_create_pool`、探活与计费里都假设有 base_url 与密钥，为聚合名伪造一个 provider
   会把改动扩散到池与计费面，成本远高于在配置顶层加一份链定义。
4. **交给下游 Agent 自行重试换模型**：否决。多数 Agent CLI 遇 5xx 直接耗尽重试退出，
   且客户端看不到 provider 维度，无法做"用尽即换、冷却即回"的调度。
5. **仅在客户端侧做多模型轮询**：否决。并发与限流状态是网关侧事实，客户端拿不到，
   会造成多客户端同时撞同一个低频通道。

## Consequences

落地范围与验收结果（2026-11-23 实测收据）：

1. `cargo test -p ponyllm-server --test aggregate_model_tests` 全绿（非零退出即失败），覆盖：
   链序即执行序、链项缺失跳过、冷却期内跳首项、冷却到期回归首项、
   链即授权（付费链项按声明顺序使用，不再有开关剔除）。
2. `cargo test -p ponyllm-server --test model_fallbacks_tests` 与
   `model_fallbacks_e2e_tests` 不回归（既有 pinned fallback 语义不变）。
3. `GET /v1/models` 列出聚合模型名（客户端可发现），且不影响既有物理模型列表。
4. `GET /api/admin/models` 回显 `fallbacks`；聚合模型 CRUD 在 `admin_write_enabled = false`
   时仍返回 404（与现有管理面契约一致）。
5. 前端 `pnpm -C web exec vitest run src/components/governance/AggregateModelSection.test.ts`：
   5/5 通过（新建聚合模型、链序 emit、上移重排、重名阻断、只读冻结）。

实测收据（全部 exit 0，2026-11-23）：

```text
cargo test -p ponyllm-server --test aggregate_model_tests                              # 9 passed
cargo test -p ponyllm-server --test model_fallbacks_tests \
  --test model_fallbacks_e2e_tests --test request_routing_tests                        # 3 + 1 + 36 passed
pnpm -C web exec vue-tsc --noEmit                                                      # 无输出
pnpm -C web exec vitest run src/components/governance/AggregateModelSection.test.ts    # 5 passed
bash .agents/skills/write-adr/verify-note.sh                                           # 机械判据全通过
```

落地位置：`crates/ponyllm-config/src/config.rs`（`AggregateModelSpec`）、
`crates/ponyllm-server/src/config.rs`（`GatewayConfig.aggregate_models`）、
`crates/ponyllm-server/src/state.rs`（`resolve_aggregate_targets`：物理名优先、链序即执行序、
冷却跳过、付费项防护、1M/模态过滤）、`crates/ponyllm-server/src/routes/admin.rs`
（`AggregateModelView`、GET/PUT `/api/admin/aggregate-models`、`ModelView.fallbacks` 回显）、
`crates/ponyllm-server/src/routes/{chat,messages,responses}.rs`（聚合链请求跳过 quota 边界截断）、
`web/src/components/governance/AggregateModelSection.vue` 与治理页策略页签挂载。

### 残留风险

1. **首帧提交后不可切**：流式响应一旦下发首字节即锁定来源，兜底只能发生在首帧之前；
   因此 sense 的首包超时预算必须显式小于整体请求预算，否则"链"救不回已经中断的 Agent。
2. **多节点冷却态各自为政**：`record_model_outage` 是进程内状态，多副本下不同节点
   对 sense 的冷却判断不同，会出现同一分钟部分节点回撞。若要求全局一致，需额外的
   fleet 级共享（超出本条范围，仅记录）。
3. **能力不一致导致的静默降级**：三家的 `context_window` / `max_output` / thinking 档位不同，
   长上下文请求在切到后备项时可能被 `apply_thinking_output_safeguard` 夹取或直接过滤，
   表现为"兜底没兜住"。链配置需约束同级能力，运行期保留 1M 单调性过滤。
4. **计费口径漂移**：`billing_mode` 是配置声明，免费档随时可能转付费。链即授权意味着没有
   "一键止血"布尔，止血手段是编辑链（删/停用付费项）；"声明为 Free 实际已收费"的漂移
   仍只能靠配额观测发现，不属本条可解。
5. **前端新增写面**：聚合链是跨 provider 的结构化写操作，需沿用既有 config version
   冲突检查，避免与 TUI / 配置文件并发写入互相覆盖。
