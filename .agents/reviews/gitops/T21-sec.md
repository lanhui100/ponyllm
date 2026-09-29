# GitOps runbook T22 调优定向对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：GitOps runbook T22 调优增量（3 commits: 8b73e8c, 310c1a5, fec56f5；diff 169acfd..HEAD 限 `docs/gitops-pipeline-runbook.md`、`scripts/release-gate.sh`、`.agents/notes/implemented/process/*.md`）
- 审核日期：2026-09-29
- 审核方式：只读（四文件全文审阅 + 凭据面扫描 + 与 T20-sec 建议逐条比对）；无任何集群写操作
- 审核范围：① 凭据无明文；② GHCR latest 全环境禁用声明；③ 远端 `docker manifest inspect` 存在性检查；④ 门禁层级显式化与"运行镜像==目标 digest"复核

## 总体结论：**通过（无 S1/S2/S3）**

T20-sec 的 **S3×3 全部落地且超预期**：① 凭据面扫描 0 字面量（唯一命中为 dockerconfigjson 的 jq 键名 `.value.password`，非凭据值），凭据处理进一步强化（`/tmp/acr.json` 用完即删、禁落日志/消息/仓库/共享路径），现网运行 digest 改取号命令不再写死；② GHCR（含 `latest` 与 `sha-…` 标签）全环境禁用声明明确落地；③ release-gate 新增远端 `docker manifest inspect` 存在性检查（未同步/未登录即 FAIL）；④ §6 门禁层级图显式化（release-gate → §3 人工检查表 → verify 全量 → kill-drill → 观察基线 → 24h 观察）+ 检查表 #6 新增"运行镜像==目标 digest"复核，且 release-gate 把回滚 digest 从静默回退改为**必填（exit 2）**——比原建议更严格。**无残留 finding；建议本轮直接归档。**

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 凭据无明文（手册/脚本/ADR 秘密字面量扫描） | **通过**。三文件扫描仅命中 `--password-stdin <<< "$(jq -r '.auths | to_entries[0].value.password' /tmp/acr.json)"`（docs/gitops-pipeline-runbook.md:60）——命中串为 dockerconfigjson 的 **JSON 字段键名**（jq selector），非凭据值；凭据流转强化（:52-60 区域）：ACR 凭据经 Secret 提取到 `/tmp/acr.json`、`--password-stdin` 注入、**禁止落日志/消息/仓库/共享/备份路径、用完即删**；现网运行 digest 与历史回滚 digest 均改为取号命令/指针（单一真值源在集群与 rollback.md），手册不再写死易变 digest |
| ② | GHCR latest 全环境禁用声明 | **通过（T20 S3-1 完整落地）**。§5 明确："**GHCR 镜像（含 `latest` 与 `sha-…` 标签）在全部环境禁用**（sec S3-1）：GHCR 构建产物不参与任何环境的部署与回滚；生产/预演/测试一律使用 `crpi-…/job-copilot/api-v2@sha256:…`"；另补"GHCR 与 ACR 是同一 Dockerfile 的两次独立构建、产物 digest 不同"（arch S3-1，消除混淆面） |
| ③ | 远端 `docker manifest inspect` 存在性检查 | **通过（T20 S3-2 落地）**。release-gate 新增检查 #3：`docker manifest inspect "$IMAGE@$TARGET_DIGEST" >/dev/null`（失败即 exit 1），注释明确"需已 docker login（凭据提取见 runbook §2）；失败即中止，防'本地有 tag 但 registry 侧不可读/未同步'的假阳性"——**闭合"本地通过≠远端在"的绕过点** |
| ④ | 门禁层级显式化 + 运行镜像==目标 digest 复核 | **通过（T20 S3-3 完整落地 + 加固）**。§6 门禁层级图：`release-gate.sh（tag/digest 格式+本地+远端存在性+清单一致+verify bash -n+回滚预填）→ §3 人工检查表（1-10）→ phase3-verify.sh 全量（含 [6/7] 并发写 412）→ kill-drill → 观察基线 → 24h 观察`，并声明"未通过 release-gate 禁止 push/上线；§3 任一条失败即中止并按 R0'/R0/R1 回滚"；检查表新增 **#6 运行镜像复核**：`kubectl … jsonpath image` 输出须含 `@<digest>`（与清单对照）；release-gate 回滚 digest 由"静默回退硬编码"改为**必填（缺失 exit 2）**（qa S2-1）——回滚目标必须是操作者显式传入的现网 digest |引用式|

---

## Findings

### 无（含 S3 级）

T20-sec 全部采纳项已闭环：

- S3-1（GHCR 全环境禁用）→ 手:105-107 落地。
- S3-2（远端 manifest inspect）→ release-gate #3 落地。
- S3-3（门禁层级 + 运行 digest 复核）→ 手:108-119 §6 层级图 + 检查表 #6 落地。
- 附带加固（优于原建议）：回滚 digest 必填 exit 2（无静默回退）；`/tmp/acr.json` 用完即删 + 禁落共享路径；digest 格式校验覆盖 target+rollback 双值；现网 digest 单一真值源化。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| 归档 | 本轮无遗留项；T20-sec 与 T21-sec 一并归档 | 立即 |
| 备忘 | release-gate #3 的 manifest inspect 依赖本地 docker 已登录（未登录会 FAIL 而非拉取）——手册 §2 已给出凭据提取路径，属预期行为，无需改动 | — |

## 审核限制

- 只读审阅；release-gate.sh 新检查（manifest inspect / 必填回滚 digest）未实际执行（需 docker 登录 + 本地镜像环境）；
- 凭据流转命令（/tmp/acr.json 提取 + password-stdin）为机制审阅，未在集群/终端执行。