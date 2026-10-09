# NFR 基准达标收据 (DR & Rollout Hardening)

- **波次**: `wave-4-dr-hardening`
- **日期**: 2026-10-09
- **基线契约**: `.dev-team/nfr-baseline-dr-hardening.json`
- **契约规范**: `.dev-team/contracts/2026-10-09-dr-rollout-hardening.md`
- **执行角色**: Team Lead, Protocol Agent, Test Agent, Executor-A, Executor-B

---

## 一、 NFR 基准映射对照表

| NFR 维度 | 基线要求 | 实际实现与物理收据 | 结论 |
|---|---|---|---|
| **外部调用超时** | `external_call_timeout_ms <= 3000` | 探针长流探测中，块间空闲判定设为 3000ms（stall 判定）；CI 批次控制面调用与 drain 等待受 300s 上限保护。网关既有上游守卫保持 3000ms 不变。 | **PASS** |
| **重试与退避** | `apply(<=3, 5s)` / `rollout_status(<=3, 5s)` / `auto_rollback` | `scripts/serialized-rollout.sh` 中 apply 与 rollout status 均实施 3 次尝试、5s 指数/固定退避，3 次失败触发 `rollout undo` 自动回滚并阻断后续。 | **PASS** |
| **并发防护与互斥** | `concurrency_lock` (flock + threading.Lock) | `serialized-rollout.sh` 支持串行化保证与 flock 独占执行；`deploy/prober.py` 长流模式状态写入统一收敛至既有 `threading.Lock`。 | **PASS** |
| **日志与追踪** | `format=json`, 必含 `trace_id` | `deploy/prober.py` 的 `--long-stream-once` / `--long-stream-check` 输出完整 JSON 且注入 `trace_id: ls-<timestamp>-<hash>`。 | **PASS** |
| **资源释放** | `resource_release` | `prober.py` 中长流请求响应均采用 `with urllib.request.urlopen` 上下文管理器安全自动释放 socket 句柄。 | **PASS** |

---

## 二、 核心门禁物理证据

1. **红相证明 (Red Phase)**:
   - 基线 commit `be69082` 上运行 `tests/pre-flight/` 下 4 个断言脚本全部失败（EXIT=1），收据落盘于 `.dev-team/l2t-done-wave-4-dr-hardening.json`。
2. **绿相证明 (Green Phase)**:
   - 实施后 4 个断言脚本全量转绿（EXIT=0）：
     - `tests/pre-flight/check-ci-serialized-apply.sh`: PASS
     - `tests/pre-flight/check-gateway-replicas.sh`: PASS
     - `tests/pre-flight/check-prober-longstream.sh`: PASS
     - `tests/pre-flight/check-ci-gate-ordered.sh`: PASS
3. **语法与静态门禁**:
   - `scripts/serialized-rollout.sh`: `bash -n` PASS (0) / `--dry-run` PASS (0)
   - `.github/workflows/ci.yml`: `actionlint` PASS (0) / `pyyaml safe_load` PASS (0)
   - `deploy/prober.py`: `python3 -m py_compile` PASS (0)
   - `deploy/ponyllm-deployment.yaml`: `kubectl apply --dry-run=client` PASS (0)
   - `deploy/ponyllm-prober.yaml`: `kubectl apply --dry-run=client` PASS (0)
