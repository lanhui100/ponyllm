# Agent Note: Web 模型行新增「测试可用性」纯图标按钮

Status: implemented

## Problem

Web 控制台的模型管理（`ModelSubSection.vue` 每个模型行）只提供「编辑」与「删除」两个操作，运维人员无法在控制台内快速验证某个模型的真实可用性（网关是否能走通该模型的上游调度）。调试模型参数变更、排查模型不可用问题（上游 key 失效 / 模型名变更 / 代理异常）时，只能跳到第三方客户端或用 curl 手动打数据面，链路长且无法直观看到时延反馈。

## Decision

在 `web/src/components/governance/ModelSubSection.vue` 的每个模型行 action 区（「编辑」与「删除」之间）新增一个纯图标测试按钮：

- 纯图标按钮 `UiButton variant="ghost" size="icon"`，图标用 `zap`（⚡，语义=快速拨测），hover 色 `text-sky-600/hover:bg-sky-50`，带 `UiTooltip content="测试模型可用性"` 与 `aria-label`，`data-testid="test-model-btn"`。
- 点击后由前端**直调数据面** `POST /v1/chat/completions`（复用 `alova` 实例，自动携带当前会话 Bearer 凭据），极小载荷：`{ model: m.name, messages: [{ role: 'user', content: 'hi' }], max_tokens: 1, stream: false }`，配置 15s 超时（对齐网关默认 20s 预算内的保守拨测）。
- 拨测进行中：该按钮图标切换为 `refresh` 并 `animate-spin`，`disabled` 防重复点击；完成（成功/失败）后恢复。
- 结果反馈走既有 `toast`：成功 → `toast.success("模型 "name" 可用 (xxx ms)")`；失败 → `toast.error`（含后端错误信封 message，401 单飞路径由 `alova` 统一处理，不在此处重复弹错）。
- 与既有「编辑/删除」一致遵循 `adminWriteEnabled`：只读态下按钮禁用（拨测属管理动作）。

后端零改动：复用网关既有数据面端点，不新增管理接口（用户在需求澄清中明确选择「前端直调 /v1/chat/completions」方案）。

## Alternatives considered

- **新增后端管理拨测端点 `/api/admin/models/{name}/test`**：能力更强（可做 key 级拨测、结构化结果），但需要新增 Rust 路由 + OpenAPI 契约 + 鉴权 + 后端测试，改动面大；且管理面拨测最终也是转发到上游数据面，与前端直调结果等价。本次用户选择前端直调，否决。
- **调用 `GET /v1/models/{name}` 作为可用性探针**：只能证明「模型在路由字典中存在」，无法证明上游真实可达（key 失效/上游 500 时 `/v1/models` 仍 200），是假健康信号，否决。
- **直接复用 key 拨测 `POST /api/admin/keys/{id}/test`**：粒度是「key 连通性」而非「模型可用性」，且需凭 key id 定位，与模型行语义不符，否决。
- **用 `showConfirmDialog` 先弹确认**：拨测是零副作用只读操作，弹确认增加操作成本，与「测试 key 连通性」按钮（`KeySubSection.vue` 直接点击）不对称，否决。
- **不做任何反馈仅图标翻转**：用户无法得知拨测结果（成功/失败/时延），可用性验证诉求落空，否决。

## Consequences

- 模型行 action 区变为「测试 / 编辑 / 删除」三按钮，与 `KeySubSection.vue`「编辑 / 拨测 / 删除」的模式一致。
- 每次拨测产生一条真实推理请求（1 token），会消耗上游 token 费用——与既有 key 拨测（`GET {base}/models` 探测）不同，本按钮按需手动触发，低频可接受；文档在按钮 Tooltip 中注明「真实调用 1 token」语义（content=hi, max_tokens=1）。
- 前端单测：`ModelSubSection.test.ts` 新增用例断言按钮渲染、点击后发起 `/v1/chat/completions` 拨测、成功/失败 toast 反馈、进行中禁用与 spinner 态。
- 机器门禁：`cd web && npx vitest run`、`npm run typecheck`、`npm run lint` 全绿。