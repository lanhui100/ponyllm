# Agent Note: Web工具链与质量门禁

Status: implemented

## Problem

Vite 8 新构建链、Oxlint 替代 ESLint、serve 内嵌 dist 三事无统一门禁，易出现本地绿 CI 红与网关托管 404。

## Decision

web 的三条质量命令是唯一门禁，且本地与 CI 逐字同命令（`web/package.json` scripts =
`.github/workflows/ci.yml` 的 web job）：

- `pnpm --dir web lint` → `oxlint --deny-warnings`（豁免 `node_modules`、`dist`、`src/generated`）；
- `pnpm --dir web typecheck` → `vue-tsc`；
- `pnpm --dir web test` → vitest；
- `pnpm --dir web build` 在 CI 兜底构建，捕获 base/资源断链。

`oxfmt` 不进任何门禁（避免与 oxlint 双轨脱节，格式靠 review）。`serve` 内嵌 `web/dist` 并做 SPA fallback：`--no-web` 可关闭托管；缺 dist 时不崩，打固定告警 `[web] web/dist 缺失，Web 控制台未托管（网关转发不受影响）`，退出码 0，`/app*` 定态 503。托管契约由 `cargo test -p ponyllm-server --test web_hosting_tests` 锁住（有/无 dist 各一组断言 + 不吞 `/v1/models`）。

## Alternatives considered

- **保留 ESLint 双跑：否定。双 lint 规则漂移，Oxlint 速度 50x，直接单轨。**
- **E2E 全页覆盖：否定。首版只锁主链路，全量覆盖拖慢 M4。**
- **缺 dist 即 serve 报错退出：否定。网关优先保转发，Web 缺失只告警。**

## Consequences

- 本地绿 = CI 绿的前提是命令同字，改任一侧必须同步另一侧（否则判决分叉）。
- Playwright 主链路 3 用例（connect→dashboard→recorder）与 Lighthouse 90 阈值为显式 Non-Goal：`web/package.json` 至今无 playwright 依赖，E2E 仍靠仓内 `*.flow.test.ts` 与 CI 构建兜底。
- Vite 8 rolldown 插件生态滞后的风险以「精确版本 + 提交 `pnpm-lock.yaml`」处置，升级须同提交改 lock。
- `web/src/generated/**` 是 orval 生成区，只许增量加文件；`web/openapi.json` 为后端交付的只读契约快照，前端只消费不改。
