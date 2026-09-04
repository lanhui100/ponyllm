# Agent Note: 反代 vs 直连传输实测结论与未证事项

Status: implemented

## Problem

用户体感 muse-spark 经 pproxy 反代链路生成更慢，需要数据验证而非推测。直连链（opencode CLI 经 pony-desktop 前向代理直达 opencode.ai）与反代链（经 CF Tunnel、本地网关、Vercel 函数中转）共享首跳，差异只能来自反代新增段。

## Decision

- 用免鉴权 `/v1/models` 端点做传输层 A/B（各 8 次交替，均经前向代理，与用户真实环境一致）：直连 TTFB 中位数 3.98s（1.35–7.16s，抖动大，TLS-over-WS 握手占主导），反代 TTFB 中位数 2.26s（1.82–2.50s，紧致），反代另有 1 次 60s 超时。结论是小包首字节反代反而更快更稳。
- 免费档恢复后用 `mimo-v2.5-free`（匿名、`max_tokens=96`）跑流式 A/B 各 5 次交替（`sse_bench.py`，同 prompt、同代理首跳）：TTFT 直连中位 4726ms vs 反代中位 2680ms（反代更快更稳）；常态 TPOT 两边一致（gap p50 均约 37ms，p95 约 50–60ms）；反代出现 1/5 退化事件（B#4：全程 gap p50 521ms、TTLB 18.5s vs 常态约 4s），直连 5 次无退化（最大间隔 246ms）。用户“生成慢”体感对应的是这类偶发全程退化，而非常态速度。
- 用户指正 muse 系走 OpenAI Responses 接口后，用 `muse-spark-1.3-contributor-free` 本体经 `/v1/responses`（匿名、`max_output_tokens=256`）复跑流式 A/B 各 5 次：10/10 成功，chunk 结构两边完全一致（4 事件/1828 字节，3 快 + 1 推理间隔）；TTFT 直连中位 3881ms vs 反代中位 2026ms，TTLB 直连中位 4956ms vs 反代中位 3119ms，反代 5/5 全胜且无退化事件。muse 本体上“反代更慢”不成立，体感差异应来自偶发退化事件、错误端点重试或客户端侧开销，而非反代常态速度。
- 应要求补测 usage 口径 tokens/s（`response.completed` 的 output_tokens /（TTLB−TTFT），`sse_bench.py` 同步支持 `SSE_BENCH_API/EFFORT/PROMPT/MAXTOK`）：短配额下无可见文本（推理吃掉全部预算），故用 700 词故事、`max_output_tokens=2048` 各跑 2 次。文本流平稳度两边一致（A gap p50/p95 约 19/34ms，B 约 10/29ms）；tokens/s 直连约 114–132、反代约 157–166（N 小且推理/文本配比 nondeterministic，只读方向不读精确值）；反代 chunk 更大颗（同故事 146 vs 525/241 事件，链上合并的轻痕迹，但未转化为可感知的突发）。TTFT/TTLB 仍是反代全胜。
- 用三组错误形态证明反代链透明无语义损耗：无鉴权两边同返上游 500，去余额 key 两边同返 CreditsError，跨端点模型两边同返 ModelError not supported。
- muse-spark 本体流式对比仍未执行：go key 无余额、第一方 zen 凭证不在本地可提取位置；但传输链与本次实测完全相同（同路由、同 Vercel 函数、同网关），结论可迁移。复跑配方固定为 `sse_bench.py`（SSE_BENCH_KEY/SSE_BENCH_MODEL/SSE_BENCH_SUFFIX/PONY_TOKEN 经环境变量注入）与本次落地的 `/v1/telemetry/stream` + TUI 流速行。
- 机理判定维持代码级结论：流式突发风险位在 Vercel 函数逐 chunk 转写无 flush、网关 `to_bytes` 全缓冲请求体与 `Body::from_stream` 无免缓冲头、CF Tunnel 二次回国。

## Alternatives considered

- **用 DeepSeek 付费 key 跑流式 A/B**：本地只有 `DEEPSEEK_API_KEY`，但 pproxy 无 deepseek 路由，测不到反代链。否定。
- **TLS 拦截提取第一方凭证后裸测 muse-spark**：需自签 CA 与信任库改动，风险高于收益。否定。
- **仅凭 models 端点断言流式结论**：小包 TTFB 与长流 TPOT 是不同动力学，诚实做法是分开展示。采纳后者。

## Consequences

- 用户生成期卡顿体感与首字节无关，应看 TPOT/stall 分布；新遥测的 `stream_flow` 与 TUI stall 列即为此设计。
- 反代链有偶发整体超时（1/8），高于直连的抖动但低于直连的中位数；稳定产生态建议保留直连兜底。
- `cargo test --workspace` 全绿；探针脚本已脱敏（token 改环境变量），临时的 `zenrev` provider 配置已删除。
