# Agent Note: 发布验收 [5/7] 改按副本配对判 delta——跨副本采样不再误判锁错误增长

Status: implemented

## Problem

`main` 上连续两次生产部署（`45405367795`、`45418316435`）都在 `Post-deploy full verification
(phase3-verify)` 这一步红，红点分别是：

```
run 45405367795：[5/7] FAIL metrics 不可达（E0='' E1='0 0 0'）——无法判定锁错误 delta，中止
run 45418316435：[5/7] 30s 窗口: acquired 0->32539 | skipped 0->18110 | errors 0->191
                 FAIL refresh_lock_error_total delta +191 != 0
```

第二次的 `+191` 不是真实增长。逐副本直采真值（`kubectl port-forward` 逐 pod 打
`/v1/telemetry/metrics`，同一时刻）：

| 副本 | acquired | skipped | errors |
|---|---|---|---|
| `ponyllm-gateway-dev` | 32539 | 18110 | **191** |
| preprod ×2 / proserver ×2 / tencent ×2 | 0 | 0 | 0 |

dev 副本的 191 是自 48 分钟前启动以来**恒定**的历史累计（连采三次不变，`git` 里没有任何本次
变更触及 `refresh_lock.rs`）。它只在**那一个副本**上非零，而另外 6 个副本全 0。

根因是测量口径而非被测行为：HA 计数器（`refresh_lock_*_total`、`config_reload_total`）是
**进程内 `AtomicU64`**，各副本独立累加、Pod 重建即归零。`phase3-verify.sh` 的 [5/7] 经
`https://tokens.ponyjob.top`（LB）各采一次快照，两次可能落到不同副本——本机复采 8 次即命中
1 次 dev、7 次其它副本。于是「换了副本」与「计数器真增长」在数值上完全不可区分，旧口径只看
`R1 > R0` 就判 FAIL。负 delta 当时已判 WARN，正 delta 是盲区。

同源缺陷在 [4/7] 也存在（`config_reload_total` 同样按进程累加），只是这次没被触发。

## Decision

**给计数器加「副本身份」，把 delta 判定限制在同副本样本之间**，而不是继续靠「负 delta = 换了
副本」这种单侧启发式。

1. **`MetricsSummary.instance_id`**（`ponyllm-core/src/telemetry/metrics.rs`）：进程级
   `OnceLock<String>` 生成一次 UUIDv4，`get_summary()` 带出。
   - 进程级而非 collector 级：一个副本 = 一个进程，HA 计数器也按进程累加，两者必须同粒度；
     collector 级会让同进程内两个 collector 伪装成两个副本。
   - `#[serde(default)]`：旧载荷（无该字段）反序列化为空串，跨版本滚动期间新旧副本并存不炸。
   - PG 聚合的集群汇总（`cluster_telemetry::query_cluster_metrics`）显式留空串——它跨副本 SUM，
     本来就不是任何单一进程的计数器，不能参与 delta 判定。

2. **`scripts/lib/ha-window.sh`**：把「采样 + 判定」从 `phase3-verify.sh` 抽成可单测的库。
   - `ha_snapshot_line` 输出 `"<instance_id> <acquired> <skipped> <error>"`；不可达 / 非 JSON /
     缺 `ha_ops` 一律输出空行（不伪造数字）。
   - `ha_window_verdict` 按 `instance_id` 分组，只在同副本的前后样本之间判 delta：
     同副本 error delta > 0 → FAIL；同副本 Δ==0 且有活动 → OK；同副本无活动 → WARN 证据不足；
     **没有任何副本同时出现在前后两侧 → WARN 跨副本，exit 0（不判 FAIL）**。
     最后一档是本次修复的要害：跨副本 delta 不是「锁错误增长」的证据，不得据以拒绝发布。

3. **`phase3-verify.sh` [5/7]**：前后各采 3 个样本（经 LB 单次采样可能只命中一个副本，多点
   采样才凑得出同副本前后对），交给 `ha_window_verdict`。硬证据路径不变：需要逐副本严格口径
   时用 `--pod-ips` 直采。

4. **`tests/pre-flight/ha-window.spec.sh`**：14 条断言，含 P4「跨副本采样不判 FAIL」与 P5
   「复刻 0→191 误判场景必须放行」——把这次的承诺变成非零退出判据。

## Alternatives considered

- **继续用「负 delta = 换副本、正 delta = 真增长」的单侧启发式**：本次事故就是它的反例——
  从全 0 副本切到 dev 副本恰好是**正** delta。启发式无法覆盖「低值副本 → 高值副本」这一方向，
  加更多分支只是把猜测堆高。
- **把 dev 副本的计数器在发布前清零（重启 Pod）**：会打断生产流量，且把「观测口径缺陷」
  转成「运维每次发布多做一次重启」，等于用运维纪律兜测量缺陷；下次换别的副本又会复发。
- **在 `phase3-verify.sh` 里内联 if/else 判副本，不抽库**：判定逻辑需要负样本自测（跨副本、
  历史高值、单侧空快照等 6 类），内联在 400 行脚本里无法独立单测，也不符合既有
  `scripts/lib/http-retry.sh` + `tests/pre-flight/http-retry.spec.sh` 的成对范式。
- **只用 `--pod-ips` 强制逐副本直采，不做身份字段**：CI runner 在集群外，Pod IP 不可达
  （脚本注释已明示），逐副本直采在 CI 里根本跑不了；身份字段是唯一能在「经 LB」这条路径上
  也给出正确结论的做法。

## 验证

| 命令 | 结果 |
|---|---|
| `bash tests/pre-flight/ha-window.spec.sh` | **pass=14 fail=0**（含 P4/P5 跨副本不判 FAIL） |
| `cargo test -p ponyllm-core --test telemetry_tests` | **11 passed**（新增 instance_id 恒定性 + 序列化契约 2 条） |
| `cargo test -p ponyllm-server --test admin_store_conflict_http_tests --test telemetry_full_gate_tests` | 6 + 2 passed |
| `cargo check --workspace --all-targets` | rc=0 |
| `bash scripts/check-verify-contract-drift.sh` | PASS（脚本语法 / API 契约 / Deployment 名 / TOML 示例） |
| `bash tests/pre-flight/http-retry.spec.sh` | rc=0（既有探针重试自测未回归） |

### 上线实测（PR #8 合并后，run 45444791468）

`main` 部署**全绿**（此前连续两次红），`[5/7]` 实际输出转为 WARN 而非 FAIL：

```
== [5/7] 锁 + 刷新健康（同副本窗口 delta 口径）
WARN no replica sampled on both sides (before=91e611a2…,ed3a29a4…,def9152f… after=91437cc3…,5f18eefc…,6d9eaeca…)
     -- sampling through a LB hits random replicas; a cross-replica delta is NOT evidence of lock error growth.
```

**同时暴露第二个缺陷**：每侧只采 3 次，7 副本下 6 次命中 6 个不同副本、零配对 → 门禁降级成
「证据不足」，等于没判（生日问题：n=7/k=3 碰撞率仅约 39%）。已改为自适应：
`HA_SAMPLES = 2 × 副本数 + 2`（7 副本 → 16 次）。k ≥ n+1 时由鸽笼原理保证**必然**存在重复，
同副本配对不再是概率事件。实测 16 次采样命中全部 7 个副本（3/3/2/2/2/2/2），
`ha_window_verdict` 输出 7 条配对行，dev 副本 `errors 191->191` 恒定 —— 正是本缺陷要拦的形态，
现在被判为「无活动 WARN」而不是「+191 FAIL」。
