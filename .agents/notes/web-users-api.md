# 网关凭证管理后端 API 设计（gateway-keys）

日期： 2026-09-21 ｜ 状态： 设计（只写文档不写代码，实现任务照此开工） ｜ 输入： 前端用户管理页设计（独立成篇以前端为准）、`auth-eval.md` §3 冻结契约、P0（task-20）/P1 鉴权（task-21，`auth.rs` 已落地）现状

## 结论

- 新增三端点（全部 `AdminWrite` 资源类，走既有 `auth` 中间件 + handler 内门控双层）：
  - `GET /api/admin/gateway-keys`——凭证列表（永不含明文/哈希，只回 `id/scope/prefix/尾4位/revoked/expires_at`）。
  - `POST /api/admin/gateway-keys`——签发（201，一次性明文 + `no-store`，沿用 `CreateKeyResponse` 范式）。
  - `POST /api/admin/gateway-keys/{id}/revoke`——吊销（200，幂等；吊销即 401 fail-closed，见 `auth.rs:178`）。
- 并发与门禁照抄上游 key CUD 范式：`check_admin_write_enabled`（404 `admin_write_disabled`）→ `admin_write_lock` → `load_store_config` → `check_if_match`（412 `precondition_failed`）→ 改 `file.gateway.gateway_keys` → `save_store_config`（`config_version+1`）。
- 403 矩阵：签发/吊销是 `AdminWrite`——`inference`/`readonly` 调用一律 403 `forbidden`（`auth.rs:scope_allows` 已实现，设计只列矩阵不断言新逻辑）；`GET` 列表是 `AdminRead`——`inference` 403、`readonly` 放行（见矩阵表）。
- OpenAPI 同步点：`AdminApiDoc.paths` + `components.schemas` 注册三视图/两载荷，`web/openapi.json` 用 dump helper 重生成（`admin_contract_tests::openapi_dump`），`test_openapi_no_real_secret` 断言 `sk-pony-(admin|infer|read)-` 真密钥零命中（示例只用占位前缀）。

## 端点明细

### 1. `GET /api/admin/gateway-keys` → 200 `[GatewayKeyView]`

- 资源类：`AdminRead`（`classify_resource` 需新增精确匹配分支，`GET /api/admin/gateway-keys` → `AdminRead`；注意现状 `GET /api/admin/*` 未命中精确表即 `AdminWrite`——实现时必须加精确分支，否则 readonly 被误 403）。
- 鉴权矩阵：admin 放行；readonly 放行；inference 403（无管理读口，防 agent 窥视凭证清单）；未认证 401（顺序铁律：401 → 门控 404 → 403）。
- 响应视图（`GatewayKeyView`，只读投影，**永不含 `key_hash`/`salt`/明文**）：

```json
{
  "id": "agent-ci-1",
  "scope": "inference",
  "prefix": "sk-pony-infer-",
  "last4": "9a70",
  "revoked": false,
  "expires_at": null,
  "config_version": 42
}
```

- `last4` 来源：服务端不存明文故无法从哈希反推——签发时把尾 4 位随 entry 一并持久化（`GatewayKeyEntry` 加 `last4: String` 字段，旧条目缺字段反序列化默认为 `"****"`；磁盘格式变更，属实现任务内容，设计先行声明）。
- 错误：401（同现状信封）/ 403 `{"code":"forbidden"}` / 门控 404（`admin_write_enabled=false` 时连读口一并 404，与 `GET /api/admin/keys` 现状一致——读口同样受写通道门控，见 `handle_admin_keys` 前置 `check_admin_write_enabled`）。

### 2. `POST /api/admin/gateway-keys` → 201 `IssueGatewayKeyResponse`

- 资源类：`AdminWrite`。载荷（`IssueGatewayKeyPayload`）：

```json
{ "id": "agent-ci-1", "scope": "inference", "expires_at": null }
```

- 校验（400 `invalid_key_id` / 409 `gateway_key_already_exists`，照抄 `handle_admin_create_key` 的 id 非空 + 重名检查，`admin.rs:2148-2162`）：
  - `id` 去空后非空（`trim` 为空 → 400）。
  - `id` 在 `file.gateway.gateway_keys` 内唯一（重名 → 409）。
  - `scope` 三值 `admin|inference|readonly`（大小写不敏感归一；他值 → 400 `invalid_scope`）。
  - `expires_at` 若提供必须为未来 UNIX 秒（过去时间 → 400 `invalid_expiry`）。
- 签发：`generate_scoped_gateway_key(id, scope)`（`ponyllm-config:167`，uuid 熵 + 前缀 + salt 哈希）；`expires_at` 写入 entry；`last4 = plaintext[^4..]` 写入 entry；`save_store_config` 后返回：

```json
{
  "id": "agent-ci-1",
  "scope": "inference",
  "api_key": "sk-pony-infer-<uuid32>（明文，仅此一次）",
  "expires_at": null,
  "config_version": 43
}
```

- 一次性明文规则（照抄 `CreateKeyResponse` 范式，`admin.rs:2201-2223`）：响应头强制 `Cache-Control: no-store` + `Pragma: no-cache`；服务端只存哈希+salt，**绝不落明文**；`tracing::info!` 只记 `key_id/scope` 不记 key（`sanitize_key` 也不用——连掩码都不进日志）。
- 错误顺序：401 → 404 门控 → 412（缺 `If-Match` / 版本冲突）→ 400/409 业务校验。

