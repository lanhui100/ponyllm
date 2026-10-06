# Agent Note: 模型级"走代理"开关 + opencode-zen 默认直连

Status: implemented

## Problem

1. **opencode-zen 上游默认整体走 pproxy 代理链**（base_url = 本机 8899 的 token 路径），
   `fledge-alpha-free` / `mimo-v2.5-free` 这类本可直连原 zen
   （`https://opencode.ai/zen/v1`）的模型也被迫绕代理，链路长且依赖 pproxy/VPS 存活。
2. **Web 管理端无法按模型指定"走代理/直连"**：模型编辑表单没有该控件；
   且 admin API 的模型 update 语义有缺口——`proxy` 发 `null` 被跳过（无法清空）、
   发 `""` 会触发出口 URL 策略门禁 400（无法回关），开关即使做了也无法真正"关掉"。

## Decision

1. **架构**：`opencode-zen` 默认直连原 zen
   （`base_url = "https://opencode.ai/zen/v1"`）。ponyllm 内建的 OpenCode Zen 路由门
   （`is_opencode_zen_target`：x-opencode-session/x-opencode-client/UA
   `opencode/1.18.31` + 12 名工具列表 body 注入 + `*-free` 模型强制流式聚合）使其直连可用。
   需要代理的模型（如 `muse-spark-1.3-contributor-free`）在**模型级**指定
   `proxy = "http://127.0.0.1:8899"`（pproxy 出海代理，回环地址为出口策略文档化放行形态）。
2. **前端**（`web/src/components/governance/ModelSubSection.vue`）：模型创建/编辑表单新增
   **"走代理"checkbox + 代理地址输入**（开关打开默认填 `http://127.0.0.1:8899`，
   可改地址；关闭 = 直连/继承服务商）。提交 `CreateModelPayload`/`UpdateModelPayload.proxy`
   （类型已存在）。模型列表行 tooltip 显示 `走代理: <url>`。
3. **后端**（`crates/ponyllm-server/src/routes/admin.rs`）：
   - `ModelView` 增加 `proxy` 字段（GET 两个端点 / create / update 四处分叉都填充）；
   - create/update 归一化 `proxy`：空串或 `"direct"` → `None`（直连），
     真实 URL 走既有 `check_proxy_url_fast` 出口策略门禁。开关"关闭即清空"由此生效。

## Alternatives considered

- *保持现状（opencode-zen 整体走 8899 链）*：fledge/mimo 无谓绕代理，且与"直连可用"事实不符——拒绝。
- *开关只写死 8899、不允许改地址*：不同部署的代理地址不同，写死不便迁移——采用开关 + 可编辑地址输入。
- *update 继续 `proxy:null` 跳过语义*：开关回关无法清空既有 proxy（缺口仍在）——拒绝，
  归一化空值→None 使"关"成为一等操作。
- *前端用自绘 switch 组件*：代码库现有控件一致风格为原生 checkbox（如"缓存 token 计入 TPM"），
  复用该模式避免新增组件面——采用 checkbox。

## Consequences

- 本地网关（`/tmp/ponyllm-local.toml`）实测：`fledge-alpha-free`、`mimo-v2.5-free` 直连原 zen 可用，
  `muse-spark-1.3-contributor-free` 经 per-model 代理可用。
- 出口守卫不削弱：直连目标是公网 `opencode.ai`；代理是回环 pproxy（既有合法形态）。
- **部署提醒（靠 review）**：Web 改动需重建 web/dist 并随镜像发布（k8s 网关）；
  镜像配置 `job_copilot_marketing/.ponyllm-gw/ponyllm.toml` 若上线需同样迁移为直连 + per-model proxy。