# WEB-01 脚手架与Shell

## Basic Info
- ID: WEB-01
- Status: Done
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-06
- Updated At: 2026-09-07
- Branch: task/WEB-01-scaffold
- Merge: dd4fb10c953752b40df5bda97c0d91a061ee2055
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
- `web/.oxlintrc.json`（标准配置位；`web/src/generated/**` 豁免以 lint 脚本 `--ignore-pattern` 落定，orval 生成目录契约；oxfmt 本卡弃用）
- `crates/ponyllm-server/src/app.rs`（web 路由与 API 路由分离合并：`/app` 挂载免 `auth_middleware`，fallback 不吞 `/v1/*` 与 `/telemetry/*`）
- `crates/ponyllm-server/tests/web_hosting_tests.rs`（托管集成测试）
- `crates/ponyllm-cli/src/main.rs` + config（`--no-web` 开关；缺 dist 固定 warn 文案）
- `.github/workflows/ci.yml`（web job：lint + typecheck + vitest，与本地同命令）

## Acceptance Criteria
1. `pnpm --dir web lint`（`oxlint --deny-warnings --ignore-pattern 'node_modules/**' --ignore-pattern 'dist/**' --ignore-pattern 'src/generated/**'`）退出码 0，CI web job 同命令绿。
2. `pnpm --dir web typecheck`（vue-tsc）退出码 0；`pnpm-lock.yaml` 已提交，版本 pin 可复现。
3. `pnpm --dir web test`（vitest 守卫单测 14 用例）全绿：无 token 跳 `/connect`；`/connect` 自放行；401 只跳一次 + 停轮询 + toast 一次；登出/登录复位；免鉴直放；meta 驱动（无 requiresAuth 不探针）；P1-1 真 handler 双 401 单回调；哨兵 instanceof；发头唯一；baseURL 归一化。
4. `cargo test -p ponyllm-server --test web_hosting_tests` 全绿（4 测试）：有 dist 深链 200 + `/v1/models` 未被吞 + `..` 收敛 + 裸 `/app`、`/app/` 回入口；secured 模式静态免鉴 + API 401/200 矩阵；无 dist 双 503（固定 `code`）；`--no-web` 404 + health 200。
5. 无 dist 启动 `serve` 退出码 0、可转发，stderr/日志含固定告警文案（`[web] web/dist 缺失`），`--no-web` 可关闭托管。
6. 存储禁令 + 发头断言双命令绿：`Get-ChildItem web/src -Recurse -Include *.ts,*.vue -Exclude *.test.ts | Select-String 'localStorage|sessionStorage|indexedDB|document\.cookie|x-api-key|X-Api-Key'` 零命中（禁一切持久化 + 唯一 `Authorization: Bearer` 头；测试断言需写出被禁字面量故排除 `*.test.ts`）；vitest 断言发出头仅含 `Authorization: Bearer <trimmed>` 且无二次头（含前后空格 token 用例）。零 review 项。

## Current Progress
- 双路终审 PASS（2026-09-07），认领提交 `69c0e7e` 已落（不推）。
- Rust 托管实现完成（2026-09-07）：`GatewayConfig.web_enabled/web_dist_dir` + `app.rs` web/API 分离合并（`/app` 免鉴，`ServeFile` fallback）+ `web_hosting_tests.rs` 3 测试绿 + `--no-web`（serve/restart 透传，热更新 pin 住开关）+ wizard/config 补字段。
- 实机验证（2026-09-07，`ponyllm serve --port 18082` 无 dist）：`/app/dashboard` → 503 `web_dist_missing`；`/health` → 200；固定文案 `[web] web/dist 缺失` stderr + 日志双通道可 grep；`--no-web` 待前端联调时复验。
- 双码审修复（2026-09-07，A 有条件通过 7 gating + B 4P1，去重 10 项全修）：P1-1 single-flight 自毁（`clearToken` 不清旗 + 真 handler 双 401 测试）；P1-2 守卫 meta 单源化；P1-3 `web_dist_dir` 贯通 CLI（`--web-dist-dir` + toml + watcher）+ serve 启动日志（绝对路径 + 状态）；P2-1 watcher 条件 pin + restart-only 文档；P2-2 共享 `probeOpenMode`；P2-3 `UnauthorizedError` 哨兵 + 调用约定；P2-4 CI 加 build 门；Connect `resp.ok` 收紧；自查 `/app/` 漏网已修；裸 `/app` 经 nest 实测回入口。
- 待：delta 复审 → 双门禁 → 完工归档（本轮）。
- 前端脚手架完成（2026-09-07）：`web/package.json`（pin + lockfile + engines）+ `vite.config.ts`（base `/app/` + dev proxy）+ `tsconfig` + `index.html` + `.oxlintrc.json`（标准位；lint 脚本钉死 ignore）+ `router.ts`（3 路由 + `decideRoute` 纯函数 + 401 single-flight 接线 + 探活 `GET /v1/models`）+ `alova.ts`（唯一 Bearer + trim + 运行时 baseURL）+ `session.ts`（内存-only）+ 3 view（stub 不调接口）+ `router.guard.test.ts`（9 用例）。
- 三绿：`lint` 0 warn/0 err（10 文件），`typecheck` 0，`vitest` 9/9。`vite build` 成功，dist asset 路径 `/app/assets/*`。
- E2E 实机（真实 dist + serve :18083 免鉴）：`/app/dashboard` → 200 html；`/v1/models` → 200 json（未被吞）；`/app/assets/*.js` → 200（免鉴可达）。
- CI web job 已加（frozen-lockfile + 三同字命令）。`.gitignore` 补 `web/dist/` + `web/node_modules/`。

## Next Action
- 双码审（architect-正确性 + security-边界）→ 修 → 双门禁 → 完工归档提交（不推）。

## Next Action
- 建 `web/` 前端脚手架并跑 `pnpm --dir web lint|typecheck|test` 三绿。

## Resume Hint
- 先跑 Next Action；复审结论见双路 reviewer 回复，需 rationale 见 Related Files ADR。

## Review Summary
- 首轮 FAIL（architect + security，2026-09-07）：token 存放未定义、401 踢回竞态、免鉴语义冲突、任务边界错位、三处“靠 review”违宪、双门禁分裂、serve 托管无交付物、8 路由与 M1 矛盾、workspace/lockfile 基线缺失。全部采纳 → 本卡改写为 6 条命令式验收 + Rust 托管交付物并入；唯一偏离：守卫测试用 vitest 而非 Playwright（Windows 免浏览器下载，Playwright 主链路归 WEB-02，理由已记）。
- delta 复审（architect 有条件通过，2026-09-07）：C1 WEB-02 承接 Playwright（本轮已加验收 4）+ C2 验收 6 改 grep 双零命中（本轮已改）。
- delta-2（security 仍 FAIL → 逐条封，2026-09-07）：P0-2 single-flight 复位 + 停轮询/toast 断言进验收 3（≥5 用例）；P0-3 点名 `GET /v1/models` + 明文禁 `/health`；P0-5 验收 6 改机查双命令、删 review 项；P0-1 ban 名扩大到一切持久化。
- 终审双 PASS（2026-09-07）：security（3 条件逐条满足）+ architect（C1+C2 落地）。认领提交 `69c0e7e` 已落（不推）。开工实现。

## Related Files
- ADR: `.agents/notes/proposed/architecture/2026-09-06-web-console-ia-and-stack.md`
- ADR: `.agents/notes/proposed/process/2026-09-06-web-toolchain-quality.md`