### 3. `POST /api/admin/gateway-keys/{id}/revoke` → 200 `GatewayKeyView`

- 资源类：`AdminWrite`。语义：`entry.revoked = true`（软删除，保留审计痕迹；`authenticate` 对 revoked 直接 `Invalid` → 401 fail-closed，无需重启，内存态随热重载/下次 `reload_config_with_pools` 生效——实现任务需确认内存 entry 与磁盘 entry 的同步点，设计声明要求：revoke 响应返回前必须同步内存态，不允许"磁盘已吊销、内存仍放行"窗口）。
- 幂等：对已吊销 id 重复 revoke 返回 200（同视图，`revoked:true`），不 409。
- 不存在 id → 404 `gateway_key_not_found`（注意与门控 404 码区分：`admin_write_disabled` vs `gateway_key_not_found`）。
- 为什么是 POST 而非 DELETE：吊销是状态翻转不是删除（审计要留痕）；DELETE 留给 P2 的"彻底删除"（硬删除 entry，另行设计，不在本篇）。

## 403 矩阵（实现照抄 `scope_allows`，设计冻结调用结果）

| 调用者＼端点 | `GET /gateway-keys` (AdminRead) | `POST /gateway-keys` (AdminWrite) | `POST /…/revoke` (AdminWrite) |
|---|---|---|---|
| admin（含 legacy 映射） | 200 | 201 | 200 |
| inference（agent key） | **403**（无管理读口） | **403** | **403** |
| readonly（viewer key） | 200 | **403** | **403** |
| 未认证/过期/吊销 | 401 | 401 | 401 |

- 403 信封（`auth.rs:forbidden` 现状，一字不改）：`403 {"error":{"message":"insufficient scope for <resource>","type":"insufficient_scope","code":"forbidden"}}`。
- 顺序铁律（`auth.rs:1-7` 注释 + `app.rs:182`）：401 → 门控 404 → 403。`admin_write_enabled=false` 时非 admin 调用先见门控 404（不是 403）——与上游 key CUD 现状一致。
- 人机隔离（契约 §3.2）：机器 key 永不签发 `operator`；`admin` 作用域 key 可调 rotate（现状 `handle_admin_auth_rotate` 无 scope 门——实现任务需补 `AdminWrite` 鉴权，设计声明要求不遗漏）。

## OpenAPI 同步点（实现 checklist）

1. `AdminApiDoc` 的 `paths(...)` 追加三 handler；`components(schemas(...))` 追加 `GatewayKeyView`、`IssueGatewayKeyPayload`、`IssueGatewayKeyResponse`（`ToSchema` 派生）。
2. rotate 声明现状只有 200/401/404/409——三新端点照抄该模式：`201→200` 按端点、401/404 门控/412 全声明（412 当前 rotate 未声明是欠账，新端点必须声明，不追溯改 rotate）。
3. `cargo test -p ponyllm-server --test admin_contract_tests dump_openapi_json -- --ignored` 重生成 `web/openapi.json`；`test_openapi_no_real_secret_and_schema_committed` 全绿（含新增 `assert!(!schema_str.contains("sk-pony-infer-"))` 等真前缀零命中——示例 payload 只用 `"sk-pony-infer-***"` 占位）。
4. `classify_resource` 新增 `GET /api/admin/gateway-keys` 精确 `AdminRead` 分支 + 单测（`auth.rs` 现有 `classify_matrix_spot_checks` 风格追加，不断言改旧用例）。

## Alternatives considered

- **A. 复用 `POST /api/admin/keys`（上游 key CUD）兼管网关凭证（否决）**：少三端点；但名称空间隔离律（契约 §3.2：网关 key 与上游 key 互用必须失败）要求两套存储/两套视图混用即埋下误用口，且 `KeySection.api_key` 存明文与 `GatewayKeyEntry` 只存哈希的持久化语义根本冲突。否决：独立 `/gateway-keys` 名称空间。
- **B. 吊销用 `DELETE /gateway-keys/{id}`（否决）**：REST 更"正交"；但吊销 ≠ 删除（审计留痕 + 幂等语义 + 与未来硬删除动词冲突）。否决：POST revoke；DELETE 预留给 P2 硬删除。
- **C. 列表返回 `key_hash` 供前端比对（否决）**：排障方便；但哈希是离线爆破的输入（salt 同行存储即构成完整彩虹表输入），且前端无任何需要哈希的合法场景。否决：`last4` 识别 + 服务端比对。
- **D. `GET` 列表放行 inference（否决）**：agent 自查方便；但清单含全部凭证 id/scope/吊销态，是标准的横向移动侦察输入，`scope_allows` 矩阵已冻结 inference 无 AdminRead。否决：403。
- **E. 签发支持自定义明文（`auth <KEY>` 风格，否决）**：用户自选好记；但网关凭证的熵必须由服务端保证（P0 弱口令教训），且自定义即引入弱口令校验分支。否决：服务端 `generate_scoped_gateway_key` 唯一签发路径。
- **F.（采纳）本设计：三端点 + 照抄 CUD 门禁链 + 403 矩阵 + openapi 同步**——优点是零新范式（门禁/并发/信封/no-store 全是既有代码的复制），实现任务主要是搬运；缺点是 `last4` 需加磁盘字段（旧条目默认 `****`）。采纳。
