# ADR: 分布式网关备灾与滚动发布加固 (DR & Rolling Update Hardening)

Status: implemented
Date: 2026-10-09
Authors: Team Lead, Protocol Agent, Test Agent, Executor-A, Executor-B

## Context & Problem Statement

2026-10-09 16:45（CST），一次由 CI 部署驱动的网关发版滚动（commit `be69082`）触发了 4 个 gateway Deployment（`dev` / `preprod` / `proserver` / `tencent`，均绑定独立物理节点）的**同时滚动更新**。

由于历史清单中各 Deployment 均为 `replicas: 1`，且 CI deploy job 采用单次 `kubectl apply` 同时下发 4 个 Deployment 配置：
1. 整个集群 4 个节点的旧 Pod 在同一时间窗口内全部进入终止流程（preStop 25s + 进程内 graceful drain 上限 60s）；
2. 导致下游所有持存活长连接（SSE 流式推理）的 DSH 会话在途请求被强制掐断，全网大面积出现 `connection error`；
3. 现役多层容灾架构（Traefik 加权分发 + readinessProbe 摘除 + model fallback chain）的有效窗口仅覆盖**新连接路由**与**首字节前重路由**，无法迁移已建立的在途长流；优雅停机 60s 预算远小于 LLM 深度思考长流时长，且合成探针此前仅做单 token 离散探活，存在长流连续性验证盲区。

## Decision

由 Dev Team 经对抗审核、契约冻结、红相断言、正交实施与栅栏审查，完成以下三项工程加固：

### 1. C1: CI 流水线分批串行滚动 (Serialized Rollout across Roles)
- 将单次全量 `kubectl apply` 重构为分批串行调度脚本 `scripts/serialized-rollout.sh`，按权重从大到小（`dev` → `preprod` → `proserver` → `tencent`）依次发布；
- 相邻批次间执行确定性硬门禁：
  1. `rollout status` 收敛检查（300s 超时、3 次重试）；
  2. 旧 Pod drain 收尾确认（等待旧 Pod 实例数归零且无 Terminating 状态 Pod，最长轮询 300s）；
  3. C3 长流连续性门禁判读；
- 任一环节失败自动执行 `rollout undo` 回滚并阻断后续节点滚动，避免全集群雪崩。

### 2. C2: 副本数扩容与单写者持久化语义隔离 (Replicas Scaling with Single-Writer Invariant)
- `preprod`、`proserver`、`tencent` 三个无状态节点 Deployment 副本数提升至 `replicas: 2`，即使发生单 Pod 滚动/崩溃，节点内仍保留存活副本承接连接；
- `dev` 节点受制于 local-path RWO PVC（`ponyllm-data`）单节点挂载限制及 ADR 2026-10-01 确立的 telemetry snapshot 单写者契约，**保持 `replicas: 1`**，并在清单中显式固化契约标记注释：
  `# contract C2: dev single-writer exception — replicas MUST stay 1 — ponyllm-data (RWO local-path) single-mount + telemetry snapshot single-writer (ADR 2026-10-01); preprod/proserver/tencent = 2`

### 3. C3: 合成探针长流连续性断言与 CI 批次门禁 (Long-Stream Continuity Assertion in Prober)
- 增强 `deploy/prober.py`，新增 SSE 流式长流探测模式（`stream: true`）与 CLI 门禁形态（`--long-stream-once` / `--long-stream-check`）；
- 确立清晰的 85s drain 预算截断判据（`LONG_STREAM_DRAIN_BUDGET_SECONDS = 85` = preStop 25s + drain 60s）：
  - 流中断且 `duration >= 85s` 判定为干净截断（Clean Truncation，PASS 语义）；
  - 早于 85s 中断判定为缺陷掐断（Defect Cut，FAIL 语义，非零退出）；
- 暴露 Prometheus 指标 `ponyllm_synthetic_long_stream_total`、`_complete_total`、`_cut_total`、`_defects_total`，并在 `deploy/ponyllm-prober.yaml` 注入环境变量及将内存限制提升至 256Mi。

## Alternatives considered

1. **将 dev 节点同样强行扩为 replicas: 2**：
   - 否决。`ponyllm-data` 是 local-path RWO PVC。两个 dev Pod 同节点挂载会导致底层 snapshot 文件被并发双写撕裂，违背 ADR 2026-10-01 的 telemetry 数据真相源承诺。保留 dev=1 并由另外 3 个节点的 6 副本吸收主要流量是最佳权衡。
2. **改用蓝绿双 Deployment 架构（dev-blue / dev-green 等 8 个 Deployment）**：
   - 否决。配置管理复杂度翻倍，Traefik IngressRoute 规则数量翻倍，且在节点资源有限（4C4G 边缘节点）场景下会占用过多常驻资源与空闲端口。串行滚动 + replicas: 2 已足实现同等防雪崩效果。
3. **将进程内 drain 超时调大至 300s 以上**：
   - 否决。LLM 慢思考可能持续数分钟甚至更久，无限期等待会导致 CI 发布流程严重阻塞，且无法根治 TCP 连接中途被客户端掐断的问题；截断不可避免，关键在于避免全集群同一时刻集体截断。

## Acceptance & Verification

1. **红绿相双向证明**：
   - 红相阶段：当前基线 HEAD 运行 `tests/pre-flight/` 下 4 个断言脚本全红（EXIT=1）；
   - 绿相阶段：全部实施完毕后重新运行 4 个脚本全部转绿（EXIT=0）。
2. **静态与编译校验**：
   - `scripts/serialized-rollout.sh` 通过 `bash -n` 与 `bash scripts/serialized-rollout.sh --dry-run` 语法与逻辑模拟（EXIT=0）；
   - `deploy/prober.py` 通过 `py_compile` 及 mock SSE 截断判据单元测试（EXIT=0）；
   - `.github/workflows/ci.yml` 通过 `actionlint` 与 `python yaml` 语法解析（EXIT=0）；
   - `deploy/ponyllm-deployment.yaml` 与 `deploy/ponyllm-prober.yaml` 通过 `kubectl apply --dry-run=client` 客户端校验（EXIT=0）。
