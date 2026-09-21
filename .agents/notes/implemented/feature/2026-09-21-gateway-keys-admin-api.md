# Agent Note: 网关凭证管理 API（gateway-keys 三端点）

Status: implemented

## Problem

P1 落地了 scoped key（`admin/inference/readonly`）与资源矩阵，但只能靠 CLI
（`ponyllm keys issue|list|revoke`）管理，Web 控制台无凭证治理面：登录后既看不到
现有凭证，也无法签发/吊销，运维被迫回终端，且凭证清单对 inference 登录者
完全不可见（这是特性）但对 readonly/admin 也无 UI（这是缺口）。

## Decision

按契约 `.agents/notes/web-users-api.md`（设计冻结）与
`.agents/notes/web-users-design.md`（前端功能清单）落地三个端点：

1. `GET /api/admin/gateway-keys`（资源类 `AdminRead`）→ `[GatewayKeyView]`：
   只回 `id/scope/prefix/last4/revoked/expires_at/config_version`，
   **永不回明文/盐/哈希**；`classify_resource` 增加该 GET 精确分支
   （否则落入 `AdminWrite` 兜底导致 readonly 误 403）。
2. `POST /api/admin/gateway-keys`（`AdminWrite`）→ 201
   `IssueGatewayKeyResponse{api_key}` 一次性明文 + `Cache-Control: no-store`；
   id 空 400 `invalid_key_id`、scope 非法 400 `invalid_scope`、
   expires_at 非未来 400 `invalid_expiry`、重名 409 `gateway_key_already_exists`；
   签发后同步内存态（同请求即可认证，无"磁盘已发、内存未知"窗口）。
3. `POST /api/admin/gateway-keys/{id}/revoke`（`AdminWrite`）→ 200 幂等
   （重复吊销仍 200，不 409）；未知 id 404 `gateway_key_not_found`；
   吊销即 fail-closed（`authenticate` 对 revoked 直接 Invalid → 401）。

门禁链照抄上游 key CUD：401（凭证）→ 404 门控（`admin_write_disabled`）→
412（`If-Match`）→ 403（作用域不足，middleware 层）→ 业务 400/409。
`GatewayKeyEntry` 新增 `last4`（签发时随 entry 持久化，旧条目反序列化默认
`"****"`——服务端不存明文故无法从哈希反推尾部）。OpenAPI 注册三 handler +
三 schema 并重生成 `web/openapi.json`。

## Alternatives considered

1. **复用上游 key CUD（`/api/admin/keys`）兼管网关凭证**——否决：命名空间隔离律
   要求两套存储/视图分离；上游 `KeySection.api_key` 存明文与 `GatewayKeyEntry`
   只存哈希的持久化语义冲突。
2. **吊销用 `DELETE`**——否决：吊销是状态翻转（留审计痕 + 幂等），DELETE 预留给
   未来硬删除。
3. **列表返回 `key_hash` 供前端比对**——否决：哈希+同行盐即完整离线爆破输入；
   前端无合法需求。改 `last4` 识别。
4. **列表放行 inference**——否决：凭证清单是横向移动侦察输入，矩阵已冻结
   inference 无 `AdminRead`。
5. **签发允许自定义明文**——否决：熵必须服务端保证（P0 弱口令教训）。
6. **仅靠 middleware 分类、不写精确分支**——否决：`GET /api/admin/*` 未知路径
   兜底为 `AdminWrite`，readonly 会被误 403，与矩阵冲突。

## Consequences

- `cargo test -p ponyllm-server`：198 passed / 0 failed（新增
  `gateway_keys_api_tests.rs` 5 用例：签发-列表-吊销全链路 + 作用域矩阵 +
  校验/If-Match + 门控 404 + last4 磁盘兼容）。
- 明文零泄漏已机械验证：列表行字段白名单断言、openapi 无真实 key 前缀命中。
- 遗留：前端治理 Tab 属 task-28；硬删除（DELETE）与审计持久化仍为 P2/P3。
