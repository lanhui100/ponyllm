# Agent Note: 移除全局分流调度策略，Auto 路由接管 + Auto 路由模型顺序 UI 重构

Status: proposed

## Problem

网关此前提供一个「全局分流调度策略」四选一开关（Economy / Speed / Reliable / Balanced），
落地为三层契约：

- 前端 `StrategySection.vue` 的四张策略卡片（`data-testid="strategy-card"`）
- 后端 `GET/PUT /api/admin/strategy`（`StrategyView` / `PutStrategyPayload`）
- 配置字段 `GatewayConfig.default_strategy: GatewayRoutingStrategy`

同时存在第二张卡片「Auto 智能路由优先级与高可用候选」，用于配置 `gateway.auto_models`
有序列表，作为纯 `auto` 请求的选型优先级。

两个决策叠在一起造成三重问题：

1. **概念重叠**：两套「路由优先级」并列在同一 Tab，用户无从判断该调哪个。
   全局策略是「评分权重方向」，auto_models 是「候选集合与次序」，二者正交却呈现为竞争关系。
2. **全局策略正在被架空**：`GatewayRoutingStrategy::from_str` 里
   `"balanced" | "auto" | "b" => Ok(Self::Balanced)` —— `auto` 已经是 `Balanced` 的别名。
   用户面对四张卡片时选 `auto`，实际落到的是 Balanced 评分分支，即"Auto 已经接管"。
   继续暴露四选一只是让人误以为存在四种不同的全局接管方式。
3. **Auto 卡片本身可用性差**：标题冗长（「优先级与高可用候选」重复表达了两件事）；
   顶部冗余渲染「当前网关实时动态首选执行链路」，与下方可编辑列表表达同一事实且随实时状态抖动；
   单行 `p-3` + 24px 序号徽标占用过多纵向空间；排序只能靠 ↑/↓ 逐格点；新增模型要手打模型名，
   打错只有运行时才发现。

## Proposal

### 1. 移除全局分流调度策略（UI + API + 配置字段三层）

- 删 `StrategySection.vue` 的策略卡片、`strategies` 数组、`handleSelect`、
  `currentStrategy` prop、`update` emit。
- 删 `GET/PUT /api/admin/strategy` 两个 handler、`StrategyView`、`PutStrategyPayload`、路由注册与
  utoipa paths 条目；同时从 `auth.rs` 的 compat allowlist 摘除该路径。
- 删 `GatewayConfig.default_strategy`（`ponyllm-config` 与 `ponyllm-server` 两处镜像）。
- `state.rs` 的 `unwrap_or(config.default_strategy)` 改为 `unwrap_or(GatewayRoutingStrategy::Balanced)`，
  即「Auto 即 Balanced」这条既有事实显式化，不再由配置字段隐式承载。
- 删 `OverviewView.strategy`（该字段是全局策略的对外镜像，留着会诱导客户端重新依赖它）。

**明确不删**（名字相近但语义不同，误删即事故）：

- `GatewayRoutingStrategy` 枚举本体与 `sort_candidates_internal` 的全部评分分支 —— 它仍是 auto
  排序与 `x-pony-strategy` 请求头覆盖的实现基础。
- `ProviderConfig.strategy`（`"priority"` 等密钥池调度算法）及其同名辅助函数
  `fn default_strategy() -> String`。`crates/ponyllm-config/src/config.rs` 与
  `crates/ponyllm-server/src/config.rs` **两个文件里都存在这个陷阱**。
- `ProviderConfig.egress_strategy`（`round_robin` / `priority`，出口轮转）。

### 2. Auto 卡片重构

- 标题「Auto 智能路由优先级与高可用候选」→「**Auto智能路由**」。
- 删除「当前网关实时动态首选执行链路」整块及其 `activeModelsOrder` prop。
- 列表标题「自定义优先配置序列表 (上移下移调整优先级)」→「**Auto路由模型顺序**」。
- 行高压缩（`p-3`→`p-1.5`，序号徽标 24px→20px）。
- 新增 HTML5 原生拖拽排序，**同时保留 ↑/↓ 按钮**作为键盘与读屏可达路径。
- 「添加候选」改为弹窗：列出**所有**模型，复选框勾选、**无输入框**，按**勾选次序**追加到列表末尾。
  勾选次序用数组记录（不用 Set，避免渲染 key 不稳定）。

