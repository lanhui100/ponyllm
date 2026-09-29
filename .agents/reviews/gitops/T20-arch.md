# T21 GitOps 发布流水线对抗审核报告（架构红队）

- 审核对象：2 commits `58b3d46..2a67c22`（`docs/gitops-pipeline-runbook.md` 新增 + `scripts/release-gate.sh` 新增 + ADR `2026-09-29-gitops-pipeline-runbook.md` proposed→implemented 迁移）
- 审核人：arch-reviewer
- 聚焦：① 链路事实准确性（与生产实测对照）② 单一真相源（重复事实是否只引用不复述）③ 偏差声明（GHCR vs 实测链路）④ ADR 迁移格式合规
- 核查方式：源码/manifest/CI 走读 + 只读 kubectl（含 keel 日志实证）+ `verify-note.sh` 机械校验

---

## 总体结论：**有条件通过**（链路事实经生产实测逐项核对基本准确、偏差声明清晰、ADR 迁移格式合规且 `verify-note.sh` 全过；1 项 S2 发布检查表机械失效 + 1 项 S2 单一真相源在 ADR 内部被违犯，需小改）

`docs/gitops-pipeline-runbook.md` 方向正确（单一真相源声明、只引用不复述、逐条非零退出命令），发布门禁自洽；但 §3 检查表的镜像冒烟步骤**在真实镜像上必然失败**（镜像无 HTTP 客户端），且被宣布为"唯一权威"的 ADR 自己在 `## 链路事实` 一节复述了手册事实。

---

## 一、聚焦逐条核验

### ① 链路事实准确性 —— 基本准确，逐项实测对照通过
【证据（每项均与生产/仓库实测一致）】
| 手册声明 | 实测证据 | 结论 |
|---|---|---|
| 多阶段构建 rust:1.91-bookworm（glibc 2.36）→ debian:bookworm-slim 运行、web COPY 进 /opt/ponyllm/web/dist | `deploy/Dockerfile`：builder `rust:1.91-bookworm`，runtime `debian:bookworm-slim@sha256:60eac…`（定 pin），`COPY web/dist /opt/ponyllm/web/dist`，USER 10001 | ✓ |
| ACR 坐标 `crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2` | live 镜像与其完全一致（4 副本 image=…3bfad2f9…，imagePullSecrets=aliyun-registry） | ✓ |
| 当前生产 digest 3bfad2f9… | live Deployment image `@sha256:3bfad2f9dd7c…70acbf070` | ✓ |
| 回滚 digest b1788e90… | `deploy/ponyllm-phase2-rollback.md` R2 同值 | ✓ |
| Keel 0.20.0 / poll / @every 10m / policy minor | `deploy/keel-autodeploy.yaml:99` image=keelhq/keel:0.20.0；gateway annotations keel.sh/trigger=poll, pollSchedule=@every 10m, policy=minor | ✓ |
| RollingUpdate maxSurge1/maxUnavailable0 + 4 副本零中断（T18） | live strategy 一致；T19 已实证 | ✓ |
| IngressRoute 路径集 → 同一 Pod /opt/ponyllm/web/dist；favicon subPath | `deploy/ponyllm-ingress-routes.yaml:117` match 路径集与手册 §4 逐一对应；favicon.yaml subPath 语义 | ✓ |
| GHCR build-and-push-image 存在但不参与生产 | `.github/workflows/ci.yml` 任务存在，packages: write 下放，push ghcr.io/${{ github.repository }} | ✓ |
| **Keel "poll 监听 digest 变化 → 滚动"（机制性）** | keel 日志实证（只读）：`trigger.poll.RepositoryWatcher: new watch tag digest job added … digest=sha256:3bfad2f9… schedule=@every 10m`（01:41:45Z），且 digest 切换（b1788e90→c8ba8363→3bfad2f9）时 watcher 对应对换（added/removed）——**watch 机制真实运行** | ✓（机制准确） |
【S3 精度补注】keel 日志中未见任何"keel 自身触发 rollout/update"的行——历史滚动均为手动 apply/set image 引起（keel 因 Deployment 镜像变化而重挂 watcher）。手册 §1"Keel poll 监听 digest 变化 → 滚动"建议补半句："watch 机制已实证；生产滚动目前均由手动 apply/set image 触发，keel 自动触发尚未发生"。

