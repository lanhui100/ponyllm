# Agent Note: 单key透传与诚实耗尽文案加Retry-After

Status: implemented

## Problem

`opencode-zen/muse-spark-1.3-contributor-free` 单 key 在 dev 网关上一次上游 429 后，后续 20s（现 3s 基线）内全部本地 429，一次上游都不打，coding 工具体感断流。根因有三：单 key 复用多 key 冷却隔离、无收益却制造黑洞；`last_pool_exhausted = NoAvailableKey || active==0` 把本请求打过上游全败与本请求没打上游混成同一句 `Local key pool exhausted`；本地 429 不带 `Retry-After`，客户端只能盲重试。

## Decision

单 key 池 RateLimit 只计数不冷却，永远可选中透传上游；多 key 保持 3s 指数退避上限 60s 即时换 key 不等待；耗尽判定只认 `CoreError::NoAvailableKey`，不再附加 `active==0`；本地耗尽与上游耗尽文案分别标注网关侧与上游侧，并在 429 上回 `Retry-After` 头（上游值优先，否则最早解锁向上取整）。

## Alternatives considered

- **单 key 也统一 3s 冷却，不做特判**：保持 bf9cc13 无分叉，单 key 仍有 3s 黑洞；未采纳，3s 内 coding 工具第二轮重试仍撞墙，违背单 key 无轮替语义。
- **网关内 sleep 2s/5s/10s 再重试同一 key**：单请求最长被捂 17s，再叠客户端 5-10 次退避到分钟级，且阻塞流式 TTFT；未采纳，等待留给客户端，网关只做即时换 key。
- **修改已落地的 2026-09-07 退避 ADR 原文**：把新决策并入旧记录省一条文件；未采纳，已实施记录是历史事实，改写会丢失 20s 到 3s 的演进可审计性，按 write-adr 多类拆条与状态轴不漂移原则补新条。

## Consequences

- 单 key 不再有本地 429 黑洞，永远返回真实上游状态与 body。
- 多 key 行为不变，只是误报消除，客户端能按 `Retry-After` 退避。
- `cargo test -p ponyllm-core --test pool_tests` 与网关耗尽文案测试通过；`bash .agents/skills/write-adr/verify-note.sh` 通过。
