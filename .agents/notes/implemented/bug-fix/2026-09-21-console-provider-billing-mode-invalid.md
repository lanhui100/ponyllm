# Agent Note: 控制台新建服务商计费模式非法导致创建失败

Status: implemented

## Problem

Web 控制台新增服务商必然失败，报后端校验错误：
`无效的计费模式 'token': 仅支持 metered, plan, free`。

根因：`GovernanceView.vue` 的 `newProviderForm` 把 `billing_mode` 硬编码为
`'token'`——这是 **auth_mode** 的取值，被误填进计费模式字段；同文件另有一处
初始化（`openAddProvider`）与死代码组件 `ProviderSection.vue` 两处同样如此，共 4 处。
表单里根本没有计费模式控件，用户无法纠正，等于控制台建服务商功能整体不可用
（CLI `ponyllm provider add` 不受影响）。

值得注意的是前端单测把 `billing_mode: 'token'` 一并 mock 进了假数据，所以
单测全绿也没暴露这个跨层契约错配。

## Decision

1. 4 处硬编码 `'token'` 全部改为合法默认 `'metered'`（按量计费，与表单中
   input/cached/output 价格字段语义一致，也与 CLI wizard 默认一致）。
2. `GovernanceView.vue` 标准服务商表单新增「计费模式」选择器
   （`data-testid="provider-billing-mode-select"`，选项 `metered | plan | free`），
   让套餐类/免费类上游（如 antigravity、opencode-zen）可在控制台创建——此前
   连选择入口都没有，属功能缺口而非仅默认值错误。
3. 回归测试 `governance.flow.test.ts`：断言默认值已在 `{metered,plan,free}` 内且
   为 `metered`，并断言切换到 `plan` 后提交的 payload 原样转发（只增不改旧用例）。
4. 真实验证：Playwright 走控制台表单创建服务商成功（错误消失、卡片出现），
   验证后经 API `DELETE` 清理测试服务商，6 个既有上游无损。

## Alternatives considered

1. **只把 `'token'` 改成 `'metered'`，不加选择器**——最小改动；但控制台仍无法创建
   free/plan 类上游，用户得回 CLI，属功能缺口。否决：补选择器成本极低。
2. **改后端放宽校验，接受 `'token'` 并映射为 metered**——否决：`token` 是认证概念，
   把它塞进计费字段是数据污染；后端严格枚举正是这次拦住脏值的原因，不应削弱。
3. **前端静默兜底：非枚举值一律改发 metered**——否决：掩盖契约错配，下一个字段
   再错配时依然静默，属"把 bug 藏起来"。
4. **只加单测、不加浏览器验证**——否决：单测用的假数据本身就带着同一个错值
   （mock 里也是 `'token'`），正是漏网原因；必须真机走一遍表单。
5. **顺手删除死代码 `ProviderSection.vue`**——暂缓：删除前需确认无消费者（本次
   已 grep 确认零 import），但属独立简化变更，不与 bug 修复混提；仅同步修正其
   非法值以免复用踩雷。

## Consequences

- 控制台新增服务商恢复可用，并可按上游类型选择计费模式。
- 门禁：`web` vitest 109 passed、typecheck 0 error、oxlint 0 warning；构建通过，
  现网 `web/dist` 已重建并实测。
- 遗留：`ProviderSection.vue` 为死代码（零 import），建议后续单独走 simplification
  流程删除；`billing_mode` 目前无后端枚举的 OpenAPI 说明，前端靠约定，属后续打磨。
