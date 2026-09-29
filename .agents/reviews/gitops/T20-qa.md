# T20 质量/测试对抗审核报告：GitOps 发布流水线手册与门禁

- 审核对象：GitOps runbook 交付（2 commits：58b3d46 docs+scripts、2a67c22 ADR 迁移；diff a9b4d40..HEAD 限 `docs/gitops-pipeline-runbook.md`、`scripts/release-gate.sh`、`.agents/notes/.../2026-09-29-gitops-pipeline-runbook.md` 迁移）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立验证（实测）：
  - `bash -n scripts/release-gate.sh` → OK
  - 坏输入退出码：无参数 → **exit 2**；非法 tag → **exit 1**；合法 tag + 不存在镜像 digest → **exit 1**（check 2）
  - 回滚 digest 正则快测：合法 sha256 通过、`bad`/`sha256:abc` 拒绝
  - `ldd /bin/ls` → rc=0（诊断性工具退出码对符号缺失不可靠的佐证）
  - `deploy/Dockerfile`：runtime = `debian:bookworm-slim`，仅 `apt-get install ca-certificates`，**无 curl/wget**（生产 Pod 亦实测 `curl: not found`）
  - ADR 迁移合规：`verify-note.sh` 对该文件 → 全部通过
  - 回滚 digest `b1788e90…` 出现在 runbook §2 与 `deploy/ponyllm-phase2-rollback.md` R2 两处（跨文件重复）

## 总体结论：有条件通过

手册结构完整、声明清晰（单一真相源/偏差声明/凭据不落明文），门禁 5 项检查逻辑大体正确且实测坏输入均非零退出；docs/AGENTS.md 合规性良好（现在时态、无废话基本达标）。但存在 **3 条 S2**：① 门禁 check 5 在 `--rollback-digest` 缺失时**自动回退到硬编码的"Phase 1 前"镜像 digest**（陈旧回滚目标，恰是门禁要防的失败模式）；② runbook §3 检查 1（glibc ldd）文档判据"无 GLIBC_2.39 not found 即过"未被命令执行（丢弃 stdout、只依赖 ldd 退出码，可假绿）；③ runbook §3 检查 3（容器冒烟 /health）在标准运行镜像（无 curl/wget）上**必挂**（假阴性，检查表不可用）。另 5 条 S3（tag 正则与文档规范对齐、check 3 注释失实、TARGET_DIGEST 格式校验、jq 依赖、单一真值源执行不彻底）。修完 3 条 S2 后可放行发布链路使用。

---

## S1（阻断）：无

## S2（重要）

### S2-1 release-gate check 5：`--rollback-digest` 缺失时静默回退到硬编码陈旧 digest

【证据】
- `scripts/release-gate.sh:40`：`ROLLBACK_DIGEST` 未提供时 `ROLLBACK_DIGEST="sha256:b1788e90…"`（注释标注为"Phase 1 前"镜像），随后仅做格式正则校验即打印 "OK 回滚 digest 已预填" 并 PASS。
- runbook §3 usage：`--rollback-digest <现网digest>`（回滚目标应为**升级前现网** digest）。

【问题】
回滚目标的正确值是"本次升级前的现网镜像"，而 b1788e90 是 Phase 1 之前的古旧镜像。操作者漏传参数时门禁**静默放行错误回滚目标**——这正是 check 5 存在的意义（强制声明回滚点），自动回退使其形同虚设；且"FALLBACK"与"OK 已预填"措辞误导。

【修复建议】
缺参即 `exit 2`（强制显式声明）；或自动从现网只读推导：`kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[0].image}'`（拆出 digest）作为默认回滚目标并打印来源；两者都比硬编码 b1788e90 正确。

### S2-2 runbook §3 检查 1（glibc 兼容）：文档判据未被命令执行，可假绿

【证据】
- 检查 1 命令：`docker run --rm --entrypoint ldd <镜像>:<tag> /usr/local/bin/ponyllm >/dev/null`，判据注释"无 GLIBC_2.39 not found 即过"。
- 命令把 stdout 丢弃、stderr 未检查，**仅依赖 ldd 退出码**；`ldd` 是诊断工具（实测 `/bin/ls` rc=0），对"缺符号/缺版本"场景通常仍退出 0（not found 行是 stderr 诊断而非错误退出）——即判据"grep 不到 not found"没有被执行。

【问题】
glibc 不匹配（GLIBC_2.39 not found）时该检查可能照样 PASS → 正是它要防的运行时崩溃场景假绿。

【修复建议】
按文档判据显式 grep：
`! docker run --rm --entrypoint ldd <镜像>:<tag> /usr/local/bin/ponyllm 2>&1 | grep -q 'GLIBC_2.39 not found'`
（`grep` 找到即非零退出；或用 `&& exit 1 || exit 0` 显式反转）。

### S2-3 runbook §3 检查 3（容器冒烟 /health）：标准运行镜像无 curl/wget，必挂

【证据】
- 检查 3 命令：`docker exec pg-smoke sh -c 'wget -qO- … 2>/dev/null || curl -s …'`。
- `deploy/Dockerfile` runtime 只 `apt-get install ca-certificates`，**未装 curl/wget**（bookworm-slim 默认无）；生产 Pod 实测 `curl: not found`（P2 审核记录）。

【问题】
容器内 `wget`/`curl` 均不存在 → `sh -c` 返回 127 → 冒烟步骤**在标准镜像上必然非零退出**（假阴性），§3 检查表不可通过；或操作者为绕过而改用别的镜像形态，偏离生产制品。

