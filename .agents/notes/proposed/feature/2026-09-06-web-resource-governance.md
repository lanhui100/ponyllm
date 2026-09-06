# Agent Note: Web资源治理表单

Status: proposed

## Problem

Provider/Model/Key/Strategy 的增删改仍依赖 CLI 长参数与 TUI 键盘表单，字段多（协议覆盖、三地址、计费、thinking）易错，且 Key 明文易泄露。

## Proposal

将以 Sheet 表单平移 TUI Modal：Provider 三栏表（策略与协议徽、计费单价）、Model 表（Tier F/S/L、多模态图标、默认星）、Key 表（脱敏指纹、priority/weight、拨测徽），操作 `a/e/d/s` 对齐 TUI。新增 Key 只显一次，删除二次确认并提示在途请求不受影响。全局策略四卡单选 Economy/Speed/Reliable/Balanced。

思考强度沿用已落地的统一 4 档标尺（`Off/Low/Medium/High`，见已落地决策，不另起标尺）：Model 表单仅暴露 `thinking_default/thinking_max` 两下拉，语义为 ModelThinkingSpec 地板与天花板（`effective = min(requested.unwrap_or(default), max)`，非推理模型天花板为 `Off`）；Playground 与请求头走 `X-Pony-Thinking` 三级优先级（头 > 模型名后缀 > 请求体），网关派发前对非思考模型清洗思考字段防上游 400，超出上限截断夹紧不报错。

## Alternatives considered

- **单页大表单全堆一屏：否定。字段超 12 项，大表单误触率高，分 Sheet 按资源拆。**
- **Key 明文回显以便复制：否定。与 recorder 脱敏冲突，明文仅新增瞬间可复制一次。**
- **策略用下拉单字段：否定。四策略语义差异大，四卡配适用场景更不易误选。**

## Acceptance criteria

- 四表单字段与 TUI Modal 1:1，空 Provider 时引导模板创建可用 review 演示。
- Model 表单 thinking 两下拉仅 Off/Low/Medium/High 可选，默认回退与截断语义与网关 `ModelThinkingSpec::resolve` 一致（靠 review 对 `thinking_gateway_tests.rs` 用例）。
- `POST /api/admin/keys/test` 延迟徽与全部拨测进度条可用，命令见 WEB-04。
- Token 与 Provider Key 区隔警示条常驻，误用串 Key 单测拦截。

## Risks

- 三协议 URL 覆盖填错导致路由失败，需表单内联校验 base_url 格式。
- 并发编辑同一 Provider 覆盖，需后端写队列版本号校验。
- thinking 天花板误设为 High 导致弱模型被注入思考参数，需表单按模型名推断上限并提示（与网关推断同源，靠 review）。