### 3. 不做的事（Non-Goals）

- 不改 auto 排序算法本身（用户明示「auto 目前的实现还不完善」，但那是独立议题）。
- 不引入新前端依赖；弹窗复用现有 `web/src/components/ui/` 与既有 modal 范式。
- 不修 `crates/ponyllm-cli/tests/cli_tests.rs:1115` 的既有编译错误 —— 它来自另一条 in-flight
  工作流，属夹带重构，隔离处理。

## Alternatives considered

**A. 连 `GatewayRoutingStrategy` 枚举与评分分支一并删除。** 否决。该枚举是 auto 排序与
`x-pony-strategy` 请求头的公共实现基础，删除会把改动面从「删一个开关」膨胀为「重写路由核心」，
并使 `sort_candidates_internal` 的四个评分分支失去调用方。在 auto 尚未完备（用户明示）的当下，
拆掉唯一的降级评分体系风险不可接受。

**B. 只删前端 UI，后端接口与配置字段原样保留。** 否决。留着无人调用的 API 与配置字段，
就是留着下一个人重新接线的诱因，属于常载命约第 3 条否定的「以后可能用得上」式怀旧。
用户明确要求「及其相关代码」。

**C. 保留策略卡片但把 Balanced 重命名为 Auto。** 否决。这会让用户以为存在一个「Auto 策略模式」，
而实际上「auto」请求走的是 auto_models 有序列表 + Balanced 评分，两条链路叠在一起会更难解释。
名字的修正应该落在真正承载 auto 语义的卡片上。

**D. 排序改为「只用拖拽、删掉 ↑/↓ 按钮」。** 否决。拖拽对键盘用户与读屏用户不可达，
把唯一的排序手段变成 pointer-only 交互是无障碍回退。保留双通道。

**E. 新增模型弹窗保留搜索框。** 否决。模型集合来自 `useAdminConfig` 已加载的 `models`，
体量在可勾选范围内；搜索框是「用户不知道有哪些模型」这一已被证伪的前提的残留。
用户亦明确要求「不要输入框」。

**F. 勾选后按弹窗原始列表顺序追加。** 否决。用户明示「按照勾选次序加入」——
按原始顺序追加会让「我先勾 A 再勾 B，结果 B 排在 A 前面」，与操作直觉相反。

## Acceptance criteria

- `cargo check -p ponyllm-config -p ponyllm-core -p ponyllm-server --all-targets` → Exit 0
- `cargo test -p ponyllm-config -p ponyllm-core -p ponyllm-server` → 0 failed
- `cd web && npx vue-tsc --noEmit` → Exit 0
- `cd web && npm run lint` → Exit 0（oxlint `--deny-warnings`）
- `cd web && npx vitest run` → 0 failed
- `GET /api/admin/strategy` 与 `PUT /api/admin/strategy` 返回 404，且不出现在 `openapi_json()` 与
  `web/openapi.json` 的 paths 中
- `OverviewView` 响应中不再含 `strategy` 字段
- `crates/ponyllm-cli` 的既有编译错误保持原样（不得被本次变更"顺手修复"）

## Risks

- **同名符号误删**：`default_strategy` 在 config 层有两种完全不同的语义（全局路由策略 vs 密钥池策略），
  且分布在两个 crate 各一份。缓解：契约文件显式标注红线，实施者按行号精确删除。
- **配置向后兼容**：已有部署的 `config.toml` 里若写有 `default_strategy`，serde 未加
  `deny_unknown_fields` 时会被静默忽略（安全）；若将来启用严格模式则会报错（可接受，属显式失败）。
- **拖拽与按钮的状态竞争**：两者共用 `localAutoModels`，若拖拽结束后未清理 `dragover` 高亮态会导致
  视觉残留。缓解：验收测试断言拖拽后的数组顺序而非仅样式。