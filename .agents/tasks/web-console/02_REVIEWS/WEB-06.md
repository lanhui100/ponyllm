# Review: WEB-06 Admin写路径与治理

- ID: WEB-06
- Verdict: Pass
- Date: 2026-09-07
- Reviewer: codex-orchestrator（汇总双路 ADR 审核 + 双路代码审核；Correctness Pass + Security Pass 全绿）

## 验收逐项

- [x] 1. CUD 端点全部经灰度开关（off 时 404 + 审计记录）：`check_admin_write_enabled` 门禁守卫所有 CUD（POST/PUT/DELETE providers/models/keys）及拨测端点；关闭时拦截并返回 404 `admin_write_disabled` 并记录 warning 审计日志。单测 `test_admin_write_disabled_gate` 全绿。
- [x] 2. 写前备份 + If-Match 版本号冲突拒绝 + 写队列串行化：
  - 写前备份：`ConfigFile::save_to_path` 在原子覆盖前自动生成 `ponyllm.toml.bak`。
  - If-Match 乐观锁：所有 CUD 写操作强制校验 `If-Match: "<config_version>"` 请求头，缺失或版本冲突立即拒绝并返回 412 `precondition_failed`。
  - 写队列串行化：`AppState` 增加 `admin_write_lock: tokio::sync::Mutex<()>`，CUD 处理全流程排他持有锁，消除读改写竞态窗口。
  - 单测 `test_write_before_backup_created`、`test_if_match_validation`、`test_write_queue_concurrency` 验证并发写入 10 个模型最终版本号准确递增且无脏写。
- [x] 3. keys/test 拨测响应 schema（超时/401/429 语义）+ 脱敏日志：`handle_admin_test_key` 针对 upstream 发起带严格 3s 超时（`Duration::from_secs(3)`）的探活，覆盖 200 OK、401 Unauthorized、429 Rate Limited、3s 超时及 404 Key 不存在，日志使用 key_id 审计且严格脱敏，单测 `test_key_dial_test_matrix` 基于独立 mock 服务全绿。
- [x] 4. 新增 Key 明文仅创建响应一次性回显：`POST /api/admin/keys` 成功创建时响应一次性明文 `api_key` 并附带 `Cache-Control: no-store` 与 `Pragma: no-cache`；后续 `GET /api/admin/keys` 严格返回 `masked_key` 脱敏掩码；`DELETE /api/admin/keys/{id}` 热同步重构 KeyPool，单测 `test_key_cud_one_time_plaintext_and_masking` 全绿。

## 测试证据

- `cargo test -p ponyllm-server --test admin_write_tests` → 8 passed, 0 failed (100% pass)。
- `cargo test -p ponyllm-server --test admin_contract_tests` → 10 passed, 0 failed, 1 ignored (dump helper)。
- `cargo test --workspace` → 全量工作区单元与集成测试全绿通过。
- `pnpm --dir web test` → 4 test files, 27 passed, 0 failed (100% pass)。
- `pnpm --dir web build` → 成功无告警。
- `bash .meta/gates/check-tasks.sh` → 全部通过。
- `bash .agents/skills/write-adr/verify-note.sh` → 全部通过。

## 审核链

- ADR 阶段：
  - 架构审阅（Architect Reviewer）：Pass。写队列应用层排他互斥，`If-Match` 乐观锁防并发覆盖，写前自动 `.bak` 备份，灰度开关默认 off。
  - 安全边界审阅（Security Reviewer）：Pass。明文 Key 一次性回显带 `no-store`，后续读接口维持脱敏；拨测接口 3s 硬超时防慢连接攻击，日志全脱敏。
- 代码审阅：
  - 正确性审阅员 (Correctness Reviewer): **Pass**。CUD 操作后内存配置 `state.config` 与连接池 `state.pools` 实时热同步，OpenAPI utoipa 契约完整同步至 `web/openapi.json`。
  - 安全边界审阅员 (Security Reviewer): **Pass**。`admin_write_enabled` 灰度控制严格，If-Match 版本号校验防覆写，敏感凭据脱敏严密。

## 遗留风险

- 无。
