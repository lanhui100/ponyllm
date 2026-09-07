# WEB-01 脚手架与Shell

## Basic Info
- ID: WEB-01
- Status: In Progress
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-06
- Updated At: 2026-09-07
- Branch: task/WEB-01-scaffold
- Estimated Effort: 1周
- Blocker: 无
- Unblock Condition: 无
- Review Round: 双路首轮 FAIL（architect + security）→ 修订中，复审通过后开工

## Goal
建 `web/` 最小壳（3 路由：`/connect` + `/dashboard` stub + `404`）+ `serve` 经 `/app/*` 同源托管 `web/dist`（缺 dist 仅告警不崩，可 `--no-web` 关闭）+ 双门禁（lint/typecheck）+ CI web job。8 路由全表、shadcn-vue/motion-vue/RemixIcon 全量、CmdK、Playwright 主链路均延后（见 M2/M4）。

## Output
- `web/package.json`（scripts pin：lint/typecheck/test；engines；精确版本 + `pnpm-lock.yaml` 已提交）
- `web/vite.config.ts`（`base: '/app/'` 锁定）
- `web/tsconfig.json` + `web/index.html`
- `web/src/router.ts`（3 路由 + 守卫：未登录跳 `/connect`，`/connect` 自放行，免鉴探活直放）
- `web/src/lib/alova.ts`（运行时可配 baseURL；唯一鉴权头 `Authorization: Bearer` + trim；401 single-flight：首次 401 者跳转并停轮询）
- `web/src/stores/session.ts`（token 内存-only，禁 localStorage；登出/401 清除）
- `web/src/router.guard.test.ts`（vitest 守卫单测；Playwright 主链路归 WEB-02）
- `web/oxlint.config.json`（`web/src/generated/**` 豁免，orval 生成目录契约；oxfmt 本卡弃用）
- `crates/ponyllm-server/src/app.rs`（web 路由与 API 路由分离合并：`/app` 挂载免 `auth_middleware`，fallback 不吞 `/v1/*` 与 `/telemetry/*`）
- `crates/ponyllm-server/tests/web_hosting_tests.rs`（托管集成测试）
- `crates/ponyllm-cli/src/main.rs` + config（`--no-web` 开关；缺 dist 固定 warn 文案）
- `.github/workflows/ci.yml`（web job：lint + typecheck + vitest，与本地同命令）

## Acceptance Criteria
1. `pnpm --dir web lint`（oxlint --deny-warnings）退出码 0，CI web job 同命令绿。
2. `pnpm --dir web typecheck`（vue-tsc）退出码 0；`pnpm-lock.yaml` 已提交，版本 pin 可复现。
3. `pnpm --dir web test`（vitest 守卫单测 ≥5 用例）全绿：无 token 访保护路由跳 `/connect`；`/connect` 自放行；401 回调只跳转一次 + 停轮询 + toast 一次；登出/登录后 single-flight 标志复位（第二会话 401 可再跳）；免鉴模式（带错 token 调 `GET /v1/models` 不 401）直放。
4. `cargo test -p ponyllm-server --test web_hosting` 全绿：有 dist 时 `/app/dashboard` 直刷 200 且 `/v1/models` 不被吞；无 dist 时 `/app/*` 定态 503（固定 `code`）且网关路由不受影响。
5. 无 dist 启动 `serve` 退出码 0、可转发，stderr/日志含固定告警文案（`[web] web/dist 缺失`），`--no-web` 可关闭托管。
6. 存储禁令 + 发头断言双命令绿：`grep -rE 'localStorage|sessionStorage|indexedDB|document\.cookie' web/src` 零命中（禁一切持久化）；vitest 断言发出头仅含 `Authorization: Bearer <trimmed>` 且无 `x-api-key`（含前后空格 token 用例）。零 review 项。

## Current Progress
- 双路首轮评审 FAIL（2026-09-07）：5+5 项 P0 已收敛为本卡修订；分支已建，待复审通过后实现。
- 事实锚点：`auth_middleware` 存在（空 key/`none` 全放行），`admin.rs` 与 dist 托管逻辑缺失，`tower-http 0.6.11` 含 `fs/ServeDir`。

## Next Action
- 修订两 ADR → 双路复审（delta）→ 通过后先 Rust 托管 + 测试，再前端脚手架 + 测试，最后 CI job。

## Resume Hint
- 先跑 Next Action；复审结论见双路 reviewer 回复，需 rationale 见 Related Files ADR。

## Review Summary
- 首轮 FAIL（architect + security，2026-09-07）：token 存放未定义、401 踢回竞态、免鉴语义冲突、任务边界错位、三处“靠 review”违宪、双门禁分裂、serve 托管无交付物、8 路由与 M1 矛盾、workspace/lockfile 基线缺失。全部采纳 → 本卡改写为 6 条命令式验收 + Rust 托管交付物并入；唯一偏离：守卫测试用 vitest 而非 Playwright（Windows 免浏览器下载，Playwright 主链路归 WEB-02，理由已记）。
- delta 复审（architect 有条件通过，2026-09-07）：C1 WEB-02 承接 Playwright（本轮已加验收 4）+ C2 验收 6 改 grep 双零命中（本轮已改）。
- delta-2（security 仍 FAIL → 逐条封，2026-09-07）：P0-2 single-flight 复位 + 停轮询/toast 断言进验收 3（≥5 用例）；P0-3 点名 `GET /v1/models` + 明文禁 `/health`；P0-5 验收 6 改机查双命令、删 review 项；P0-1 ban 名扩大到一切持久化。

## Related Files
- ADR: `.agents/notes/proposed/architecture/2026-09-06-web-console-ia-and-stack.md`
- ADR: `.agents/notes/proposed/process/2026-09-06-web-toolchain-quality.md`
