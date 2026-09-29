# P2 生产部署对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 2 已上线生产部署（ponyllm-gateway fc75cb8d7-9s85z，image `3bfad2f9…`，`--config-backend=kubernetes`，sa=ponyllm-gateway-sa；锁库 ponyllm-lockdb；ponyllm-live-config rv159）
- 审核日期：2026-09-29
- 审核方式：只读实况核查（ssh dev 上的 kubectl get/describe/auth can-i/logs；Secret 值仅 decoded 非敏感 `rotated_at` 时间戳，`ponyllm.toml` 与 DSN 只看大小/来源分类、未打印内容）；无任何写操作
- 审核范围：① RBAC 生产生效矩阵；② 锁库 env/DSN 无泄漏；③ rotated_at 前进真实性；④ admin 412 语义；⑤ 回滚预案凭据面

## 总体结论：**通过（有条件通过）**

Phase 2 生产部署的凭据面与权限面**实测与 ADR 最小权限设计完全一致，无 S1**。①RBAC 矩阵、②DSN 走 SecretRef + CA 走 Secret 卷 + 日志零泄漏、③rotated_at 前进与真实刷新活动强相关、④412 映射（代码 + seam 测试确认，实况写验证待办）均通过；⑤回滚预案存在一个 **S2 凭据面缺口**（回滚到 file 后端时旧 PVC 文件携带已轮换的陈旧 refresh_token，若不强制从 live-config 重播种将批量 invalid_grant 隔离），另有 S3×4。

## 复核清单逐条结论（lead 五项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | RBAC 生产生效（auth can-i 矩阵） | **通过，与 ADR §5 逐字一致**。Role `ponyllm-config-rw` = `resources:secrets / resourceNames:[ponyllm-live-config] / verbs:[get,patch]`，RoleBinding 绑定 SA 于 ponyllm ns。实测矩阵见下 |
| ② | 锁库 env/DSN 无泄漏 | **通过**。`PONYLLM_LOCK_DATABASE_URL` = `valueFrom.secretKeyRef(ponyllm-lock-dsn)`（非明文）；锁库 `POSTGRES_PASSWORD`/`PONYLLM_LOCK_ROLE_PASSWORD` 同为 secretRef；CA 经 Secret 卷 `ponyllm-lock-tls`（0644）注入、`PONYLLM_LOCK_SSLMODE=require`；Pod 日志无 DSN/password/connect 错误泄漏（sanitize 生效）；gateway SA **无法读取** `ponyllm-lock-dsn`/`ponyllm-lock-tls` |
| ③ | rotated_at 前进真实性 | **通过（实况佐证充分）**。Secret `rotated_at` 当前值 **1790648243**（仅解码该非敏感时间戳键），高于 lead 观察端点 1790647897 且距当前 UTC（1790648342）仅 ~99s；Pod 日志显示 02:15–02:19 UTC 真实 antigravity 刷新事件（ag-ariateellani / ag-lanhui100 / ag-city968645 / ag-bruthus08 多 key 成功刷新）与标记前进窗口吻合 |
| ④ | admin 412 语义 | **静态+测试确认**。代码层 `Conflict→HTTP 412 precondition_failed`（P1 审）+ `admin_store_conflict_http_tests` seam 全绿（P1.1）；生产 image `3bfad2f9…` 含该逻辑。**实况 412 写请求验证需写操作，本次未执行**——建议 impl/运维在受控窗口补一次"陈旧 If-Match→412、并发 1×200+1×412"实测（A 项） |
| ⑤ | 回滚预案凭据面 | **有条件通过（S2-1 缺口，见下）**。回滚路径（FileConfigStore + 单副本 + nodeSelector）不会向 Secret 写回旧凭据（file 后端 persist 只写本地文件）；但旧 PVC 文件含已轮换的陈旧 refresh_token，回滚加载即触发 invalid_grant |

### ① RBAC 实测矩阵（`kubectl auth can-i --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa`）

