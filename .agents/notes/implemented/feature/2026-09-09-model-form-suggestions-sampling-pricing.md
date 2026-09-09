# Agent Note: 模型表单支持名称下拉与采样价格高级参数

Status: implemented

## Decision

模型新建/编辑表单现在提供两组新能力（前后端同版本落地，`web/dist` 已重建，
线上网关已换 release 二进制并重启）：

- 模型名称输入框接入 `<datalist>` 下拉：候选项为全量 provider 已配置模型的
  并集（`GovernanceView` 计算 `allModelNames`，经 `ProviderCard` 透传），并
  排除本服务商已有模型以避免 409；输入框仍可自由手输任意名称。
- 高级折叠区新增默认采样参数（`temperature` 0–2、`top_p` 0–1，留空=继承/
  不覆盖）与模型专属价格三项（输入/缓存命中/输出，$/1M，留空继承服务商）。
  后端新增 `ModelConfig.temperature/top_p`（TOML 持久化），经 `ModelSpec` →
  `RoutedTarget` 流到 chat/messages/responses 三路由：仅当请求未传对应参数
  时填入默认值，请求显式传参永远优先。Admin 读写 API 与 `ModelView` 同步五
  字段，越界 temperature/top_p 与负价格返回 400；`web/openapi.json` 已重生成。

## Alternatives considered

- 下拉候选走各上游 `/models` 实时拉取：落选。需逐 provider 凭证与协议适配
  （chat/responses/antigravity/antigravity 四形态），延迟与失败面大；已配置
  并集覆盖"把别家已验证过的模型名挂到本家"的真实场景，零网络成本。
- 自定义下拉组件替代原生 datalist：落选。datalist 原生支持键盘过滤与手输，
  无障碍与维护成本最优；样式定制需求不足以抵消自研下拉的长期成本。
- 采样参数只做前端透传（每次请求由客户端填）：落选。coding 工具多不暴露
  temperature，模型级默认值是唯一能让"该模型永远用固定采样"的落点；
  且后端已有 `max_output` 钳制的同类先例（路由层按模型声明修正请求）。
- presence/frequency penalty 一并加入：落选。超出本次明确需求，Anthropic
  侧亦无原生对应字段，留待有真实用例时再加（拒绝怀旧式预留）。

## Consequences

- 验证：`cargo test` 全工作区绿（含新增网关采样默认值测试与 Admin 读写
  测试）；vitest 64 项绿（含新增表单提交/建议/徽标 3 项）；`vue-tsc`、
  `oxlint`、生产构建通过；线上 E2E 建→查→400 负例→删全通，探针模型已清理。
- 行内模型行新增 `T=`/`￥定制` 徽标，悬停显示明细；编辑时自动展开高级区
  （当且仅当存在定制值）。
- 恢复条件：若上游某模型要求 temperature 范围超出 0–2（如有），放宽
  `validate_model_sampling` 上限而非绕过校验。
