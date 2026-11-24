# Agent Note: Web控制台信息架构与技术栈

Status: implemented

## Problem

TUI 四面板与 CLI 全量能力尚无浏览器入口，多网关切换与新人上手依赖手敲命令。前端技术栈需一次锁定，避免中途换请求库与组件库导致返工。

## Decision

控制台是 `web/` 独立 pnpm 包，技术栈锁定为 Vue 3.5 + TS(strict) + Vite 8 + Oxlint 1.81 + Alova 3.5 + Pinia（`web/package.json` 精确版本 + 提交 `pnpm-lock.yaml`）。

路由在 `web/src/router.ts` 平铺到根路径：`/connect`（免鉴）、`/dashboard`、`/recorder`、`/governance`、`/tokens`（自助 token）、`/users`（`meta.requiresAdmin`）、`404`；`/` 重定向 `/dashboard`，`meta.requiresAuth` 守卫除 `/connect` 外的全部页面。`/app/*` 不再作为构建前缀，只保留成组重定向（`/app/dashboard → /dashboard` 等）承接旧链接；Vite `base: '/'`。

认证三项落地形态：token 只在内存（Pinia store；`web/src/stores/session.ts` 明确 Bearer token 不再写 `sessionStorage`，该存储只留发版版本钉等非敏感 UI 状态）；发头唯一中间件在 `web/src/lib/alova.ts`，产出 `Authorization: Bearer <trim(token)>`，不附加第二个鉴权头；401 走 single-flight——首个 401 认领跳转 `/connect` + 停轮询 + toast 一次，后续并发 401 直接丢弃，alova 不 import vue-router（由 router 注册回调，避免环依赖）。

`serve` 同源托管 `web/dist`，覆盖 `/*` 与 `/app/*`；`--no-web` 关闭托管；无 dist 时不崩，打固定告警 `[web] web/dist 缺失，Web 控制台未托管（网关转发不受影响）`（`crates/ponyllm-server/src/app.rs`），退出码 0，`/app*` 定态 503 且 `error.code=web_dist_missing`。

## Alternatives considered

- **TanStack Query + Axios：否定。Vue 亲和弱于 Alova，需另包轮询与 SSE；请求共享与缓存需手写。**
- **Element Plus / Ant Design Vue：否定。重型中台味，与纸感极简冲突；shadcn-vue 按需拷贝更克制。**
- **独立域名部署：否定。首版由 `serve` 同源托管 `dist/`，免 CORS 与双运维。**
- **Vite `base: '/app/'` 前缀构建（原提案取值）：后被取代。** 根路径托管让 `/` 直达控制台，旧链接靠 `/app/*` 重定向承接；`web/vite.config.ts` 现为 `base: '/'`，`serve` 同时托管 `/*` 与 `/app/*`。

## Consequences

- 门禁同命令化：`pnpm --dir web lint|typecheck|test|build` 在本地与 `.github/workflows/ci.yml` 的 web job 逐字一致；`cargo test -p ponyllm-server --test web_hosting_tests` 覆盖有/无 dist 与「不吞 `/v1/models`」。
- 8 路由目标态未全量落地：现为 6 个业务页 + `404` + 兼容重定向；新增页面须同步 `router.ts` 与守卫测试。
- 残留：shadcn-vue 仍按需拷贝，`web/SHADCN_PIN` 未建立 → 组件升级只能靠 review 发现漂移。
- 残留：`/app/*` 重定向表是为旧书签保留的过渡面，8 路由稳定后应复核可否整表删除。
- Playwright 主链路 E2E 与 Lighthouse 90 均不在本决策范围（`web/package.json` 无 playwright 依赖）。
