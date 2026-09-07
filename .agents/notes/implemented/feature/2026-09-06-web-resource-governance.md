# Agent Note: Web资源治理表单

Status: implemented

## Problem

Provider/Model/Key/Strategy 的增删改仍依赖 CLI 长参数与 TUI 键盘表单，字段多（协议覆盖、三地址、计费、thinking）易错，且 Key 明文易泄露。

## Decision

采用 Tab 面板与抽屉表单平移 TUI Modal：Provider 三栏表（策略与协议徽、计费单价）、Model 表（Tier F/S/L、多模态图标、默认星）、Key 表（脱敏指纹、priority/weight、拨测徽）。全局策略四卡单选 Economy/Speed/Reliable/Balanced。

灰度只读与并发安全控制：
- 顶部只读警示条：网关未开启写权限（`admin_write_enabled=false`）时，顶部常驻警示，所有 CUD 变更动作置灰禁用并明确提示。
- `If-Match` 乐观并发控制：表单操作携带全局配置版本凭证（`config_version`），遇 412 `precondition_failed` 时弹出冲突告警并引导刷新获取最新配置，避免相互覆盖。
- 新增 Key 安全约束：明文仅在创建成功响应时通过专用弹窗一次性展示与复制，关闭后立即在前端内存中销毁，严格禁止写入 localStorage/sessionStorage/cookie/IndexedDB。删除二次确认并提示在途请求不受影响。Token 与 Provider Key 明确区隔显示。

思考强度沿用已落地的统一 4 档标尺（`Off/Low/Medium/High`）：Model 表单暴露 `thinking_default/thinking_max` 两下拉，语义为 ModelThinkingSpec 地板与天花板（`effective = min(requested.unwrap_or(default), max)`，非推理模型天花板为 `Off`）；网关派发前对非思考模型清洗思考字段防上游 400，超出上限截断夹紧不报错。

## Alternatives considered

- **单页大表单全堆一屏：否定。字段超 12 项，大表单误触率高，分 Sheet 按资源拆。**
- **Key 明文回显以便复制：否定。与 recorder 脱敏冲突，明文仅新增瞬间可复制一次。**
- **策略用下拉单字段：否定。四策略语义差异大，四卡配适用场景更不易误选。**
- **冲突直接自动覆写重试：否定。后端带 If-Match 校验，并发冲突时强制弹窗让管理员确认差异，防止静默丢配置。**

## Consequences

- 资源治理全套视图落地（`/governance`），包含 Provider、Model、Key、Strategy 四大维度管理，管理员脱离终端即可完成配置运维。
- 保证了并发安全与灰度安全：412 冲突拦截与提示闭环，`admin_write_enabled=false` 状态下写操作严格禁止。
- 零持久化敏感凭据：Key 明文仅创建瞬间于内存一次性显式展示，关闭后立即销毁，完全符合零泄露安全禁令。
- 探针拨测能力前置：提供行内单 Key 实时拨测与全量排队拨测，进度与延迟状态一目了然。
