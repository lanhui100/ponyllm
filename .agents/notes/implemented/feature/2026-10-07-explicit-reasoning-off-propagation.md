# Agent Note: 显式 reasoning_effort=off 透传上游并回程清洗

Status: implemented

## Problem

客户端显式传 `reasoning_effort: "off"`（或等价 `thinking: {type: "disabled"}`）时代，
网关把它与「调用方没传思考参数」这两种情形折叠成同一个处理分支：既不向
`Chat` 协议上游写 `reasoning_effort`，也从 `extra` 里删掉同名键。对
OpenAI/Sense 兼容链路上默认强制推理的上游（如 `deepseek-v4-flash`、zen free
层），字段缺失等于「用上游默认」，于是上游继续吐 `reasoning_content`：

- 请求侧：用户要的是「别思考」，上游收到的是「随便你」；
- 响应侧：即便上游因自身默认（强制推理）返回了思考内容，网关原样透出，
  token 成本与首字延迟都被抬高，而调用方已声明不要。

这两个折叠点必须拆开：「显式 off」是调用方的指令，必须落到线上字节；
「未指定」才允许省略。

## Decision

1. **请求构造区分显式 off**：当 `requested_thinking == ReasoningEffort::Off`
   且解析后的 `effective_thinking` 非激活时，把 `reasoning_effort` 保留为
   `Some(Off)`，不再走「置 None + 从 `extra` 移除 `reasoning_effort`/`thinking`」
   的省略分支。适用面：`chat.rs`（OpenAI `/v1/chat/completions`）与
   `messages.rs`（Anthropic `/v1/messages`）两条入口。
2. **上线字节双写**：序列化后对 body 对象补写 `reasoning_effort = "none"` 与
   `thinking = {"type": "disabled"}`。前者命中 OpenAI/Sense 系，后者命中
   Anthropic 系，两键并存以覆盖异构上游；不依赖目标 provider 的协议猜测。
3. **响应回程清洗**：仅对 `UpstreamProtocol::Chat` 的原始响应，在显式 off 时
   遍历 `choices[].message` 删除 `reasoning_content` 与 `reasoning` 两键。
   协议转换路径（Antigravity 等）不做额外清洗——它们由各自的转换器负责。
4. **回归面锁定**：`thinking_gateway_tests::test_thinking_scrubbing_for_non_reasoning_models`
   扩到 4 个请求——前 3 个「未指定」请求仍必须完全不带
   `reasoning_effort`/`thinking`，第 4 个「显式 off」请求必须带
   `reasoning_effort == "none"` 与 `thinking.type == "disabled"`。同一测试同时
   钉住两侧语义：省略分支没有被这次改动顺带污染。

## Alternatives considered

- **只在响应侧清洗，不改请求体**：落选。上游仍按其强制推理默认执行全部思考
  （烧 token、拖 TTFT），只是结果被丢掉——治标不治本，且账单照付。
- **只发 `reasoning_effort: "none"`，不发 `thinking` 键**：落选。Anthropic 系
  上游不认 `reasoning_effort`，单写会退化成「字段缺失 = 用默认」，对强制推理的
  Anthropic 上游完全无效。双写是覆盖两个协议族的最小手段。
- **把 `Off` 映射成网关内部的「跳过思考路由」，由池选择器挑非推理模型**：
  落选。思考开关属于请求语义，不应改变选路——同一模型在不同调用方下可能需要
  相反的思考档位，混进路由层会让池的评分与配额语义失真。
- **在协议转换层统一处理（改 `ponyllm-protocol`）**：落选。`/v1/messages` 与
  `/v1/chat/completions` 的思考参数在各自 handler 里已各自解析成型，上提到协议层
  需要把 pool 的 `UpstreamProtocol` 分派一并搬动，改动面远大于收益，且两条入口
  的字段名并不一致。

## Consequences

- 显式 off 的调用方在上游侧真实关闭推理，TTFT 与 token 成本下降；响应不再携带
  思考内容。
- 未指定思考参数的调用方行为**不变**（回归测试第 1–3 个请求钉住）。
- `pool/` 模块同批次的纯 `cargo fmt` 重排与本决策无关，可独立回滚。