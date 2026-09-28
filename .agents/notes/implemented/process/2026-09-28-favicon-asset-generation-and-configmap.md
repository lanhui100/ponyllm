# Agent Note: favicon 资产生成脚本与 ConfigMap 清单落仓库

Status: implemented

## Problem

favicon 在集群里的真实来源是 ConfigMap `ponyllm-favicon`，但它的定义只存在于
集群（2026-09-27T02:39Z 手工 `kubectl apply`），**仓库内没有任何清单**：
`deploy/ponyllm-deployment.yaml` 的 subPath volumeMount 引用它，换一个全新集群
直接 apply 该 yaml 时 Pod 会因找不到 ConfigMap 而无法启动。同时，`favicon.ico`
此前没有真实产物，也没有可复现的生成方式（改图标 = 手工临场操作）。

## Decision

1. **生成脚本入库**：新增 `scripts/gen-favicon.sh` —— 用 headless Chromium 把
   `web/public/favicon.svg` 渲染成高分辨率 PNG，再用 Pillow 缩放为 16/32/48
   并打包为 PNG 压缩条目的 `favicon.ico`，落盘 `web/public/favicon.ico`。
   脚本自带产物校验（ICO magic、条目数、PNG 签名），任何一步失败以非零退出。
   生成产物一并提交入库，改图标时重跑脚本即可。
2. **ConfigMap 清单入库**：新增 `deploy/ponyllm-favicon.yaml`，与集群现有对象
   逐字段一致：`data.favicon.svg`（文本 SVG）+ `binaryData.favicon.ico`
   （base64 ICO，二进制数据必须走 `binaryData`）。`ponyllm-deployment.yaml`
   保持对同名 ConfigMap 的引用不变，部署顺序文档化为
   "先 apply ConfigMap，再 apply Deployment"。
3. **集群同步（随本次变更执行）**：apply 新 ConfigMap 清单 → 重建并推送镜像
   （新二进制带 favicon 路由与缓存头、新 dist 带 `?v=2` 的 index.html）→ 同步
   更新 deployment 两处镜像 digest（initContainer 与主容器同一镜像）→ 滚动
   重启 `ponyllm-gateway`。两条链路缺一不可：subPath 挂载不随 ConfigMap 更新
   热生效，必须重启 Pod 才会重新读取；旧镜像不含新路由与 `?v=2`，光重启 Pod
   无效。

## Alternatives considered

- **favicon 完全内嵌进镜像（删除 ConfigMap 机制）**：`Dockerfile` 已 COPY
  `web/dist`，把 favicon 放回镜像最省事；但集群里 `ponyllm-favicon` 已存在且
  Deployment 已引用，删机制属于更大的行为变更，与"修 favicon 显示"正交。
  保留 ConfigMap，把清单补进仓库，同时镜像内也携带同内容（双保险，任一路径
  生效都不出错）。落选。
- **favicon.ico 用 ImageMagick/rsvg-convert 生成**：本机无此工具；headless
  Chromium（系统已有）对 SVG 渲染像素级保真，零新增依赖。落选。
- **favicon.ico 用纯 PIL 手绘形状**：曲线路径（碗形弧线）手工近似会失真，
  与品牌图标不一致。落选：真渲染器优先。

## Consequences

- 全新集群部署顺序变为：先 `kubectl apply -f deploy/ponyllm-favicon.yaml`，
  再 `kubectl apply -f deploy/ponyllm-deployment.yaml`；`ponyllm-favicon.yaml`
  是 favicon 内容的唯一事实源，改图标 = 改 SVG → 重跑生成脚本 → 更新清单。
- 现有集群上线 favicon 修复 = 重建并推送镜像 → 同步两处 digest → apply
  ConfigMap → apply Deployment → 滚动重启（顺序与理由见 Decision 3）。
- 生产集群在本次变更完成滚动重启后，同时从新 ConfigMap 拿到真实 ICO，且由新
  二进制按 `image/x-icon` 服务（配合 bug-fix 记录）。
