# Agent Note: 服务商协议多选无法持久化（chat+responses 不能同时选中）

Status: implemented

## Problem

Web 模型管理页的服务商「协议与调度配置」胶囊支持多选，但选中 chat+responses
保存后，刷新只剩 chat——responses 胶囊灭掉，用户以为"无法同时选择"。

根因是前后端契约错配：后端 `ProviderConfig` 里根本没有"支持协议列表"字段，
只有 `default_protocol`（取胶囊第 0 个）+ 三个 per-protocol URL（`chat_url` /
`messages_url` / `responses_url`）。前端保存时只把**手填了专属 URL** 的协议发过去，
URL 留空的发空串（后端解为空 = 未声明）。Moyo 这类同 base 双通的上游三个 URL
全空，于是 responses 的选中态落盘即丢，`getInitialProtocols` 下次只能回显 chat。

## Decision

1. `ProviderCard.handleSaveProtocols`：选中的协议若专属 URL 留空，自动用 `base_url`
   回填做显式声明；未选中的协议仍发空串（显式关闭）。同 base 双通上游的派生
   结果与原来一致，行为不变，只是落盘声明了支持。
2. `GovernanceView` 新建服务商表单同规则：选中协议的空 URL 用当次 `base_url` 回填。
3. 回归测试 `ProviderCard.test.ts`：选中留空 → `chat_url`/`responses_url` 回填
   base、未选的 `messages_url` 为空、`default_protocol` 取首个；回填后重渲染两
   胶囊均高亮。

## Alternatives considered

1. **后端加 `supported_protocols: Vec` 字段**——最正交；但要动磁盘格式、运行时
   配置、resolve 逻辑与 openapi，面太大。现有三 URL 即支持声明，语义已够用。
   否决：前端回填达到同样持久化效果。
2. **回填时把所有协议（含未选）都填 base_url**——否决：未选=用户显式关闭该协议面；
   全填等于多选框失效，且会覆盖用户之前的手动清空。
3. **模型级也允许多选**——否决：后端 `native_protocol(model)` 是单值 schema，
   模型协议单选与后端一致，不动；Moyo 模型 `protocol` 为空走 provider 继承，
   双入站都通，无需模型级多选。
4. **只修 ProviderCard，不修新建表单**——否决：同一病根两处发病，新建时选
   chat+responses 同样丢，顺手同规则。

## Consequences

- moyo 已在控制台勾选 chat+responses 并持久化（`chat_url`/`responses_url` 均显式落盘），
  经网关 `/v1/chat/completions` 与 `/v1/responses` 双 200。
- 门禁：`web` vitest 111 passed、typecheck 0 error、oxlint 0 warning；构建通过；
  Playwright 真机（登录→展开卡片→双选→保存→重载回显）ALL PASS。
- 遗留：若某上游 chat 与 responses 实际是**不同 base**，回填 base_url 会指错——
  此时用户必须手填专属 URL（校验仍在：必须 http(s) 开头）。属已知边界，已在
  保存逻辑注释中说明。