【修复建议】
改宿主机侧探测：`docker run --rm -d --name pg-smoke -p 127.0.0.1:18080:8080 <镜像>:<tag> serve --bind 0.0.0.0:8080` + 宿主机 `curl -sf http://127.0.0.1:18080/health`（宿主机有 curl 即可）+ `docker rm -f pg-smoke`；或给运行镜像装 curl（增体积，慎选）。与 phase3-verify.sh [3/7] 的"in-pod 无 curl → --pod-ips"教训一致。

---

## S3（建议）

### S3-1 tag 正则允许裸 semver，与文档"<semver>-<suffix>"规范不一致
`release-gate.sh:43` 后缀 `(-…)?` 可选 → `v0.2.45`（无后缀）也 PASS；runbook §1 写"tag 命名规范：<semver>-<suffix>"。对齐：后缀必填（去掉 `?`）或文档注明"后缀可选"。

### S3-2 gate check 3 注释失实
`release-gate.sh:8` 注释"kubectl 只读确认线上形态 = 清单形态"，实现为 `grep -q "$TARGET_DIGEST" "$DEPLOY_YAML"`（仅本地文件）。发布前校验本地清单本就是正确口径（线上尚为旧 digest），改注释为"部署清单引用目标 digest"即可，勿承诺 kubectl 比对。

### S3-3 TARGET_DIGEST 格式未校验
仅 ROLLBACK_DIGEST 有 `^sha256:[0-9a-f]{64}$` 校验；TARGET_DIGEST 畸形时到 check 3 才以混乱的 grep 信息失败。补同款格式校验。

### S3-4 §2 凭据提取命令依赖 jq
`kubectl … | base64 -d | jq -r '.auths | keys[]'` 需宿主机有 jq（未保证）；且该命令输出的是 auths map 的 key（registry 主机名）而非凭据本身，与"输出含密"文案不一致（保守方向可接受）。建议注明 jq 依赖与输出语义（或改用 `--field-selector`/python3 解析）。

### S3-5 单一真值源执行不彻底（docs/AGENTS.md 规则）
runbook 声明"冲突以本手册为准并修正该文件"，但：
- 旧 note `.agents/notes/implemented/process/2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md` 的 GHCR 描述未被修正（仅 §5 声明偏差）；
- 回滚 digest `b1788e90…` 在 runbook §2 与 `deploy/ponyllm-phase2-rollback.md` R2 两处重复（docs/AGENTS.md"同一事实不跨文件重复叙述"违规）。
建议：旧 note 顶部加"发布链路事实以 docs/gitops-pipeline-runbook.md 为准"指向；digest 单一来源，另一处引用而非复述。

### S3-6 §3 检查表 item 8（基线记录）无命令
表头声称"机械可查，逐条非零退出"，item 8 只是记录动作（记录到 24h 观察日志）。标注"记录动作（靠 review）"或给 per-pod 计数器采集命令（`kubectl … jsonpath` + metrics 端点）。

### S3-7 gate "FALLBACK" 措辞
`release-gate.sh:40` 打印 "FALLBACK …" 后 check 5 仍打印 "OK 回滚 digest 已预填"——自动回退路径下"已预填"误导（并入 S2-1 修复时一并改措辞）。

---

## docs/AGENTS.md 合规判定（focus ③）

| 规则 | 判定 | 证据 |
|---|---|---|
| 现在时态 | ✅ | 手册为现在时描述（§5 偏差声明是必要的现状-旧文对照，非战争故事） |
| 无废话 | ✅ | 语言密集直击核心 |
| 单一真值源 | ⚠️ | 声明充分（§1-5 + §5 偏差），但"修正冲突文件"义务未执行 + 回滚 digest 跨文件重复（S3-5） |
| Slop Checklist ① 无废话 | ✅ | |
| Slop Checklist ② 无重复定义 | ⚠️ | 回滚 digest 两处（S3-5） |
| Slop Checklist ③ 机械内容配非零退出命令 | ⚠️ | §3 item 8 无命令（S3-6）；item 1/3 命令判据失效（S2-2/S2-3） |

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① 每条机械断言确可非零退出 | ⚠️ 多数可（§3 2/4/5/6/7 命令有效）；item 1 ldd 退出码判据失效（S2-2）、item 3 无 curl/wget 必挂（S2-3）、item 8 无命令（S3-6） |
| ② release-gate.sh：bash -n / 5 项判定 / 坏输入 | ✅ bash -n 过；check 1/2/4 判定正确、check 5 格式校验正确但缺参自动回退陈旧值（S2-1）；坏输入实测 exit 2/1 |
| ③ docs/AGENTS.md 合规 | ⚠️ 现在时/无废话达标；单一真值源与 Slop ②③ 有缺口（S3-5/6） |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | gate check 5 缺参 FAIL 或 kubectl 只读取现网 digest 作回滚目标；改 FALLBACK 措辞 | S2-1/S3-7 | P0 |
| 2 | §3 检查 1 改显式 grep（`! … ldd … 2>&1 \| grep -q 'GLIBC_2.39 not found'`） | S2-2 | P0 |
| 3 | §3 检查 3 改 `-p` 端口映射 + 宿主机 curl | S2-3 | P0 |
| 4 | tag 正则与文档对齐；check 3 注释改实；TARGET_DIGEST 格式校验 | S3-1/2/3 | P2 |
| 5 | 旧 note 加"以 runbook 为准"指向；回滚 digest 单一来源；item 8 标"靠 review"；jq 依赖注明 | S3-4/5/6 | P2 |