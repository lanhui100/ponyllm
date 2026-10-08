# Agent Note: Web 端模型管理列表徽标轻量化与 Tier 编辑态归一化

Status: implemented

## Problem
1. 模型列表中此前存在过多信息徽标（思考强度、上下文容量、模态图标指示、底层协议、路由优先级、采样率、价格定制、频率限额），导致行内信息过载繁杂。需要仅保留模型的 Tier（主力、轻量、旗舰等）以及免费徽标，且徽标统一改为中性低调的灰色风格（`secondary` 灰色）。
2. 在模型编辑中，先前选中的 Tier（如 Flagship/Standard/Light），再次打开后无法正确高亮或表现为默认值。需要排查是后端未持久化还是前端 bug 并彻底解决。

## Decision
1. **Tier 清空/未高亮问题根因与修复**：
   - 后端成功持久化了 `ModelTier`（如 `ModelTier::Flagship`、`ModelTier::Standard`、`ModelTier::Light`）。
   - 后端 `/api/admin/models` 以及 `/api/admin/providers/{name}/models` 返回的 `ModelView` 中，`tier` 字段是通过 `format!("{:?}", m.tier)` 序列化的 Rust Debug 字符串（即 `"Standard"`, `"Flagship"`, `"Light"`）。
   - 但前端表单按钮选项组绑定的 `MODEL_TIERS` 值采用的是 `Smart`, `Large`, `Fast`；打开编辑方法 `openEditInline` 中直接执行 `form.tier = model.tier || 'Smart'`，导致 `form.tier` 被赋值为 `"Standard"` 或 `"Flagship"` 或 `"Light"`，无法与按钮的 `t.value`（`"Smart"`, `"Large"`, `"Fast"`）严格匹配（`form.tier === t.value` 结果为 `false`），因此按钮组未选中任何高亮项，用户视觉上表现为“清空了”。
   - 解决方案：在前端 `ModelSubSection.vue` 中引入 `normalizeTier`，将任何形式的输入（`Standard`/`Flagship`/`Light`/`Smart`/`Large`/`Fast`/大小写/缩写）统一归一化为表单所需的 `'Smart' | 'Large' | 'Fast'`。
2. **列表徽标清理与灰色化**：
   - 在模型行常显区移除思考强度、上下文、模态指示、协议、优先级、采样价格、频率限额徽标。
   - 保留 Tier 徽标，variant 设为 `secondary`（中性灰色）；
   - 保留免费徽标（通过 `isModelFree(m)` 判断 0 元模型或 `-free` 模型），variant 同样设为 `secondary`（中性灰色）。

## Alternatives considered
- **让后端 ModelView 更改 tier 字符串输出为 Smart/Large/Fast**：会破坏下游依赖或已有 OpenAPI 契约，且前端多服务商与旧数据兼容性不如在前端建立健全的容错归一化。因此选择在前端统一加固 `normalizeTier`。

## 验证
- 原生门禁：`pnpm test`（162 passed）、`pnpm typecheck`（Exit Code 0）、`pnpm lint`（Exit Code 0）、`pnpm build`（Exit Code 0）。
- 后端回归门禁：`cargo test -p ponyllm-server --test admin_write_tests`（15 passed）。
