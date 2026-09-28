# Agent Note: 部署后 favicon 不显示的根因修复（假 .ico 回退 + 缓存策略）

Status: implemented

## Problem

`https://tokens.ponyjob.top` 部署新版 Web 控制台后，浏览器标签页的 favicon 不显示。
线上实测（2026-09-28）服务端响应本身正确：`GET /favicon.svg` 返回 200 +
`image/svg+xml` + 合法 SVG；根因在客户端与服务端两处：

1. **假 `.ico` 回退（服务端缺陷）**：`index.html` 声明
   `<link rel="alternate icon" href="/favicon.ico">`，本意是给对 SVG favicon
   支持不全的浏览器（旧版 Safari、Chrome < 80）一个 `.ico` 兜底；但 `app.rs`
   把 `/favicon.ico` 路由到了**同一个 SVG 文件**，响应头仍是 `image/svg+xml`。
   不支持 SVG 的浏览器拿到 SVG 字节 → 渲染失败 → 永远无图标。
2. **favicon 响应无缓存头**：`/favicon.svg` 与 `/favicon.ico` 只带
   `last-modified`，没有 `cache-control`/`etag`。浏览器对 favicon 有独立于
   HTTP 语义的逐域缓存，且此域在 favicon 功能上线前（9-11 之前的 `index.html`
   无任何 icon 声明）已被访问过 → 缓存了"此站无图标"，普通刷新不会重拉。

## Decision

1. **真实 `.ico` 兜底**：生成真正的多尺寸 `favicon.ico`（16/32/48，PNG 压缩条目，
   `image/x-icon`），由 `scripts/gen-favicon.sh` 从 `web/public/favicon.svg`
   渲染并落盘到 `web/public/favicon.ico`（产物入库）。`index.html` 的 `.ico`
   链接改为带 `type="image/x-icon"` + `sizes` 的标准 `rel="icon"` 声明；
   `app.rs` 在 dist 中存在真实 `favicon.ico` 时按 `image/x-icon` 服务，不存在时
   才回退到 SVG 字节（保持旧行为，绝不让隐式 `/favicon.ico` 请求 404）。
2. **显式缓存头**：favicon 两条路由由 `get_service(ServeFile)` 直服 dist 文件
   （保留 ServeFile 自带的 Last-Modified/HEAD 语义），外包一层只作用于这两条
   路由的中间件，仅对成功响应注入 `Cache-Control: public, max-age=86400`。
   favicon 是高频且发布期才变更的资源，1 天新鲜度 + 版本化 URL 双保险；
   `index.html` 的 icon 链接加 `?v=2` 查询串破浏览器逐域 favicon 缓存（换图标
   时手动 bump 版本号）。
3. **上线联动（随本次变更执行）**：新二进制 + 新 dist 必须随镜像重建上线——
   rebuild 镜像 → push → 同步更新 deployment 两处 digest（initContainer 与主
   容器同一镜像）→ apply ConfigMap → apply Deployment → 滚动重启。旧镜像不含
   本路由与 `?v=2`，光重启 Pod 不会生效（详见 process 记录）。
3. **服务端验证已锁定**：新增集成测试断言 `/favicon.svg` 的 content-type 与
   cache-control、`/favicon.ico` 的真实 ICO magic（`00 00 01 00`）与
   `image/x-icon`、以及旧 dist（仅有 SVG）的降级回退行为。

## Alternatives considered

- **删除 `.ico` 链接、只留 SVG**：Safari 等不支持 SVG favicon 的浏览器会
  隐式请求 `/favicon.ico` 并拿到 SVG → 依然无图标；且删链接不解决隐式请求。
  落选：真 `.ico` 才覆盖全部浏览器。
- **启动时预加载字节、以显式 handler 服务**：免去 per-request 磁盘读；但对
  axum 0.8，给 `Router<Arc<AppState>>` 内单独路由绑定自有 state 不成立
  （路由 state 类型必须与路由器一致），闭包 handler 的 `Handler` 推断也编译
  不过；把 payload 塞进 `AppState` 属更大重构。落选：改动最小且保留 ServeFile
  自带 Last-Modified/HEAD 语义的方案胜出（见 Decision 2）。
- **favicon 用 `Cache-Control: max-age=31536000, immutable` + 版本化 URL**：
  对未版本化的隐式 `/favicon.ico` 请求会把旧图标缓存一年，发布期改图标后
  无痕窗口外仍旧；1 天新鲜度对 tiny 资源代价可忽略。落选：`max-age=86400`。
- **不落真实 `.ico`、把 `/favicon.ico` 的内容类型标为 `image/x-icon` 继续给
  SVG 字节**：对 Safari 是欺骗性 MIME，渲染必然失败。落选。

## Consequences

- Chrome/Firefox（支持 SVG）继续走 `/favicon.svg?v=2`；Safari/旧浏览器走
  `/favicon.ico?v=2` 拿到真实 ICO，两类浏览器都有图标。
- 部署流程多一步：改图标后需重跑 `scripts/gen-favicon.sh` 并 bump
  `index.html` 的 `?v=` 版本号（见 process 记录）。
- 旧镜像（dist 无 `favicon.ico`）行为不变：`/favicon.ico` 仍回退 SVG 字节，
  但新镜像统一携带真实 ICO。
