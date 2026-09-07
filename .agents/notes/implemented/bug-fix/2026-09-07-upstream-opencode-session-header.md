# Agent Note: 上游补发 x-opencode-session 会话头与自有 UA

Status: implemented

## Problem

AI coding 工具经网关调用 `muse-spark-1.3-contributor-free`（上游 `responses` 协议）时，
上游以 `400 MissingSessionID` 拒绝：“Request is missing x-opencode-session and
cannot be routed efficiently”（OpenCode Go 文档要求每会话携带稳定的
`x-opencode-session` 做路由与 prompt caching，并用自有 UA 而非通用 SDK 标识）。
网关 `UpstreamExecutor::build_headers` 只发 `Authorization`/`x-api-key`，
三入口（chat/responses/messages）把下游头全部丢弃，直调上游复现了同一 400；
带上该头直调即 200，确认根因在缺头而非请求体。

## Decision

会话头逻辑集中收敛到 `ponyllm-core::executor::upstream`，三路由只透传下游
`HeaderMap` 并按目标计算作用域，单点契约：

- 作用域门 `is_opencode_zen_target(provider, url)`：`opencode*` provider 名或
  URL 含 `opencode` 段命中，`/go/` 路径恒排除——仅 zen 接口生效，go 与其他
  上游不受影响。
- 作用域内 `resolve_upstream_session` 按 `x-opencode-session > x-pony-session >
  x-session-affinity > x-session-id` 透传下游会话（覆盖 opencode 原生头与
  Codex/Claude 等原生会话头形态），缺失时合成 `ponyllm-<uuid>`（过
  MissingSessionID 门；跨请求的会话稳定仍要求下游发头，已在文档注明）。
- `UpstreamExecutor::with_downstream_headers` 在网关请求级解析一次，
  同一请求内所有 key 重试复用同一 session，不逐 attempt  churn；
  `with_opencode_zen(bool)` 默认关闭，非 zen 目标保持历史线头形状
  （client 级不再设全局 UA，避免对无关上游的任何行为漂移）。
- 作用域内 `build_headers` 必带 `x-opencode-session` 及其别名
  `x-session-affinity`/`x-session-id`、`x-opencode-client`（下游有则透传，
  否则 `ponyllm`），以及 `User-Agent: ponyllm/<version>`。
- 无下游头的调用方（SDK/embedded/旧测试构造的 executor）在作用域内默认即得
  合成 session，纵深防御，zen 不再有漏头路径。

## Alternatives considered

- 逐路由各自拼头：三处重复优先级与合成逻辑，与现有 translator 集中化方向相悖，
  新增协议入口必漏，否决。
- 每请求随机 session 但不透传下游：能过 400 门，但破坏上游按会话的路由亲和与
  缓存，且多会话用户被并入同一桶；透传优先+缺失合成是其超集，否决纯随机。
- 网关内存按（token+model）缓存合成 session 实现跨请求稳定：引入状态与过期
  策略，多实例下仍不一致，且伪造的稳定不如客户端真实会话；留作后续
  “完整会话亲和”档，不做在本修。
- 伪装 `User-Agent: opencode/...`：直调验证 `ponyllm/0.2.26` 即过门，文档要求
  自报家门，伪装增加滥用误判风险，否决。
- 全局无条件加头：简单但把 opencode 专用头泄漏到 sense/bai 等无关上游，
  未来对端收紧未知头即无谓故障；改为 zen 作用域门 + 默认关闭，否决全局。

## Consequences

- `cargo test -p ponyllm-core -p ponyllm-server` 全绿（含
  `session_header_tests` 6 例：优先级、别名回退、缺失合成、zen 必带头、
  非 zen 零漂移、`is_opencode_zen_target` 含 go 排除与大小写）。
- 本机构造回放 harness 双分支验证：非 zen provider 出站头与历史完全一致
  （无 session/client/UA 冗余头）；`opencode*` provider 有头→原样透传；
  真实 zen 上游 `/v1/responses` 与 `/v1/chat/completions` 均由 400 转 200
 （429 仅为验证期高频触发的上游限流，门已过）。
- 约束：合成 session 仅单请求内稳定；需要跨请求缓存命中率的调用方必须发送
  `x-opencode-session`（或 `x-pony-session`）。
