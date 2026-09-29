# Phase 4 准备对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 4 准备产物（commit a5e1c12：`docs/phase4-cleanup.md`、`docs/phase4-observation.md` + T25 消费者搜索结论）
- 审核日期：2026-09-29
- 审核方式：只读（两份文档全文审阅 + T25 零引用结论只读复现 + 主机 8080 监听实况核查）；无任何集群写操作
- 审核范围：① 清理凭据面（旧 Secret/PVC/svc Endpoints 残留、删除前零引用、备份位置权限、删除后无残留）；② 观察期凭据巡检项；③ Endpoints 指向已下线主机进程的清理语义

## 总体结论：**通过（有条件通过）**

T25 零引用结论**复现一致**（svc/PVC/Secret 均零消费者，唯一 auth 命中为 Deployment 自身名前缀非引用）；主机进程下线**实况佐证成立**（devserver `ss -ltnp` 无 8080 监听、curl refused）——③ 清理语义正确。发现 **S2×1**（备份命令权限声明失实：声明"权限 600"但命令无 chmod/umask，且 Secret yaml 备份含全部明文凭据、留存 30 天无到期删除）与 **S3×3**（② 观察框架未纳入 emptyDir 巡检与 rbac 基线 diff、③ 主机下线确认建议机械命令化、PVC 删除后 local-path 卷清理验证缺失）。

## 复核清单逐条结论（lead 三项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 清理凭据面（零引用/备份权限/删除后无残留） | **有条件通过（S2-1）**。零引用：只读复现与 T25 一致——svc `ponyllm-gateway`（10.43.66.57:8080）无 deploy/sts/ds env 引用（唯一 jsonpath 命中为 Deployment 自身名前缀）、无 ingressroute/ingress 引用；PVC `ponyllm-data` 无 live claimer；Secret `ponyllm-config` 无 env/volume secretRef——三对象**删除前零引用成立**；清理后验证（§4 svc/pvc/secret NotFound + 健康复核）非零退出。**缺口**：备份命令无显式 600（见 S2-1）；PVC 删除后 local-path 卷回收验证缺失（S3-3） |
| ② | 观察期凭据巡检项 | **缺口（S3-1）**。`phase4-observation.md` 判定表（A7-1..A7-5/A10-1/A10-2）**未纳入** T19 P3-exec S3-1（节点 emptyDir 无凭据残留）与 S3-3（rbac-audit.sh 基线 + 观察期末 diff）两个安全巡检落点；观察日志模板已正确做到"token 不落日志"（经 $TOKEN env） |
| ③ | Endpoints 指向已下线主机进程的清理语义 | **通过（实况佐证成立）**。Endpoints `100.95.193.103:8080`（devserver 宿主 Tailscale IP）目标进程**已下线**：`ss -ltnp`（普通+sudo）均无 :8080 监听、`curl 127.0.0.1:8080` refused → 清理该手工 Endpoints 语义正确（svc 删除同步清同名 ep + 独立 ep 对象 `--ignore-not-found` 一并删）；文档要求"Lead 确认下线"——建议机械命令化（S3-2） |

---

## Findings

### S2-1（重要）备份命令权限声明失实：Secret yaml 备份含明文凭据、无 600 落实、留存 30 天

【证据】
- `docs/phase4-cleanup.md` §0 备份命令：`mkdir -p /root/ponyllm-phase4-backup-<date>/` + `kubectl get secret ponyllm-config -o yaml > .../secret-ponyllm-config.yaml`——**无 `chmod 600`、无 `umask 077`**（mkdir 默认 755、文件默认 644 依 umask 022）。
- 文档 :16-17 自知"kubectl get -o yaml 含 data（base64 明文）"并声明"备份文件权限 600 且随删即清"——**声明与实际命令不符**（唯一兜底是 /root 家目录默认 700 权限）。
- §3 :48 "备份文件保留至观察期后 30 天"——**含全部 provider keys 的明文副本留存 30 天**，且无到期删除命令。

【问题】
- Secret 备份 yaml 内为 `ponyllm-config`（125）**全部 provider keys/refresh_token 的 base64 明文**；一旦备份目录权限配置偏离（umask 被改、目录被共享、root 账户被共用），30 天留存期即成为可被读取的凭据副本。
- "权限 600"是文档对其自身安全性的承诺——命令未落实即为文档与实施不一致（0 信任承诺落空）。

