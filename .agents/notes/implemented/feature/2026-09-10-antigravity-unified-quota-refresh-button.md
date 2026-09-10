# Agent Note: Antigravity 密钥统一刷新配额用量交互重构

Status: implemented

## Problem

在 Web 端模型管理（`GovernanceView`）中，对于 Google Antigravity 服务商，此前在每个密钥行（Key Row）的右侧都放置了一个单独的刷新查询配额用量图标按钮。当用户配置了多个 Antigravity 账号密钥时，界面存在如下痛点：
1. **视觉冗余与杂乱**：每个密钥行都有独立的刷新按钮，加上可能还有删除按钮，使得列表右侧按钮密集，重复度高；
2. **多账号操作繁琐**：用户想要更新 Antigravity 提供商下所有账号的额度与重置时间时，必须手动逐个点击每一个密钥行的刷新按钮；
3. **交互层级不符合直觉**：配额查询属于针对该提供商下全体已授权账号的健康与用量概览，将刷新操作收拢至“密钥”标题行（与“授权账号”、“+ 密钥”同级）能够让层级更统一清晰。

因此需要将各密钥条目的单个刷新图标按钮移除，改为在密钥标题行（Header）提供一个统一刷新图标按钮，点击后统一刷新该 Antigravity 服务商下的所有密钥配额用量。

## Decision

1. **统一刷新按钮上移至密钥标题行（`web/src/components/governance/KeySubSection.vue`）**：
   - 移除各密钥行（`key-row`）中属于 Antigravity 专用的单个刷新按钮（`:data-testid="\`refresh-key-quota-${k.id}\`"`）；
   - 在密钥标题栏（Header 操作区，位于“授权账号”或“+ 密钥”旁边）新增一个纯图标刷新按钮（`data-testid="refresh-antigravity-quota-btn"`），仅在 `isAntigravity` 为 true 且已有密钥时显示；
   - 按钮具备状态感知：当正在刷新时展示旋转加载动画（`animate-spin`），并在处于只读模式（`!adminWriteEnabled`）或正在刷新时处于 disabled 状态；Tooltip 提示文案明确为“刷新全部账号配额用量”或“正在刷新配额用量...”。
   - 提供 `handleRefreshAll` 方法，对当前 Antigravity 服务商的所有密钥执行并发/批处理拨测刷新，并触发 `test-batch-keys` 或逐一触发 `test-single` 事件，并在完成时通过轻量 Toast 提示更新结果。

2. **保留通用提供商与 Antigravity 的职责分明**：
   - 非 Antigravity 的普通提供商仍可按需测试单密钥连通性（若需要），而 Antigravity 彻底统一为密钥行统一刷新配额用量，消除了单个刷新按钮带来的界面割裂。
   - 保留各账号密钥行原有的 Gemini / Claude 紧凑胶囊双进度条展示，当统一刷新完成后各行数据即时响应式更新。

3. **测试用例同步适配（`web/src/components/governance/ProviderCard.test.ts`）**：
   - 更新 Antigravity 刷新测试，断言各密钥行不再渲染单个刷新按钮；
   - 断言密钥标题行正确渲染统一刷新按钮，点击该统一刷新按钮后可正确触发全部密钥的刷新逻辑并更新双进度条胶囊。

## Alternatives considered

- **方案 A：保留单个刷新按钮，同时在标题行添加统一刷新按钮**：虽然兼顾单刷新与批刷新，但违反了用户“不再需要单个刷新了”的明确要求，且无法解决列表视觉冗余问题，否决。
- **方案 B：将刷新按钮移至服务商卡片最外层头部**：外层卡片头部已有较多状态徽标和操作，且刷新操作仅针对“密钥”及其配额用量，将其放在“密钥”标题行语义更精确且不侵入全局模型卡片头部，否决。
- **方案 C：在密钥标题行放置统一刷新图标按钮，彻底移除各行单个刷新按钮（采纳）**：视觉最简洁、语义最清晰、操作最便捷，完全契合需求。

## Consequences

- Antigravity 密钥列表视觉更清爽整洁，消除重复图标冗余；
- 用户一键即可完成该 Antigravity 服务商下所有账号的额度用量刷新，交互体验大幅提升；
- 自动化测试与质量门禁保持 100% 绿灯。
