# Agent Note: 上游模型多选弹窗与显示名称

Status: implemented

## Decision

模型添加交互改为"上游名单优先、手动回退"两档（`web/dist` 已重建，
线上网关已换新二进制并重启）：

- 点「模型」按钮先调新增只读接口
  `GET /api/admin/providers/{name}/upstream-models`：chat/responses/未设协议
  走上游 OpenAI 式 `GET {base}/v1/models`（10s 超时，取首个非 Antigravity
  key 做 Bearer）；Antigravity 走既有 quota 探针取模型 ID；Anthropic 原生
  协议与无可用 key/上游非 200/空名单一律回 4xx/502。非 200 即无弹窗，
  toast 提示后直接展开原手输表单。
- 名单非空则开 `UpstreamModelPicker` 弹窗：搜索过滤、全选当页可选、
  已存在项禁用标注、多选确认后逐个复用 `saveModel`（版本控制内聚），
  409 记跳过并 toast 汇总（已添加/跳过/失败）。
- 表单「模型名称」改名为「模型 ID」（含义即上游物理标识），新增选填
  「显示名称」：`ModelConfig.display_name` 全链路（TOML/ModelSpec/Admin
  API/ModelView），仅控制台展示（行内主标题），路由仍用 ID；上一轮的
  跨 provider 联想 datalist 已整体移除（GovernanceView/ProviderCard 的
  透传一并清理）。

## Alternatives considered

- 前端直连上游 `/models`：落选。跨域与上游凭证暴露问题大；经网关代拉
  可复用 provider 代理配置与服务端 key，且失败语义统一收敛为 toast 回退。
- 后端批量创建接口：落选。N 次复用现有单体创建即满足（localhost 下 70 个
  约数十秒内完成且带逐项进度），新端点徒增版本并发语义；409 逐项判定比
  批量事务更符合"跳过已存在"的诉求。
- 显示名称参与路由/作为别名：落选。上游物理名即请求名，别名层会把本地
  404 换成上游 404 且引入永久映射机制；纯展示字段零路由风险。
- 全局 toast 系统：落选。当前仅治理页需要两处提醒，`UiToast` 单条轻量实现
  已覆盖；待第三处调用出现时再提为全局。

## Consequences

- 验证：cargo 全工作区绿（含 upstream-models 四分支测试与 display_name
  回显）；vitest 67 项绿（含 picker/回退/提交 6 项）；typecheck/lint/
  生产构建通过；线上 E2E（zen 名单 22 个、deepseek 502 回退、建→删）全通。
- 恢复条件：某 provider 的名单接口路径非 OpenAI 式时，扩展
  `upstream_models_url` 而非在前端 hardcode 特例。