【修复建议】
- 备份命令补 `umask 077`（或 `chmod 600 .../*.yaml` 一次性），并在 §0 加一条只读断言：`stat -c '%a' <备份目录>` = 700/750 且备份文件 = 600。
- **Secret 备份降级为仅 metadata**：`kubectl get secret ponyllm-config -o jsonpath='{.metadata}'`（内容失去不可重建性由 live-config 替代，125 内容本就废弃）；若坚持全量备份，必须加密（如 `openssl enc` 对称加密后落盘）。
- 明确 30 天到期删除命令（`rm -rf /root/ponyllm-phase4-backup-<date>`）写入 §4 或回滚节。
- 删除执行后复核备份目录无残留（`ls` 确认）。

---

### S3（建议）

**S3-1 观察期凭据巡检项未纳入判定表**
【证据】`phase4-observation.md` 判定表仅 A7-1..A7-5/A10-1/A10-2；T19 P3-exec S3-1（节点 emptyDir 无凭据残留巡检）与 S3-3（rbac-audit.sh 基线快照 + 观察期末 diff）**未落地**到观察框架。
【问题】7 天观察期内"RBAC 意外放宽 / 节点 emptyDir 出现凭据残留"无检测手段，安全侧回归漏检。
【修复建议]观察框架日采命令追加 2 条只读项：① 节点 kubelet emptyDir 目录抽查（`ssh <node> ls /var/lib/kubelet/pods/*/volumes/kubernetes.io~empty-dir/*` 无 ponyllm 凭据文件）；② `bash scripts/rbac-audit.sh` 输出存快照，观察期首/末 diff（RBAC 不得变化）。

**S3-2 ③ 主机下线确认机械命令化**
【证据】文档要求"清理前需 Lead 确认该主机进程已下线"（cleanup.md:66）为人工确认；本次实况已证无监听（`ss -ltnp` 空 + curl refused）。
【修复建议]cleanup.md §1 前置补一条可复核命令：`ssh dev 'ss -ltnp | grep -E ":8080" && exit 1 || echo "OK no host listener on :8080"'`（非零退出即失败），取代纯人工确认，使 ③ 机械可查。

**S3-3 PVC 删除后 local-path 卷清理验证缺失**
【证据】cleanup.md §4 验证只查 `pvc NotFound`，未验证底层 local-path 卷目录（含 file 时代明文配置）是否被回收。
【问题】PVC 删除后 local-path provisioner 通常删除卷目录，但无验证；若回收异常，旧明文配置卷残留节点磁盘。
【修复建议]§4 追加：`kubectl get pv | grep ponyllm-data`（期望无 Bound 卷或状态 Released/Failed）或节点侧 `ls /var/lib/rancher/k3s/storage/*ponyllm-data*` 不存在。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | 备份命令补 umask 077/chmod 600 + 权限断言；Secret 备份降级为仅 metadata（或加密）；30 天到期删除命令 | Phase 4 清理执行前（文档修订） |
| S3 | 观察框架追加 emptyDir 巡检 + rbac-audit 基线首末 diff | Phase 4 观察期 |
| S3 | ③ 主机下线确认改机械命令（ss 断言） | Phase 4 清理执行前 |
| S3 | PVC 删除后 local-path 卷回收验证 | Phase 4 清理执行后 |

## 复核确认项

- T25 零引用结论复现一致：svc/PVC/Secret 三对象零消费者；唯一 jsonpath "命中"为 Deployment 自身名前缀（`Deployment/ponyllm/ponyllm-gateway:`），非 env 引用——与文档注释吻合。
- A7-4（pg_locks advisory granted ≤1）口径正确（瞬时权威，CONNECT-only 角色可查 pg_lock_status）；其 psql 命令中 `${PONYLLM_LOCK_ROLE_PASSWORD}` 经**单引号 + pod 内 sh 展开**，密码不经过外层 shell/历史/进程参数——无泄漏。
- 观察日志模板"token 不落日志"（$TOKEN env）合规。

## 审核限制

- 只读审阅与实况核查；备份/删除命令未执行（写操作，Lead 授权后由运维执行）。
- local-path 卷回收行为（S3-3）以 k3s 默认语义评估，未做破坏性验证。