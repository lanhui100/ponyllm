# Agent Note: 审核整改三批 18 项

Status: implemented

## Decision

按审核路线图 5.5 分三批落地 18 项加 1 项特批加固：P0 闭环 6 致命——并发工具合并（chat/anthropic 双侧聚合器加同角色归一化）、两处流式 `item_id` 纠正、`ResponsesToAnthropicFsm` 改 done 驱动生命周期（done 事件闭合、终端关全块、惰性文本块、done 锁存与 `finish_if_open`）、6 个流式封装错误透传（`Infallible` 改泛型 `E`，遥测断流记 `StreamFailed`，出错后不再合成成功帧）、SDK 与网关统一复用 `core::endpoints` 归一化、`resolve_effective_protocol` 注入 `inbound` 使合一 DeepSeek 原生直通；P1 加固 9 高危——排序改 DSU 快照、`responses` 归一对称、旧封装同步错误透传与 FSM 终端合成、SSE 64KB 有界与 EOF 残缺丢弃、非流式工具调用回 `ToolCalls`、旧 FSM 补 `done` 锁存与 `finish_if_open`、SDK 路由确定性字典序加未知模型严格报错；P2 落工程 5 项——启发式单轨统一、messages snippet 取 wire 形、池耗尽改结构化聚合（首跳 `NoAvailableKey` 直返）、CLI 严格校验加向导覆盖确认、translator 对抗断言（合并/交替/混合序列）；图片丢弃打 `warn`，纯图片跨协议翻译网关前置 400；驳回 3 项维持。

## Alternatives considered

- **中危/低危一并整改：否定。路线图未采纳，扩散范围拖延发版门禁；记入后续跟踪。**
- **流式错误保留错误帧再透传 Err：否定。单槽位二选一，透传 Err 保遥测真实，客户端表现为截断不断言成功。**
- **透传优先改主序：否定。P4 tiebreak 语义不变，审核未推翻。**
- **chat 侧连续 FunctionResponse 合并入单条 Tool 消息：否定。单条 Tool 消息只能携带一个 `tool_call_id`，合并即丢映射；anthropic 侧可合并因 ToolResult 是块。**

## Consequences

- `cargo test --workspace` 全绿（含新增对抗断言与回归测试），`clippy` 零新增告警；旧分体配置与旧 TOML 加载不受影响。
- SSE 有界丢弃与 EOF 残缺丢弃改变既有宽容行为，已由新增单测锁定；chat 流终止符 `[DONE]` 保留（终止语义非成功断言）。