### ② 单一真相源 —— 手册自身守约，但 ADR 违犯
【证据】
- 手册 §1/§3/§4 正确"只引用不复述"：Dockerfile/Deployment/keel/ingress/favicon/verify 均以路径引用 ✓；§3 检查表逐条非零退出命令 ✓。
- **违犯点 S2-2**：迁移后的 ADR（`.agents/notes/implemented/process/2026-09-29-gitops-pipeline-runbook.md`）在 `## Decision` 声明"以 docs/gitops-pipeline-runbook.md 作为…单一真相源"之后，又新增 `## 链路事实（2026-09-29 实测校准）` 一节**复述**镜像坐标、`sha256:3bfad2f9…`（省略号）、keel 0.20.0/poll/10m/minor、GHCR 坐标——与手册 §1/§2/§5 重复；digest 等易变值在 ADR 里再写一份即第二个漂移点（下次发版换 digest，ADR 不更新则静默失真）。
【修复建议（S2）】把 `## 链路事实` 改为指针+时点快照：`## 链路事实（2026-09-29 执行时点快照；真相源见 docs/gitops-pipeline-runbook.md §1-§2）`，其中易变值（digest）删除或补"以运行 command 取号（kubectl get deploy … -o jsonpath image）"，不再落常量。
【S3 附加】手册 §2 自身也硬编码了"当前生产 digest/回滚 digest"常量（§4 已注明"取号命令"，常量即冗余）：建议当前 digest 直接给出取号命令，回滚 digest 引用 rollback 文档（避免与 deploy/ponyllm-deployment.yaml、ponyllm-phase2-rollback.md 三处漂移）。

### ③ 偏差声明（GHCR 链路 vs 实测链路）—— 清晰
【证据】手册 §5 显式声明 `2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md` 描述的"GHCR + build-and-push-image 自动推镜像 + sha-<GITHUB_SHA> 标签治理"与实际链路（本地构建 → ACR 手动 push → <semver>-<suffix> → digest 引用 → Keel poll）不一致，"以本手册为准"；GHCR 制品仍存在但"不参与生产发布"。ADR Decision 第 5 点镜像同一声明的摘要。✓
【S3 补注】GHCR 与 ACR 是**同一 Dockerfile 的两次独立构建**（非位级一致，Rust 构建一般不可复现），可加一句"GHCR 镜像与 ACR 镜像 digest 不同、仅作 CI 制品"，杜绝未来有人把 GHCR 镜像当"同源码产物"误用。

### ④ ADR 迁移格式合规 —— 合规（机械校验通过）
【证据】
- 迁移三件事同一次变更完成（2a67c22：移动文件 proposed/process → implemented/process + Status 行 proposed→implemented + 提案标题 `## Proposal` 改写为现在时 `## Decision`）；原提案 `## Proposal`（将来时）已不存在；
- implemented 骨架齐备：`## Problem` → `## Decision` → `## Alternatives considered` → `## Consequences`；`Status: implemented` 与目录一致；
- **机械校验**：`bash .agents/skills/write-adr/verify-note.sh .agents/notes/implemented/process/2026-09-29-gitops-pipeline-runbook.md` → "全部通过（机械影子 1.1–1.8）"；
- 时序合规：决策先以 proposed 记录（a9b4d40）→ 交付（58b3d46）→ 迁移 implemented（2a67c22）。
【S3 格式注】`## 链路事实` 一节插在 Decision 与 Alternatives 之间（规范骨架为 Decision → Alternatives → Consequences 三节）；机械校验未判（标题存在性检查），但顺序建议移入 Consequences 或作为决策附录，与 S2-2 修复一并处理。

---

## 二、S 级发现

