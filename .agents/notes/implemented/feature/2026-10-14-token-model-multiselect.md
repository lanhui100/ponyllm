# Agent Note: token model allowlist multiselect (web 新增 Token 弹窗)

Status: implemented

## Problem

Web 控制台「我的 Token → 新增 Token」弹窗的「允许模型」字段原为自由文本输入框，要求用户手工输入模型 id 并用英文逗号分隔（`web/src/views/TokensView.vue` 原 `token-models-input`）。存在三大缺陷：
1. **不可发现**：用户无法得知系统当前提供哪些可用模型 id，极易拼错产生静默 403。
2. **易错**：逗号分隔无校验、去重与格式提示。
3. **数据源缺失**：用户面（JWT plane，`/api/user/**`）此前无任何只读模型清单端点。

## Decision

1. **新增用户面只读端点 `GET /api/user/models`**（`crates/ponyllm-server/src/routes/user.rs`，挂载入 `user_routes()`）：
   - 200 返回 `{"models":[{"id","provider","display_name"}]}`，顺序对齐 `AppState::list_all_models()` 原生顺序；
   - 严格复用 `state.user_tracker.check_access(uid, id)` 实施模型权限过滤，与推理面鉴权同源同语义，零解析漂移；
   - 鉴权由既有中间件把守，无凭据/坏凭据统一返回 401。
2. **前端 API 封装 `listAvailableModels()`**（`web/src/lib/userApi.ts`）：
   - 调用 `GET /api/user/models` 携带 JWT Bearer 凭据；
   - 畸形/缺失载荷严格归一化为 `[]`，不崩溃且非 2xx 抛出携带 code/status 的 Error。
3. **TokensView 多选下拉**（`web/src/views/TokensView.vue`）：
   - 替换文本输入为自包含多选下拉组件，支持触发器、搜索框、选项列表（`role="option"`）、已选 chips 及移除操作；
   - 落地 `modelsInFlight` 单飞复用与 `modelsReqSeq` 代次守卫，防止开窗与挂载并发引起的陈旧响应覆盖；
   - 下拉浮层采用向上展开（`bottom-full mb-1`）与高度限制（`max-h-48`），彻底解决卡片 `overflow-hidden` 导致的裁切与点击不可达问题；
   - 提交契约对齐：选中 ≥1 项输出数组，0 项输出 `undefined`。
4. **优雅降级**：模型列表加载失败时不阻塞弹窗与提交，支持用户留空提交全部模型并提供重试入口。

## Alternatives considered

- **复用 `GET /api/admin/models` 或 `GET /v1/models`**：否定。二者分别要求 admin key 或推理面 key，普通用户仅持有 JWT，跨鉴权面引入次级 Key 破坏凭据隔离与单头 Bearer 契约。
- **在 `GET /api/user/me` 响应中附带模型列表**：否定。破坏现有只读契约，将轻量会话与资源清单耦合。
- **手写通配逻辑替代 `check_access`**：否定。两套解析器存在天然漂移风险，易导致“可选但实际 403”的越权或旁路缺陷。
- **前端本地静态写死模型列表**：否定。无法响应动态热载与上游变更。
- **通用 `UiMultiSelect.vue` 抽象**：否定。目前单点需求，遵循最小闭包与防夹带重构，暂不抽象全局组件。

## Verification

1. **Rust 契约与对抗测试**：`red_user_models_api_tests` 8/8 通过，`adversarial_user_models_tests` 50 路并发一致性测试 Exit Code 0。
2. **Web 单元与回归测试**：`TokensView.models-race.test.ts`（单飞代次回归测试）、`TokensView.test.ts`、`userApi.models*.test.ts` 全部全绿（34/34 passed），全量 Web 测试 238/238 passed。
3. **类型与代码风格门禁**：`vue-tsc --noEmit` 与 `oxlint` 全部 Exit Code 0。
4. **真实浏览器端到端 E2E (agent-browser)**：全链路真实表单登录、弹窗展开、9 项模型 0 裁切全部可命中、真实鼠标多选、提交创建成功并回验服务端 `model_limits` 一致，控制台错误 0，截图收据归档于 `docs/release/`。
