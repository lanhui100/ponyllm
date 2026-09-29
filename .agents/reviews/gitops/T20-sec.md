# GitOps runbook 对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：GitOps runbook 交付（2 commits: 58b3d46 docs+scripts, 2a67c22 ADR 迁移；diff a9b4d40..HEAD 限 `docs/gitops-pipeline-runbook.md`、`scripts/release-gate.sh`、`.agents/notes/process/2026-09-29-gitops-pipeline-runbook.md`）
- 审核日期：2026-09-29
- 审核方式：只读（三文件全文审阅 + 全仓 GHCR 引用扫描 + 秘密字面量扫描 + ci.yml build-and-push-image 凭据核对 + 与 P2/P3 生产实况比对）
- 审核范围：① 凭据面（ACR 凭据/DSN/token 无明文、Secret 引用正确）；② GHCR 偏差声明的安全含义；③ 门禁绕过面（release-gate.sh 可跳过性与强制卡点）

## 总体结论：**通过（无 S1/S2）**

凭据面干净（三文件秘密字面量扫描 **0 命中**，Secret 引用与生产实况一致；CI 推 GHCR 仅用仓库级 GITHUB_TOKEN，**无任何 ACR/生产凭据进入 CI**）；GHCR 偏差声明充分且无残留误导；release-gate.sh 为本地自洽门禁（可跳过、无强制钩子）——**真正的上线硬卡点是 rollout 后的 phase3-verify.sh**，需在手册中把该层级关系写明。仅 S3×3。

## 复核清单逐条结论（lead 三项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 凭据面（ACR 凭据/DSN/token 无明文、Secret 引用正确） | **通过**。三文件 grep 密码/token/私钥/`sk-`/AKIA/`-----BEGIN` 类字面量 **0 命中**；runbook §2（:47-52）仅记录凭据存放处（集群拉取=Secret `ponyllm/aliyun-registry` + imagePullSecrets 引用，与生产 Secret 名一致[P2 实测]）与本地推送提取方式（只读命令、受控终端、禁写文件/日志）；§3 检查表中的网关 admin token 为 `<…>` 占位符；**CI 凭据面**：build-and-push-image 仅 `permissions: contents:read/packages:write` + `if: main push only`（PR 绝不执行）+ GHCR 登录用 `github.actor`+`secrets.GITHUB_TOKEN`（仓库级自动撤销 token）——**无 ACR 凭据、无 secrets 注入**，生产凭据不进 CI |
| ② | GHCR 偏差声明的安全含义 | **通过**。声明充分（runbook §5 :84-89 + ADR :26-27/:38-39 均明确"GHCR build-and-push-image 不参与生产发布"，生产=本地构建→ACR 手动 push→`<semver>-<suffix>` tag→digest 引用→Keel poll 滚动）；**无残留误导**（全仓 ghcr.io 引用仅 ci.yml + 两文档，无任何 deployment/脚本指向 GHCR）；安全含义：(a) CI 零生产凭据 → 无泄露面；(b) GHCR 镜像为同 Dockerfile 公开制品（无内嵌秘密，可接受）；(c) **GHCR `latest` 为可变标签**——可被仓库写权限者或受攻陷 action 覆盖——手册仅声明生产不用，未覆盖测试/演示环境（S3-1） |
| ③ | 门禁绕过面（release-gate.sh 可跳过性、强制卡点） | **有条件通过（S3-2/S3-3）**。release-gate.sh 5 项检查（tag 格式/本地 RepoDigests 含目标 digest/yaml grep/verify 存在+bash -n/回滚 digest 格式）为**发布前物料自洽**校验，本地手动调用、无 CI/钩子强制——可被跳过且不检测；runbook §6"未通过禁止 push/上线"为政策声明非硬卡点。**真正的硬门禁 = rollout 后 phase3-verify.sh**（对运行形态机械断言：4 副本分布/健康/412/锁指标/kill-drill）——该层级关系应显式化（S3-3）。两处可机械补强的绕过点见 S3-2 |

---

## Findings

### S3（建议）

**S3-1 GHCR 可变 `latest` 标签的禁用范围应扩展到所有环境**
【证据】ci.yml build-and-push-image 推 `ghcr.io/<repo>:latest|sha-<GITHUB_SHA>`；runbook §1 仅声明"生产不使用 GHCR 镜像"（:31）。
【问题】`latest` 可变且公开可拉：仓库写权限者/受攻陷 action 供应链可覆盖推毒；若任何测试/演示/k3d 环境按 tag 拉 GHCR，即引入供应链风险（生产 digest 固定不受影响）。
【修复建议]手册新增一句"任何环境（含本地测试/演示/k3d）一律禁用 GHCR `latest`/可变 tag 引用，镜像引用必须 digest 固定"；可选：CI 停推 `latest`（仅 sha 不可变 tag）或制品置私有。

**S3-2 release-gate #2 补远端 registry 存在性检查**
【证据】gate #2 仅 `docker image inspect` 本地 RepoDigests（push 后本地才有该 digest，故通过隐式意味着曾 push 过）；未验证**远端 ACR 当前是否真含该 digest**（例如 tag 被覆盖后 digest 漂移、push 到错误 repo）。
【问题]本地通过 ≠ 远端在；上线拉取时可能 404/错 digest。
【修复建议]gate #2 追加只读 `docker manifest inspect <IMAGE>@<TARGET_DIGEST> >/dev/null`（远端存在性，非零退出即失败）。

**S3-3 门禁层级显式化 + 上线后 running-digest 复核**
【证据]release-gate（发布前、可跳过）与 phase3-verify（上线后、机械断言）分工未在手册显式说明；检查表 #5 为 `set image` 后立即 rollout，之后无"运行镜像 == 目标 digest"复核。
【问题]操作员跳过 release-gate 或 set image 后清单被改，均无兜底；门禁可绕过面未被文档定性。
【修复建议]手册 §6 改写为："release-gate 为发布前自洽检查，跳过即失去物料校验（无强制钩子）；上线后门禁以 phase3-verify.sh 为准（机械断言运行形态，含 412/锁指标/kill-drill），gate 通过是 verify 的前置非充分条件"。检查表 #5 后补一条只读复核：`kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[0].image}'` 必含目标 digest。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S3 | GHCR `latest` 禁用范围扩到所有环境（digest 固定）；可选停推 latest | 随 T21 顺手 / 下次发布 |
| S3 | release-gate #2 补 `docker manifest inspect <IMAGE>@<digest>` 远端存在性检查 | 随 T21 顺手 |
| S3 | 手册 §6 门禁层级显式化 + 检查表补 running-digest 复核 | 随 T21 顺手 |

## 复核确认项

- 凭据存放命令（runbook :52）实际输出为 `.auths | keys[]`（仅 registry 主机名），**比文档所述"输出含密"更安全**——文档措辞偏保守，无信息泄露；建议保留保守措辞不删。
- `PONYLLM_ADMIN_TOKEN` 在手册/脚本中均为环境变量占位，无字面量（verify.sh 经 env 注入，T13 起 TOKEN 缺失 exit 2）。
- gate #5 的 `FALLBACK` 分支缺参时回退已知回滚 digest（硬编码已知值，非敏感）。

## 审核限制

- 只读审阅；release-gate.sh 未实际执行（需本地 docker 镜像与 kubectl 只读环境）；ci.yml 的 GHCR 推送行为以代码审阅为准（未触发 CI）。
- 生产 ACR token（`crpi-…`）本仓库内除 Secret 外无其他引出（P2 实况确认 imagePullSecrets 引用正确）。