| 查询 | 结果 |
|---|---|
| get/patch secrets/ponyllm-live-config | **yes**（唯一放行项，与 ADR 逐字一致） |
| get secrets/ponyllm-config · aliyun-registry · ponyllm-lock-dsn · ponyllm-lock-tls | no（非白名单全部拒绝） |
| get/list/watch secrets（不指名） | no |
| create / delete / **update** secrets/ponyllm-live-config | no（仅 patch，无 update——与 patch-CAS 决策一致） |
| get configmaps / pods / deployments | no |
| get secrets/… -n kube-system · get secrets -A（跨 ns） | no |

补充：Pod 实际以 UID 10001 + fsGroup 10001 成功执行 get/patch（rotated_at 前进、刷新写回均为该 SA 所为）→ **T0 ⑥ fsGroup/token 可读性项在生产闭环**。

### ② 凭据流转实测

- 网关容器 env：`PONYLLM_LOCK_DATABASE_URL`(secretRef) / `PONYLLM_LOCK_CA_FILE=/etc/ponyllm-lock/ca.crt`(Secret 卷) / `PONYLLM_LOCK_SSLMODE=require` / `PONYLLM_PROBE_ALLOWLIST`(非敏感)。无任何明文凭据 env。
- 锁库容器 env：`POSTGRES_PASSWORD`、`PONYLLM_LOCK_ROLE_PASSWORD` 均为 `valueFrom:ponyllm-lock-dsn`；明文 env 仅为 PGDATA/POSTGRES_USER/POSTGRES_DB 等非敏感项。
- 锁库安全形态：专用 DB `ponyllm_lock` + CONNECT-only 角色 `ponyllm_lock`（PUBLIC 对 DB/schema 全部 REVOKE）——**lock-only 最小权限落地**；服务 `job-copilot-lockdb`(ClusterIP:5432)，server.crt SAN=`job-copilot-lockdb.ponyllm.svc(.cluster.local)` 与主机名校验匹配；网关与锁库**同节点（devserver）**但 SSLMODE=require 生效 → **P11 S2-1 的"同节点 cbr0 明文"场景已被生产 TLS 覆盖**。
- 日志扫描（全文）：无 connect/dsn/password 相关泄漏行。

---

## Findings

### S2-1（重要）回滚预案凭据面：旧 PVC 文件携带已轮换的陈旧 refresh_token，回滚加载即批量 invalid_grant

【证据】
- PVC `ponyllm-data` 仍 Bound（26h，local-path 1Gi）——内含 file 后端时代的 `/var/lib/ponyllm/ponyllm.toml`（全部 provider keys + 轮换前的 refresh_token）。
- init 容器（`init-config-and-snapshot`）只在**文件不存在**时从只读 Secret 播种（`if [ ! -f ...ponyllm.toml ]`），或 `FORCE_CONFIG_SYNC=true` 才强制覆写；Phase 2 起 persist 写 Secret、文件静止在迁移时点。
- ADR 回滚演练验收（2026-09-28-proposed:133）："切回 file 后端 + 单副本可正常启动**并读回 Secret 最新配置**"——机制存在但**无任何强制/校验**，全靠操作员执行 FORCE_CONFIG_SYNC 或删文件。

【问题】
- 直接切回 file 后端（不强制重播种）→ 加载旧 refresh_token（Phase 2 期间已被 Secret 写回轮换）→ 首次刷新 `invalid_grant`；P1.1 的 N=3 缓冲只延缓 3 个周期，之后**批量永久隔离 antigravity key**（需人工 console 重授权）。
- 该 PVC 文件本身是明文凭据库（file 时代遗留），与 `ponyllm-config` 旧 Secret 一样属待清理资产（ADR Phase 4 计划清理，未到期）。
- 回滚**不会**把旧凭据写回 Secret（FileConfigStore 与 persist hook 只写本地文件），故"旧凭据回写"风险不存在——缺口只在"回滚读取侧"。

【修复建议】
- 回滚 runbook 第一行即强制重播种：`kubectl ... set env DEPLOYMENT FORCE_CONFIG_SYNC=true`（或回滚前删除 PVC 内 ponyllm.toml），并加一条启动断言：file 后端加载的 config_version/内容与 live-config 一致才放行流量。
- 把"回滚演练"的验收从"可正常启动"升级为"**启动后 24h 内无 invalid_grant 隔离、antigravity 刷新成功率 >95%**"。
- Phase 4 按期清理 PVC 与 `ponyllm-config` 旧 Secret（见 S3-1）。

