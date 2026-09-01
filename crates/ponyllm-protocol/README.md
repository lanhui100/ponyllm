# ponyllm-protocol

## 模块职责
各大主流 LLM 协议数据结构及双向转译状态机。

## 契约与核心组件
- `openai::chat`: OpenAI Chat Completions 协议模型；
- `openai::responses`: OpenAI Responses API 协议模型；
- `anthropic::messages`: Anthropic Messages 协议模型；
- `translator`: 双向协议转译器（含 Tool Calling、Multimodal Data URI 与 DeepSeek Reasoning 思考链无损传递）。
