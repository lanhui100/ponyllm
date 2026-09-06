# Agent Note: Web拨测游乐场与接入指南

Status: proposed

## Problem

`curl` 手测三协议费时，跨协议转译（thinking 保留）无处对比；Claude Code、Codex、OpenCode 三工具接入串靠手抄 BaseURL 易错，后续工具需可插拔追加。

## Proposal

将新增 Playground：左选模型与协议 Tab（chat/messages/responses）加 thinking 四档下拉（Off/Low/Medium/High，沿用统一标尺），中为 SSE 打字机并计时 TTFT/TPS，右为请求 JSON、路由解释（`X-Pony-*` 头）与 cURL 一键复制，支持同 prompt 双协议对比。thinking 请求走 `X-Pony-Thinking` 头，回显 effective 档位与是否被天花板截断。Integrations 按工具注册表给 BaseURL、Key、Model 三行复制（首版 Claude Code、Codex、OpenCode，见下），附 `获取模型列表` 联通测试。

## Alternatives considered

- **接入完整 Chat 客户端：否定。Web 定位运维控制台，Playground 只做拨测，不做历史会话存储。**
- **路由解释靠前端猜：否定。以网关响应头为准，无头则显未知，不编造命中 Key。**
- **双协议对比经两次串行：否定。并行发并分栏渲染，TTFT 可比。**
- **每工具硬编码一 Tab：否定。工具增删改走注册表数据（id/名称/配置模板/测试端点），首版只预置三工具，后续加工具不改代码。**

## Acceptance criteria

- 同 prompt 经 chat 与 messages 双发，thinking 保留差异可见靠 review 演示。
- Playground thinking 下拉仅四档，截断时显 `requested High → effective Medium（天花板）`。
- 首版三工具（Claude Code、Codex、OpenCode）复制串与当前网关地址同源生成，`/v1/models` 联通测试成功；新增工具仅加注册表一行，无需改页面结构（靠 review）。
- SSE 中断重连不丢已吐 token，已吐部分保留并标中断点。

## Risks

- 大 SSE 流前端内存涨，需上限截断并提示下载全文。
- 模型名含转译后缀时回显规则需与网关 Model Echo 对齐。
