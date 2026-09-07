# Review: WEB-03 Admin API契约

- ID: WEB-03
- Verdict: Pass
- Date: 2026-09-07
- Reviewer: codex-orchestrator（汇总双路 ADR 审核 + 双路代码审核；Correctness Pass + Security Conditional Pass 并已闭环修复）

## 验收逐项

- [x] 1. `cargo test -p ponyllm-server --test admin_contract_tests` 全绿（target 精确）：8 端点矩阵 × secured/免鉴双模式 + openapi 一致性 + keys 脱敏 + 空 key rotate 409 + SDK None 503 + config_version 兼容：`cargo test -p ponyllm-server --test admin_contract_tests` → 10 passed, 1 ignored (dump utility)。
- [x] 2. `web/openapi.json` utoipa 生成并提交，schema 可校验：utoipa 导出生成已落盘并提交，OperationId 与 Schemas 齐备。
- [x] 3. `overview.hot_reload_ms` = 500 可查；admin 写主动 reload（测试断言写后立即可见）；service/status 与 overview 不回显绝对路径（单测断言）：`test_overview_hot_reload_ms_and_no_path_leak` 断言通过。

## 测试证据

- `cargo test -p ponyllm-server --test admin_contract_tests` → 10/10 绿（0 failed）。
- `cargo test -p ponyllm-server` → 全套单测与集成测试（100+ 项）全部通过（0 failed）。
- `cargo check --workspace` → 退出码 0。
- `pnpm --dir web lint|typecheck|test` → 0 err / 0 warn / typecheck 0 / vitest 14/14 全绿。
- `check-tasks.ps1` → 全部通过；`verify-note.ps1` → 全部通过。

## 审核链

- ADR 阶段：architect 有条件通过（A2′方案 + 8端点分期，CUD移WEB-06）+ security 有条件通过（7条件闭环）。
- 代码审阅：
  - 正确性审阅员 (Correctness Reviewer): **Pass**。8 端点行为、ConfigStore 注入与 SDK None 隔离、单调 version 递增、内存即刻热重载全部确认。采纳建议：将 PUT strategy requestBody 强类型化为 `PutStrategyPayload`。
  - 安全边界审阅员 (Security Reviewer): **Conditional Pass**。7 项安全条件（掩码同源、no-store 头、409 拦截、零路径泄露、零密钥泄露）全部达标。指出高危临时文件并发写踩踏隐患与底层报错外泄。
  - 审阅意见闭环：
    1. 临时文件名追加 UUID 随机后缀，彻底杜绝多协程/多线程并发写临时文件踩踏。
    2. 对底层 IO 错误信息脱敏，记录 `tracing::error!`，对外屏蔽环境内部细节。
    3. PUT strategy 增加强类型 `PutStrategyPayload` schema 并重新生成 `web/openapi.json`。
    4. 其余治理债（写队列、乐观并发 If-Match、专属 CORS）按既定规划登记至 WEB-06。

## 遗留风险与承接

- load→save 期间外部编辑可能被覆盖的竞态窗：已记录在 ADR，由 WEB-06 写队列与 If-Match 乐观并发收口。
- 开放模式管理路由 CORS 暴露：已记录在 ADR，由 WEB-06 专属防跨域策略收口。
