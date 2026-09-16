# Agent Note: pool 默认路由由 round_robin 改为 priority 粘滞

Status: implemented

## Problem

pool 内多 Key 默认 `round_robin`，每个请求 `rr_counter + 1` 逐个轮转健康 Key。
上游（Anthropic/OpenAI/GCP）的 prompt-cache 亲和性按 `账号/Key + prompt` 计算，
轮换等于每个账号各暖一份缓存：首轮全是 cold start，多付全量 input tokens，
缓存命中率被人为摊薄。同时 N 个 Plan/免费账号被同时缓慢打光，而不是逐个打光，
`QuotaExhausted -> CoolingDown` 的逐个 failover 纵深失效。

## Decision

所有“缺省/空/未知/新建”路径的 pool 默认策略为 `priority`（粘滞主备）：
永远选中仍 `Active` 的最低 priority 数字 Key，仅当主 Key 非 `Active`
（Quota 枯竭/429 冷却/永久隔离）时 failover，恢复后自动粘回。
显式 `strategy = "round_robin"` 的存量 provider 行为不变（各映射点保留显式 RR 臂），
`round_robin` 降级为分摊 RPM 的显式 opt-in。

改动点（生产 8 文件）：

- `ponyllm-config/src/config.rs`：`default_strategy()` 缺省 `"priority"`；
  示例配置 openai/anthropic 同步为 `priority`。
- `ponyllm-server/src/config.rs`：同名缺省同步。
- `ponyllm-server/src/routes/admin.rs`：`default_strategy_str()`、
  create-provider 空串兜底、`parse_pool_strategy` 未知臂（+ 显式 `round_robin` 臂
  与 `weighted` 别名统一）、OAuth 自建 provider、OAuth 热建池复用
  `parse_pool_strategy`（消灭内联 match 漂移）；未知值 `tracing::warn` 留痕。
- `ponyllm-cli/src/main.rs`：抽出 `parse_pool_strategy`（trim+lower 归一，
  与 server 语义对齐，含 `weighted_round_robin` 别名），serve 建池调用它；
  未知值 `tracing::warn` 留痕。
- `ponyllm-cli/src/cli.rs`：`provider add --strategy` 默认 `"priority"`。
- `ponyllm-cli/src/oauth_agy.rs`：agy OAuth 自建 provider 默认 `"priority"`。
- `ponyllm-cli/src/wizard.rs`：选项文案标注默认推荐与伤缓存提示，
  兜底回退 `"priority"`。
- `ponyllm-cli/src/tui.rs`：`STRATEGIES` 首位 `"priority"`，
  Edit 未知回退下标同步指向 priority。

测试（4 新增）：`pool_tests` 粘滞回归（20 次 pin 主 → 429 failover →
恢复粘回 → excluded 绕行）；`strategy_cli_tests` 缺字段反序列化得 priority；
CLI/server 各一组字符串→枚举映射测试（显式 RR/别名/归一/未知回退）。

## Alternatives considered

- A（采用）：未知/空值 fallback → Priority + warn 日志。创建路径已有
  `validate_provider_fields` 严格校验（非法直接 400/报错），读/重启路径的
  脏数据兜底走粘滞并留痕；改动面最小，不引入新错误码。
- B（落选）：未知值直接报错 fail-fast。更干净，但改动面扩大到错误码、
  前端与历史脏数据的启动阻断；留待后续：先 warn 可观测一版，确认无脏数据后再收紧。
- C（落选）：删除 `round_robin` 实现。分摊单 Key RPM/TPM 仍是合法场景
  （无状态 API、无缓存亲和需求），删实现是过度收缩；保留为显式 opt-in。

## Consequences

- 缺字段老 TOML 重启后自动从轮询变粘滞（单 Key 用户无感；多 Key 靠 RR
  分摊 RPM 的用户压力集中到主 Key，直到 failover）。发布说明须声明 breaking
  与 `strategy = "round_robin"` opt-out 写法；回滚单向：已落盘 `priority`
  的新建 provider 回滚后保持 priority。
- 网关层 `GatewayRoutingStrategy`（Economy 等）未动，两层语义正交。
- 验证：`cargo test -p ponyllm-core -p ponyllm-config -p ponyllm-cli`
  全绿；`cargo test -p ponyllm-server` 全绿（含新增映射测试）；
  独立 reviewer + 兼容性复核双 LGTM。TUI 默认高亮与向导文案靠 review。
