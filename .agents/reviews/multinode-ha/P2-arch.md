# T7 Phase 2 生产部署对抗审核报告（架构红队）

- 审核对象：线上生产部署（Pod `ponyllm-gateway-fc75cb8d7-9s85z`，image `3bfad2f9…`，kubernetes 后端 config_version=159）+ `deploy/ponyllm-lockdb.yaml` + `deploy/ponyllm-phase2-rollback.md` + impl 验证 a-f（对应事实已通过只读 kubectl 复核）
- 审核人：arch-reviewer
- 核查方式：只读 kubectl（get/describe/logs/exec-只读 curl 失败后改用日志证据），无任何写操作

---

## 总体结论：**有条件通过**（无 S1；2 项 S2 建议尽早修正：回滚预案 R1 的 lock env 语义错误、仓库部署清单与线上 Phase 2 漂移。观察期可照常进行）

线上状态与设计一致：kubernetes 后端（159 版本、9 providers）、`ponyllm-live-config`（rv=52317818）、锁库 TLS + 专用 role、RBAC 白名单最小权限、热更新无 2s 风暴、rotated_at 独立 key 隔离全部实测成立。回滚预案整体可执行，但 R1 有一条 env 语义错误。以下按聚焦逐条 + 发现分级。

---

## 一、聚焦逐条核验

