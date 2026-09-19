# Agent Note: 客户端可见错误信息携带请求模型名

Status: implemented

## Problem

实网上报的请求（`gemini-3.8-flash`，`req_18d6914225281097`）在全部候选上游耗尽后，返回给客户端的错误为：

```
All candidate upstream providers exhausted (upstream-side failure, gateway did attempt upstream). Last error: Upstream error (status 400 Bad Request): {"error":{"code":400,"message":"User location is not supported for the API use.",...}} (request_id: req_18d6914225281097)
```

整条消息没有出现模型名。客户端（尤其是多模型转发的 agent / bench 工具）无法从错误本身判断是哪个模型失败，必须回查 `x-ponyllm-request-id` 与服务器日志才能对上号；三级端点（`chat / responses / messages`）的耗尽消息同源同缺失。

## Decision

`format_exhausted_message` 增加 `model` 参数，在两分支（本地池耗尽 / 上游全部耗尽）的消息中嵌入 `for model '<model>'`：

- `Local key pool exhausted for model '{model}' (gateway-side cooling, ...)`
- `All candidate upstream providers exhausted for model '{model}' (upstream-side failure, ...)`

三个路由调用点（`routes/chat.rs`、`routes/responses.rs`、`routes/messages.rs`）统一传入 `ParsedRequestModel::raw_requested_model`（客户端原始请求串，含 `[1m]`/`:strategy` 等后缀，便于与客户端所发内容一一对应）；`tests/request_routing_tests.rs` 的断言同步更新。

## Alternatives considered

- **把模型名塞进 `CoreError`（如 `AllRetriesFailed`/`UpstreamStatusError`）Display：否定。** 核心执行器是提供商无关的，模型名属于路由/入站层；在 core 层改动错误契约会让所有消费者（遥测、frame、SDK 路径）跟着变，收益仅限单一外层消息。
- **只改用户命中的 chat 端点：否定。** 三个端点共用同一 `format_exhausted_message`，只改一处会造成跨端点不一致的客户端契约。
- **嵌入 `physical_model`（路由后的真实模型）而非 `raw_requested_model`：否定。** 客户端看到的是自己发的名字；`physical_model` 已通过 `x-ponyllm-routed-model` 响应头暴露，不需要重复进错误文案。
- **在 `UpstreamStatusError` 文案里补 key/provider 信息：否定。** 属于核心错误契约变更，超出本次"错误信息带模型名"的边界，留待后续。

## Consequences

- 客户端错误信封（OpenAI `error.message` / Anthropic `error.message`）自 v0.2.43 起携带 `for model '<请求模型名>'`，无需回查日志即可定位失败模型。
- `cargo test -p ponyllm-server` 与 `cargo test --workspace` 全绿。