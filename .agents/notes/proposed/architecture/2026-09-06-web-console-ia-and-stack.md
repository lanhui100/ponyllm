# Agent Note: Web控制台信息架构与技术栈

Status: proposed

## Problem

TUI 四面板与 CLI 全量能力尚无浏览器入口，多网关切换与新人上手依赖手敲命令。前端技术栈需一次锁定，避免中途换请求库与组件库导致返工。

## Proposal

将以 `web/` 独立 pnpm workspace 落地 SPA：Vue3 + TS(strict) + Vite 8 + Oxlint + Alova。M1 最小壳只锁 Vue/TS/Vite/Oxlint/Alova/Pinia 六件（shadcn-vue 按需拷贝、motion-vue、Remix Icon 全量、CmdK、vue-echarts 延后到 M2/M4，8 路由全表延后）。信息架构沿 TUI 四 Tab 平移为 8 路由为目标态；M1 路由表只建 3 个（`/connect` + `/dashboard` stub + `404`），stub 页为空态文案且不调接口，其余 5 路由随页面增量加。Shell 首版为侧边栏 + 顶栏状态带（220px 精确像素与 CmdK、深浅双主题延后，默认深色单主题先行）。Server State 全进 Alova，Pinia 只留主题与会话。

认证三件（双路评审 P0 后增补，security delta-2 后硬化）：token 内存-only（Pinia store，不持久化，禁一切持久化：localStorage/sessionStorage/IndexedDB/cookie；页刷掉重连可接受；登出/401 清除）；Alova 唯一鉴权头 `Authorization: Bearer <trim(token)>`（后端虽同时认 `x-api-key`，前端只发 Bearer，对齐后端 trim/大小写）；401 single-flight（首次 401 者负责跳转 `/connect` + 停轮询 + toast 一次，登出/登录后标志复位，守卫放行 `/connect` 本身并保留 redirect 回跳）。免鉴探活：进 `/connect` 前先带错 token 调 `GET /v1/models`（auth 覆盖的 GET 端点；明文禁 `/health`——`app.rs:21` 恒免鉴，用它做探针则检测恒为真即静默绕过认证），无 401 则免鉴直放（对齐 Rust `auth_middleware` 空 key/`none` 全放行语义，`app.rs:29-32`）。

运行时配置：Alova baseURL 运行时可配（不写死 `VITE_*` 构建时变量），同源默认相对路径；dev 双端口（vite :5173 → serve）走 vite proxy。

首版由 `serve` 同源托管 `dist/` 于 `/app/*`（Vite `base: '/app/'` 锁定二选一已拍板 `/app/*`）：API 路由优先于静态 fallback；fallback 仅 GET + `Accept: text/html`；静态挂载约束在 dist 内；无 dist 时退出码 0 + 固定 warn 文案 + `/app/*` 定态 503；`serve` 支持 `--no-web` 关闭托管；web 构建失败不阻断 `cargo test --workspace`。Rust 交付物与测试归 WEB-01（边界修订：托管 owner 明确给 WEB-01，不许“占位”无验收）。

## Alternatives considered

- **TanStack Query + Axios：否定。Vue 亲和弱于 Alova，需另包轮询与 SSE；请求共享与缓存需手写。**
- **Element Plus / Ant Design Vue：否定。重型中台味，与纸感极简冲突；shadcn-vue 按需拷贝更克制。**
- **独立域名部署：否定。首版由 `serve` 同源托管 `dist/`，免 CORS 与双运维。**

## Acceptance criteria

- `web/src/router.ts` 含 3 路由表（`/connect` + `/dashboard` stub + `404`）与守卫：未登录访保护路由跳 `/connect`，`/connect` 自放行，免鉴探活直放。8 路由全表为目标态，不在本卡验收。
- `pnpm --dir web lint`（oxlint --deny-warnings）与 `pnpm --dir web typecheck`（vue-tsc）双退出码 0，CI web job 同命令；`web/src/router.guard.test.ts`（vitest ≥5 用例：跳 `/connect`、自放行、401 只跳一次 + 停轮询 + toast 一次、复位后可再跳、免鉴直放）全绿；存储禁令 `grep -rE 'localStorage|sessionStorage|indexedDB|document\.cookie' web/src` 零命中 + 发头 vitest 断言（仅 Bearer + trim + 无 x-api-key），命令见任务卡 WEB-01。零 review 验收项。
- Alova 实例含 token 中间件（唯一 `Authorization: Bearer` 头 + trim）与 401 single-flight 过期处理；metrics 轮询 1.5s 演示归 WEB-02，本卡不验数据链路。
- `serve` 托管契约：有 dist 时 `/app/dashboard` 直刷 200 且不吞 `/v1/models`；无 dist 时退出码 0、可转发、固定告警文案；`cargo test -p ponyllm-server --test web_hosting` 全绿。
- orval 生成目录契约：`web/src/generated/**` 为 oxlint/vue-tsc 豁免区，WEB-03 只许增量加文件不许改既有 lint/构建配置（文件级清单，替代口头“不碰”约束）；`web/openapi.json` 为后端交付的只读契约快照，前端只消费不改。

## Risks

- shadcn-vue 按需拷贝版本漂移：延后到 M2/M4，届时以 `web/SHADCN_PIN`（提交哈希 + 拷贝组件清单）钉死并记入对应卡，不在本卡。
- Vite 8 rolldown 插件生态滞后：WEB-01 以 `web/package.json` 精确版本 + `pnpm-lock.yaml` 提交 + `vite.config.ts` 插件清单 pin 落定，CI web job 同命令复现。
- 深浅主题 token 增殖不在 WEB-01（首版单深色主题），新增色走本 ADR 修订，不随手加。
