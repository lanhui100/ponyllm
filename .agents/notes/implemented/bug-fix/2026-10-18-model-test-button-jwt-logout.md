# Agent Note: 修复模型测试按钮在 JWT 管理面下触发登出

Status: implemented

## Problem

`feat(web): add per-model availability test icon button`（2026-10-18）上线后，web 控制台在「治理 → 服务商 → 模型行」点击⚡「测试可用性」按钮会**直接登出**并跳回 `/connect`。

根因链（已定位）：

1. 按钮实现为前端用共享 `alova` 实例直调数据面 `POST /v1/chat/completions`（`adminApi.testModelChat`）。
2. 控制台为管理面登录（JWT，`/api/user/login`），`alova.beforeRequest` 自动携带 `Authorization: Bearer <JWT>`。
3. `crates/ponyllm-server/src/app.rs::auth_middleware` 的 JWT 管理面桥**显式排除 Inference 资源**：
   `is_bearer_scheme && matches!(resource, AdminRead|AdminWrite|TeleFull|TeleSummary|Quota)` ——
   `/v1/chat/completions` 被 `classify_resource` 归类为 `Resource::Inference`，不走 JWT 桥。
4. JWT 随后落入网关 key 家族 `authenticate`（`sk-` 前缀等），验签必然失败 → **401**。
5. `alova.responded.onSuccess` 对任意 401 触发全局单飞 `handleUnauthorizedResponse()` → 清内存态 + 重定向 `/connect`，表现为「点击测试即登出」。

即：会话并未过期，是**数据面 401 被误判为会话失效**。

## Decision

拨测改走**管理面**端点，JWT 桥对 AdminWrite 放行、401 语义回归正确：

- **后端**新增 `POST /api/admin/models/{name}/test?provider=<optional>`（`crates/ponyllm-server/src/routes/admin.rs`）：
  - 写门控与既有 CUD 一致（`check_admin_write_enabled` → 404 admin_write_disabled）；
  - 模型解析语义与 `handle_admin_delete_model` 一致（可选 provider 消歧 → 404 model_not_found / provider_not_found）；
  - 由服务端经该 provider 池/凭据发起真实 1-token 推理（`hi` + `max_tokens=1`，15s 内），H2 egress 门禁（`check_probe_url` + no-redirect probe client）复用 key 拨测路径；
  - 返回 `ModelTestView { success, latency_ms, http_status, error_code, message }`（上游失败 → 200 + success=false，与 key 拨测一致，不因上游失败给 5xx）。
- **前端**（`web/src/lib/adminApi.ts`）：`testModelChat`（直调数据面）删除，新增 `testModel(model, provider)` → `POST /api/admin/models/{name}/test?provider=…`；`ModelSubSection.handleTestModel` 读取 `res.success / latency_ms / message` 反馈 toast；401 仍由 alova 单飞处理（此时登出才是正确语义）。

## Alternatives considered

- **前端改用裸 `fetch` 直调数据面并吞掉 401**：可消除误登出，但生产网关数据面是鉴权模式（Fail-Closed，`sk-` 网关 key 家族），浏览器仅持有管理面 JWT，无数据面凭据 → 拨测必然 401，功能变成「永远不可用」——治标不治本，否决。
- **后端在 JWT 桥上放行 Inference（管理面 JWT 可调数据面）**：扩大管理面 JWT 的攻击面（管理凭据可直接消费上游额度），且改变既有鉴权矩阵契约（auth 冻结测试与契约文档需改），超出最小闭包，否决。
- **复用 key 拨测端点 `POST /api/admin/keys/{id}/test` 并在前端按模型定位 key**：粒度是 key 连通性而非模型可用性，且模型行无 key 映射关系，语义错位，否决。
- **仅前端改用管理面端点但后端不新增（调 provider 级现有接口组合）**：无现成「按模型拨测」管理面接口，仍须新增端点，否决。

## Consequences

- 拨测链路：浏览器 → 管理面（JWT 桥放行，401 语义正确）→ 服务端池凭据 → 上游；真实 1-token 推理按需手动触发，低频可接受。
- 后端新增端点 + OpenAPI 契约 + `admin_write_tests`（写门控矩阵追加、成功、404）与 `admin_contract_tests` 回归；前端 `ModelSubSection.test.ts` 6 条拨测用例改测 `testModel`。
- 机器门禁：`cargo test -p ponyllm-server --test admin_write_tests`、`admin_contract_tests`；web `vitest run`、`typecheck`、`lint`、`build` 全绿后由 Lead 统一提交推送部署，线上复测点击不再登出。