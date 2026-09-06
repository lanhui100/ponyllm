# Agent Note: Web控制台信息架构与技术栈

Status: proposed

## Problem

TUI 四面板与 CLI 全量能力尚无浏览器入口，多网关切换与新人上手依赖手敲命令。前端技术栈需一次锁定，避免中途换请求库与组件库导致返工。

## Proposal

将以 `web/` 独立 pnpm workspace 落地 SPA：Vue3 + TS(strict) + Vite 8 + Oxlint + shadcn-vue + motion-vue + Remix Icon + Alova。信息架构沿 TUI 四 Tab 平移为 8 路由（dashboard/providers/keys/recorder/playground/strategy/settings/integrations/ops），Shell 为侧边栏 220px + 顶栏状态带 + `CmdK`，深浅双主题默认深葡萄夜。Server State 全进 Alova，Pinia 只留主题与会话。

## Alternatives considered

- **TanStack Query + Axios：否定。Vue 亲和弱于 Alova，需另包轮询与 SSE；请求共享与缓存需手写。**
- **Element Plus / Ant Design Vue：否定。重型中台味，与纸感极简冲突；shadcn-vue 按需拷贝更克制。**
- **独立域名部署：否定。首版由 `serve` 同源托管 `dist/`，免 CORS 与双运维。**

## Acceptance criteria

- `web/src/router.ts` 含 8 路由表与 `/connect` 守卫，401 踢回可用 review 演示。
- `oxlint --deny-warnings` 与 `vue-tsc` 双绿，命令见任务卡 WEB-01。
- Alova 实例含 token 中间件与 401 过期处理，metrics 轮询 1.5s 可用 review 演示。

## Risks

- shadcn-vue 按需拷贝版本漂移，锁定提交哈希并记入 WEB-01。
- 深浅主题 token 增殖，新增色必须走本 ADR 修订，不随手加。
