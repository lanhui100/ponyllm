# Agent Note: Web工具链与质量门禁

Status: proposed

## Problem

Vite 8 新构建链、Oxlint 替代 ESLint、serve 内嵌 dist 三事无统一门禁，易出现本地绿 CI 红与网关托管 404。

## Proposal

将锁定 `oxlint --deny-warnings` 加 `oxfmt --check` 为秒级门禁，`vue-tsc` 为类型门禁，Playwright 覆盖 connect→dashboard→recorder 主链路，Lighthouse 阈值 90。`serve` 内嵌 `web/dist` 并 SPA fallback 到 `/app/*`，缺 dist 时 serve 不崩仅告警。

## Alternatives considered

- **保留 ESLint 双跑：否定。双 lint 规则漂移，Oxlint 速度 50x，直接单轨。**
- **E2E 全页覆盖：否定。首版只锁主链路，全量覆盖拖慢 M4。**
- **缺 dist 即 serve 报错退出：否定。网关优先保转发，Web 缺失只告警。**

## Acceptance criteria

- `pnpm --dir web lint` 与 `pnpm --dir web typecheck` 双绿，CI 与 pre-push 同命令。
- Playwright 主链路 3 用例全绿，Lighthouse 报告归档至 review。
- 无 dist 启动 `serve` 仍可转发，告警文案可用 review 演示。

## Risks

- Vite 8 rolldown 插件生态滞后，锁定版本并记入 WEB-01。
- Windows 下 Playwright 浏览器下载慢，CI 缓存目录需固定。
