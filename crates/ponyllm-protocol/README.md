# ponyllm-protocol

## 模块职责
各大主流 LLM 协议数据结构及双向转译状态机。

## 契约与核心组件
- `openai::chat`: OpenAI Chat Completions 协议模型；
- `openai::responses`: OpenAI Responses API 协议模型；
- `anthropic::messages`: Anthropic Messages 协议模型；
- `translator`: 3×3 双向协议转译器与流式 FSM：
  - **角色交替保证**：连续同角色消息（如并发工具调用）自动合并文本与 `tool_calls`，消除连续 Assistant 触发下游 400 校验异常；
  - **因果偏序保证**：前置思考链（`Thinking`）与说明文本严格先于工具调用（`ToolUse` / `FunctionCall`）发射；
  - **流式 FSM 状态机**：`ChatToResponsesFsm`, `ResponsesToChatFsm`, `AnthropicToResponsesFsm`, `ResponsesToAnthropicFsm` 四组状态机，具备 `done` 状态锁存、`saw_tool` 动态终止符（`ToolUse` / `EndTurn`）、首帧空增量拦截与 EOF 自动全块闭合。
