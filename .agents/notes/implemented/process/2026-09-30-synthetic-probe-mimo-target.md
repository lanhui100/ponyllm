# Agent Note: 合成探针修复与目标切换（mimo-v2.5-free / pproxy 链路）

Status: implemented

## Problem

合成探针设计为每 30s 一次真实推理 canary（ADR 2026-09-26），但现网 `ponyllm-prober-script` ConfigMap 是**旧版脚本**：`__main__` 里 `run_probe()` 只在启动时执行一次后直接 `serve_forever()`，没有循环。实测 `probe_total=1`、`probe_last_timestamp` 落后约 50h（与 Pod 启动时间一致）——**"每 30s 真实推理"从未真正运行**，Metrics 展示的是 50 小时前的单次结果。且目标 `deepseek-flash`（deepseek 直连）只 canary 直连链路，pproxy 出海路径（muse-spark / gemini 一类）完全盲区。

## Decision

1. 用仓库新版 `deploy/prober.py`（含 `background_loop` + `PROBE_INTERVAL` + 线程锁 + `status_class` 标签 + mgmt token 鉴权）重新生成 `ponyllm-prober-script` ConfigMap 并 apply Deployment（env 变更触发 rollout）。
2. 探针目标按用户指定改为 **`mimo-v2.5-free`**（`PROBE_MODEL` env）：路由 provider = `opencode-zen`，base_url 走 pproxy 节点（100.95.193.103:8899）——canary 覆盖面从"deepseek 直连"扩到"pproxy 出海链路"。
3. 新增 `PROBE_TIMEOUT` env（默认 90s，对齐网关 90s TTFB 预算）：opencode-zen 首 token 实测 10–40s（同 provider 的 muse-spark 压测 TTFT 8–39s），原 10s 硬超时必 100% 误报。

## Alternatives considered

- 保持 `deepseek-flash`：仍只 canary 直连链路，pproxy 盲区不解决，弃。
- 目标轮换（round-robin 多 provider）：覆盖更均衡，但超出当前单目标探针设计，留作后续。
- `PROBE_TIMEOUT` 保持 10s：mimo 首 token 10–40s 必然全红误报，否决。

## Consequences

- 探针恢复 30s 真实推理循环：实测 `probe_total` 持续递增（1→2→3）、`last_timestamp` 新鲜、`probe_success=1 / status_class=2xx / failures=0`；`probe_duration_seconds` 5.1s→33.8s，如实反映 opencode-zen 上游思考时长（在 90s 预算内）。
- 副作用：此前"每 30s 真实推理是否浪费"的讨论是空谈（探针根本没在跑）；现在它**真正开始消耗配额与上游算力**（2 次/分，全部经 pproxy 打 opencode-zen）——后续如需降本可评估降频或目标轮换。
- 注意 mimo/opencode-zen 链路故障（pproxy 节点、上游）现在会直接反映为 `probe_success=0`。