### ① 锁库部署形态与生产一致性 —— 通过
【证据】（manifest `deploy/ponyllm-lockdb.yaml` 与 live 对照）
- replicas=1 / strategy=Recreate / emptyDir `pgdata`+`cert`（lockdb.yaml:63-65, 202-205）；live pod `ponyllm-lockdb-8456b79695-rpzf5` Ready、0 重启、devserver 上
- 安全：pod-level `runAsNonRoot: true / runAsUser 999 / fsGroup 999 / seccomp RuntimeDefault`（yaml:77-83）；initContainer 以 root + 仅 `CHOWN/FOWNER/DAC_OVERRIDE`（yaml:107-115，为 postgres uid 999 预置目录与证书所必需，范围收敛）；主容器 `allowPrivilegeEscalation: false + drop ALL`（yaml:188-191）
- probes：startup pg_isready(60×5s) + readiness(6×5s) + liveness(6×10s)（yaml:166-180）；resources req 100m/256Mi → lim 500m/512Mi（yaml:181-187）
- TLS：`ssl=on` + 绝对路径 /certs/* + `pg_hba.conf` scram-sha-256（yaml:130-144, 98-101）；凭据经 Secret `ponyllm-lock-dsn` env 注入；网关侧 `PONYLLM_LOCK_SSLMODE=require` + `PONYLLM_LOCK_CA_FILE=/etc/ponyllm-lock/ca.crt`（live pod env，`refresh_lock.rs:182-215` `load_lock_roots` 已实现 CA bundle 校验，非系统根）
- 一致性：manifest == live ✓
【备注（S3 级，不阻断）】单副本 + Recreate：锁库自身滚动/节点驱逐窗口内刷新串行化短暂失效（网关 fail-closed 跳过）——可接受；观察口径应预期 `refresh_lock_error_total` 偶发 +1，勿误判。

### ② 热更新 A3（注释行变更 → reload → 还原）—— 真实
【证据】网关日志（pod 36m 生命周期内恰好 4 次 reload，身份序列）：
- 02:07:17 `config change detected (identity=2044f7149ed6)` → 同秒 `Gateway configuration reloaded`
- 02:07:51 `(identity=c570b5ffed72)` → reload（34s 后第二次变更）
- 02:10:50 `(identity=82ec482d7571)` → reload
- 02:12:50 `(identity=90faddd675b1)` → reload —— **该身份 = `deploy/ponyllm-phase2-rollback.md:16/50` PVC sha256 前缀 90faddd675b1860…**：还原步骤确实把 Secret 字节恢复到与基线一致的 90faddd…（A3 闭环）
【局限】容器内无 curl/wget（尝试 exec curl/wget 均 127），`reload_total` 计数器无法直接读；日志身份序列是机械替代证据，且与 impl 报告口径一致。观察期建议用 `kubectl port-forward` 读 `/v1/telemetry/metrics` 落计数器基线（见 S3-2）。

### ③ reload 稳定性（无 2s 风暴）—— 支撑观察期
【证据】02:12:50 之后至核对时刻（≥25 分钟）**0 次 reload**、身份恒为 90faddd…；若 P1 S1-1 缺陷仍在，日志会每 ~2s 出现一条 "config change detected"（约 750 条/25min），实际为 0。keepalive 于 02:10:56 正常启动，且**无任何** "lock backend unavailable / refresh skipped: lock backend unavailable" WARN → PG 锁路径可用（单副本必获锁）。impl“95s 恒定”与本次 ≥25min 恒定一致且更充分。

### ④ rotated_at 独立 key 不触 reload —— 隔离成立且可持续（有约束）
【证据】`admin_store.rs load_raw_hash` 只对 `data['ponyllm.toml']` 字节哈希（`snap.data.get(self.data_key)`）；`rotated_at` 是独立 data key（现值 1790648243，写入走 `patch_rotated_at` 独立 merge-patch）。观测：Secret rv 已到 52317818（大量 rotated_at/写回产生的 rv 变化），但 02:12:50 后 0 次 reload —— rv/aux key 变化确实不触发 reload。
【可持续性约束（S3）】隔离成立的前提是“身份永远只算 ponyllm.toml 数据键”。未来若把身份改为“整个 data map 哈希”或整 Secret 哈希，隔离即被破坏；请在代码注释与 ADR 固化该约束（写回任何新 aux key 前复查）。

### ⑤ 回滚预案 R1/R2/R3 可执行性 —— 基本可执行，1 项 S2 修正
【证据】逐条过脑 + 线上存量核对：
- **R1 命令语法**：strategic patch 的 command/args/SA/automount 还原（rollback.md:17-25）、config-ro 播种源还原（:27-28）、rollout status + 验证（:31-33）均有效、幂等。
- **“PVC 与 live-config 逐字节一致、config_version=159”**：日志 02:12:50 还原身份 90faddd675b1 == 文档 sha256 前缀 90faddd675b1860… → 该断言当前成立（附注：这是把“还原坐标”与“回滚验收”统一在同一哈希体系的良好闭环）。
- **R2**：`patch image sha256:b1788e90…` 语法正确，但 **digest 被截断（"…"）不可直接复制执行**，且需确认该旧镜像仍存在于仓库（S3）。
- **R3**：诊断命令有效。
- **R1#2 陈旧播种**：切回 `ponyllm-config`（125 陈旧）仅在 PVC 缺失时触发丢 159 —— 文档备注（kubectl cp / live-config 手动播种 159）已覆盖（rollback.md:53-54）；建议把“先确认 PVC 文件存在”写入步骤 0（S3）。

---

## 二、发现清单

### S2-1 回滚 R1 保留 PONYLLM_LOCK_* env + 步骤 5 删除 lockdb ⇒ 回滚后 antigravity 刷新 fail-closed、token 停止刷新
【证据】rollback.md:29-30 明确“env 可保留无副作用；如需彻底移除：kubectl set env …”；:34 同时给出“锁库可保留或下线（kubectl delete deploy ponyllm-lockdb + svc job-copilot-lockdb）”；CLI 逻辑（cli/main.rs）：`PONYLLM_LOCK_DATABASE_URL` 存在即构造 PostgresRefreshLock 并注入 refresh_gate；`refresh_lock.rs` PG 不可达 → `RefreshGateError::Unavailable` → antigravity.rs gate 分支 → `RefreshSkipped`/`Err`（fail-closed，永不绕过锁）。
【问题】R1 回滚到 file 后端后若仍保留 env、又执行“下线 lockdb”，则每次刷新 `try_acquire` 连接失败 → fail-closed 跳过 → **所有 antigravity token 不再刷新直至过期**——而 Phase 2 之前（file 后端、无 env、无 gate）刷新是无条件直刷。“无副作用”陈述在 delete-lockdb 分支下不成立，属行为回归且无声（仅 WARN）。
【修复建议】R1 把 env 移除改为**必选**步骤（置于 rollout 前）：`kubectl -n ponyllm set env deploy/ponyllm-gateway PONYLLM_LOCK_DATABASE_URL- PONYLLM_LOCK_CA_FILE- PONYLLM_LOCK_SSLMODE-`；或明确二选一（保留 lockdb 则锁库不可删）。回滚验收追加一条：`/api/admin/overview` 后观察 gateway 日志出现 antigravity keepalive 且无 "lock backend unavailable"。

### S2-2 仓库部署清单与线上 Phase 2 漂移：任何 `kubectl apply -f deploy/ponyllm-deployment.yaml` 会静默回退 Phase 2
【证据】HEAD 的 `deploy/ponyllm-deployment.yaml`（:34 replicas=1 / :52-53 nodeSelector / 旧 init / config-ro 挂 `ponyllm-config` / 无 SA / 无 PONYLLM_LOCK_* env）仍是 Phase 2 前形态；线上是 patch 过的 Phase 2（args `--config-backend=kubernetes`、SA ponyllm-gateway-sa、config-ro→ponyllm-live-config、lock-tls 卷、CA_FILE env，均经 live pod jsonpath 实测）。而 `ponyllm-lockdb.yaml` 已提交。
【问题】keel 只盯镜像所以当前不自动回退；但任何一次手工/gitops `apply` 会把线上静默退回 file 后端单节点（丢 kubernetes 真相源、丢刷新串行化——S2-1 同源风险），且与 rollback 文档“现状基线”矛盾。
【修复建议】Phase 3 前把 `deploy/ponyllm-deployment.yaml` 同步为线上 Phase 2 状态（以 rollback.md “现状基线” 为蓝本），或在清单头部显式标注 “live 为 patch-managed，清单待 Phase 3 重写”。

### S3 项
1. **S3-1 容器无 HTTP 客户端**：exec curl/wget 均为 127；观察期 `/v1/telemetry/metrics`（reload_total / refresh_lock_hold_seconds / 冲突计数）需 `kubectl -n ponyllm port-forward deploy/ponyllm-gateway 8080:8080` 从运维侧读取。建议把该命令连同 impl 已报告的“95s 恒定 / reload_total 基线”正式写入观察期 SOP。
2. **S3-2 lockdb 每 ~6min 的 SSL reset 噪音**（`SSL error: unexpected eof while reading` / `Connection reset by peer`，00:59→01:42 共 13 次/2h）：均发生在当前网关 Pod（02:07 启动）之前；当前 Pod 生命周期内网关侧 0 条 PG connection error、keepalive 锁轮成功 → 高度疑似旧 Pod 空闲 TLS 连接被对端关闭的良性日志噪音。观察期盯 `refresh_lock_error_total` 与 PG 连接数确认无泄漏（10min 节流的 rotated_at 也与此无关）。
3. **S3-3 R2 镜像 digest 截断**：补全 `sha256:b1788e90…` 后放入脚本；通过 `kubectl get deploy ... -o jsonpath image` 留档旧 digest 备查。
4. **S3-4 rotated_at 隔离的代码级约束文档化**（见聚焦④）。
5. **S3-5 锁库单副本/无 PDB**：自身滚动或节点驱逐窗口内刷新串行化短暂失效；可接受，观察口径预期偶发 `refresh_lock_error_total` +1。

---

## 三、采纳清单建议

### 必须采纳（S2）
1. **S2-1**：R1 的 lock env 移除改为必选步骤（或“保留 lockdb”为唯一下线路径），并给回滚验收加“keepalive 无 lock-err”断言。
2. **S2-2**：Phase 3 前同步 `deploy/ponyllm-deployment.yaml` 至线上 Phase 2 形态（或显式标注 patch-managed）。

### 建议采纳（S3）
3. S3-1 观察期 SOP 落 `port-forward` + 计数器基线快照；S3-2 观察项加入“lockdb SSL reset 与 refresh_lock_error_total 相关性”；S3-3 补全 R2 digest；S3-4 代码注释固化“身份只算 ponyllm.toml 键”约束；S3-5 观察口径预期偶发锁错误。

### 可驳回
- 其余（锁库形态、TLS/CA 方案、RBAC、probe 参数）维持现状。

---

## 复核命令（只读，本报告已执行）

```bash
kubectl -n ponyllm get deploy,po -o wide                                  # 1 副本 + nodeSelector + 3 deployments
kubectl -n ponyllm get pod ponyllm-gateway-fc75cb8d7-9s85z -o jsonpath=…  # args/SA/env/volumes/probes（已核）
kubectl -n ponyllm get secret ponyllm-live-config -o json | …             # rv=52317818, config_version=159, providers=9, rotated_at 存在
kubectl -n ponyllm logs deploy/ponyllm-gateway -c ponyllm --tail=3000 | grep -nE "config change detected|Gateway configuration reloaded"   # 4 次 reload + 身份序列
kubectl -n ponyllm logs deploy/ponyllm-lockdb --since=2h | grep -cE "SSL error: unexpected eof"   # 13 次良性噪音
kubectl -n ponyllm get role ponyllm-config-rw -o jsonpath=…               # secrets get/patch + resourceNames=[ponyllm-live-config]
# 观察期基线（写 SOP）：
kubectl -n ponyllm port-forward deploy/ponyllm-gateway 18080:8080 & curl -s -H "Authorization: Bearer $KEY" http://127.0.0.1:18080/v1/telemetry/metrics | jq '.ha_ops'
```