---

### S3（建议）

**S3-1 陈旧凭据资产待清理（PVC + 旧 Secret）**
【证据】`ponyllm-config`（Opaque，2d11h）已无挂载（config-ro 卷现指向 `ponyllm-live-config`）；PVC `ponyllm-data` 含明文配置副本。
【问题】双份凭据副本增加漂移与泄露面；`ponyllm-config` 若被误删/误改不影响运行（已隔离），但存在被巡检/备份抓取的风险。
【修复建议】Phase 4 清理 PVC 与旧 Secret；迁移完成确认单真相源前，运维清单标注"ponyllm-config 为废弃资产、禁止继续更新"。

**S3-2 init 容器镜像滞后 + 播种逻辑在 kubernetes 后端下已冗余**
【证据】主容器 image `3bfad2f9…`，init 容器仍为旧 digest `9276fefe…`（播种逻辑仅用于文件不存在时）；`--config-backend=kubernetes` 下主进程经 kube client 直读 Secret，本地文件仅回滚路径使用。
【问题】旧镜像若含历史漏洞面（供应链残留）且随每次重启执行文件拷贝；逻辑冗余。
【修复建议】Phase 3/4 裁剪 init 容器为仅 telemetry-snapshot 播种（或删除 config 播种半），并同步两处 digest 注释（P1 S3-2 跟进项）。

**S3-3 pg_hba 的 hostssl-only 未从集群侧证实（靠 review）**
【证据】锁库部署 `-c hba_file=/certs/pg_hba.conf` 由 init 容器脚本生成（CM 仅见 init-lockdb.sh 与角色/库初始化），pg_hba 内容不可直接只读检视。
【问题】若 pg_hba 含非 TLS 的 `host` 行（scram 密码认证），则持 DSN 者可绕 TLS 直连（DSN 泄露前提）；当前 SSLMODE=require 客户端侧已强制 TLS，属纵深缺口而非现役漏洞。
【修复建议】Phase 2 收尾加一条只读验证命令（psql `SHOW hba_file` 内容巡检）并纳入部署清单；确认 `hostssl` + `scram-sha-256` 为唯一认证路径。

**S3-4 P11 S2-2 后续项未落地（rotated_at 未来时间戳识别）**
【证据】生产 rotated_at 当前值正常（1790648243 且与刷新活动吻合），但 state.rs 未见"marker > now+容差 视为不可信并告警"分支（P11 采纳清单项）。
【问题】低概率绕过路径（持有 patch 权者写未来时间戳 → 隔离缓冲恒真 → zombie key）仍开放。
【修复建议】Phase 2 观察期内补该识别逻辑（容差 60s），或至少在指标/告警层覆盖"rotated_at 异常前跳"。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | 回滚 runbook 强制 FORCE_CONFIG_SYNC/删文件重播种 + 回滚验收升级（24h 无 invalid_grant、刷新成功率>95%） | Phase 2 收尾（回滚演练前置） |
| S3 | 清理 PVC `ponyllm-data` + 废弃 `ponyllm-config` Secret；单真相源确认 | Phase 4 |
| S3 | init 容器裁剪（config 播种半）并同步镜像 digest | Phase 3/4 |
| S3 | pg_hba hostssl-only 只读巡检命令入部署清单 | Phase 2 收尾 |
| S3 | rotated_at 未来时间戳识别（P11 S2-2 落地） | Phase 2 观察期 |
| 待办 | ④实况 412 写验证（陈旧 If-Match→412、并发 1×200+1×412）由 impl/运维在受控窗口执行 | Phase 2 收尾 |

## 审核限制

- `ponyllm.toml` 与 DSN 值未打印（遵守 Secret 只读纪律）；`rotated_at` 为非敏感 epoch 时间戳已解码。
- ④实况 412 与 ⑤回滚实操需要写操作，本次未执行；回滚凭据面结论基于机制静态分析。
- pg_hba.conf 内容由 init 容器生成，无法只读检视（S3-3，靠 review）。