### S2-1 发布检查表 §3 第 3 步（容器冒烟 /health）在真实镜像上必然失败
【证据】手册 §3 第 3 步：`docker run --rm -d … serve --bind 0.0.0.0:8080 && … docker exec pg-smoke sh -c 'wget -qO- http://127.0.0.1:8080/health 2>/dev/null || curl -s http://127.0.0.1:8080/health'`——但运行镜像 `deploy/Dockerfile` 仅装 `ca-certificates`，**无 wget/curl**（T19 已在网关容器实测 `curl/wget: not found`）；`docker exec … sh -c 'wget… || curl…'` 两个命令均 127 → exec 非零 → **该检查表行每次发布必 FAIL**，"逐条非零退出命令"对该行不成立，操作者被迫即兴绕过。
【修复建议】冒烟改宿主机侧：`docker run --rm -d -p 127.0.0.1:18080:8080 --name pg-smoke <镜像>:<tag> serve --bind 0.0.0.0:8080 && sleep 2 && curl -sf http://127.0.0.1:18080/health`（宿主 devserver 有 curl；结束 `docker rm -f pg-smoke`）。

### S2-2 ADR `## 链路事实` 复述手册事实，违犯其自宣的单一真相源（见聚焦②）

### S3 项
1. **S3-1** 手册 §2 当前/回滚 digest 常量冗余（改为取号命令 + 引用 rollback 文档）。
2. **S3-2** `release-gate.sh` 的 `--rollback-digest` 缺省回填 `b1788e90` 硬编码常量（与手册/rollback 文档三处漂移点；建议必填或从单一源读取）。
3. **S3-3** 检查表第 4 步 `kubectl set image` 只改线上不改 `deploy/ponyllm-deployment.yaml` → 发布后清单与线上漂移（gate 第 3 步只保证发布前文件一致）；建议改为"更新清单 digest 字段 → `kubectl apply`"以保持文件==线上（与 Phase 3 清单同步纪律一致）。
4. **S3-4** §2 凭据提取命令 `… | jq -r '.auths | keys[]'` 实际只输出 registry 主机名（口令只过管道不进输出），与注释"输出含密"不符；如需 `docker login` 应给出 username/password 的完整投影（同样受控终端约束）。
5. **S3-5** Keel 自动触发未曾在生产发生（历史滚动均手动），手册补半句精度（见聚焦①）。
6. **S3-6** ADR `## 链路事实` 节序（Decision 与 Alternatives 之间）随 S2-2 一并调整。
7. **S3-7** GHCR 与 ACR 为两次独立构建、digest 不同，偏差声明可补一句（见聚焦③）。

---

## 三、采纳清单建议

### 必须采纳（S2，下次发布前）
1. S2-1：冒烟改宿主侧 curl（发布检查表第 3 步），并实际 dry 跑一遍该命令。
2. S2-2：ADR `## 链路事实` 改为指针 + 时点快照（删除/去常量化的易变值）。

### 建议采纳（S3）
3. S3-1/S3-2：digest 常量收敛为"取号命令 + 单一引用源"；S3-3：发布机制改 manifest-apply 保持文件==线上；S3-4：凭据说明/命令补全；S3-5/S3-7：精度补句；S3-6：节序随 S2-2 调整。

### 可驳回
- 其余维持现状。

---

## 复核命令（只读，本报告已执行）

```bash
bash .agents/skills/write-adr/verify-note.sh .agents/notes/implemented/process/2026-09-29-gitops-pipeline-runbook.md  # 1.1-1.8 全过
kubectl -n keel logs deploy/keel --tail=40 | grep -iE "RepositoryWatcher|digest"    # keel digest-watch 实证
sed -n '117p' deploy/ponyllm-ingress-routes.yaml   # IngressRoute 路径集与手册 §4 对照
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[0].image}'  # 3bfad2f9 一致
# 冒烟修复后自测（非集群写）：
docker run --rm -d -p 127.0.0.1:18080:8080 <镜像>:<tag> serve --bind 0.0.0.0:8080 && sleep 2 && curl -sf http://127.0.0.1:18080/health && docker rm -f <容器>
```