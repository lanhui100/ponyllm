# Review: WEB-01 脚手架与Shell

- ID: WEB-01
- Verdict: Pass
- Date: 2026-09-07
- Reviewer: codex-orchestrator（汇总双路 ADR 审核 + 双路代码审核 + 双路 delta 复审；门禁只验存在，深度见下）

## 验收逐项

- [x] 1. `pnpm --dir web lint` 退出码 0（0 warn/0 err，10 文件），CI web job 同命令：`pnpm --dir web lint` → 0。
- [x] 2. `pnpm --dir web typecheck` 退出码 0；`pnpm-lock.yaml` 已提交：`pnpm --dir web typecheck` → 0；`web/pnpm-lock.yaml` 存在。
- [x] 3. `pnpm --dir web test` 14 用例全绿（守卫/单飞/发头/baseURL）：`pnpm --dir web test` → 14/14。
- [x] 4. `cargo test -p ponyllm-server --test web_hosting_tests` 全绿（4 测试：深链/API 优先级/`..`/裸前缀/secured 矩阵/无 dist 503/`--no-web` 404）：`cargo test -p ponyllm-server --test web_hosting_tests` → 4/4。
- [x] 5. 无 dist 启动 `serve` 退出码 0、可转发，固定文案可 grep，`--no-web` 可关：实机 `:18082` `/app/dashboard`→503 `web_dist_missing` + `/health`→200 + 日志 grep `[web] web/dist 缺失` 命中。
- [x] 6. 存储禁令 + 发头断言双命令绿，零 review 项：非测试源码 grep 零命中 + vitest 发头断言；实机 `:18083`（真实 dist）深链→200 html + `/v1/models`→200 json + asset→200。

## 测试证据

- `cargo test -p ponyllm-server` → 全 11 target 绿（含 web_hosting 4/4）。
- `cargo check --workspace` → 绿。
- `pnpm --dir web lint|typecheck|test` → 绿/绿/14-14。
- `vite build` → 成功，asset `/app/assets/*`。
- `check-tasks.ps1` → 全部通过；`verify-note.ps1` → 全部通过。
- 实机 serve ×2（无 dist :18082 / 真实 dist :18083）→ 见验收 5/6。

## 审核链

- ADR 首轮：architect FAIL + security FAIL → 修订（6 条命令式验收 + Rust 托管并入 + 认证三件）。
- ADR delta：architect 有条件通过（C1+C2）→ 落地；security 仍 FAIL（3 口子）→ 逐条封 → 双终审 PASS。
- 代码首轮：A 有条件通过（7 gating）+ B（4P1）→ 去重 10 项全修。
- 代码 delta：A 通过 + B 通过。

## 遗留风险

- `web_dist_dir` 相对 serve CWD（banner 打绝对路径可诊断；文档推绝对路径）。
- `index.html` 先检后删 TOCTOU → fallback 404（低概率，可接受）。
- web 开关 restart-only（已文档化；热更新只换 config+pools）。
- 另一会话 Rust 主线施工中（同目录 error.rs 等）：本卡零交集，已用 reset 隔离暂存区。
