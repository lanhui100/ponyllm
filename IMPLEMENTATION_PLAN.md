# ponyllm 实施路线规划 (IMPLEMENTATION_PLAN)

## 阶段 1: Workspace 架构与核心协议模型

**目标**: 建立 Cargo Workspace 拓扑；定义强类型的 OpenAI Chat Completions、OpenAI Responses API 与 Anthropic Messages 协议数据模型（包含 Request、Response、Streaming SSE Chunk、Tool Definition/Call、Usage、Reasoning 字段），提供零拷贝/低开销的 serde 序列化与反序列化。
**成功标准**: Workspace 编译通过，所有协议模型序列化与反序列化测试覆盖率 100%，覆盖典型标准 payload 与边缘情况。
**测试**: 各协议官方样例的 serde 单元测试（单轮/多轮会话、工具调用、思考链、流式事件 chunk）。
**状态**: 已完成

## 阶段 2: 双向透明协议转译引擎

**目标**: 实现 OpenAI Chat ↔ OpenAI Responses ↔ Anthropic Messages 三方协议的非流式与流式 SSE 双向转译状态机（FSM）。
**成功标准**: 任意入参协议可无损转译为对端协议，流式 tool_call 参数增量拼接与 usage 结算完全对齐。
**测试**: 双向协议互转金标测试集、流式 chunk 状态机聚合与输出对比测试。
**状态**: 已完成

## 阶段 3: 多 Key 账户池化与故障倒换调度器

**目标**: 实现多 Provider、多 Key 账户池；支持权重/轮询/健康度调度；集成 429 限流检测、配额耗尽判定与指数退避熔断器；实现首字喷出前的透明重试倒换。
**成功标准**: Key 状态机自适应流转，模拟 429/5xx 故障时自动无感倒换至下一个健康 Key，并发安全。
**测试**: 故障注入单元测试、并发 Key 竞争与倒换测试、熔断冷却恢复测试。
**状态**: 已完成

## 阶段 4: Axum HTTP 网关服务与黑匣子遥测录波

**目标**: 基于 Axum 构建对外兼容端点（`/v1/chat/completions`, `/v1/responses`, `/v1/messages`）；集成 Tracing、实时 Metrics（TTFT、TPS、Token 消耗）与环形缓冲区故障录波（Flight Recorder）系统。
**成功标准**: 标准客户端直连网关完成端到端请求与流式消费；异常请求现场自动脱敏录制落盘。
**测试**: 端到端 HTTP/SSE 网关集成测试、流式客户端模拟测试、故障录波输出校验。
**状态**: 未开始

## 阶段 5: CLI 交互终端、配置系统与库形态封装

**目标**: 编写 CLI 命令行交互工具（配置管理、Key 状态实时查看、服务启动）；梳理 `ponyllm` 根库接口，支持作为嵌入式 crate 直接被 `pony-agent` 调用。
**成功标准**: CLI 子命令完备（`serve`, `keys`, `status`）；库形态提供简洁的内存级调用 Trait/Struct。
**测试**: CLI 命令解析与执行测试、嵌入式调用集成测试。
**状态**: 未开始
