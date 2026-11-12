# Review: WEB-02 只读大盘与录波

- ID: WEB-02
- Verdict: Pass
- Date: 2026-09-07
- Reviewer: codex-orchestrator（汇总双路 ADR 审核 + 双路代码审核；Correctness Pass + Security Pass 全绿）

## 验收逐项

- [x] 1. metrics 1.5s 轮询与 SSE 双链路可降级：`web/src/composables/useTelemetry.ts` 实现优先 EventSource 探测，无缝回退 1.5s 轮询聚合与 `useDocumentVisibility` 后台节流，单测 `useTelemetry.test.ts` 4/4 绿。
- [x] 2. 录波脱敏单测绿，全 Key 为 `sk-***`：`web/src/utils/scrub.ts` 覆盖密钥清洗、json payload 及错误文本清洗，单测 `scrub.test.ts` 6/6 绿，全 Key 及 snippet 严格展示为 `sk-***`，零泄漏。
- [x] 3. DOWN 置灰加重试可用 review 演示：网关无响应或 503 DOWN 时整页应用 `.is-down` 灰阶样式（filter: grayscale(0.85)），中断自动轮询并提供手动“重试连接”按钮，`views.flow.test.ts` 自动化断言通过。
- [x] 4. 主链路 3 用例（connect→dashboard→recorder）全绿：`web/src/views/views.flow.test.ts` 3/3 用例全绿（未认证跳转与输入、大盘 KPI 渲染与 DOWN 降级、录波 50 帧筛选/j/k切帧/Enter展开抽屉/脱敏断言），全套前端单测 4 模块 27/27 全绿，与 CI web job 命令一致。

## 测试证据

- `pnpm --dir web test` → 4 test files, 27 passed, 0 failed (100% pass)。
- `pnpm --dir web lint` → 0 warnings, 0 errors。
- `pnpm --dir web typecheck` → 0 errors。
- `pnpm --dir web build` → 成功，生产产物按路由分割（Dashboard 异步 chunk + Recorder 异步 chunk）。
- `cargo check --workspace` → 退出码 0。
- `cargo test -p ponyllm-server --test web_hosting_tests` → 4/4 passed。
- `bash .meta/gates/check-tasks.sh` → 全部通过。
- `bash .agents/skills/write-adr/verify-note.sh` → 全部通过。

## 审核链

- ADR 阶段：
  - 架构审阅（Architect Reviewer）：Pass。明确后端 `/v1/telemetry/stream` 快照与 SSE 双模兼容方案；图表采用按需引入 `echarts/core`；Recorder 采用定高虚拟列表保证常数级 DOM 节点。
  - 安全边界审阅（Security Reviewer）：Pass。全文本字段（snippets、error、key）经由前端二次严格脱敏为 `sk-***`；cURL 导出命令实施单引号安全转义；DOWN 状态整页灰阶且切断连续打满请求。
- 代码审阅：
  - 正确性审阅员 (Correctness Reviewer): **Pass**。动态路由切片解耦体积，虚拟列表滚动流畅，键盘 `j/k/Enter` 快捷导航无事件污染，Alova 与 Pinia 鉴权头严格挂载。
  - 安全边界审阅员 (Security Reviewer): **Pass**。全端脱敏无死角，cURL 导出强制使用安全占位符及转义，401 与网关掉线无死循环重试或敏感数据残留。

## 遗留风险

- 无。写路径操作保留在 WEB-06（Admin写路径与治理）。
