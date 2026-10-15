# Decision Record: 切换合成推理探针基准模型为 deepseek-flash 并增强 HTTP 429 限流感知

Status: implemented
Date: 2026-10-15

## Context
1. `ponyllm-synthetic-prober` 作为整个生产 AI 网关的存活监控探针（指标 `ponyllm_synthetic_probe_success{service="ponyllm"}` 决定服务是否健康报警）。
2. 此前探针目标模型为 `mimo-v2.6-flash-free`。OpenCode 免费层近期对客户端门禁与频控收紧，探针探测触发 403 / 429 后导致网关将 `opencode-zen` 密钥全池打入冷却，且探针误报整个网关宕机（实际 deepseek/claude/antigravity 等商业模型全量健康）。
3. 且探针 Pod 镜像是 10 月 11 日的旧版本镜像，未随网关自动更新。

## Decision
1. **探针模型变更**：
   - 将 `deploy/ponyllm-prober.yaml` 中的 `PROBE_MODEL` 与 `deploy/prober.py` 默认值切换为 `deepseek-flash`（高可用稳定直连模型）。
   - 将探测间隔 `PROBE_INTERVAL_SECONDS` 恢复为 30s（直连稳定模型无免费额度污染风险）。
2. **HTTP 429 限流分流感知**：
   - 在 `prober.py` 中增加对 HTTP 429 状态的处理：上游 429 属于 `throttled`（上游限流，网关链路响应正常），新增 `probe_throttled` 状态，不作为致命 `failure` 计入 `probe_success=0`，避免上游频控误报服务宕机。
3. **镜像同步**：
   - 更新 `deploy/ponyllm-prober.yaml` 镜像 Digest 与网关生产运行镜像保持一致（`sha256:0f1ab544d34a9ff10d5c08de19485352c31a81d4ddfd03b385d04620d54ae2ee`）。
4. **集群热更新**：
   - 更新 `ponyllm` 命名空间下的 ConfigMap `ponyllm-prober-script`，应用最新的 `deploy/ponyllm-prober.yaml` 并执行滚动重启。

## Alternatives considered
- 维持 `mimo-v2.6-flash-free`：免费模型易被上游封控和波动影响，不适合作为全网关 SLA 级探针。
- 不区分 429：429 会导致夜间批处理或突发流量时误触发运维值班告警。
