# Agent Note: Web工具链与质量门禁

Status: proposed

## Problem

Vite 8 新构建链、Oxlint 替代 ESLint、serve 内嵌 dist 三事无统一门禁，易出现本地绿 CI 红与网关托管 404。

## Proposal

将锁定 `oxlint --deny-warnings` 为秒级门禁（oxfmt 本卡弃用：从门禁删除，避免双轨脱节），`vue-tsc` 为类型门禁，三命令（lint/typecheck/test）在任务卡、`web/package.json` scripts、CI web job、pre-push 同字。Playwright 主链路 3 用例（connect→dashboard→recorder）归 WEB-02，不在 WEB-01；WEB-01 守卫测试用 vitest（Windows 免浏览器下载）。Lighthouse 阈值 90 移出 M1（空壳测 90 无意义，实页优化专项另卡）。`serve` 内嵌 `web/dist` 并 SPA fallback 到 `/app/*`（Vite `base: '/app/'` 锁定），缺 dist 时 serve 不崩仅告警（固定文案 `[web] web/dist 缺失` + `/app/*` 定态 503 + 退出码 0），`--no-web` 可关闭。

## Alternatives considered

- **保留 ESLint 双跑：否定。双 lint 规则漂移，Oxlint 速度 50x，直接单轨。**
- **E2E 全页覆盖：否定。首版只锁主链路，全量覆盖拖慢 M4。**
- **缺 dist 即 serve 报错退出：否定。网关优先保转发，Web 缺失只告警。**

## Acceptance criteria

- `pnpm --dir web lint`、`pnpm --dir web typecheck`、`pnpm --dir web test` 三绿，CI web job 与 pre-push 同命令。
- serve 托管集成测试 `cargo test -p ponyllm-server --test web_hosting` 全绿（有/无 dist 各一条 curl 断言 + `/v1/models` 不被吞断言）。
- 无 dist 启动 `serve` 退出码 0 仍可转发，固定告警文案 `[web] web/dist 缺失` 可 grep。
- Playwright 主链路 3 用例归 WEB-02；Lighthouse 90 移出 M1 另卡。

## Risks

- Vite 8 rolldown 插件生态滞后，锁定版本并记入 WEB-01。
- Windows 下 Playwright 浏览器下载慢，CI 缓存目录需固定。
